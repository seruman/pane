//! Piped input and command output into rows. Port of `Sources/MenuCore/RecordDecoder.swift`.

use std::collections::HashSet;
use std::fmt;

use crate::config::InputFormat;
use crate::item::MenuItem;

impl InputFormat {
    fn delimiter(self) -> u8 {
        match self {
            Self::Lines | Self::Jsonl => b'\n',
            Self::Nul => 0,
        }
    }

    fn strips_cr(self) -> bool {
        match self {
            Self::Lines | Self::Jsonl => true,
            Self::Nul => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    RecordTooLarge { record: usize },
    InvalidUtf8 { record: usize },
    MalformedJson { record: usize, reason: String },
    DuplicateId { record: usize, id: String },
    AlreadyFinished,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::RecordTooLarge { record } => write!(f, "Record {record} exceeds the 1 MiB limit."),
            RecordError::InvalidUtf8 { record } => write!(f, "Record {record} contains invalid UTF-8."),
            RecordError::MalformedJson { record, reason } => write!(f, "Invalid JSONL record {record}: {reason}"),
            RecordError::DuplicateId { record, id } => write!(f, "Record {record} has duplicate ID {id:?}."),
            RecordError::AlreadyFinished => write!(f, "Cannot append after finishing the input generation."),
        }
    }
}

impl std::error::Error for RecordError {}

/// One input generation. Errors are terminal; a failed append returns no partial rows.
pub struct RecordDecoder {
    format: InputFormat,
    pending: Vec<u8>,
    seen_ids: HashSet<String>,
    records: usize,
    next_id: usize,
    state: State,
}

enum State {
    Open,
    Finished,
    Failed(RecordError),
}

impl RecordDecoder {
    /// Per record, excluding its delimiter (including any CR).
    pub const MAXIMUM_RECORD_BYTES: usize = 1_048_576;

    pub fn new(format: InputFormat) -> Self {
        RecordDecoder {
            format,
            pending: Vec::new(),
            seen_ids: HashSet::new(),
            records: 0,
            next_id: 0,
            state: State::Open,
        }
    }

    pub fn decode_all(format: InputFormat, bytes: &[u8]) -> Result<Vec<MenuItem>, RecordError> {
        let mut decoder = Self::new(format);
        let mut items = decoder.append(bytes)?;
        items.extend(decoder.finish()?);
        Ok(items)
    }

