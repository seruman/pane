use super::*;
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::symlink,
    sync::{atomic::AtomicUsize, mpsc},
};

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn connect(dir: &Path) -> UnixStream {
    let stream = UnixStream::connect(dir.join("control.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    stream
}

fn reply(stream: UnixStream) -> Value {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn request(dir: &Path, bytes: &[u8]) -> Value {
    let mut stream = connect(dir);
    stream.write_all(bytes).unwrap();
    reply(stream)
}

#[test]
fn creates_private_directory_and_socket_and_cleans_up() {
    let root = private_dir();
    let dir = root.path().join("inspection");
    let server = serve(&dir, Ok).unwrap();
    let metadata = fs::symlink_metadata(&dir).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o700);
    // SAFETY: geteuid has no preconditions.
    assert_eq!(metadata.uid(), unsafe { nix::libc::geteuid() });
    let socket = fs::symlink_metadata(dir.join("control.sock")).unwrap();
    assert!(socket.file_type().is_socket());
    assert_eq!(socket.mode() & 0o7777, 0o600);
    drop(server);
    assert!(!dir.join("control.sock").exists());
    assert!(dir.is_dir());
}

#[test]
fn refuses_nonprivate_directory_without_changing_it() {
    for mode in [0o755, 0o750, 0o770, 0o1700] {
        let dir = private_dir();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).unwrap();
        assert!(serve(dir.path(), Ok).is_err());
        assert_eq!(fs::metadata(dir.path()).unwrap().mode() & 0o7777, mode);
        assert!(!dir.path().join("control.sock").exists());
    }
}

#[test]
fn refuses_symlink_directory_or_regular_file() {
    let root = private_dir();
    let dir = private_dir();
    let link = root.path().join("link");
    symlink(dir.path(), &link).unwrap();
    assert!(serve(&link, Ok).is_err());
    assert!(serve(&link.join(""), Ok).is_err());
    assert!(!dir.path().join("control.sock").exists());
    let file = root.path().join("file");
    fs::write(&file, "untouched").unwrap();
    assert!(serve(&file, Ok).is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "untouched");
}

#[test]
fn refuses_wrong_owner_when_chown_is_available() {
    // Only root can create a directory owned by another user for this test.
    // SAFETY: geteuid has no preconditions.
    if unsafe { nix::libc::geteuid() } != 0 {
        return;
    }
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let dir = private_dir();
    let path = CString::new(dir.path().as_os_str().as_bytes()).unwrap();
    // SAFETY: path is a valid, NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { nix::libc::chown(path.as_ptr(), 1, 1) }, 0);
    assert!(serve(dir.path(), Ok).is_err());
    assert!(!dir.path().join("control.sock").exists());
}

#[test]
fn refuses_existing_socket_file_and_symlink_without_unlinking() {
    let dir = private_dir();
    let path = dir.path().join("control.sock");
    let stale = UnixListener::bind(&path).unwrap();
    let ino = fs::symlink_metadata(&path).unwrap().ino();
    drop(stale);
    assert!(serve(dir.path(), Ok).is_err());
    assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), ino);
    fs::remove_file(&path).unwrap();
    fs::write(&path, "keep me").unwrap();
    assert!(serve(dir.path(), Ok).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");
    fs::remove_file(&path).unwrap();
    symlink("missing", &path).unwrap();
    assert!(serve(dir.path(), Ok).is_err());
    assert_eq!(fs::read_link(&path).unwrap(), Path::new("missing"));
}

#[test]
fn shutdown_preserves_replacement_socket() {
    let dir = private_dir();
    let server = serve(dir.path(), Ok).unwrap();
    let path = dir.path().join("control.sock");
    fs::rename(&path, dir.path().join("original.sock")).unwrap();
    let replacement = UnixListener::bind(&path).unwrap();
    let ino = fs::symlink_metadata(&path).unwrap().ino();
    drop(server);
    assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), ino);
    drop(replacement);
}

#[test]
fn calls_handler_off_thread_and_returns_success_and_error_envelopes() {
    let dir = private_dir();
    let caller = thread::current().id();
    let server = serve(dir.path(), move |value| {
        assert_ne!(thread::current().id(), caller);
        if value == json!({"command": "fail"}) {
            Err("handler failed".into())
        } else {
            Ok(value)
        }
    })
    .unwrap();
    assert_eq!(
        request(dir.path(), b"{\"command\":\"info\"}\n"),
        json!({"ok": true, "result": {"command": "info"}})
    );
    assert_eq!(
        request(dir.path(), b"{\"command\":\"fail\"}\n"),
        json!({"ok": false, "error": "handler failed"})
    );
    drop(server);
}

