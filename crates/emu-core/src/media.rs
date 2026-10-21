//! Drives hold media in slots; media bytes live in a backend.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaError {
    NoMedia,
    WriteProtected,
    OutOfRange,
    /// The slot does not accept this kind of media.
    Unsupported(MediaKind),
    /// The image is malformed.
    Format(String),
    /// The backend failed (file system, hardware).
    Backend(String),
    /// A control command the slot does not know or cannot do now.
    Control(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::NoMedia => f.write_str("no media inserted"),
            MediaError::WriteProtected => f.write_str("media is write-protected"),
            MediaError::OutOfRange => f.write_str("access beyond the end of the media"),
            MediaError::Unsupported(kind) => write!(f, "slot does not accept {kind:?} media"),
            MediaError::Format(msg) => write!(f, "invalid image: {msg}"),
            MediaError::Backend(msg) => write!(f, "media backend error: {msg}"),
            MediaError::Control(msg) => write!(f, "media control: {msg}"),
        }
    }
}

impl std::error::Error for MediaError {}

/// Where media bytes live: image file, copy-on-write overlay, host directory,
/// real hardware.
pub trait MediaBackend: Send {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError>;

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError>;

    /// Make writes permanent, e.g. apply a copy-on-write overlay to the original.
    fn commit(&mut self) -> Result<(), MediaError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Disk,
    Tape,
    Cartridge,
    /// Loadable program file (PRG, XEX).
    Program,
}

/// A piece of media: a name, its kind and the backend holding its bytes.
pub struct Media {
    pub name: String,
    pub kind: MediaKind,
    pub backend: Box<dyn MediaBackend>,
}

impl fmt::Debug for Media {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Media")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("len", &self.backend.len())
            .finish()
    }
}

/// A drive slot: drive 8, DF0:, D1:, tape, cartridge port.
pub trait MediaSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError>;
    fn eject(&mut self) -> Option<Media>;
    fn write_protect(&mut self, on: bool);
    /// Write back through the backend.
    fn flush(&mut self) -> Result<(), MediaError>;

    /// Operate the drive's own controls, e.g. a datasette's "play",
    /// "record", "stop" and "rewind". Slots without controls refuse.
    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        Err(MediaError::Control(format!(
            "'{command}': this slot has no controls"
        )))
    }

    /// What the drive shows right now (activity LED, motor, error), for
    /// status displays. Side-effect free. Slots without a drive report
    /// [`DriveStatus::default`].
    fn status(&self) -> DriveStatus {
        DriveStatus::default()
    }
}

/// A drive's front panel at one moment: derived from emulated hardware
/// (LED and motor lines, a virtual drive's error channel), never stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriveStatus {
    /// Reading, writing or seeking: the activity LED is lit or the motor
    /// runs (tape: the motor runs).
    pub active: bool,
    /// The drive signals an error: a flashing LED, or an error code (20 or
    /// above) in a virtual drive's status channel.
    pub error: bool,
    /// Track under the head (1-based), where the drive has one.
    pub track: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaSlotInfo {
    pub id: SlotId,
    /// Hardware name, e.g. "drive 8", "DF0:".
    pub name: &'static str,
    pub kind: MediaKind,
    pub inserted: bool,
}
