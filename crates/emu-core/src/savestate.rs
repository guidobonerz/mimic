//! Savestate serialization (golden rule 8).
//!
//! The format is a sequence of tagged chunks: a 4-byte tag, a `u16` version,
//! a `u32` body length, then the body. All integers are little-endian. There
//! are no floats and no host-dependent layouts, so states are identical on
//! every platform including wasm.
//!
//! Readers look chunks up in write order and skip chunks they do not ask for,
//! so a newer writer can append chunks without breaking older readers.

use std::fmt;

/// Four-character chunk identifier, e.g. `*b"CPU0"`.
pub type Tag = [u8; 4];

/// Error while loading a savestate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// The data ended in the middle of a value or chunk.
    Truncated,
    /// A required chunk is not present.
    Missing(Tag),
    /// The chunk was written by a newer format version than this reader knows.
    UnsupportedVersion { tag: Tag, version: u16 },
    /// A value is out of range for its field.
    Invalid(&'static str),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Truncated => f.write_str("savestate is truncated"),
            StateError::Missing(tag) => {
                write!(
                    f,
                    "savestate chunk {} is missing",
                    String::from_utf8_lossy(tag)
                )
            }
            StateError::UnsupportedVersion { tag, version } => write!(
                f,
                "savestate chunk {} has unsupported version {version}",
                String::from_utf8_lossy(tag)
            ),
            StateError::Invalid(what) => write!(f, "savestate contains invalid {what}"),
        }
    }
}

impl std::error::Error for StateError {}

/// A component that can be saved and restored.
///
/// If `load` fails, the component's state is unspecified; callers that need
/// atomic loads restore into a copy and swap on success.
pub trait Savestate {
    fn save(&self, w: &mut StateWriter);
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError>;
}

/// Serializes a savestate.
#[derive(Clone, Debug, Default)]
pub struct StateWriter {
    buf: Vec<u8>,
}

/// Position of an open chunk; pass it back to [`StateWriter::end_chunk`].
#[must_use]
#[derive(Debug)]
pub struct ChunkMark {
    len_pos: usize,
}

impl StateWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn begin_chunk(&mut self, tag: Tag, version: u16) -> ChunkMark {
        self.buf.extend_from_slice(&tag);
        self.u16(version);
        let len_pos = self.buf.len();
        self.u32(0);
        ChunkMark { len_pos }
    }

    pub fn end_chunk(&mut self, mark: ChunkMark) {
        let len = (self.buf.len() - mark.len_pos - 4) as u32;
        self.buf[mark.len_pos..mark.len_pos + 4].copy_from_slice(&len.to_le_bytes());
    }

    /// Write a complete chunk whose body is produced by `body`.
    pub fn chunk(&mut self, tag: Tag, version: u16, body: impl FnOnce(&mut Self)) {
        let mark = self.begin_chunk(tag, version);
        body(self);
        self.end_chunk(mark);
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    /// Fixed-length bytes; the reader must know the length.
    pub fn raw(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    /// Length-prefixed bytes.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.raw(v);
    }

    /// Length-prefixed UTF-8 string.
    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
}

/// Deserializes a savestate. Every read is bounds-checked; malformed input
/// yields a [`StateError`], never a panic.
#[derive(Clone, Debug)]
pub struct StateReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> StateReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn is_at_end(&self) -> bool {
        self.pos == self.data.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        let end = self.pos.checked_add(n).ok_or(StateError::Truncated)?;
        let s = self.data.get(self.pos..end).ok_or(StateError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], StateError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    /// Find the next chunk with `tag`, skipping chunks with other tags.
    /// Returns the chunk version and a reader over its body.
    pub fn chunk(
        &mut self,
        tag: Tag,
        max_version: u16,
    ) -> Result<(u16, StateReader<'a>), StateError> {
        loop {
            if self.is_at_end() {
                return Err(StateError::Missing(tag));
            }
            let found: Tag = self.array()?;
            let version = self.u16()?;
            let len = self.u32()? as usize;
            let body = self.take(len)?;
            if found == tag {
                if version > max_version {
                    return Err(StateError::UnsupportedVersion { tag, version });
                }
                return Ok((version, StateReader::new(body)));
            }
        }
    }

