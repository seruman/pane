//! Owned subprocesses. Only the supervisor task may signal or reap a child.
//!
//! Process-group cleanup is best effort, not descendant containment. Timeout and
//! kill supervise the direct child only until it is reaped. After a normal exit,
//! descendants may survive and keep output pipes open; we must not signal the
//! cached PGID because it can already have been reused. Collect then waits for
//! pipe EOF (subject to the stream idle timeout), not the process timeout. Children
//! that leave the process group can also escape cleanup. Stronger guarantees
//! require OS-level containment or a separate, unreaped group supervisor.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, watch};
use tokio_util::sync::CancellationToken;

use super::streams::{ReadGuard, Reader, Writer};
use super::{Error, Reply, Resource, Result, Scope};

pub(super) struct Options {
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) cwd: Option<PathBuf>,
    pub(super) env: BTreeMap<String, String>,
    pub(super) inherit_env: bool,
    pub(super) stdin: bool,
    pub(super) timeout_ms: u64,
}

#[derive(Clone)]
enum Completion {
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Failed {
        kind: &'static str,
        message: String,
    },
}

impl Completion {
    fn reply(self) -> Result<Reply> {
        match self {
            Self::Exited { code, signal } => Ok(Reply::map([
                (
                    "exitCode",
                    code.map_or(Reply::Nil, |n| Reply::Number(i64::from(n))),
                ),
                (
                    "signal",
                    signal.map_or(Reply::Nil, |n| Reply::Number(i64::from(n))),
                ),
            ])),
            Self::Failed { kind, message } => Err(Error::new(kind, message)),
        }
    }
}

pub(super) struct Process {
    stdout: Arc<Reader>,
    stderr: Arc<Reader>,
    status: watch::Receiver<Option<Completion>>,
    kill: CancellationToken,
    collecting: AtomicBool,
}

impl Process {
    pub(super) async fn wait(&self) -> Result<Reply> {
        let mut status = self.status.clone();
        loop {
            if let Some(done) = status.borrow().clone() {
                return done.reply();
            }
            status.changed().await.map_err(|_| {
                Error::new(
                    "io",
                    "process supervisor stopped before reporting an exit status",
                )
            })?;
        }
    }

    /// Drain both pipes concurrently, with a single shared output budget.
    pub(super) async fn collect(&self, max: usize) -> Result<Reply> {
        if max == 0 || max > super::streams::HARD_LIMIT {
            return Err(Error::new(
                "invalid_argument",
                "maxBytes must be between 1 and 16777216",
            ));
        }
        self.collecting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::new("busy", "process already has an active collect operation"))?;
        let _collecting = Collecting(&self.collecting);
        // Reserve both readers before consuming either. Busy failures must not
        // consume output or kill a process another operation is still using.
        let stdout = self.stdout.exclusive()?;
        let stderr = self.stderr.exclusive()?;
        let used = AtomicUsize::new(0);
        let output = tokio::try_join!(drain(&stdout, &used, max), drain(&stderr, &used, max));
        let (stdout, stderr) = match output {
            Ok(output) => output,
            Err(error) => {
                self.kill();
                self.stdout.close();
                self.stderr.close();
                // A supervisor timeout/cancellation is more informative than the
                // resulting pipe-close error. Always wait for reaping on failure.
                self.wait().await?;
                return Err(error);
            }
        };
        let Reply::Map(mut status) = self.wait().await? else {
            unreachable!()
        };
        status.insert("stdout".into(), Reply::Bytes(stdout));
        status.insert("stderr".into(), Reply::Bytes(stderr));
        Ok(Reply::Map(status))
    }

    pub(super) fn kill(&self) {
        // Never signal a cached PID here: the supervisor may already have reaped
        // it, allowing the OS to reuse it for an unrelated process group.
        self.kill.cancel();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

struct Collecting<'a>(&'a AtomicBool);
impl Drop for Collecting<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn drain(reader: &ReadGuard<'_>, used: &AtomicUsize, max: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let remaining = max.saturating_sub(used.load(Ordering::Acquire));
        let size = remaining.saturating_add(1).min(8192);
        let Some(chunk) = reader.read(size).await? else {
            return Ok(output);
        };
        let before = used.fetch_add(chunk.len(), Ordering::AcqRel);
        if chunk.len() > max.saturating_sub(before) {
            return Err(Error::new(
                "limit",
                "combined process output exceeds maxBytes",
            ));
        }
        output.extend_from_slice(&chunk);
    }
}

