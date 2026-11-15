//! Streaming HTTP for trusted scripts. Status codes (including 4xx/5xx) are data,
//! while transport failures, deadlines, and byte-budget exhaustion are errors.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::{Instant, Sleep};
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

use super::streams::Reader;
use super::{Error, Reply, Resource, Result, Scope};

pub(super) struct Options {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: u64,
    pub max_body_bytes: usize,
}

fn client() -> Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<std::result::Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
        })
        .as_ref()
        .map_err(|error| Error::new("io", error.to_string()))
}

fn transport_error(error: reqwest::Error) -> Error {
    Error::new(
        if error.is_timeout() { "timeout" } else { "io" },
        error.to_string(),
    )
}

// StreamReader may buffer part of a chunk between script reads. Check the total
// deadline outside that buffer too, rather than only when reqwest is polled.
struct DeadlineReader<R> {
    inner: R,
    deadline: Pin<Box<Sleep>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for DeadlineReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::other(Error::new(
                "timeout",
                "HTTP request timed out",
            ))));
        }
        Pin::new(&mut this.inner).poll_read(cx, buffer)
    }
}

pub async fn request(
    scope: Arc<Scope>,
    options: Options,
    cancel: CancellationToken,
) -> Result<Reply> {
    let url = reqwest::Url::parse(&options.url)
        .map_err(|error| Error::new("invalid_argument", error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::new("invalid_argument", "URL must use HTTP or HTTPS"));
    }
    if options.timeout_ms == 0 || options.max_body_bytes == 0 {
        return Err(Error::new(
            "invalid_argument",
            "timeoutMs and maxBodyBytes must be positive",
        ));
    }
    let method = reqwest::Method::from_bytes(options.method.as_bytes())
        .map_err(|error| Error::new("invalid_argument", error.to_string()))?;
    let mut headers = HeaderMap::new();
    for (name, value) in options.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| Error::new("invalid_argument", error.to_string()))?;
        let value = HeaderValue::from_bytes(value.as_bytes())
            .map_err(|error| Error::new("invalid_argument", error.to_string()))?;
        headers.append(name, value);
    }

    // Reserve before sending: requests, including their open bodies, count against
    // the same resource budget as files and process pipes.
    let permit = scope.reserve()?;
    let mut request = client()?
        .request(method, url)
        .headers(headers)
        // Reqwest's request timeout covers redirects and the entire response body,
        // not just waiting for headers or a fresh timeout for every read.
        .timeout(Duration::from_millis(options.timeout_ms));
    if let Some(body) = options.body {
        request = request.body(body);
    }
    let deadline = Instant::now() + Duration::from_millis(options.timeout_ms);
    let response = request.send().await.map_err(transport_error)?;
    let status = i64::from(response.status().as_u16());
    let url = response.url().to_string();
    let headers = response
        .headers()
        .keys()
        .map(|name| {
            let values = response
                .headers()
                .get_all(name)
                .iter()
                // Header values need not be UTF-8. Preserve them just like bodies.
                .map(|value| Reply::Bytes(value.as_bytes().to_vec()))
                .collect();
            (name.as_str().to_owned(), Reply::List(values))
        })
        .collect::<BTreeMap<_, _>>();
    let mut remaining = options.max_body_bytes;
    let stream = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(|error| io::Error::other(transport_error(error)))?;
        if chunk.len() > remaining {
            return Err(io::Error::other(Error::new(
                "limit",
                "HTTP response exceeds maxBodyBytes",
            )));
        }
        remaining -= chunk.len();
        Ok(chunk)
    });
    // The owned stream keeps the response alive. Closing/dropping Reader drops
    // the stream and hence the connection; child cancellation stays local.
    let body = Reader::new(
        DeadlineReader {
            inner: StreamReader::new(Box::pin(stream)),
            deadline: Box::pin(tokio::time::sleep_until(deadline)),
        },
        cancel.child_token(),
        Some(permit),
    );
    Ok(Reply::map([
        ("status", Reply::Number(status)),
        ("url", Reply::Text(url)),
        ("headers", Reply::Map(headers)),
        ("body", Reply::Handle(Resource::Reader(body))),
    ]))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    struct Server {
        url: String,
        task: JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    // A raw local server makes repeated headers, invalid UTF-8, chunking and
    // unfinished bodies explicit, without external network calls or extra deps.
    async fn server(response: &'static [u8], delay: Duration, hold: bool) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if socket.read(&mut byte).await.unwrap() == 0 {
                        break;
                    }
                    request.extend_from_slice(&byte);
                }
                tokio::time::sleep(delay).await;
                if socket.write_all(response).await.is_err() {
                    continue;
                }
                if hold {
                    std::future::pending::<()>().await;
                }
            }
        });
        Server { url, task }
    }

    fn options(url: &str) -> Options {
        Options {
            url: url.to_owned(),
            method: "GET".into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: 5_000,
            max_body_bytes: 1024,
        }
    }

    async fn send(options: Options) -> Result<Reply> {
        request(
            Scope::new(std::env::temp_dir()),
            options,
            CancellationToken::new(),
        )
        .await
    }

    fn fields(reply: Reply) -> BTreeMap<String, Reply> {
        match reply {
            Reply::Map(fields) => fields,
            _ => panic!("expected response map"),
        }
    }

    fn body(fields: &mut BTreeMap<String, Reply>) -> Arc<Reader> {
        match fields.get("body").unwrap() {
            Reply::Handle(Resource::Reader(reader)) => reader.clone(),
            _ => panic!("expected body reader"),
        }
    }

    #[tokio::test]
    async fn status_repeated_headers_and_binary_body() {
        let server = server(
            b"HTTP/1.1 201 Created\r\nContent-Length: 4\r\nX-Test: one\r\nX-Test: two\r\nX-Raw: \xff\r\nConnection: close\r\n\r\na\0\xffb",
            Duration::ZERO,
            false,
        )
        .await;
        let mut reply = fields(send(options(&server.url)).await.unwrap());
        assert!(matches!(reply.get("status"), Some(Reply::Number(201))));
        assert!(
            matches!(reply.get("url"), Some(Reply::Text(url)) if url == &format!("{}/", server.url))
        );
        let headers = fields(reply.remove("headers").unwrap());
        let Some(Reply::List(values)) = headers.get("x-test") else {
            panic!("expected repeated header array");
        };
        assert_eq!(values.len(), 2);
        assert!(matches!(&values[0], Reply::Bytes(bytes) if bytes == b"one"));
        assert!(matches!(&values[1], Reply::Bytes(bytes) if bytes == b"two"));
        assert!(
            matches!(headers.get("x-raw"), Some(Reply::List(values)) if matches!(&values[0], Reply::Bytes(bytes) if bytes == b"\xff"))
        );
        assert_eq!(body(&mut reply).read_all(1024).await.unwrap(), b"a\0\xffb");
    }

    #[tokio::test]
    async fn error_status_is_a_response() {
        let server = server(
            b"HTTP/1.1 503 Unavailable\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad",
            Duration::ZERO,
            false,
        )
        .await;
        let mut reply = fields(send(options(&server.url)).await.unwrap());
        assert!(matches!(reply.get("status"), Some(Reply::Number(503))));
        assert_eq!(body(&mut reply).read_all(1024).await.unwrap(), b"bad");
    }

    #[tokio::test]
    async fn headers_and_chunks_arrive_before_eof() {
        let server = server(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n",
            Duration::ZERO,
            true,
        )
        .await;
        let mut reply = fields(
            tokio::time::timeout(Duration::from_secs(1), send(options(&server.url)))
                .await
                .unwrap()
                .unwrap(),
        );
        let reader = body(&mut reply);
        let chunk = tokio::time::timeout(Duration::from_secs(1), reader.read(3))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(chunk, Some(b"abc".to_vec()));
        reader.close();
    }

    #[tokio::test]
    async fn timeout_before_headers() {
        let server = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            Duration::from_secs(1),
            false,
        )
        .await;
        let mut opts = options(&server.url);
        opts.timeout_ms = 50;
        let error = send(opts).await.err().unwrap();
        assert_eq!(error.kind, "timeout");
    }

    #[tokio::test]
    async fn total_timeout_includes_stream_and_time_between_reads() {
        let server = server(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n",
            Duration::ZERO,
            true,
        )
        .await;
        let mut opts = options(&server.url);
        opts.timeout_ms = 200;
        let mut reply = fields(send(opts).await.unwrap());
        let reader = body(&mut reply);
        assert_eq!(reader.read(1).await.unwrap(), Some(b"a".to_vec()));
        tokio::time::sleep(Duration::from_millis(250)).await;
        let error = tokio::time::timeout(Duration::from_secs(1), reader.read(1))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind, "timeout");
    }

    #[tokio::test]
    async fn total_timeout_also_covers_buffered_chunks() {
        let server = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd",
            Duration::ZERO,
            false,
        )
        .await;
        let mut opts = options(&server.url);
        opts.timeout_ms = 200;
        let mut reply = fields(send(opts).await.unwrap());
        let reader = body(&mut reply);
        assert_eq!(reader.read(1).await.unwrap(), Some(b"a".to_vec()));
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(reader.read(1).await.unwrap_err().kind, "timeout");
    }

    #[tokio::test]
    async fn chunked_body_limit_applies_to_reads_and_collection() {
        let server = server(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
            Duration::ZERO,
            false,
        )
        .await;
        for collect in [false, true] {
            let mut opts = options(&server.url);
            opts.max_body_bytes = 5;
            let mut reply = fields(send(opts).await.unwrap());
            let reader = body(&mut reply);
            let error = if collect {
                reader.read_all(1024).await.unwrap_err()
            } else {
                loop {
                    match reader.read(1).await {
                        Ok(Some(_)) => {}
                        Ok(None) => panic!("oversized body reached EOF"),
                        Err(error) => break error,
                    }
                }
            };
            assert_eq!(error.kind, "limit");
        }
        let mut opts = options(&server.url);
        opts.max_body_bytes = 6;
        let mut reply = fields(send(opts).await.unwrap());
        assert_eq!(body(&mut reply).read_all(6).await.unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn follows_redirects_and_caps_redirect_loops() {
        let destination = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            Duration::ZERO,
            false,
        )
        .await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let location = destination.url.clone();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut incoming = [0; 4096];
            assert!(socket.read(&mut incoming).await.unwrap() > 0);
            socket
                .write_all(
                    format!("HTTP/1.1 302 Found\r\nLocation: {location}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
        });
        let redirect = Server { url, task };
        let mut reply = fields(send(options(&redirect.url)).await.unwrap());
        assert!(
            matches!(reply.get("url"), Some(Reply::Text(url)) if url == &format!("{}/final", destination.url))
        );
        assert_eq!(body(&mut reply).read_all(1024).await.unwrap(), b"ok");

        let looping = server(
            b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            Duration::ZERO,
            false,
        )
        .await;
        assert_eq!(send(options(&looping.url)).await.err().unwrap().kind, "io");
    }

    #[tokio::test]
    async fn rejects_non_http_urls_and_invalid_headers() {
        assert_eq!(
            send(options("file:///etc/passwd"))
                .await
                .err()
                .unwrap()
                .kind,
            "invalid_argument"
        );
        let mut opts = options("http://127.0.0.1:1");
        opts.headers.push(("X-Test".into(), "bad\r\nheader".into()));
        assert_eq!(send(opts).await.err().unwrap().kind, "invalid_argument");
    }
}
