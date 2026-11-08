//! argv commands in their own process group. Port of `Sources/MenuCore/CommandRunner.swift`.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Exit code, or 128 + signal number.
    pub status: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    InvalidArguments,
    System { operation: String, code: i32 },
    TimedOut,
    OutputLimitExceeded,
    Cancelled,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::InvalidArguments => f.write_str("Invalid command arguments or environment"),
            CommandError::System { operation, code } =>
                write!(f, "{operation}: {} ({code})", io::Error::from_raw_os_error(*code)),
            CommandError::TimedOut => f.write_str("Command timed out"),
            CommandError::OutputLimitExceeded => f.write_str("Command exceeded the combined output limit"),
            CommandError::Cancelled => f.write_str("Command cancelled"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Cancels a running command from another thread; the command's whole process group is killed
/// and reaped before `run` returns. Cancelling before `run` starts means nothing is spawned.
#[derive(Clone, Debug, Default)]
pub struct Cancel {
    cancelled: Arc<AtomicBool>,
}

impl Cancel {
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Relaxed); }
    pub fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::Relaxed) }
}

/// Bounds how long a cancel waits when no stream is active.
const POLL_INTERVAL_MS: i32 = 10;

fn system(operation: &str, error: io::Error) -> CommandError {
    CommandError::System { operation: operation.to_owned(), code: error.raw_os_error().unwrap_or(libc::EIO) }
}

fn set_nonblocking(fd: RawFd) -> Result<(), CommandError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
        return Err(system("nonblocking pipe", io::Error::last_os_error()));
    }
    Ok(())
}

/// Runs `argv` without a shell, `env` overlaid on ours, PATH resolved from the overlay.
/// `input: None` connects stdin to /dev/null: tools such as glow or bat prefer a piped stdin
/// over their file arguments. The limit is shared by stdout and stderr. Blocks the calling thread.
pub fn run(argv: &[String], input: Option<&[u8]>, env: &[(String, String)], timeout: Duration,
           output_limit: usize, cancel: &Cancel) -> Result<CommandResult, CommandError> {
    if cancel.is_cancelled() { return Err(CommandError::Cancelled); }
    let has_nul = |s: &str| s.contains('\0');
    if argv.first().is_none_or(|program| program.is_empty())
        || argv.iter().any(|a| has_nul(a))
        || env.iter().any(|(k, v)| k.is_empty() || k.contains('=') || has_nul(k) || has_nul(v)) {
        return Err(CommandError::InvalidArguments);
    }
    let deadline = Instant::now() + timeout;
    let child = Command::new(&argv[0])
        .args(&argv[1..])
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| system("posix_spawn", e))?;
    let mut group = Group { child, reaped: false };
    let result = pump(&mut group.child, input.unwrap_or_default(), deadline, output_limit, cancel)?;
    group.reaped = true;
    Ok(result)
}

/// Kills and reaps the whole process group unless the leader was reaped after a clean finish.
/// The leader stays unreaped until then, so its PID still names the group.
struct Group {
    child: Child,
    reaped: bool,
}