// A single resource-group permit lives until the supervisor AND all open pipe
// descriptors have gone away. Retaining completed handles does not retain the
// permit after EOF/close. Pipes do not consume independent resource slots.
struct Pipe<T> {
    inner: T,
    _permit: Arc<OwnedSemaphorePermit>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Pipe<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Pipe<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub(super) async fn spawn(
    scope: Arc<Scope>,
    options: Options,
    cancel: CancellationToken,
) -> Result<Reply> {
    #[cfg(not(unix))]
    {
        let _ = (scope, options, cancel);
        Err(Error::new(
            "unsupported",
            "subprocesses require Unix process-group support",
        ))
    }
    #[cfg(unix)]
    {
        if scope.cancel.is_cancelled() || cancel.is_cancelled() {
            return Err(Error::new("cancelled", "process spawn was cancelled"));
        }
        let permit = Arc::new(scope.reserve()?);
        let mut child = command(&scope, &options).spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| Error::new("io", "spawned child has no PID"))?;
        let pid = nix::unistd::Pid::from_raw(
            i32::try_from(pid).map_err(|e| Error::new("io", e.to_string()))?,
        );
        // Pipe tokens are separate children: closing one pipe must not cancel
        // its siblings or the process itself.
        let pipes = cancel.child_token();
        let stdout = Reader::new(
            Pipe {
                inner: child
                    .stdout
                    .take()
                    .ok_or_else(|| Error::new("io", "missing stdout pipe"))?,
                _permit: permit.clone(),
            },
            pipes.child_token(),
            None,
        );
        let stderr = Reader::new(
            Pipe {
                inner: child
                    .stderr
                    .take()
                    .ok_or_else(|| Error::new("io", "missing stderr pipe"))?,
                _permit: permit.clone(),
            },
            pipes.child_token(),
            None,
        );
        let stdin = child.stdin.take().map(|inner| {
            Writer::new(
                Pipe {
                    inner,
                    _permit: permit.clone(),
                },
                pipes.child_token(),
                None,
            )
        });
        let stdin_reply = stdin.as_ref().map_or(Reply::Nil, |writer| {
            Reply::Handle(Resource::Writer(writer.clone()))
        });
        let kill = CancellationToken::new();
        let (sender, status) = watch::channel(None);
        let process = Arc::new(Process {
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            status,
            kill: kill.clone(),
            collecting: AtomicBool::new(false),
        });
        tokio::spawn(
            Supervisor {
                group: ChildGroup {
                    child,
                    pid,
                    live: true,
                },
                scope,
                cancel,
                kill,
                pipes,
                stdout,
                stderr,
                stdin,
                _permit: permit,
                timeout_ms: options.timeout_ms,
                sender,
            }
            .run(),
        );
        Ok(spawned_reply(&process, stdin_reply))
    }
}

#[cfg(unix)]
fn spawned_reply(process: &Arc<Process>, stdin: Reply) -> Reply {
    Reply::map([
        ("handle", Reply::Handle(Resource::Process(process.clone()))),
        (
            "stdout",
            Reply::Handle(Resource::Reader(process.stdout.clone())),
        ),
        (
            "stderr",
            Reply::Handle(Resource::Reader(process.stderr.clone())),
        ),
        ("stdin", stdin),
    ])
}