    pub fn append(&mut self, bytes: &[u8]) -> Result<Vec<MenuItem>, RecordError> {
        match &self.state {
            State::Open => {}
            State::Finished => return Err(RecordError::AlreadyFinished),
            State::Failed(error) => return Err(error.clone()),
        }
        let delimiter = self.format.delimiter();
        let mut items = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let end = rest.iter().position(|&b| b == delimiter);
            let chunk = &rest[..end.unwrap_or(rest.len())];
            if self.pending.len() + chunk.len() > Self::MAXIMUM_RECORD_BYTES {
                return Err(self.fail(RecordError::RecordTooLarge { record: self.records + 1 }));
            }
            self.pending.extend_from_slice(chunk);
            let Some(end) = end else { break };
            match self.consume(true) {
                Ok(item) => items.extend(item),
                Err(error) => return Err(self.fail(error)),
            }
            rest = &rest[end + 1..];
        }
        Ok(items)
    }

    /// Flushes an unterminated final record. A trailing delimiter makes no extra empty row.
    pub fn finish(&mut self) -> Result<Vec<MenuItem>, RecordError> {
        match &self.state {
            State::Open => {}
            State::Finished => return Ok(Vec::new()),
            State::Failed(error) => return Err(error.clone()),
        }
        self.state = State::Finished;
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        self.consume(false).map(|item| item.into_iter().collect()).map_err(|error| self.fail(error))
    }

    fn fail(&mut self, error: RecordError) -> RecordError {
        self.pending.clear();
        self.state = State::Failed(error.clone());
        error
    }

    fn consume(&mut self, terminated: bool) -> Result<Option<MenuItem>, RecordError> {
        self.records += 1;
        let record = self.records;
        let mut bytes = std::mem::take(&mut self.pending);
        if terminated && self.format.strips_cr() && bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let text = String::from_utf8(bytes).map_err(|_| RecordError::InvalidUtf8 { record })?;
        match self.format {
            InputFormat::Jsonl => {
                if text.chars().all(char::is_whitespace) {
                    return Ok(None);
                }
                let item: MenuItem = serde_json::from_str(&text)
                    .map_err(|e| RecordError::MalformedJson { record, reason: e.to_string() })?;
                if !self.seen_ids.insert(item.id.clone()) {
                    return Err(RecordError::DuplicateId { record, id: item.id });
                }
                Ok(Some(item))
            }
            InputFormat::Lines | InputFormat::Nul => {
                let item = MenuItem::new(self.next_id.to_string(), text);
                self.next_id += 1;
                Ok(Some(item))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use InputFormat::*;

    fn decode(bytes: &[u8], format: InputFormat, chunk: usize) -> Result<Vec<MenuItem>, RecordError> {
        let mut decoder = RecordDecoder::new(format);
        let mut items = Vec::new();
        for piece in bytes.chunks(chunk) {
            items.extend(decoder.append(piece)?);
        }
        items.extend(decoder.finish()?);
        assert!(decoder.finish()?.is_empty());
        Ok(items)
    }

    fn titles(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(MenuItem::title).collect()
    }

    #[test]
    fn whole_input_matches_streamed_input() {
        for (format, bytes) in [
            (Lines, b"one\r\ntwo\nlast".as_slice()),
            (Nul, b"one\0two\n\0last\r".as_slice()),
            (Jsonl, b"{\"id\":\"1\",\"title\":\"one\"}\n{\"id\":\"2\",\"title\":\"two\"}".as_slice()),
            (Lines, b"one\n\xff".as_slice()),
            (Jsonl, b"{\"id\":\"1\",\"title\":\"one\"}\ninvalid".as_slice()),
        ] {
            for chunk in 1..=bytes.len() {
                assert_eq!(
                    RecordDecoder::decode_all(format, bytes),
                    decode(bytes, format, chunk),
                    "{format:?}, chunk {chunk}"
                );
            }
        }
    }

    #[test]
    fn plain_streaming_at_every_chunk_size() {
        let text = "café\r\n東京🧑‍💻\n\nrepeat\nrepeat\nlast".as_bytes();
        for size in 1..=text.len() {
            let items = decode(text, Lines, size).unwrap();
            assert_eq!(titles(&items), ["café", "東京🧑‍💻", "", "repeat", "repeat", "last"]);
            assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["0", "1", "2", "3", "4", "5"]);
            assert!(items.iter().all(|i| i.value() == i.title()));
        }
    }

    #[test]
    fn nul_preserves_newlines_and_cr() {
        let items = decode("a\r\n\0\0東京\0last\r".as_bytes(), Nul, 1).unwrap();
        assert_eq!(titles(&items), ["a\r\n", "", "東京", "last\r"]);
        assert_eq!(decode(b"a\0b\n", Lines, 1).unwrap()[0].title(), "a\0b");
        assert_eq!(decode(b"a\r", Lines, 1).unwrap()[0].title(), "a\r");
    }

    #[test]
    fn empty_and_trailing_records() {
        for format in [Lines, Nul, Jsonl] {
            assert!(decode(b"", format, 1).unwrap().is_empty());
        }
        assert_eq!(titles(&decode(b"\n\n", Lines, 1).unwrap()), ["", ""]);
        assert_eq!(titles(&decode(b"\0\0", Nul, 1).unwrap()), ["", ""]);
        assert!(decode(b"\r\n\n \t\n", Jsonl, 1).unwrap().is_empty());
        let mut decoder = RecordDecoder::new(Lines);
        assert!(decoder.append(b"").unwrap().is_empty());
        assert!(decoder.finish().unwrap().is_empty());
        assert_eq!(decoder.append(b""), Err(RecordError::AlreadyFinished));
    }

    #[test]
    fn structured_streaming_and_defaults() {
        let text = concat!(
            r#"{"id":"a","title":"café 東京","subtitle":"sub","search":"find","detail":"detail"}"#,
            "\r\n\n",
            r#"{"id":"b","title":"café 東京","value":"different"}"#,
        )
        .as_bytes();
        let mut a = MenuItem::new("a", "café 東京");
        a.subtitle = Some("sub".into());
        a.search = Some("find".into());
        a.detail = Some("detail".into());
        let b = MenuItem::new("b", "café 東京").with_value("different");
        for size in 1..=text.len() {
            assert_eq!(decode(text, Jsonl, size).unwrap(), [a.clone(), b.clone()]);
        }
    }

    #[test]
    fn duplicate_ids_across_appends_and_finish() {
        let record = br#"{"id":"a","title":"same"}
"#;
        let duplicate = RecordError::DuplicateId { record: 2, id: "a".into() };
        let mut decoder = RecordDecoder::new(Jsonl);
        assert_eq!(decoder.append(record).unwrap().len(), 1);
        assert_eq!(decoder.append(record), Err(duplicate.clone()));
        assert_eq!(decoder.finish(), Err(duplicate.clone()));
        let mut fresh = RecordDecoder::new(Jsonl);
        assert_eq!(fresh.append(record).unwrap().len(), 1);
        assert!(fresh.append(&record[..record.len() - 1]).unwrap().is_empty());
        assert_eq!(fresh.finish(), Err(duplicate));
    }

    #[test]
    fn malformed_json_and_required_fields() {
        for text in [
            "not json",
            "{}",
            "[]",
            "null",
            r#"{"id":"x"}"#,
            r#"{"title":"x"}"#,
            r#"{"id":1,"title":"x"}"#,
            r#"{"id":"x","title":null}"#,
            r#"{"id":"x","title":"x","value":2}"#,
            r#"{"id":"x","title":"x"} trailing"#,
        ] {
            match decode(text.as_bytes(), Jsonl, 1) {
                Err(RecordError::MalformedJson { record: 1, reason }) => assert!(!reason.is_empty()),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_utf8_and_incomplete_final_scalar() {
        let invalid = RecordError::InvalidUtf8 { record: 1 };
        for format in [Lines, Nul, Jsonl] {
            let delimiter = if format == Nul { 0 } else { b'\n' };
            let mut decoder = RecordDecoder::new(format);
            assert_eq!(decoder.append(&[0xFF, delimiter]), Err(invalid.clone()));
            assert_eq!(decoder.finish(), Err(invalid.clone()));
            let mut partial = RecordDecoder::new(format);
            assert!(partial.append(&[0xF0, 0x9F]).unwrap().is_empty());
            assert_eq!(partial.finish(), Err(invalid.clone()));
        }
    }

    #[test]
    fn bounded_record_and_unbounded_stream() {
        let limit = RecordDecoder::MAXIMUM_RECORD_BYTES;
        let block = [b'a'; 4096];
        let mut decoder = RecordDecoder::new(Lines);
        for _ in 0..limit / block.len() {
            assert!(decoder.append(&block).unwrap().is_empty());
        }
        assert_eq!(decoder.append(b"\n").unwrap()[0].title().len(), limit);
        for _ in 0..limit / block.len() {
            assert!(decoder.append(&block).unwrap().is_empty());
        }
        let too_large = RecordError::RecordTooLarge { record: 2 };
        assert_eq!(decoder.append(b"a"), Err(too_large.clone()));
        assert_eq!(decoder.finish(), Err(too_large));
        for format in [Nul, Jsonl] {
            let mut other = RecordDecoder::new(format);
            assert_eq!(other.append(&vec![b'a'; limit + 1]), Err(RecordError::RecordTooLarge { record: 1 }));
        }
    }
}
