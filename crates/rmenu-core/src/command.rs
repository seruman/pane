//! argv commands in their own process group. Port of `Sources/MenuCore/CommandRunner.swift`.

use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Code(i32),
    Signal(i32),
}

impl ExitStatus {
    pub fn success(self) -> bool {
        self == Self::Code(0)
    }

    pub fn shell_code(self) -> i32 {
        match self {
            Self::Code(code) => code,
            Self::Signal(signal) => 128 + signal,
        }
    }
}

#[derive(Debug)]
pub enum CommandError {
    InvalidArguments,
    System { operation: &'static str, error: io::Error },
    TimedOut,
    OutputLimitExceeded,
    Cancelled,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::InvalidArguments => f.write_str("Invalid command arguments or environment"),
            CommandError::System { operation, error } => write!(f, "{operation}: {error}"),
            CommandError::TimedOut => f.write_str("Command timed out"),
            CommandError::OutputLimitExceeded => f.write_str("Command exceeded the combined output limit"),
            CommandError::Cancelled => f.write_str("Command cancelled"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::System { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Cancels a running command from another thread; the command's whole process group is killed
/// and reaped before `run` returns. Cancelling before `run` starts means nothing is spawned.
#[derive(Clone, Debug, Default)]
pub struct Cancel {
    cancelled: Arc<AtomicBool>,
}

impl Cancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Bounds how long a cancel waits when no stream is active.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn system(operation: &'static str, error: io::Error) -> CommandError {
    CommandError::System { operation, error }
}

/// Signals a command starts with at their default action, whatever rmenu does with them.
const DEFAULT_SIGNALS: [libc::c_int; 5] = [libc::SIGPIPE, libc::SIGINT, libc::SIGTERM, libc::SIGQUIT, libc::SIGHUP];

/// `program` resolved against PATH from the command's own environment, as execvp would:
/// `posix_spawnp` would search rmenu's PATH instead. Without PATH, execvp's default applies.
/// When no candidate is executable but one exists, the error is EACCES, not ENOENT.
fn resolve(program: &str, path: Option<&OsStr>) -> Result<CString, CommandError> {
    let cstring = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|_| CommandError::InvalidArguments);
    if program.contains('/') {
        return cstring(Path::new(program));
    }
    let mut denied = false;
    for dir in std::env::split_paths(path.unwrap_or(OsStr::new("/usr/bin:/bin"))) {
        let candidate = dir.join(program);
        let Ok(c) = cstring(&candidate) else { continue };
        // SAFETY: access only reads the NUL-terminated path.
        if unsafe { libc::access(c.as_ptr(), libc::X_OK) } != 0 {
            denied |= io::Error::last_os_error().raw_os_error() == Some(libc::EACCES);
        } else if candidate.is_file() {
            return Ok(c);
        } else {
            denied = true;
        }
    }
    let code = if denied { libc::EACCES } else { libc::ENOENT };
    Err(system("posix_spawn", io::Error::from_raw_os_error(code)))
}

fn pipe() -> Result<(File, File), CommandError> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: `fds` holds two descriptors, as pipe(2) requires.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(system("pipe", io::Error::last_os_error()));
    }
    // SAFETY: both descriptors were just created and are owned by nothing else.
    let ends = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
    set_cloexec(ends.0.as_raw_fd())?;
    set_cloexec(ends.1.as_raw_fd())?;
    Ok(ends)
}

fn set_cloexec(fd: RawFd) -> Result<(), CommandError> {
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(system("close-on-exec pipe", io::Error::last_os_error()));
    }
    Ok(())
}

/// Fails with the operation's name when a `posix_spawn*` call returns an error number.
fn spawn_call(code: libc::c_int, operation: &'static str) -> Result<(), CommandError> {
    if code == 0 { Ok(()) } else { Err(system(operation, io::Error::from_raw_os_error(code))) }
}

struct SpawnActions(libc::posix_spawn_file_actions_t);

impl SpawnActions {
    fn new() -> Result<Self, CommandError> {
        let mut actions = std::mem::MaybeUninit::uninit();
        spawn_call(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) }, "spawn actions")?;
        Ok(Self(unsafe { actions.assume_init() }))
    }
}

impl Drop for SpawnActions {
    fn drop(&mut self) {
        unsafe { libc::posix_spawn_file_actions_destroy(&raw mut self.0) };
    }
}

struct SpawnAttributes(libc::posix_spawnattr_t);

impl SpawnAttributes {
    fn new() -> Result<Self, CommandError> {
        let mut attrs = std::mem::MaybeUninit::uninit();
        spawn_call(unsafe { libc::posix_spawnattr_init(attrs.as_mut_ptr()) }, "spawn attributes")?;
        Ok(Self(unsafe { attrs.assume_init() }))
    }
}

impl Drop for SpawnAttributes {
    fn drop(&mut self) {
        unsafe { libc::posix_spawnattr_destroy(&raw mut self.0) };
    }
}

/// The started command: its process group leader and the parent's ends of its stdio pipes.
struct Spawned {
    pid: libc::pid_t,
    stdin: Option<File>,
    stdout: File,
    stderr: File,
}