#[cfg(unix)]
fn command(scope: &Scope, options: &Options) -> tokio::process::Command {
    use std::process::Stdio;

    let mut command = tokio::process::Command::new(&options.command);
    command
        .args(&options.args)
        .current_dir(
            options
                .cwd
                .as_ref()
                .map_or_else(|| scope.root.clone(), |cwd| scope.root.join(cwd)),
        )
        .stdin(if options.stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    if !options.inherit_env {
        command.env_clear();
    }
    command.envs(&options.env);
    command
}

#[cfg(unix)]
struct Supervisor {
    group: ChildGroup,
    scope: Arc<Scope>,
    cancel: CancellationToken,
    kill: CancellationToken,
    pipes: CancellationToken,
    stdout: Arc<Reader>,
    stderr: Arc<Reader>,
    stdin: Option<Arc<Writer>>,
    _permit: Arc<OwnedSemaphorePermit>,
    timeout_ms: u64,
    sender: watch::Sender<Option<Completion>>,
}

#[cfg(unix)]
impl Supervisor {
    async fn run(mut self) {
        enum Stop {
            Exited(std::io::Result<std::process::ExitStatus>),
            Killed,
            Cancelled,
            Timeout,
        }
        let stop = tokio::select! {
            // A completed wait reaps the child. Handle that outcome before
            // any pending kill so a recycled process group is never signalled.
            biased;
            status = self.group.child.wait() => Stop::Exited(status),
            () = self.scope.cancel.cancelled() => Stop::Cancelled,
            () = self.cancel.cancelled() => Stop::Cancelled,
            () = self.kill.cancelled() => Stop::Killed,
            () = tokio::time::sleep(Duration::from_millis(self.timeout_ms)) => Stop::Timeout,
        };
        let done = match stop {
            Stop::Exited(status) => {
                self.group.live = false;
                completion(status)
            }
            reason => {
                self.group.signal();
                self.pipes.cancel();
                // Cancellation is not allowed to interrupt this reap.
                let status = self.group.child.wait().await;
                self.group.live = false;
                match reason {
                    Stop::Killed => completion(status),
                    Stop::Cancelled => Completion::Failed {
                        kind: "cancelled",
                        message: "process was cancelled".into(),
                    },
                    Stop::Timeout => Completion::Failed {
                        kind: "timeout",
                        message: "process exceeded timeoutMs".into(),
                    },
                    Stop::Exited(_) => unreachable!(),
                }
            }
        };
        // Close OS pipe descriptors even when no caller ever reads/writes.
        // Preserve buffered output after a normal exit.
        if self.pipes.is_cancelled() {
            self.stdout.close();
            self.stderr.close();
            if let Some(stdin) = self.stdin.as_ref() {
                stdin.abort();
            }
        }
        // The resource may already have been dropped; reaping still had to finish.
        let _receiver_gone = self.sender.send(Some(done));
    }
}

#[cfg(unix)]
fn completion(status: std::io::Result<std::process::ExitStatus>) -> Completion {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(status) => Completion::Exited {
            code: status.code(),
            signal: status.signal(),
        },
        Err(error) => Completion::Failed {
            kind: "io",
            message: error.to_string(),
        },
    }
}

#[cfg(unix)]
struct ChildGroup {
    child: tokio::process::Child,
    pid: nix::unistd::Pid,
    live: bool,
}

#[cfg(unix)]
impl ChildGroup {
    fn signal(&mut self) {
        if self.live {
            // Best effort: an exited group/child may already be absent. Reaping
            // still runs even when signalling fails (including during Drop).
            let _group_signal_result =
                nix::sys::signal::killpg(self.pid, nix::sys::signal::Signal::SIGKILL);
            // Also kill the direct child if it changed its process group.
            let _child_signal_result = self.child.start_kill();
        }
    }
}