#[test]
fn malformed_and_disconnected_clients_do_not_call_handler_or_kill_server() {
    let dir = private_dir();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = Arc::clone(&calls);
    let _server = serve(dir.path(), move |value| {
        handler_calls.fetch_add(1, Ordering::SeqCst);
        Ok(value)
    })
    .unwrap();
    for bytes in [b"nope\n".as_slice(), b"\n", b"\xff\n", b"{} {}\n"] {
        let response = request(dir.path(), bytes);
        assert_eq!(response["ok"], false);
        assert!(response["error"].as_str().unwrap().contains("invalid JSON"));
    }
    let mut stream = connect(dir.path());
    stream.write_all(b"{\"partial\":").unwrap();
    drop(stream);
    assert_eq!(request(dir.path(), b"null\n")["ok"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn accepts_exact_request_limit_and_rejects_oversize() {
    let dir = private_dir();
    let _server = serve(dir.path(), Ok).unwrap();
    let mut bytes = vec![b' '; MAX_REQUEST];
    bytes[..4].copy_from_slice(b"null");
    bytes[MAX_REQUEST - 1] = b'\n';
    assert_eq!(request(dir.path(), &bytes)["ok"], true);
    bytes[MAX_REQUEST - 1] = b' ';
    bytes.push(b'\n');
    let mut stream = connect(dir.path());
    // An oversize request may be rejected before the client finishes writing.
    drop(stream.write_all(&bytes));
    let response = reply(stream);
    assert_eq!(response["ok"], false);
    assert!(response["error"].as_str().unwrap().contains("64 KiB"));
    assert_eq!(request(dir.path(), b"null\n")["ok"], true);
}

#[test]
fn oversize_response_becomes_small_error_and_server_recovers() {
    let dir = private_dir();
    let _server = serve(dir.path(), |value| {
        if value == true {
            Ok(Value::String("x".repeat(MAX_RESPONSE)))
        } else {
            Ok(value)
        }
    })
    .unwrap();
    let response = request(dir.path(), b"true\n");
    assert_eq!(response["ok"], false);
    assert!(response["error"].as_str().unwrap().contains("4 MiB"));
    assert_eq!(request(dir.path(), b"null\n")["ok"], true);
}

#[test]
fn response_limit_includes_envelope_and_newline() {
    let overhead = encode_response(Ok(json!(""))).len();
    assert_eq!(
        encode_response(Ok(json!("x".repeat(MAX_RESPONSE - overhead)))).len(),
        MAX_RESPONSE
    );
    let response = encode_response(Ok(json!("x".repeat(MAX_RESPONSE - overhead + 1))));
    let value: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(value["ok"], false);
}

#[test]
fn slow_byte_dribble_cannot_extend_total_read_deadline() {
    let (mut reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    let finished = Arc::new(AtomicBool::new(false));
    let writer_finished = Arc::clone(&finished);
    let worker = thread::spawn(move || {
        while !writer_finished.load(Ordering::Acquire) {
            if writer.write_all(b" ").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let start = Instant::now();
    let error = read_request(
        &mut reader,
        &AtomicBool::new(false),
        start + Duration::from_millis(150),
    )
    .unwrap_err();
    finished.store(true, Ordering::Release);
    worker.join().unwrap();
    assert!(error.contains("deadline"));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn slow_reader_cannot_extend_total_write_deadline() {
    let (mut writer, mut reader) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    reader.set_nonblocking(true).unwrap();
    let finished = Arc::new(AtomicBool::new(false));
    let reader_finished = Arc::clone(&finished);
    let worker = thread::spawn(move || {
        while !reader_finished.load(Ordering::Acquire) {
            drop(reader.read(&mut [0; 1]));
            thread::sleep(Duration::from_millis(10));
        }
    });
    let start = Instant::now();
    let error = write_response(
        &mut writer,
        &vec![b'x'; MAX_RESPONSE],
        &AtomicBool::new(false),
        start + Duration::from_millis(150),
    )
    .unwrap_err();
    finished.store(true, Ordering::Release);
    worker.join().unwrap();
    assert!(error.contains("deadline"));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn idle_and_partial_request_shutdown_are_prompt() {
    for partial in [false, true] {
        let dir = private_dir();
        let server = serve(dir.path(), Ok).unwrap();
        let mut client = connect(dir.path());
        if partial {
            client.write_all(b"{\"unfinished\":").unwrap();
        }
        thread::sleep(Duration::from_millis(20));
        let start = Instant::now();
        drop(server);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!dir.path().join("control.sock").exists());
    }
}

#[test]
fn shutdown_during_handler_waits_only_for_bounded_handler() {
    let dir = private_dir();
    let (entered_tx, entered_rx) = mpsc::channel();
    let server = serve(dir.path(), move |value| {
        entered_tx.send(()).unwrap();
        thread::sleep(Duration::from_millis(150));
        Ok(value)
    })
    .unwrap();
    let mut client = connect(dir.path());
    client.write_all(b"null\n").unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn shutdown_interrupts_blocked_response_write() {
    let dir = private_dir();
    let (entered_tx, entered_rx) = mpsc::channel();
    let server = serve(dir.path(), move |_| {
        entered_tx.send(()).unwrap();
        Ok(json!("x".repeat(MAX_RESPONSE - 100)))
    })
    .unwrap();
    let mut client = connect(dir.path());
    client.write_all(b"null\n").unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(1));
}