impl Drop for Group {
    fn drop(&mut self) {
        if self.reaped { return; }
        // SIGKILL also handles children that ignore TERM. Descendants that setsid are not ours.
        unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

fn pump(child: &mut Child, input: &[u8], deadline: Instant, limit: usize, cancel: &Cancel)
        -> Result<CommandResult, CommandError> {
    let mut stdin = child.stdin.take().filter(|_| !input.is_empty());
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    for fd in [stdin.as_ref().map(|s| s.as_raw_fd()), stdout.as_ref().map(|s| s.as_raw_fd()),
               stderr.as_ref().map(|s| s.as_raw_fd())].into_iter().flatten() {
        set_nonblocking(fd)?;
    }
    let mut written = 0;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut buffer = [0u8; 16_384];
    loop {
        if cancel.is_cancelled() { return Err(CommandError::Cancelled); }
        if Instant::now() >= deadline { return Err(CommandError::TimedOut); }
        if stdin.is_none() && stdout.is_none() && stderr.is_none() {
            if let Some(status) = child.try_wait().map_err(|e| system("waitpid", e))? {
                let status = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
                return Ok(CommandResult { stdout: out, stderr: err, status });
            }
            std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
            continue;
        }
        let mut fds: Vec<libc::pollfd> = [
            stdout.as_ref().map(|s| (s.as_raw_fd(), libc::POLLIN)),
            stderr.as_ref().map(|s| (s.as_raw_fd(), libc::POLLIN)),
            stdin.as_ref().map(|s| (s.as_raw_fd(), libc::POLLOUT)),
        ].into_iter().flatten().map(|(fd, events)| libc::pollfd { fd, events, revents: 0 }).collect();
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, POLL_INTERVAL_MS) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted { continue; }
            return Err(system("poll", error));
        }
        for ready in fds.iter().filter(|p| p.revents != 0) {
            if stdin.as_ref().is_some_and(|s| s.as_raw_fd() == ready.fd) {
                let pipe = stdin.as_mut().expect("matched above");
                match pipe.write(&input[written..input.len().min(written + buffer.len())]) {
                    Ok(n) => {
                        written += n;
                        if written == input.len() { stdin = None; }
                    }
                    // Rust's runtime ignores SIGPIPE, so a child that exits early surfaces here.
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => stdin = None,
                    Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(system("write stdin", e)),
                }
                continue;
            }
            let is_stdout = stdout.as_ref().is_some_and(|s| s.as_raw_fd() == ready.fd);
            let read = if is_stdout { stdout.as_mut().map(|s| s.read(&mut buffer)) }
                       else { stderr.as_mut().map(|s| s.read(&mut buffer)) };
            match read.expect("polled descriptors are open") {
                Ok(0) => if is_stdout { stdout = None } else { stderr = None },
                Ok(n) => {
                    if out.len() + err.len() + n > limit { return Err(CommandError::OutputLimitExceeded); }
                    if is_stdout { &mut out } else { &mut err }.extend_from_slice(&buffer[..n]);
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(system("read output", e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn strings(argv: &[&str]) -> Vec<String> { argv.iter().map(|s| s.to_string()).collect() }

    fn run_with(argv: &[&str], input: Option<&[u8]>, env: &[(&str, &str)], timeout: Duration, limit: usize)
                -> Result<CommandResult, CommandError> {
        let env: Vec<_> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        run(&strings(argv), input, &env, timeout, limit, &Cancel::default())
    }

    fn quick(argv: &[&str]) -> Result<CommandResult, CommandError> {
        run_with(argv, Some(b""), &[], Duration::from_secs(10), 1_048_576)
    }

    fn text(bytes: &[u8]) -> &str { std::str::from_utf8(bytes).unwrap() }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("rmenu command {name} {} {n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path.canonicalize().unwrap())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn stdin_and_environment_overlay() {
        let input = b"hello\0world\nwith spaces\n";
        let result = run_with(&["/bin/sh", "-c", "printf '%s\\n' \"$RUNNER_VALUE\"; /bin/cat"], Some(input),
                              &[("RUNNER_VALUE", "a value with spaces")], Duration::from_secs(10), 1 << 20).unwrap();
        assert_eq!(result.stdout, [b"a value with spaces\n".as_slice(), input].concat());
        assert!(result.stderr.is_empty());
        assert_eq!(result.status, 0);
    }

    #[test]
    fn nil_input_connects_stdin_to_dev_null() {
        let check = ["/bin/sh", "-c", "if [ -c /dev/stdin ]; then echo chardev; else echo pipe; fi; /bin/cat"];
        let result = run_with(&check, None, &[], Duration::from_secs(10), 1 << 20).unwrap();
        assert_eq!(text(&result.stdout), "chardev\n");
        assert_eq!(text(&quick(&check).unwrap().stdout), "pipe\n");
    }

    #[test]
    fn directory_and_environment_are_inherited() {
        let result = quick(&["/bin/pwd"]).unwrap();
        let actual = Path::new(text(&result.stdout).trim_end()).canonicalize().unwrap();
        assert_eq!(actual, std::env::current_dir().unwrap().canonicalize().unwrap());
        let env = run_with(&["/usr/bin/env"], Some(b""), &[("RUNNER_OVERLAY", "yes")], Duration::from_secs(10), 1 << 20).unwrap();
        let lines: Vec<&str> = text(&env.stdout).lines().collect();
        assert!(lines.contains(&"RUNNER_OVERLAY=yes"));
        if let Ok(home) = std::env::var("HOME") { assert!(lines.contains(&format!("HOME={home}").as_str())); }
    }

    #[test]
    fn path_overlay_and_literal_arguments() {
        let dir = TempDir::new("path");
        let bin = dir.0.join("bin with spaces");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("/usr/bin/printf", bin.join("literal printer")).unwrap();
        let path = bin.to_str().unwrap();
        let marker = dir.0.join("SHOULD_NOT_EXIST");
        let literal = format!("$(touch '{}'); `echo bad` | * > file & $HOME ' \"", marker.display());
        let result = run_with(&["literal printer", "%s", &literal], Some(b""), &[("PATH", path)],
                              Duration::from_secs(10), 1 << 20).unwrap();
        assert_eq!(text(&result.stdout), literal);
        assert!(!marker.exists());
        let absolute = format!("{path}/literal printer");
        assert_eq!(text(&quick(&[&absolute, "%s", "absolute"]).unwrap().stdout), "absolute");
    }

    #[test]
    fn simultaneous_stdout_stderr_and_large_stdin() {
        // Both writers exceed pipe capacity before stdin is consumed; sequential I/O deadlocks.
        let input: Vec<u8> = (0..400_000u32).map(|i| i as u8).collect();
        let script = "/usr/bin/head -c 200000 /dev/zero &\n/usr/bin/head -c 200000 /dev/zero >&2 &\nwait\n/bin/cat";
        let result = run_with(&["/bin/sh", "-c", script], Some(&input), &[], Duration::from_secs(10), 1 << 20).unwrap();
        assert_eq!(result.stdout, [vec![0; 200_000], input].concat());
        assert_eq!(result.stderr, vec![0; 200_000]);
        assert_eq!(result.status, 0);
    }

    #[test]
    fn nonzero_and_signal_statuses_are_returned() {
        let result = quick(&["/bin/sh", "-c", "printf out; printf err >&2; exit 23"]).unwrap();
        assert_eq!((result.status, text(&result.stdout), text(&result.stderr)), (23, "out", "err"));
        assert_eq!(quick(&["/bin/sh", "-c", "kill -KILL $$"]).unwrap().status, 128 + libc::SIGKILL);
    }

    #[test]
    fn early_exit_with_large_stdin_does_not_fail() {
        let input = vec![42; 2_000_000];
        for _ in 0..10 {
            let result = run_with(&["/bin/sh", "-c", "exit 7"], Some(&input), &[], Duration::from_secs(10), 1 << 20);
            assert_eq!(result.unwrap().status, 7);
        }
    }

    #[test]
    fn missing_executable_and_invalid_inputs_fail() {
        assert_eq!(quick(&[]), Err(CommandError::InvalidArguments));
        assert_eq!(quick(&["/bin/echo", "bad\0argument"]), Err(CommandError::InvalidArguments));
        assert_eq!(quick(&["missing-command-rmenu-test"]),
                   Err(CommandError::System { operation: "posix_spawn".into(), code: libc::ENOENT }));
    }

    #[test]
    fn combined_output_limit_and_exact_boundary() {
        let argv = ["/bin/sh", "-c", "printf 1234; printf 5678 >&2"];
        let exact = run_with(&argv, Some(b""), &[], Duration::from_secs(10), 8).unwrap();
        assert_eq!(exact.stdout.len() + exact.stderr.len(), 8);
        assert_eq!(run_with(&argv, Some(b""), &[], Duration::from_secs(10), 7), Err(CommandError::OutputLimitExceeded));
        assert!(run_with(&["/usr/bin/true"], Some(b""), &[], Duration::from_secs(10), 0).unwrap().stdout.is_empty());
        let unbounded = ["/bin/sh", "-c", "while :; do printf 1234567890; done"];
        assert_eq!(run_with(&unbounded, Some(b""), &[], Duration::from_secs(10), 128), Err(CommandError::OutputLimitExceeded));
    }

    #[test]
    fn timeout_with_blocked_stdin_and_term_ignoring_child() {
        let start = Instant::now();
        let result = run_with(&["/bin/sh", "-c", "trap '' TERM; exec /bin/sleep 30"], Some(&[1; 1_000_000]), &[],
                              Duration::from_millis(100), 1 << 20);
        assert_eq!(result, Err(CommandError::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn timeout_after_all_pipes_close() {
        let result = run_with(&["/bin/sh", "-c", "exec 0<&- 1>&- 2>&-; exec /bin/sleep 30"], Some(b""), &[],
                              Duration::from_millis(100), 1 << 20);
        assert_eq!(result, Err(CommandError::TimedOut));
    }

    /// The nested shell waits, so it can't exec away the middle generation. Every generation
    /// ignores TERM and inherits the output pipes.
    fn descendant_command(dir: &Path, leader_action: &str) -> Vec<String> {
        let script = format!(r#"trap '' TERM
printf '%s\n' "$$" > "$1/leader"
/bin/sh -c 'trap "" TERM; /bin/sleep 30 & printf "%s\n%s\n" "$$" "$!" > "$1/descendants"; wait' child "$1" &
{leader_action}
"#);
        strings(&["/bin/sh", "-c", &script, "runner", dir.to_str().unwrap()])
    }

    fn pids(path: &Path, count: usize) -> Vec<libc::pid_t> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let found: Vec<libc::pid_t> = std::fs::read_to_string(path).unwrap_or_default()
                .lines().filter_map(|l| l.parse().ok()).collect();
            if found.len() == count { return found; }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{} did not appear", path.display());
    }

    fn assert_gone(pids: &[libc::pid_t]) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let alive: Vec<_> = pids.iter().copied()
                .filter(|&p| unsafe { libc::kill(p, 0) } == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH))
                .collect();
            if alive.is_empty() { return; }
            assert!(Instant::now() < deadline, "processes survived cleanup: {alive:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn cancellation_kills_descendants_and_not_an_independent_command() {
        let dir = TempDir::new("cancel");
        let argv = descendant_command(&dir.0, "wait");
        let cancel = Cancel::default();
        let task = { let cancel = cancel.clone();
                     std::thread::spawn(move || run(&argv, Some(b""), &[], Duration::from_secs(10), 1 << 20, &cancel)) };
        let survivor = std::thread::spawn(|| quick(&["/bin/sh", "-c", "/bin/sleep 0.2; printf alive"]));
        let descendants = pids(&dir.0.join("descendants"), 2);
        let leader = pids(&dir.0.join("leader"), 1);
        let start = Instant::now();
        cancel.cancel();
        assert_eq!(task.join().unwrap(), Err(CommandError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_gone(&[leader, descendants].concat());
        assert_eq!(text(&survivor.join().unwrap().unwrap().stdout), "alive");
    }

    #[test]
    fn timeout_kills_descendants_even_after_leader_exits() {
        let dir = TempDir::new("timeout");
        let argv = descendant_command(&dir.0, "exit 0");
        let result = run(&argv, Some(b""), &[], Duration::from_millis(300), 1 << 20, &Cancel::default());
        assert_eq!(result, Err(CommandError::TimedOut), "inherited pipes stay subject to the timeout");
        assert_gone(&pids(&dir.0.join("descendants"), 2));
    }

    #[test]
    fn output_limit_kills_descendants() {
        let dir = TempDir::new("limit");
        let argv = descendant_command(&dir.0, "while [ ! -s \"$1/descendants\" ]; do /bin/sleep 0.01; done\nwhile :; do printf 1234567890; done");
        let result = run(&argv, Some(b""), &[], Duration::from_secs(10), 128, &Cancel::default());
        assert_eq!(result, Err(CommandError::OutputLimitExceeded));
        assert_gone(&pids(&dir.0.join("descendants"), 2));
    }

    #[test]
    fn pre_cancelled_command_does_not_spawn() {
        let dir = TempDir::new("precancel");
        let marker = dir.0.join("must not exist");
        let cancel = Cancel::default();
        cancel.cancel();
        let argv = strings(&["/usr/bin/touch", marker.to_str().unwrap()]);
        assert_eq!(run(&argv, Some(b""), &[], Duration::from_secs(10), 1 << 20, &cancel), Err(CommandError::Cancelled));
        assert!(!marker.exists());
    }

    #[test]
    fn concurrent_invocations_and_cancellation_races() {
        let handles: Vec<_> = (0..30).map(|i| std::thread::spawn(move || {
            let cancel = Cancel::default();
            if i % 2 == 0 { cancel.cancel(); }
            let input = i.to_string();
            match run(&strings(&["/bin/cat"]), Some(input.as_bytes()), &[], Duration::from_secs(10), 1 << 20, &cancel) {
                Ok(result) => assert_eq!((result.stdout, result.status), (input.into_bytes(), 0)),
                Err(CommandError::Cancelled) => assert_eq!(i % 2, 0),
                Err(e) => panic!("{e}"),
            }
        })).collect();
        for handle in handles { handle.join().unwrap(); }
    }

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(CommandError::TimedOut.to_string(), "Command timed out");
        assert!(CommandError::OutputLimitExceeded.to_string().contains("output limit"));
        let missing = CommandError::System { operation: "posix_spawn".into(), code: libc::ENOENT };
        assert!(missing.to_string().starts_with("posix_spawn: No such file or directory"));
    }
}