/// Starts `program` in a new process group with only stdio. `POSIX_SPAWN_CLOEXEC_DEFAULT` closes
/// every other descriptor in the exec itself, as mmenu does, so a descriptor another thread
/// opens at the same moment can't slip through.
fn spawn(program: &CString, argv: &[CString], envp: &[CString], input: bool) -> Result<Spawned, CommandError> {
    let mut arg_ptrs: Vec<*mut libc::c_char> = argv.iter().map(|a| a.as_ptr().cast_mut()).collect();
    arg_ptrs.push(std::ptr::null_mut());
    let mut env_ptrs: Vec<*mut libc::c_char> = envp.iter().map(|e| e.as_ptr().cast_mut()).collect();
    env_ptrs.push(std::ptr::null_mut());

    let stdin = if input { Some(pipe()?) } else { None };
    let (out_r, out_w) = pipe()?;
    let (err_r, err_w) = pipe()?;
    let null = c"/dev/null";

    let mut actions = SpawnActions::new()?;
    let mut attrs = SpawnAttributes::new()?;
    match &stdin {
        Some((read, _)) => spawn_call(
            unsafe { libc::posix_spawn_file_actions_adddup2(&raw mut actions.0, read.as_raw_fd(), 0) },
            "spawn dup2",
        )?,
        None => spawn_call(
            unsafe { libc::posix_spawn_file_actions_addopen(&raw mut actions.0, 0, null.as_ptr(), libc::O_RDONLY, 0) },
            "spawn open",
        )?,
    }
    spawn_call(
        unsafe { libc::posix_spawn_file_actions_adddup2(&raw mut actions.0, out_w.as_raw_fd(), 1) },
        "spawn dup2",
    )?;
    spawn_call(
        unsafe { libc::posix_spawn_file_actions_adddup2(&raw mut actions.0, err_w.as_raw_fd(), 2) },
        "spawn dup2",
    )?;
    spawn_call(unsafe { libc::posix_spawnattr_setpgroup(&raw mut attrs.0, 0) }, "spawn process group")?;
    let mut empty: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe { libc::sigemptyset(&raw mut empty) };
    spawn_call(unsafe { libc::posix_spawnattr_setsigmask(&raw mut attrs.0, &raw const empty) }, "spawn signal mask")?;
    let mut defaults: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe { libc::sigemptyset(&raw mut defaults) };
    for signal in DEFAULT_SIGNALS {
        unsafe { libc::sigaddset(&raw mut defaults, signal) };
    }
    spawn_call(
        unsafe { libc::posix_spawnattr_setsigdefault(&raw mut attrs.0, &raw const defaults) },
        "spawn signal defaults",
    )?;
    let flags = libc::POSIX_SPAWN_SETPGROUP
        | libc::POSIX_SPAWN_SETSIGMASK
        | libc::POSIX_SPAWN_SETSIGDEF
        | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
    spawn_call(
        unsafe { libc::posix_spawnattr_setflags(&raw mut attrs.0, flags.try_into().expect("spawn flags fit c_short")) },
        "spawn flags",
    )?;
    let mut pid: libc::pid_t = 0;
    spawn_call(
        unsafe {
            libc::posix_spawn(
                &raw mut pid,
                program.as_ptr(),
                &raw const actions.0,
                &raw const attrs.0,
                arg_ptrs.as_ptr(),
                env_ptrs.as_ptr(),
            )
        },
        "posix_spawn",
    )?;
    Ok(Spawned { pid, stdin: stdin.map(|(_, write)| write), stdout: out_r, stderr: err_r })
}

#[cfg(not(target_vendor = "apple"))]
compile_error!("rmenu needs POSIX_SPAWN_CLOEXEC_DEFAULT to give commands only stdio, and only Apple targets have it");

/// The caller's environment with explicit command values overlaid.
fn environment(env: &[(String, String)]) -> Result<(Vec<CString>, Option<OsString>), CommandError> {
    let mut vars: Vec<(OsString, OsString)> =
        std::env::vars_os().filter(|(k, _)| !env.iter().any(|(o, _)| OsStr::new(o) == k)).collect();
    vars.extend(env.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))));
    let path = vars.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone());
    let envp = vars
        .into_iter()
        .map(|(k, v)| {
            let mut entry = k.into_vec();
            entry.push(b'=');
            entry.extend(v.into_vec());
            CString::new(entry).map_err(|_| CommandError::InvalidArguments)
        })
        .collect::<Result<_, _>>()?;
    Ok((envp, path))
}

fn set_nonblocking(fd: RawFd) -> Result<(), CommandError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
        return Err(system("nonblocking pipe", io::Error::last_os_error()));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub output_bytes: usize,
}