    pub fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, StateError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, StateError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, StateError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn bool(&mut self) -> Result<bool, StateError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StateError::Invalid("bool")),
        }
    }

    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        self.take(n)
    }

    /// Copy fixed-length bytes into `out`.
    pub fn raw_into(&mut self, out: &mut [u8]) -> Result<(), StateError> {
        out.copy_from_slice(self.take(out.len())?);
        Ok(())
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], StateError> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub fn string(&mut self) -> Result<String, StateError> {
        std::str::from_utf8(self.bytes()?)
            .map(str::to_owned)
            .map_err(|_| StateError::Invalid("utf-8 string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        let mut w = StateWriter::new();
        w.chunk(*b"TEST", 1, |w| {
            w.u8(0xab);
            w.u16(0x1234);
            w.u32(0xdead_beef);
            w.u64(u64::MAX - 1);
            w.bool(true);
            w.raw(&[1, 2, 3]);
            w.bytes(&[9, 8]);
            w.str("Lives");
        });
        let data = w.into_bytes();

        let mut r = StateReader::new(&data);
        let (version, mut c) = r.chunk(*b"TEST", 1).unwrap();
        assert_eq!(version, 1);
        assert_eq!(c.u8().unwrap(), 0xab);
        assert_eq!(c.u16().unwrap(), 0x1234);
        assert_eq!(c.u32().unwrap(), 0xdead_beef);
        assert_eq!(c.u64().unwrap(), u64::MAX - 1);
        assert!(c.bool().unwrap());
        assert_eq!(c.raw(3).unwrap(), &[1, 2, 3]);
        assert_eq!(c.bytes().unwrap(), &[9, 8]);
        assert_eq!(c.string().unwrap(), "Lives");
        assert!(c.is_at_end());
        assert!(r.is_at_end());
    }

    #[test]
    fn unknown_chunks_are_skipped() {
        let mut w = StateWriter::new();
        w.chunk(*b"NEW1", 1, |w| w.u32(7));
        w.chunk(*b"CPU0", 2, |w| w.u8(42));
        let data = w.into_bytes();

        let (version, mut c) = StateReader::new(&data).chunk(*b"CPU0", 2).unwrap();
        assert_eq!(version, 2);
        assert_eq!(c.u8().unwrap(), 42);
    }

    #[test]
    fn missing_chunk_is_reported() {
        let mut w = StateWriter::new();
        w.chunk(*b"CPU0", 1, |w| w.u8(1));
        let data = w.into_bytes();
        let err = StateReader::new(&data).chunk(*b"VIC2", 1).unwrap_err();
        assert_eq!(err, StateError::Missing(*b"VIC2"));
    }

    #[test]
    fn newer_version_is_rejected() {
        let mut w = StateWriter::new();
        w.chunk(*b"CPU0", 3, |_| {});
        let data = w.into_bytes();
        let err = StateReader::new(&data).chunk(*b"CPU0", 2).unwrap_err();
        assert_eq!(
            err,
            StateError::UnsupportedVersion {
                tag: *b"CPU0",
                version: 3
            }
        );
    }

    #[test]
    fn truncated_data_never_panics() {
        let mut w = StateWriter::new();
        w.chunk(*b"CPU0", 1, |w| {
            w.u32(1);
            w.str("abc");
        });
        let data = w.into_bytes();
        for cut in 0..data.len() {
            let mut r = StateReader::new(&data[..cut]);
            let result = r.chunk(*b"CPU0", 1).and_then(|(_, mut c)| {
                c.u32()?;
                c.string()
            });
            assert!(result.is_err(), "cut at {cut} must fail");
        }
    }

    #[test]
    fn invalid_bool_and_utf8_are_rejected() {
        assert_eq!(
            StateReader::new(&[2]).bool(),
            Err(StateError::Invalid("bool"))
        );
        let mut w = StateWriter::new();
        w.bytes(&[0xff, 0xfe]);
        let data = w.into_bytes();
        assert!(StateReader::new(&data).string().is_err());
    }
}
