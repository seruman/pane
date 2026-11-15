//! Byte streams shared by files, HTTP bodies, and process pipes.

use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, OwnedSemaphorePermit};
use tokio_util::sync::CancellationToken;

use super::{Error, Result};

pub(super) const CHUNK: usize = 64 * 1024;
pub(super) const DEFAULT_LIMIT: usize = 4 * 1024 * 1024;
pub(super) const HARD_LIMIT: usize = 16 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

type SharedSource<T> = Arc<Mutex<Option<T>>>;
type Permit = Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>;

fn close_source<T: Send + 'static>(inner: &SharedSource<T>, permit: &Permit) {
    if let Ok(mut source) = inner.try_lock() {
        source.take();
        if let Ok(mut permit) = permit.lock() {
            permit.take();
        }
    } else {
        // Cancellation can race a successful read/write. Arrange unconditional cleanup
        // after it unlocks, even if that operation never observes the cancelled token.
        // Keep the resource slot until the descriptor has actually been dropped.
        let inner = inner.clone();
        let permit = permit.clone();
        let _cleanup = super::runtime().spawn(async move {
            inner.lock().await.take();
            if let Ok(mut permit) = permit.lock() {
                permit.take();
            }
        });
    }
}

struct Busy<'a>(&'a AtomicBool);

impl<'a> Busy<'a> {
    fn take(flag: &'a AtomicBool) -> Result<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::new("busy", "stream already has an active operation"))?;
        Ok(Self(flag))
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(super) struct ReadGuard<'a> {
    reader: &'a Reader,
    _busy: Busy<'a>,
}

impl ReadGuard<'_> {
    pub(super) async fn read(&self, max: usize) -> Result<Option<Vec<u8>>> {
        self.reader.read_inner(max).await
    }
}

pub(super) struct Reader {
    inner: SharedSource<Pin<Box<dyn AsyncRead + Send>>>,
    cleanup_started: AtomicBool,
    busy: AtomicBool,
    cancel: CancellationToken,
    // Release the slot at EOF/close even when scripts retain the closed handle.
    permit: Permit,
}

impl Reader {
    pub(super) fn new(
        source: impl AsyncRead + Send + 'static,
        cancel: CancellationToken,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Mutex::new(Some(Box::pin(source)))),
            cleanup_started: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            cancel,
            permit: Arc::new(std::sync::Mutex::new(permit)),
        })
    }

    fn release(&self) {
        if let Ok(mut permit) = self.permit.lock() {
            permit.take();
        }
    }

    pub(super) fn exclusive(&self) -> Result<ReadGuard<'_>> {
        Ok(ReadGuard {
            reader: self,
            _busy: Busy::take(&self.busy)?,
        })
    }

    pub(super) async fn read(&self, max: usize) -> Result<Option<Vec<u8>>> {
        self.exclusive()?.read(max).await
    }

    async fn read_inner(&self, max: usize) -> Result<Option<Vec<u8>>> {
        if max == 0 || max > CHUNK {
            return Err(Error::new(
                "invalid_argument",
                "read size must be between 1 and 65536",
            ));
        }
        let mut inner = self.inner.lock().await;
        if self.cancel.is_cancelled() {
            inner.take();
            self.release();
            return Err(Error::new("closed", "reader is closed"));
        }
        let Some(source) = inner.as_mut() else {
            return Ok(None);
        };
        let mut bytes = vec![0; max];
        let read = tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(Error::new("closed", "reader is closed")),
            result = tokio::time::timeout(IDLE_TIMEOUT, source.read(&mut bytes)) => match result {
                Ok(Ok(n)) => Ok(n),
                Ok(Err(e)) => Err(e.into()),
                Err(_) => Err(Error::new("timeout", "reader was idle for 30 seconds")),
            },
        };
        match read {
            Ok(0) => {
                inner.take();
                self.release();
                Ok(None)
            }
            Ok(n) => {
                bytes.truncate(n);
                Ok(Some(bytes))
            }
            Err(error) => {
                inner.take();
                self.cancel.cancel();
                self.release();
                Err(error)
            }
        }
    }

    pub(super) async fn read_all(&self, max: usize) -> Result<Vec<u8>> {
        if max == 0 || max > HARD_LIMIT {
            return Err(Error::new(
                "invalid_argument",
                "maxBytes must be between 1 and 16777216",
            ));
        }
        let _busy = Busy::take(&self.busy)?;
        let mut output = Vec::new();
        loop {
            // Read one extra byte to distinguish an exact-size body from an overflow.
            let next = (max - output.len() + 1).min(CHUNK);
            let Some(chunk) = self.read_inner(next).await? else {
                return Ok(output);
            };
            if chunk.len() > max - output.len() {
                self.close();
                return Err(Error::new("limit", "stream exceeds maxBytes"));
            }
            output.extend_from_slice(&chunk);
        }
    }

    /// Closing a reader aborts an in-flight read rather than waiting for its lock.
    pub(super) fn close(&self) {
        self.cancel.cancel();
        if self.cleanup_started.swap(true, Ordering::AcqRel) {
            return;
        }
        close_source(&self.inner, &self.permit);
    }
}

pub(super) struct Writer {
    inner: SharedSource<Pin<Box<dyn AsyncWrite + Send>>>,
    cleanup_started: AtomicBool,
    busy: AtomicBool,
    cancel: CancellationToken,
    permit: Permit,
}