/// Runs `argv` without a shell, `env` overlaid on ours, PATH resolved from the overlay.
/// `input: None` connects stdin to /dev/null: tools such as glow or bat prefer a piped stdin
/// over their file arguments. The limit is shared by stdout and stderr. Blocks the calling thread.
pub fn run(
    argv: &[String],
    input: Option<&[u8]>,
    env: &[(String, String)],
    limits: Limits,
    cancel: &Cancel,
) -> Result<CommandResult, CommandError> {
    if cancel.is_cancelled() {
        return Err(CommandError::Cancelled);
    }
    if argv.first().is_none_or(String::is_empty) || env.iter().any(|(k, _)| k.is_empty() || k.contains('=')) {
        return Err(CommandError::InvalidArguments);
    }
    let args: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_str()))
        .collect::<Result<_, _>>()
        .map_err(|_| CommandError::InvalidArguments)?;
    let deadline = Instant::now().checked_add(limits.timeout);
    let (envp, path) = environment(env)?;
    let program = resolve(&argv[0], path.as_deref())?;
    let spawned = spawn(&program, &args, &envp, input.is_some())?;
    let mut group = Group { pid: spawned.pid, reaped: false };
    pump(&mut group, spawned, input.unwrap_or_default(), deadline, limits.output_bytes, cancel)
}

/// Kills and reaps the whole process group unless the leader was reaped after a clean finish.
/// The leader stays unreaped until then, so its PID still names the group.
struct Group {
    pid: libc::pid_t,
    reaped: bool,
}

impl Group {
    /// Blocks until the leader exits or `limit` passes, whichever comes first, so `run` can
    /// still check cancellation and its deadline.
    fn wait_for_exit(&self, limit: Duration) -> Result<(), CommandError> {
        // SAFETY: kqueue takes no arguments and returns a new descriptor or -1.
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            return Err(system("kqueue", io::Error::last_os_error()));
        }
        // SAFETY: the descriptor was just created and has no other owner.
        let queue = unsafe { OwnedFd::from_raw_fd(queue) };
        let change = libc::kevent {
            ident: self.pid.try_into().expect("spawned pid is positive"),
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let mut event = change;
        let timeout = libc::timespec {
            tv_sec: limit.as_secs().try_into().expect("bounded exit wait fits time_t"),
            tv_nsec: limit.subsec_nanos().into(),
        };
        // SAFETY: one change and room for one event, both live locals; the timeout is a live local.
        let ready =
            unsafe { libc::kevent(queue.as_raw_fd(), &raw const change, 1, &raw mut event, 1, &raw const timeout) };
        let error = if ready < 0 {
            Some(io::Error::last_os_error())
        } else if ready > 0 && event.flags & libc::EV_ERROR != 0 {
            Some(io::Error::from_raw_os_error(event.data.try_into().expect("kernel errno fits c_int")))
        } else {
            None
        };
        // ESRCH can race with registration when the leader has already exited.
        if let Some(error) = error
            && !matches!(error.raw_os_error(), Some(libc::EINTR | libc::ESRCH))
        {
            return Err(system("kevent", error));
        }
        Ok(())
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, CommandError> {
        let mut status = 0;
        loop {
            // SAFETY: waits on our own child without blocking.
            let pid = unsafe { libc::waitpid(self.pid, &raw mut status, libc::WNOHANG) };
            if pid == 0 {
                return Ok(None);
            }
            if pid < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(system("waitpid", error));
            }
            self.reaped = true;
            return Ok(Some(if libc::WIFSIGNALED(status) {
                ExitStatus::Signal(libc::WTERMSIG(status))
            } else {
                ExitStatus::Code(libc::WEXITSTATUS(status))
            }));
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // SIGKILL also handles children that ignore TERM. Descendants that setsid are not ours.
        // SAFETY: signals our own process group, then reaps its leader.
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
            let mut status = 0;
            while libc::waitpid(self.pid, &raw mut status, 0) < 0
                && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
            {}
        }
    }
}

