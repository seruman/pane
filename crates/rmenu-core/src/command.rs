//! argv commands in their own process group. Port of `Sources/MenuCore/CommandRunner.swift`.

use std::time::Duration;

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

/// Cancels a running command from another thread; the command's whole process group is killed
/// and reaped before `run` returns.
#[derive(Clone, Debug, Default)]
pub struct Cancel {
    _todo: std::sync::Arc<()>,
}

/// Runs `argv` without a shell, `env` overlaid on ours, PATH resolved from the overlay.
/// `input: None` connects stdin to /dev/null. The limit is shared by stdout and stderr.
/// Blocks the calling thread.
pub fn run(argv: &[String], input: Option<&[u8]>, env: &[(String, String)], timeout: Duration,
           output_limit: usize, cancel: &Cancel) -> Result<CommandResult, CommandError> { todo!() }