impl Writer {
    pub(super) fn new(
        sink: impl AsyncWrite + Send + 'static,
        cancel: CancellationToken,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Mutex::new(Some(Box::pin(sink)))),
            cleanup_started: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            cancel,
            permit: Arc::new(std::sync::Mutex::new(permit)),
        })
    }

    fn release(&self) {
        if let Ok(mut permit) = self.permit.lock() {
            permit.take();
        }
    }

    pub(super) async fn write(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > HARD_LIMIT {
            return Err(Error::new("limit", "write exceeds 16 MiB"));
        }
        let _busy = Busy::take(&self.busy)?;
        let mut inner = self.inner.lock().await;
        if self.cancel.is_cancelled() {
            inner.take();
            self.release();
            return Err(Error::new("closed", "writer is closed"));
        }
        let sink = inner
            .as_mut()
            .ok_or_else(|| Error::new("closed", "writer is closed"))?;
        let result = tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(Error::new("closed", "writer is closed")),
            result = tokio::time::timeout(IDLE_TIMEOUT, sink.write_all(bytes)) => match result {
                Ok(result) => result.map_err(Error::from),
                Err(_) => Err(Error::new("timeout", "write timed out")),
            },
        };
        if result.is_err() {
            inner.take();
            self.cancel.cancel();
            self.release();
        }
        result
    }

    pub(super) async fn close(&self) -> Result<()> {
        let _busy = Busy::take(&self.busy)?;
        let mut inner = self.inner.lock().await;
        let Some(mut sink) = inner.take() else {
            return Ok(());
        };
        let result = tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(Error::new("closed", "writer is closed")),
            result = tokio::time::timeout(IDLE_TIMEOUT, async {
                sink.flush().await?;
                sink.shutdown().await
            }) => match result {
                Ok(result) => result.map_err(Error::from),
                Err(_) => Err(Error::new("timeout", "close timed out")),
            },
        };
        self.cancel.cancel();
        drop(sink);
        self.release();
        result
    }

    pub(super) fn abort(&self) {
        self.cancel.cancel();
        if self.cleanup_started.swap(true, Ordering::AcqRel) {
            return;
        }
        close_source(&self.inner, &self.permit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bytes_eof_and_limits() {
        let reader = Reader::new(&b"a\0\xffb"[..], CancellationToken::new(), None);
        assert_eq!(reader.read(2).await.unwrap(), Some(b"a\0".to_vec()));
        assert_eq!(reader.read_all(2).await.unwrap(), b"\xffb");
        assert_eq!(reader.read(1).await.unwrap(), None);
        let reader = Reader::new(&b"12345"[..], CancellationToken::new(), None);
        assert_eq!(reader.read_all(4).await.unwrap_err().kind, "limit");
    }

    #[tokio::test]
    async fn simultaneous_read_fails_and_close_wakes_pending_read() {
        let (source, _sink) = tokio::io::duplex(16);
        let reader = Reader::new(source, CancellationToken::new(), None);
        let pending = tokio::spawn({
            let reader = reader.clone();
            async move { reader.read(1).await }
        });
        while !reader.busy.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        assert_eq!(reader.read(1).await.unwrap_err().kind, "busy");
        reader.close();
        assert_eq!(pending.await.unwrap().unwrap_err().kind, "closed");
    }

    struct Probe(Arc<AtomicBool>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    impl AsyncRead for Probe {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    impl AsyncWrite for Probe {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    async fn cleanup_completed(scope: &super::super::Scope) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while scope.budget.available_permits() != 64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reader_close_racing_a_success_releases_source_before_budget() {
        let scope = super::super::Scope::new(std::env::temp_dir());
        let dropped = Arc::new(AtomicBool::new(false));
        let reader = Reader::new(
            Probe(dropped.clone()),
            scope.cancel.child_token(),
            Some(scope.reserve().unwrap()),
        );
        // Simulate a successful read holding the source lock. After close it unlocks
        // without polling the cancellation branch or performing another read.
        let in_flight = reader.inner.lock().await;
        reader.close();
        reader.close();
        assert_eq!(scope.budget.available_permits(), 63);
        assert!(!dropped.load(Ordering::Acquire));
        drop(in_flight);
        cleanup_completed(&scope).await;
        assert!(dropped.load(Ordering::Acquire));
        assert!(reader.inner.lock().await.is_none());
    }

    #[tokio::test]
    async fn writer_abort_racing_a_success_releases_source_before_budget() {
        let scope = super::super::Scope::new(std::env::temp_dir());
        let dropped = Arc::new(AtomicBool::new(false));
        let writer = Writer::new(
            Probe(dropped.clone()),
            scope.cancel.child_token(),
            Some(scope.reserve().unwrap()),
        );
        let in_flight = writer.inner.lock().await;
        writer.abort();
        writer.abort();
        assert_eq!(scope.budget.available_permits(), 63);
        assert!(!dropped.load(Ordering::Acquire));
        drop(in_flight);
        cleanup_completed(&scope).await;
        assert!(dropped.load(Ordering::Acquire));
        assert!(writer.inner.lock().await.is_none());
    }

    #[tokio::test]
    async fn writer_flushes_and_closes() {
        let (source, sink) = tokio::io::duplex(16);
        let writer = Writer::new(sink, CancellationToken::new(), None);
        let reader = Reader::new(source, CancellationToken::new(), None);
        writer.write(b"hello\0").await.unwrap();
        writer.close().await.unwrap();
        assert_eq!(reader.read_all(16).await.unwrap(), b"hello\0");
        writer.close().await.unwrap();
        assert_eq!(writer.write(b"x").await.unwrap_err().kind, "closed");
    }
}