fn pump(
    group: &mut Group,
    spawned: Spawned,
    input: &[u8],
    deadline: Option<Instant>,
    limit: usize,
    cancel: &Cancel,
) -> Result<CommandResult, CommandError> {
    let mut stdin = spawned.stdin.filter(|_| !input.is_empty());
    let mut outputs = [(Some(spawned.stdout), Vec::new()), (Some(spawned.stderr), Vec::new())];
    for file in [stdin.as_ref(), outputs[0].0.as_ref(), outputs[1].0.as_ref()].into_iter().flatten() {
        set_nonblocking(file.as_raw_fd())?;
    }
    let mut written = 0;
    let mut buffer = [0u8; 16_384];
    loop {
        if cancel.is_cancelled() {
            return Err(CommandError::Cancelled);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(CommandError::TimedOut);
        }
        if stdin.is_none() && outputs.iter().all(|(file, _)| file.is_none()) {
            if let Some(status) = group.try_wait()? {
                let [(_, stdout), (_, stderr)] = outputs;
                return Ok(CommandResult { stdout, stderr, status });
            }
            // The pipes can close a moment before the leader is reaped.
            group.wait_for_exit(POLL_INTERVAL)?;
            continue;
        }
        let mut fds = [
            (outputs[0].0.as_ref(), libc::POLLIN),
            (outputs[1].0.as_ref(), libc::POLLIN),
            (stdin.as_ref(), libc::POLLOUT),
        ]
        .map(|(file, events)| libc::pollfd { fd: file.map_or(-1, AsRawFd::as_raw_fd), events, revents: 0 });
        if unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len().try_into().expect("poll slots fit nfds_t"),
                POLL_INTERVAL.as_millis().try_into().expect("poll interval fits c_int"),
            )
        } < 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(system("poll", error));
        }
        let mut remaining = limit - outputs.iter().map(|(_, bytes)| bytes.len()).sum::<usize>();
        for ((file, bytes), ready) in outputs.iter_mut().zip(&fds) {
            if ready.revents == 0 {
                continue;
            }
            if let Some(pipe) = file {
                match pipe.read(&mut buffer) {
                    Ok(0) => *file = None,
                    Ok(n) => {
                        if n > remaining {
                            return Err(CommandError::OutputLimitExceeded);
                        }
                        remaining -= n;
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(system("read output", e)),
                }
            }
        }
        if let Some(pipe) = stdin.as_mut().filter(|_| fds[2].revents != 0) {
            match pipe.write(&input[written..]) {
                Ok(n) => {
                    written += n;
                    if written == input.len() {
                        stdin = None;
                    }
                }
                // Rust's runtime ignores SIGPIPE, so a child that exits early surfaces here.
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => stdin = None,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(system("write stdin", e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn strings(argv: &[&str]) -> Vec<String> {
        argv.iter().map(ToString::to_string).collect()
    }

    fn run_with(
        argv: &[&str],
        input: Option<&[u8]>,
        env: &[(&str, &str)],
        timeout: Duration,
        limit: usize,
    ) -> Result<CommandResult, CommandError> {
        let env: Vec<_> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        run(&strings(argv), input, &env, Limits { timeout, output_bytes: limit }, &Cancel::default())
    }

    fn quick(argv: &[&str]) -> Result<CommandResult, CommandError> {
        run_with(argv, Some(b""), &[], Duration::from_secs(10), 1_048_576)
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

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
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run_in_child(test: &str, env: &[(&str, &str)]) -> bool {
        const CHILD_TEST: &str = "RMENU_COMMAND_CHILD_TEST";
        if std::env::var(CHILD_TEST).as_deref() == Ok(test) {
            return false;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("command::tests::{test}"), "--nocapture", "--test-threads=1"])
            .env(CHILD_TEST, test)
            .envs(env.iter().copied())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}\n{}", text(&output.stdout), text(&output.stderr));
        true
    }

    #[test]
    fn commands_preserve_the_callers_terminal_environment() {
        const CHILD: &str = "RMENU_TERMINAL_ENV_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            for value in [None, Some("0"), Some("1"), Some("custom-value")] {
                let mut child = std::process::Command::new(std::env::current_exe().unwrap());
                child
                    .args([
                        "--exact",
                        "command::tests::commands_preserve_the_callers_terminal_environment",
                        "--nocapture",
                    ])
                    .env(CHILD, "1")
                    .env_remove("RUNE_TERMINAL");
                if let Some(value) = value {
                    child.env("RUNE_TERMINAL", value);
                }
                let output = child.output().unwrap();
                assert!(
                    output.status.success(),
                    "RUNE_TERMINAL={value:?}: {}{}",
                    text(&output.stdout),
                    text(&output.stderr)
                );
            }
            return;
        }
        let result = quick(&["/usr/bin/env"]).unwrap();
        let expected = std::env::var("RUNE_TERMINAL").ok().map(|value| format!("RUNE_TERMINAL={value}"));
        let actual = text(&result.stdout).lines().find(|line| line.starts_with("RUNE_TERMINAL="));
        assert_eq!(actual, expected.as_deref());
        let overlaid =
            run_with(&["/usr/bin/env"], None, &[("RUNE_TERMINAL", "override")], Duration::from_secs(5), 1 << 20)
                .unwrap();
        assert_eq!(
            text(&overlaid.stdout).lines().filter(|line| line.starts_with("RUNE_TERMINAL=")).collect::<Vec<_>>(),
            ["RUNE_TERMINAL=override"]
        );
    }

    #[test]
    fn commands_get_only_stdio() {
        if run_in_child("commands_get_only_stdio", &[]) {
            return;
        }
        let file = std::fs::File::open("/dev/null").unwrap();
        // A descriptor opened without close-on-exec, as a framework inside rmenu would.
        // A high number, so the command's own descriptors (ls opening /dev/fd) can't collide.
        let fd = unsafe { libc::dup2(file.as_raw_fd(), 200) };
        assert_eq!(fd, 200);
        let result = quick(&["/bin/ls", "/dev/fd"]).unwrap();
        unsafe { libc::close(fd) };
        let out = text(&result.stdout);
        assert!(!out.lines().any(|l| l == fd.to_string()), "fd {fd} leaked into the command: {out}");
    }

    #[test]
    fn a_descriptor_another_thread_opens_during_spawn_never_leaks() {
        if run_in_child("a_descriptor_another_thread_opens_during_spawn_never_leaks", &[]) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let null = std::fs::File::open("/dev/null").unwrap();
        let raw = null.as_raw_fd();
        // Another thread keeps creating descriptor 250 without close-on-exec, as a framework
        // inside rmenu might while a command starts.
        let churn = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    unsafe {
                        libc::dup2(raw, 250);
                        libc::close(250);
                    }
                }
            })
        };
        let leaked = (0..300)
            .filter(|_| {
                let out = quick(&["/bin/ls", "/dev/fd"]).unwrap();
                text(&out.stdout).lines().any(|l| l == "250")
            })
            .count();
        stop.store(true, Ordering::Relaxed);
        churn.join().unwrap();
        assert_eq!(leaked, 0, "descriptor 250 reached {leaked} of 300 commands");
    }

    #[test]
    fn commands_start_with_sigpipe_at_its_default() {
        // Rust ignores SIGPIPE in rmenu, and an ignored signal survives exec: a shell that
        // inherited it would outlive `kill -PIPE $$`. At the default it dies of it (141).
        let result = quick(&["/bin/sh", "-c", "kill -PIPE $$; echo survived"]).unwrap();
        assert_eq!(result.status, ExitStatus::Signal(libc::SIGPIPE), "stdout: {}", text(&result.stdout));
        assert_eq!(text(&result.stdout), "");
    }

    #[test]
    fn a_timeout_too_large_for_a_deadline_means_no_deadline() {
        let result = run_with(&["/bin/echo", "ok"], Some(b""), &[], Duration::MAX, 1 << 20).unwrap();
        assert_eq!(text(&result.stdout), "ok\n");
    }

    #[test]
    fn exit_wait_wakes_before_its_cancellation_interval() {
        let argv = [c"/bin/cat".to_owned()];
        let spawned = spawn(&c"/bin/cat".to_owned(), &argv, &[], true).unwrap();
        let mut group = Group { pid: spawned.pid, reaped: false };
        assert_eq!(group.try_wait().unwrap(), None);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(spawned);
        });
        let started = Instant::now();
        group.wait_for_exit(Duration::from_secs(2)).unwrap();
        release.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "exit wait used the full interval");
        assert_eq!(group.try_wait().unwrap(), Some(ExitStatus::Code(0)));
    }

    #[test]
    fn exit_wait_is_bounded_while_the_child_is_alive() {
        let argv = [c"/bin/cat".to_owned()];
        let spawned = spawn(&c"/bin/cat".to_owned(), &argv, &[], true).unwrap();
        let mut group = Group { pid: spawned.pid, reaped: false };
        group.wait_for_exit(Duration::from_millis(10)).unwrap();
        assert_eq!(group.try_wait().unwrap(), None);
        drop(spawned);
    }

    #[test]
    fn exit_wait_handles_a_child_that_exited_before_registration() {
        let argv = [c"/bin/echo".to_owned(), c"hi".to_owned()];
        let mut spawned = spawn(&c"/bin/echo".to_owned(), &argv, &[], false).unwrap();
        let mut group = Group { pid: spawned.pid, reaped: false };
        let mut output = String::new();
        spawned.stdout.read_to_string(&mut output).unwrap();
        assert_eq!(output, "hi\n");
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        group.wait_for_exit(Duration::from_secs(2)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(group.try_wait().unwrap(), Some(ExitStatus::Code(0)));
    }

    #[test]
    fn stdin_and_environment_overlay() {
        let input = b"hello\0world\nwith spaces\n";
        let result = run_with(
            &["/bin/sh", "-c", "printf '%s\\n' \"$RUNNER_VALUE\"; /bin/cat"],
            Some(input),
            &[("RUNNER_VALUE", "a value with spaces")],
            Duration::from_secs(10),
            1 << 20,
        )
        .unwrap();
        assert_eq!(result.stdout, [b"a value with spaces\n".as_slice(), input].concat());
        assert!(result.stderr.is_empty());
        assert!(result.status.success());
    }

    #[test]
    fn no_input_connects_stdin_to_dev_null() {
        let check = ["/bin/sh", "-c", "if [ -c /dev/stdin ]; then echo chardev; else echo pipe; fi; /bin/cat"];
        let result = run_with(&check, None, &[], Duration::from_secs(10), 1 << 20).unwrap();
        assert_eq!(text(&result.stdout), "chardev\n");
        assert_eq!(text(&quick(&check).unwrap().stdout), "pipe\n");
    }

    #[test]
    fn directory_and_environment_are_inherited() {
        const HOME: &str = "/rmenu test home with spaces";
        if run_in_child("directory_and_environment_are_inherited", &[("HOME", HOME), ("RUNNER_OVERLAY", "parent")]) {
            return;
        }
        let result = quick(&["/bin/pwd"]).unwrap();
        let actual = Path::new(text(&result.stdout).trim_end()).canonicalize().unwrap();
        assert_eq!(actual, std::env::current_dir().unwrap().canonicalize().unwrap());
        let env =
            run_with(&["/usr/bin/env"], Some(b""), &[("RUNNER_OVERLAY", "yes")], Duration::from_secs(10), 1 << 20)
                .unwrap();
        let lines: Vec<&str> = text(&env.stdout).lines().collect();
        assert!(lines.contains(&"RUNNER_OVERLAY=yes"));
        assert!(lines.contains(&format!("HOME={HOME}").as_str()));
        assert!(!lines.contains(&"RUNNER_OVERLAY=parent"));
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
        let result = run_with(
            &["literal printer", "%s", &literal],
            Some(b""),
            &[("PATH", path)],
            Duration::from_secs(10),
            1 << 20,
        )
        .unwrap();
        assert_eq!(text(&result.stdout), literal);
        assert!(!marker.exists());
        let absolute = format!("{path}/literal printer");
        assert_eq!(text(&quick(&[&absolute, "%s", "absolute"]).unwrap().stdout), "absolute");
    }

    #[test]
    fn simultaneous_stdout_stderr_and_large_stdin() {
        // Both writers exceed pipe capacity before stdin is consumed; sequential I/O deadlocks.
        let input: Vec<u8> = (0..=u8::MAX).cycle().take(400_000).collect();
        let script = "/usr/bin/head -c 200000 /dev/zero &\n/usr/bin/head -c 200000 /dev/zero >&2 &\nwait\n/bin/cat";
        let result = run_with(&["/bin/sh", "-c", script], Some(&input), &[], Duration::from_secs(10), 1 << 20).unwrap();
        assert_eq!(result.stdout, [vec![0; 200_000], input].concat());
        assert_eq!(result.stderr, vec![0; 200_000]);
        assert!(result.status.success());
    }

    #[test]
    fn closing_either_output_keeps_the_other_stream_and_stdin_active() {
        let input = vec![b'x'; 100_000];
        for (script, stdout, stderr) in [
            ("printf out; exec 1>&-; /bin/cat >&2", b"out".as_slice(), input.as_slice()),
            ("printf err >&2; exec 2>&-; /bin/cat", input.as_slice(), b"err".as_slice()),
        ] {
            let result =
                run_with(&["/bin/sh", "-c", script], Some(&input), &[], Duration::from_secs(10), 1 << 20).unwrap();
            assert_eq!(result.stdout, stdout, "{script}");
            assert_eq!(result.stderr, stderr, "{script}");
            assert!(result.status.success(), "{script}");
        }
    }

    #[test]
    fn nonzero_and_signal_statuses_are_returned() {
        let result = quick(&["/bin/sh", "-c", "printf out; printf err >&2; exit 23"]).unwrap();
        assert_eq!((result.status, text(&result.stdout), text(&result.stderr)), (ExitStatus::Code(23), "out", "err"));
        assert_eq!(quick(&["/bin/sh", "-c", "kill -KILL $$"]).unwrap().status, ExitStatus::Signal(libc::SIGKILL));
    }

    #[test]
    fn exit_codes_and_signals_remain_distinct() {
        let exited = quick(&["/bin/sh", "-c", "exit 137"]).unwrap();
        let killed = quick(&["/bin/sh", "-c", "kill -KILL $$"]).unwrap();
        assert_ne!(exited.status, killed.status);
        assert_eq!(exited.status, ExitStatus::Code(137));
        assert_eq!(killed.status, ExitStatus::Signal(libc::SIGKILL));
        assert_eq!(exited.status.shell_code(), 137);
        assert_eq!(killed.status.shell_code(), 137);
        assert!(!exited.status.success());
        assert!(!killed.status.success());
    }

    #[test]
    fn early_exit_with_large_stdin_does_not_fail() {
        let input = vec![42; 2_000_000];
        for _ in 0..10 {
            let result = run_with(&["/bin/sh", "-c", "exit 7"], Some(&input), &[], Duration::from_secs(10), 1 << 20);
            assert_eq!(result.unwrap().status, ExitStatus::Code(7));
        }
    }

    #[test]
    fn missing_executable_and_invalid_inputs_fail() {
        assert!(matches!(quick(&[]), Err(CommandError::InvalidArguments)));
        assert!(matches!(quick(&["/bin/echo", "bad\0argument"]), Err(CommandError::InvalidArguments)));
        assert!(matches!(quick(&["bad\0program"]), Err(CommandError::InvalidArguments)));
        for env in [[("BAD\0KEY", "value")], [("KEY", "bad\0value")], [("", "value")], [("BAD=KEY", "value")]] {
            assert!(
                matches!(
                    run_with(&["/usr/bin/true"], None, &env, Duration::from_secs(10), 1024),
                    Err(CommandError::InvalidArguments)
                ),
                "{env:?}"
            );
        }
        assert!(matches!(quick(&["missing-command-rmenu-test"]),
                         Err(CommandError::System { operation: "posix_spawn", error }) if error.raw_os_error() == Some(libc::ENOENT)));
    }

    #[test]
    fn a_candidate_on_path_that_is_not_executable_is_denied_not_missing() {
        let dir = TempDir::new("denied");
        std::fs::write(dir.0.join("script"), "#!/bin/sh\n").unwrap();
        let result =
            run_with(&["script"], Some(b""), &[("PATH", dir.0.to_str().unwrap())], Duration::from_secs(10), 1 << 20);
        assert!(matches!(result, Err(CommandError::System { operation: "posix_spawn", error })
                         if error.raw_os_error() == Some(libc::EACCES)));
    }

    #[test]
    fn without_path_commands_resolve_against_usr_bin_and_bin() {
        assert_eq!(resolve("true", None).unwrap().as_c_str(), c"/usr/bin/true");
    }

    #[test]
    fn combined_output_limit_and_exact_boundary() {
        let argv = ["/bin/sh", "-c", "printf 1234; printf 5678 >&2"];
        let exact = run_with(&argv, Some(b""), &[], Duration::from_secs(10), 8).unwrap();
        assert_eq!(exact.stdout.len() + exact.stderr.len(), 8);
        assert!(matches!(
            run_with(&argv, Some(b""), &[], Duration::from_secs(10), 7),
            Err(CommandError::OutputLimitExceeded)
        ));
        assert!(run_with(&["/usr/bin/true"], Some(b""), &[], Duration::from_secs(10), 0).unwrap().stdout.is_empty());
        let unbounded = ["/bin/sh", "-c", "while :; do printf 1234567890; done"];
        assert!(matches!(
            run_with(&unbounded, Some(b""), &[], Duration::from_secs(10), 128),
            Err(CommandError::OutputLimitExceeded)
        ));
    }

    #[test]
    fn timeout_with_blocked_stdin_and_term_ignoring_child() {
        let dir = TempDir::new("blocked-stdin");
        let marker = dir.0.join("leader");
        let timeout = Duration::from_secs(1);
        let deadline = Instant::now() + timeout;
        let result = run_with(
            &[
                "/bin/sh",
                "-c",
                "trap '' TERM; echo $$ > \"$1\"; exec /bin/sleep 30",
                "runner",
                marker.to_str().unwrap(),
            ],
            Some(&vec![1; 1_000_000]),
            &[],
            timeout,
            1 << 20,
        );
        assert!(matches!(result, Err(CommandError::TimedOut)));
        assert!(deadline.elapsed() < Duration::from_secs(10), "timeout cleanup did not finish");
        assert_gone(&pids(&marker, 1));
    }

    #[test]
    fn cancellation_after_all_pipes_close() {
        let dir = TempDir::new("closed-pipes");
        let marker = dir.0.join("leader");
        let argv = strings(&[
            "/bin/sh",
            "-c",
            "exec 0<&- 1>&- 2>&-; echo $$ > \"$1\"; exec /bin/sleep 30",
            "runner",
            marker.to_str().unwrap(),
        ]);
        let cancel = Cancel::default();
        let task = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                run(&argv, None, &[], Limits { timeout: Duration::from_secs(10), output_bytes: 1024 }, &cancel)
            })
        };
        let leader = pids(&marker, 1);
        cancel.cancel();
        assert!(matches!(task.join().unwrap(), Err(CommandError::Cancelled)));
        assert_gone(&leader);
    }

    #[test]
    fn timeout_after_all_pipes_close() {
        let result = run_with(
            &["/bin/sh", "-c", "exec 0<&- 1>&- 2>&-; exec /bin/sleep 30"],
            Some(b""),
            &[],
            Duration::from_millis(100),
            1 << 20,
        );
        assert!(matches!(result, Err(CommandError::TimedOut)));
    }

    /// The nested shell waits, so it can't exec away the middle generation. Every generation
    /// ignores TERM and inherits the output pipes.
    fn descendant_command(dir: &Path, leader_action: &str) -> Vec<String> {
        let script = format!(
            r#"trap '' TERM
printf '%s\n' "$$" > "$1/leader"
/bin/sh -c 'trap "" TERM; /bin/sleep 30 & printf "%s\n%s\n" "$$" "$!" > "$1/descendants"; wait' child "$1" &
{leader_action}
"#
        );
        strings(&["/bin/sh", "-c", &script, "runner", dir.to_str().unwrap()])
    }

    fn pids(path: &Path, count: usize) -> Vec<libc::pid_t> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let found: Vec<libc::pid_t> =
                std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| l.parse().ok()).collect();
            if found.len() == count {
                return found;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{} did not appear", path.display());
    }

    fn assert_gone(pids: &[libc::pid_t]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let alive: Vec<_> = pids.iter().copied()
                .filter(|&p| unsafe { libc::kill(p, 0) } == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH))
                .collect();
            if alive.is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "processes survived cleanup: {alive:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn cancellation_kills_descendants_and_not_an_independent_command() {
        let dir = TempDir::new("cancel");
        let argv = descendant_command(&dir.0, "wait");
        let cancel = Cancel::default();
        let task = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                run(&argv, Some(b""), &[], Limits { timeout: Duration::from_secs(10), output_bytes: 1 << 20 }, &cancel)
            })
        };
        let survivor = std::thread::spawn(|| quick(&["/bin/sh", "-c", "/bin/sleep 0.2; printf alive"]));
        let descendants = pids(&dir.0.join("descendants"), 2);
        let leader = pids(&dir.0.join("leader"), 1);
        let start = Instant::now();
        cancel.cancel();
        assert!(matches!(task.join().unwrap(), Err(CommandError::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(10), "cancellation cleanup did not finish");
        assert_gone(&[leader, descendants].concat());
        assert_eq!(text(&survivor.join().unwrap().unwrap().stdout), "alive");
    }

    #[test]
    fn timeout_kills_descendants_even_after_leader_exits() {
        let dir = TempDir::new("timeout");
        let argv = descendant_command(&dir.0, "exit 0");
        let result = run(
            &argv,
            Some(b""),
            &[],
            Limits { timeout: Duration::from_millis(300), output_bytes: 1 << 20 },
            &Cancel::default(),
        );
        assert!(matches!(result, Err(CommandError::TimedOut)), "inherited pipes stay subject to the timeout");
        assert_gone(&pids(&dir.0.join("descendants"), 2));
    }

    #[test]
    fn output_limit_kills_descendants() {
        let dir = TempDir::new("limit");
        let argv = descendant_command(
            &dir.0,
            "while [ ! -s \"$1/descendants\" ]; do /bin/sleep 0.01; done\nwhile :; do printf 1234567890; done",
        );
        let result = run(
            &argv,
            Some(b""),
            &[],
            Limits { timeout: Duration::from_secs(10), output_bytes: 128 },
            &Cancel::default(),
        );
        assert!(matches!(result, Err(CommandError::OutputLimitExceeded)));
        assert_gone(&pids(&dir.0.join("descendants"), 2));
    }

    #[test]
    fn pre_cancelled_command_does_not_spawn() {
        let dir = TempDir::new("precancel");
        let marker = dir.0.join("must not exist");
        let cancel = Cancel::default();
        cancel.cancel();
        let argv = strings(&["/usr/bin/touch", marker.to_str().unwrap()]);
        assert!(matches!(
            run(&argv, Some(b""), &[], Limits { timeout: Duration::from_secs(10), output_bytes: 1 << 20 }, &cancel),
            Err(CommandError::Cancelled)
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn concurrent_invocations_and_cancellation_races() {
        let dir = TempDir::new("concurrent-cancel");
        let release = dir.0.join("release");
        let tasks: Vec<_> = (0..12)
            .map(|i| {
                let marker = dir.0.join(format!("leader-{i}"));
                let script = if i % 3 == 0 {
                    "echo $$ > \"$1\"; exec /bin/sleep 30"
                } else {
                    "echo $$ > \"$1\"; while [ ! -e \"$2\" ]; do /bin/sleep 0.01; done; exec /bin/cat"
                };
                let argv = strings(&[
                    "/bin/sh",
                    "-c",
                    script,
                    "runner",
                    marker.to_str().unwrap(),
                    release.to_str().unwrap(),
                ]);
                let cancel = Cancel::default();
                let task_cancel = cancel.clone();
                let handle = std::thread::spawn(move || {
                    run(
                        &argv,
                        Some(i.to_string().as_bytes()),
                        &[],
                        Limits { timeout: Duration::from_secs(20), output_bytes: 1024 },
                        &task_cancel,
                    )
                });
                (marker, cancel, handle)
            })
            .collect();
        let leaders: Vec<_> = tasks.iter().flat_map(|(marker, _, _)| pids(marker, 1)).collect();
        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let cancels: Vec<_> = tasks.iter().map(|(_, cancel, _)| cancel.clone()).collect();
            let start = &start;
            scope.spawn(move || {
                start.wait();
                for (i, cancel) in cancels.iter().enumerate() {
                    if i % 3 != 2 {
                        cancel.cancel();
                    }
                }
            });
            start.wait();
            std::fs::write(&release, b"go").unwrap();
        });
        let results: Vec<_> = tasks.into_iter().map(|(_, _, handle)| handle.join().unwrap()).collect();
        assert_gone(&leaders);
        let groups: Vec<_> = leaders.iter().map(|pid| -pid).collect();
        assert_gone(&groups);
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(result) => {
                    assert_ne!(i % 3, 0, "command {i} must be cancelled while running");
                    assert_eq!(result.stdout, i.to_string().as_bytes(), "command {i}");
                    assert_eq!(result.status, ExitStatus::Code(0), "command {i}");
                }
                Err(CommandError::Cancelled) => assert_ne!(i % 3, 2, "independent command {i}"),
                Err(error) => panic!("command {i}: {error}"),
            }
        }
    }

    #[test]
    fn close_on_exec_reports_invalid_descriptors() {
        assert!(matches!(set_cloexec(-1), Err(CommandError::System { error, .. })
                         if error.raw_os_error() == Some(libc::EBADF)));
    }

    #[test]
    fn pipe_ends_have_close_on_exec_set() {
        let (read, write) = pipe().unwrap();
        for end in [read, write] {
            let flags = unsafe { libc::fcntl(end.as_raw_fd(), libc::F_GETFD) };
            assert!(flags >= 0);
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }
    }

    #[test]
    fn system_errors_preserve_non_os_details() {
        let error = system("read output", io::Error::other("fixture failure"));
        assert!(error.to_string().contains("fixture failure"), "{error}");
    }

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(CommandError::TimedOut.to_string(), "Command timed out");
        assert!(CommandError::OutputLimitExceeded.to_string().contains("output limit"));
        let missing = system("posix_spawn", io::Error::from_raw_os_error(libc::ENOENT));
        assert!(missing.to_string().starts_with("posix_spawn: No such file or directory"));
    }
}