#[cfg(unix)]
impl Drop for ChildGroup {
    fn drop(&mut self) {
        // Runtime teardown/aborted supervisor: group cleanup still runs, and
        // Tokio's Child drop reaper owns the direct child afterward.
        self.signal();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn options(script: &str) -> Options {
        // Explicit test shell; spawn itself never invokes a shell.
        Options {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: None,
            env: BTreeMap::new(),
            inherit_env: true,
            stdin: false,
            timeout_ms: 5000,
        }
    }

    fn scope() -> Arc<Scope> {
        Scope::new(std::env::temp_dir())
    }

    fn field<'a>(reply: &'a Reply, name: &str) -> &'a Reply {
        let Reply::Map(fields) = reply else {
            panic!("expected map")
        };
        fields
            .get(name)
            .unwrap_or_else(|| panic!("missing field: {name}"))
    }

    fn process(reply: &Reply) -> &Arc<Process> {
        let Reply::Handle(Resource::Process(process)) = field(reply, "handle") else {
            panic!("expected process")
        };
        process
    }

    fn bytes<'a>(reply: &'a Reply, name: &str) -> &'a [u8] {
        let Reply::Bytes(bytes) = field(reply, name) else {
            panic!("expected bytes")
        };
        bytes
    }

    #[tokio::test]
    async fn separate_output_nonzero_and_repeat_wait() {
        let reply = spawn(
            scope(),
            options("printf hello; printf error >&2; exit 7"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(field(&reply, "stdin"), Reply::Nil));
        let output = process(&reply).collect(1024).await.unwrap();
        assert_eq!(bytes(&output, "stdout"), b"hello");
        assert_eq!(bytes(&output, "stderr"), b"error");
        assert!(matches!(field(&output, "exitCode"), Reply::Number(7)));
        assert!(matches!(field(&output, "signal"), Reply::Nil));
        let again = process(&reply).wait().await.unwrap();
        assert!(matches!(field(&again, "exitCode"), Reply::Number(7)));
        process(&reply).kill(); // A post-reap kill must be harmless.
    }

    #[tokio::test]
    async fn collect_and_manual_reads_are_mutually_exclusive() {
        let mut opts = options("printf preserved; cat");
        opts.stdin = true;
        let reply = spawn(scope(), opts, CancellationToken::new())
            .await
            .unwrap();
        let process = process(&reply);
        let Reply::Handle(Resource::Writer(stdin)) = field(&reply, "stdin") else {
            panic!("missing stdin")
        };

        // Poll once to acquire stderr's exclusive reader without relying on sleeps.
        let mut read = Box::pin(process.stderr.read(1));
        tokio::select! {
            biased;
            result = &mut read => panic!("stderr unexpectedly completed: {result:?}"),
            () = std::future::ready(()) => {}
        }
        let error = process.collect(1024).await.err().unwrap();
        assert_eq!(error.kind, "busy");
        // Failure to acquire stderr must release stdout without consuming it.
        assert!(process.stdout.exclusive().is_ok());
        drop(read);

        let mut collect = Box::pin(process.collect(1024));
        tokio::select! {
            biased;
            _ = &mut collect => panic!("collect unexpectedly completed"),
            () = std::future::ready(()) => {}
        }
        assert_eq!(process.stdout.read(1).await.err().unwrap().kind, "busy");
        assert_eq!(process.stderr.read(1).await.err().unwrap().kind, "busy");
        assert_eq!(process.collect(1024).await.err().unwrap().kind, "busy");
        stdin.close().await.unwrap();
        let output = tokio::time::timeout(Duration::from_secs(3), collect)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bytes(&output, "stdout"), b"preserved");
    }

    #[tokio::test]
    async fn completed_drained_handles_do_not_exhaust_resource_budget() {
        let scope = scope();
        let capacity = scope.budget.available_permits();
        let mut retained = Vec::new();
        for _ in 0..capacity + 1 {
            let reply = spawn(
                scope.clone(),
                options("printf done"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            let output = process(&reply).collect(1024).await.unwrap();
            assert_eq!(bytes(&output, "stdout"), b"done");
            retained.push(reply);
        }
        assert_eq!(scope.budget.available_permits(), capacity);
        assert_eq!(retained.len(), capacity + 1);
    }

    #[tokio::test]
    async fn open_stdin_retains_budget_until_closed() {
        let scope = scope();
        let capacity = scope.budget.available_permits();
        let mut opts = options("printf done");
        opts.stdin = true;
        let reply = spawn(scope.clone(), opts, CancellationToken::new())
            .await
            .unwrap();
        process(&reply).collect(1024).await.unwrap();
        assert_eq!(scope.budget.available_permits(), capacity - 1);
        let Reply::Handle(Resource::Writer(stdin)) = field(&reply, "stdin") else {
            panic!("missing stdin")
        };
        stdin.close().await.unwrap();
        assert_eq!(scope.budget.available_permits(), capacity);
    }

    #[tokio::test]
    async fn stdin_round_trip() {
        let mut opts = options("cat");
        opts.stdin = true;
        let reply = spawn(scope(), opts, CancellationToken::new())
            .await
            .unwrap();
        let Reply::Handle(Resource::Writer(stdin)) = field(&reply, "stdin") else {
            panic!("missing stdin")
        };
        stdin.write(b"a\0\xffb").await.unwrap();
        stdin.close().await.unwrap();
        let output = process(&reply).collect(1024).await.unwrap();
        assert_eq!(bytes(&output, "stdout"), b"a\0\xffb");
    }

    #[tokio::test]
    async fn large_pipes_are_drained_concurrently() {
        let reply = spawn(scope(), options(
            "dd if=/dev/zero bs=65536 count=8 2>/dev/null & dd if=/dev/zero bs=65536 count=8 >&2 2>/dev/null; wait"
        ), CancellationToken::new()).await.unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            process(&reply).collect(1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(bytes(&output, "stdout").len(), 512 * 1024);
        assert_eq!(bytes(&output, "stderr").len(), 512 * 1024);
    }

    #[tokio::test]
    async fn output_budget_is_combined_and_kills_on_overflow() {
        let reply = spawn(
            scope(),
            options("printf 123456; printf 123456 >&2; sleep 20"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), process(&reply).collect(10))
            .await
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(error.kind, "limit");
        let status = process(&reply).wait().await.unwrap();
        assert!(matches!(field(&status, "signal"), Reply::Number(9)));
    }

    #[tokio::test]
    async fn timeout_without_readers_reaps_direct_child() {
        let mut opts = options("printf '%s\\n' $$; exec sleep 20");
        opts.timeout_ms = 200;
        let reply = spawn(scope(), opts, CancellationToken::new())
            .await
            .unwrap();
        let pid = {
            let line = process(&reply).stdout.read(64).await.unwrap().unwrap();
            String::from_utf8(line)
                .unwrap()
                .trim()
                .parse::<i32>()
                .unwrap()
        };
        let error = tokio::time::timeout(Duration::from_secs(3), process(&reply).wait())
            .await
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(error.kind, "timeout");
        assert_eq!(
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(pid),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG)
            ),
            Err(nix::errno::Errno::ECHILD)
        );
    }

    #[tokio::test]
    async fn operation_and_scope_cancellation_clean_up() {
        for cancel_scope in [false, true] {
            let scope = scope();
            let cancel = CancellationToken::new();
            let reply = spawn(scope.clone(), options("exec sleep 20"), cancel.clone())
                .await
                .unwrap();
            if cancel_scope {
                scope.cancel.cancel();
            } else {
                cancel.cancel();
            }
            let error = tokio::time::timeout(Duration::from_secs(3), process(&reply).wait())
                .await
                .unwrap()
                .err()
                .unwrap();
            assert_eq!(error.kind, "cancelled");
        }
    }

    #[tokio::test]
    async fn kill_reaches_descendants_holding_pipes() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("escaped");
        let script = format!(
            "(sleep 0.3; printf escaped > '{}') & printf ready; wait",
            marker.display()
        );
        let reply = spawn(scope(), options(&script), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            process(&reply).stdout.read(5).await.unwrap().unwrap(),
            b"ready"
        );
        process(&reply).kill();
        let status = tokio::time::timeout(Duration::from_secs(3), process(&reply).wait())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(field(&status, "signal"), Reply::Number(9)));
        assert!(process(&reply).stdout.read(1).await.is_err());
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !marker.exists(),
            "a descendant survived process-group cancellation"
        );
    }

    #[tokio::test]
    async fn dropping_resource_kills_and_reaps() {
        let reply = spawn(scope(), options("exec sleep 20"), CancellationToken::new())
            .await
            .unwrap();
        let process = process(&reply).clone();
        drop(reply);
        let status = tokio::time::timeout(Duration::from_secs(3), process.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(field(&status, "signal"), Reply::Number(9)));
    }

    #[tokio::test]
    async fn cwd_environment_and_literal_arguments() {
        let root = tempfile::tempdir().unwrap();
        let mut opts = options("printf '%s:%s:%s' \"$PWD\" \"$PANE_PROCESS_TEST\" \"$1\"");
        opts.args
            .extend(["test".into(), "$(echo not-expanded)".into()]);
        opts.inherit_env = false;
        opts.env.insert("PANE_PROCESS_TEST".into(), "value".into());
        let canonical_root = root.path().canonicalize().unwrap();
        let reply = spawn(
            Scope::new(canonical_root.clone()),
            opts,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let output = process(&reply).collect(4096).await.unwrap();
        assert_eq!(
            bytes(&output, "stdout"),
            format!("{}:value:$(echo not-expanded)", canonical_root.display()).as_bytes()
        );
    }
}
