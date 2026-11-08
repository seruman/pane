//! Piped input and command output into rows. Port of `Sources/MenuCore/RecordDecoder.swift`.

use crate::config::InputFormat;
use crate::item::MenuItem;

/// Errors carry mmenu's exact messages (`Display`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    RecordTooLarge { record: usize },
    InvalidUtf8 { record: usize },
    MalformedJson { record: usize, reason: String },
    DuplicateId { record: usize, id: String },
    AlreadyFinished,
}

/// One input generation. Errors are terminal.
pub struct RecordDecoder {
    _todo: (),
}

impl RecordDecoder {
    pub const MAXIMUM_RECORD_BYTES: usize = 1_048_576;
    pub fn new(format: InputFormat) -> Self { todo!() }
    pub fn append(&mut self, bytes: &[u8]) -> Result<Vec<MenuItem>, RecordError> { todo!() }
    pub fn finish(&mut self) -> Result<Vec<MenuItem>, RecordError> { todo!() }
}
