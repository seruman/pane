use std::{
    fs::{self, DirBuilder},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const MAX_REQUEST: usize = 64 * 1024;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// All socket I/O runs on the worker. The handler must return within two seconds,
/// including when the UI is not polling, so dropping this server is bounded.
pub(super) struct Server {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    _socket: SocketFile,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            drop(worker.join());
        }
    }
}

struct SocketFile {
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        // Never remove a replacement file or somebody else's stale socket.
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && metadata.dev() == self.dev
            && metadata.ino() == self.ino
        {
            drop(fs::remove_file(&self.path));
        }
    }
}

pub(super) fn serve(
    dir: &Path,
    handler: impl Fn(Value) -> Result<Value, String> + Send + 'static,
) -> Result<Server, String> {
    // Strip trailing separators and `.` so lstat cannot follow a final symlink.
    let dir: PathBuf = dir.components().collect();
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create inspection directory: {error}")),
    }
    let metadata = fs::symlink_metadata(&dir)
        .map_err(|error| format!("inspect inspection directory: {error}"))?;
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let euid = unsafe { nix::libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != euid || metadata.mode() & 0o7777 != 0o700 {
        return Err("inspection directory must be a non-symlink directory owned by the current user with mode 0700".into());
    }

    // Retain an absolute path so cleanup does not depend on a later cwd change.
    let dir =
        fs::canonicalize(&dir).map_err(|error| format!("resolve inspection directory: {error}"))?;
    let path = dir.join("control.sock");
    // Some Unix implementations follow a dangling symlink when binding. Reject every
    // existing directory entry explicitly, not just entries bind considers in use.
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect socket path: {error}")),
        Ok(_) => {
            return Err(format!(
                "inspection socket path already exists: {}",
                path.display()
            ));
        }
    }
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("bind inspection socket {}: {error}", path.display()))?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("inspect inspection socket: {error}"))?;
    let socket = SocketFile {
        path,
        dev: metadata.dev(),
        ino: metadata.ino(),
    };
    // The directory is already private while the socket permissions are tightened.
    fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("secure inspection socket: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("configure inspection listener: {error}"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("pane-inspect".into())
        .spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if stream.set_nonblocking(true).is_ok() {
                            handle_client(&mut stream, &worker_stop, &handler);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(POLL_INTERVAL);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })
        .map_err(|error| format!("start inspection worker: {error}"))?;
    Ok(Server {
        stop,
        worker: Some(worker),
        _socket: socket,
    })
}

fn check_deadline(stop: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        Err("inspection server is stopping".into())
    } else if Instant::now() >= deadline {
        Err("inspection I/O deadline exceeded".into())
    } else {
        Ok(())
    }
}

fn wait(deadline: Instant) {
    thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
}

fn read_request(
    stream: &mut UnixStream,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<Value, String> {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        check_deadline(stop, deadline)?;
        match stream.read(&mut chunk) {
            Ok(0) => return Err("request disconnected before newline".into()),
            Ok(count) => {
                let newline = chunk[..count].iter().position(|byte| *byte == b'\n');
                let used = newline.map_or(count, |index| index + 1);
                if request.len() + used > MAX_REQUEST {
                    return Err("request exceeds 64 KiB".into());
                }
                request.extend_from_slice(&chunk[..used]);
                if newline.is_some() {
                    check_deadline(stop, deadline)?;
                    return serde_json::from_slice(&request)
                        .map_err(|error| format!("invalid JSON request: {error}"));
                }
                if request.len() == MAX_REQUEST {
                    return Err("request exceeds 64 KiB (including newline)".into());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait(deadline),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("read inspection request: {error}")),
        }
    }
}

// Bound allocation during serialization, rather than building an unlimited string
// and checking its length afterward. Reserve one byte for the NDJSON delimiter.
struct LimitedResponse(Vec<u8>);

impl Write for LimitedResponse {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > (MAX_RESPONSE - 1).saturating_sub(self.0.len()) {
            return Err(io::Error::other("response exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_response(result: Result<Value, String>) -> Vec<u8> {
    let envelope = match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    let mut response = LimitedResponse(Vec::new());
    if serde_json::to_writer(&mut response, &envelope).is_err() {
        return b"{\"ok\":false,\"error\":\"response exceeds 4 MiB\"}\n".to_vec();
    }
    response.0.push(b'\n');
    response.0
}

fn write_response(
    stream: &mut UnixStream,
    mut response: &[u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), String> {
    while !response.is_empty() {
        check_deadline(stop, deadline)?;
        match stream.write(response) {
            Ok(0) => return Err("response disconnected".into()),
            Ok(count) => response = &response[count..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait(deadline),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("write inspection response: {error}")),
        }
    }
    check_deadline(stop, deadline)
}

fn handle_client(
    stream: &mut UnixStream,
    stop: &AtomicBool,
    handler: &impl Fn(Value) -> Result<Value, String>,
) {
    let result = read_request(stream, stop, Instant::now() + IO_TIMEOUT).and_then(|request| {
        check_deadline(stop, Instant::now() + IO_TIMEOUT)?;
        handler(request)
    });
    if !stop.load(Ordering::Acquire) {
        let response = encode_response(result);
        drop(write_response(
            stream,
            &response,
            stop,
            Instant::now() + IO_TIMEOUT,
        ));
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
