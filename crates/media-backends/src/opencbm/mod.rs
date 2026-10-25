//! Real Commodore drives (1541, 1570, 1571, 1581) on the host's IEC bus
//! through OpenCBM — the ZoomFloppy (XUM1541) or another adapter.
//!
//! Sectors move with the drive's own DOS, the plain documented route
//! (slower than OpenCBM's turbo tools, but it needs no code uploaded into
//! the drive): a buffer channel is opened with the name `#` on secondary
//! address 2; a sector is read with `U1 2 0 t s` (block read into that
//! buffer), the error channel is checked, `B-P 2 0` sets the buffer
//! pointer to 0 and 256 bytes are read by TALK on channel 2. Writing is
//! `B-P 2 0`, 256 bytes by LISTEN on channel 2, then `U2 2 0 t s`. A 1571
//! goes into double-sided mode with `U0>M1` before tracks 36–70.
//! Sources: Commodore 1541 User's Guide (chapter "Direct access
//! commands": U1/U2, B-P, buffer channel `#`), Commodore 1571 User's
//! Guide (`U0>M1`), Commodore 1581 User's Guide; OpenCBM API (see
//! [`ffi`]).
//!
//! - [`read_disk`]/[`write_disk`]: a whole disk into a D64/D71/D81 image
//!   (with error bytes when sectors fail) and back.
//! - [`RealDriveBackend`]: the real drive as a [`MediaBackend`]: reads
//!   whole tracks into a cache, writes changed sectors through.
//!
//! The DOS calls go through [`CbmBus`], so the logic is tested with a
//! simulated drive; [`ffi::OpenCbm`] is the real one.

#[cfg(feature = "opencbm")]
pub mod ffi;

use std::collections::BTreeMap;

use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use media::d64::{Format, SECTOR_SIZE};
use media::D64;

/// The OpenCBM calls the drive layer needs (one method per library
/// function, see [`ffi`]).
pub trait CbmBus: Send {
    fn exec_command(&mut self, dev: u8, cmd: &[u8]) -> Result<(), MediaError>;
    /// The error channel: number and text ("00, OK,00,00").
    fn device_status(&mut self, dev: u8) -> Result<(u8, String), MediaError>;
    fn open(&mut self, dev: u8, secondary: u8, name: &[u8]) -> Result<(), MediaError>;
    fn close(&mut self, dev: u8, secondary: u8) -> Result<(), MediaError>;
    fn talk(&mut self, dev: u8, secondary: u8) -> Result<(), MediaError>;
    fn untalk(&mut self) -> Result<(), MediaError>;
    fn listen(&mut self, dev: u8, secondary: u8) -> Result<(), MediaError>;
    fn unlisten(&mut self) -> Result<(), MediaError>;
    fn raw_read(&mut self, buf: &mut [u8]) -> Result<usize, MediaError>;
    fn raw_write(&mut self, data: &[u8]) -> Result<usize, MediaError>;
    /// OpenCBM's device type number and name.
    fn identify(&mut self, dev: u8) -> Result<(i32, String), MediaError>;
}

/// The drive model, as OpenCBM identifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    C1541,
    C1570,
    C1571,
    C1581,
}

impl DriveKind {
    /// From `cbm_identify`'s type number (enum `cbm_device_type_e`:
    /// 0 = 1541, 1 = 1570, 2 = 1571, 3 = 1581) or its name.
    pub fn from_identify(kind: i32, name: &str) -> Option<Self> {
        match kind {
            0 => Some(DriveKind::C1541),
            1 => Some(DriveKind::C1570),
            2 => Some(DriveKind::C1571),
            3 => Some(DriveKind::C1581),
            _ if name.contains("1581") => Some(DriveKind::C1581),
            _ if name.contains("1571") => Some(DriveKind::C1571),
            _ if name.contains("1570") => Some(DriveKind::C1570),
            _ if name.contains("1541") => Some(DriveKind::C1541),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1541" => Some(DriveKind::C1541),
            "1570" => Some(DriveKind::C1570),
            "1571" => Some(DriveKind::C1571),
            "1581" => Some(DriveKind::C1581),
            _ => None,
        }
    }
}

/// The disk layout to read: a D64 of 35 or 40 tracks, a D71 or a D81.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    D64 { tracks: u8 },
    D71,
    D81,
}

impl Layout {
    /// The natural layout of a drive.
    pub fn of(kind: DriveKind) -> Self {
        match kind {
            DriveKind::C1541 | DriveKind::C1570 => Layout::D64 { tracks: 35 },
            DriveKind::C1571 => Layout::D71,
            DriveKind::C1581 => Layout::D81,
        }
    }

    /// From an image name's extension (`.d64`, `.d71`, `.d81`).
    pub fn for_name(name: &str) -> Option<Self> {
        let ext = std::path::Path::new(name)
            .extension()?
            .to_string_lossy()
            .to_lowercase();
        match ext.as_str() {
            "d64" => Some(Layout::D64 { tracks: 35 }),
            "d71" => Some(Layout::D71),
            "d81" => Some(Layout::D81),
            _ => None,
        }
    }

    /// An empty image of this layout (geometry only).
    pub fn blank(self) -> D64 {
        let sectors: usize = match self {
            Layout::D64 { tracks } => (1..=tracks)
                .map(|t| usize::from(media::d64::sectors_per_track(t)))
                .sum(),
            Layout::D71 => 2 * 683,
            Layout::D81 => 3200,
        };
        D64::parse(&vec![0; sectors * SECTOR_SIZE]).unwrap_or_else(|_| D64::blank(b"", *b"00"))
    }

    pub fn tracks(self) -> u8 {
        match self {
            Layout::D64 { tracks } => tracks,
            Layout::D71 => 70,
            Layout::D81 => 80,
        }
    }

    /// Whether a drive can read this layout.
    pub fn fits(self, kind: DriveKind) -> bool {
        match self {
            Layout::D64 { .. } => kind != DriveKind::C1581,
            Layout::D71 => kind == DriveKind::C1571,
            Layout::D81 => kind == DriveKind::C1581,
        }
    }
}

/// D64 error byte for a DOS error number (20 → 2 … 29 → 11; others 0).
fn error_byte(code: u8) -> u8 {
    match code {
        0..=1 => 1,
        20..=29 => code - 18,
        74 => 0x0f,
        _ => 0,
    }
}

/// A real drive at a device number.
pub struct CbmDrive<B: CbmBus> {
    bus: B,
    pub device: u8,
    pub kind: DriveKind,
    open: bool,
    double_sided: bool,
}

/// The buffer channel's secondary address.
const CHANNEL: u8 = 2;

impl<B: CbmBus> CbmDrive<B> {
    /// Identify the drive (or take `kind` as given) and open a buffer
    /// channel.
    pub fn new(mut bus: B, device: u8, kind: Option<DriveKind>) -> Result<Self, MediaError> {
        let kind = match kind {
            Some(k) => k,
            None => {
                let (n, name) = bus.identify(device)?;
                DriveKind::from_identify(n, &name).ok_or_else(|| {
                    MediaError::Backend(format!(
                        "drive {device}: unsupported drive type '{name}' ({n}); 1541, 1570, 1571 and 1581 work"
                    ))
                })?
            }
        };
        let mut drive = Self {
            bus,
            device,
            kind,
            open: false,
            double_sided: false,
        };
        // Clear a pending error (e.g. 73, the power-on message).
        drive.bus.device_status(device)?;
        drive.bus.open(device, CHANNEL, b"#")?;
        drive.open = true;
        drive.status("open buffer")?;
        Ok(drive)
    }

    fn command(&mut self, cmd: &str) -> Result<(), MediaError> {
        self.bus.exec_command(self.device, cmd.as_bytes())
    }

    /// The error channel; Err for anything but 00 (and 73 at power-on).
    fn status(&mut self, what: &str) -> Result<u8, MediaError> {
        let (code, text) = self.bus.device_status(self.device)?;
        match code {
            0 | 1 | 73 => Ok(code),
            26 => Err(MediaError::WriteProtected),
            20..=29 => Ok(code),
            _ => Err(MediaError::Backend(format!(
                "drive {}: {what}: {text}",
                self.device
            ))),
        }
    }

    fn prepare_track(&mut self, track: u8) -> Result<(), MediaError> {
        if track > 35 && self.kind == DriveKind::C1571 && !self.double_sided {
            self.command("U0>M1")?;
            self.double_sided = true;
        }
        Ok(())
    }

    /// Read a sector; on a read error (DOS 20–29) zeros and the DOS
    /// error number.
    pub fn read_sector(&mut self, track: u8, sector: u8) -> Result<([u8; 256], u8), MediaError> {
        self.prepare_track(track)?;
        self.command(&format!("U1 {CHANNEL} 0 {track} {sector}"))?;
        let code = self.status(&format!("read {track}/{sector}"))?;
        if (20..=29).contains(&code) {
            return Ok(([0; 256], code));
        }
        self.command(&format!("B-P {CHANNEL} 0"))?;
        let mut buf = [0u8; 256];
        self.bus.talk(self.device, CHANNEL)?;
        let mut at = 0;
        while at < buf.len() {
            match self.bus.raw_read(&mut buf[at..]) {
                Ok(0) => break,
                Ok(n) => at += n,
                Err(e) => {
                    let _ = self.bus.untalk();
                    return Err(e);
                }
            }
        }
        self.bus.untalk()?;
        if at < buf.len() {
            return Err(MediaError::Backend(format!(
                "drive {}: sector {track}/{sector}: only {at} bytes",
                self.device
            )));
        }
        Ok((buf, 0))
    }

    pub fn write_sector(&mut self, track: u8, sector: u8, data: &[u8]) -> Result<(), MediaError> {
        if data.len() != 256 {
            return Err(MediaError::OutOfRange);
        }
        self.prepare_track(track)?;
        self.command(&format!("B-P {CHANNEL} 0"))?;
        self.bus.listen(self.device, CHANNEL)?;
        let sent = self.bus.raw_write(data);
        self.bus.unlisten()?;
        if sent? != data.len() {
            return Err(MediaError::Backend(format!(
                "drive {}: short write to the buffer",
                self.device
            )));
        }
        self.command(&format!("U2 {CHANNEL} 0 {track} {sector}"))?;
        match self.status(&format!("write {track}/{sector}"))? {
            0 | 1 => Ok(()),
            code => Err(MediaError::Backend(format!(
                "drive {}: write {track}/{sector}: DOS error {code}",
                self.device
            ))),
        }
    }

    /// Close the buffer channel.
    pub fn close(&mut self) -> Result<(), MediaError> {
        if std::mem::replace(&mut self.open, false) {
            self.bus.close(self.device, CHANNEL)?;
        }
        Ok(())
    }

    pub fn bus(&mut self) -> &mut B {
        &mut self.bus
    }
}

impl<B: CbmBus> Drop for CbmDrive<B> {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// A disk read from a real drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskRead {
    /// The image; with one error byte per sector when sectors failed.
    pub image: Vec<u8>,
    /// (track, sector, DOS error) of every failed sector.
    pub errors: Vec<(u8, u8, u8)>,
}

/// Read a whole disk. `progress` hears each track as it is done.
pub fn read_disk<B: CbmBus>(
    drive: &mut CbmDrive<B>,
    layout: Layout,
    progress: &mut dyn FnMut(u8, u8),
) -> Result<DiskRead, MediaError> {
    if !layout.fits(drive.kind) {
        return Err(MediaError::Format(format!(
            "a {:?} cannot read a {layout:?} disk",
            drive.kind
        )));
    }
    let mut image = layout.blank();
    let mut codes = Vec::new();
    let mut errors = Vec::new();
    for t in 1..=layout.tracks() {
        for s in 0..image.sectors_in(t) {
            let (data, code) = drive.read_sector(t, s)?;
            if code != 0 {
                errors.push((t, s, code));
            }
            codes.push(error_byte(code));
            if let Some(dst) = image.sector_mut(t, s) {
                dst.copy_from_slice(&data);
            }
        }
        progress(t, layout.tracks());
    }
    let mut bytes = image.to_bytes();
    if !errors.is_empty() {
        bytes.extend_from_slice(&codes);
    }
    Ok(DiskRead {
        image: bytes,
        errors,
    })
}

/// Write an image to the disk in the drive (formatted, same layout).
pub fn write_disk<B: CbmBus>(
    drive: &mut CbmDrive<B>,
    image: &[u8],
    progress: &mut dyn FnMut(u8, u8),
) -> Result<(), MediaError> {
    let d = D64::parse(image)?;
    let layout = match d.format {
        Format::D64 => Layout::D64 { tracks: d.tracks },
        Format::D71 => Layout::D71,
        Format::D81 => Layout::D81,
        other => {
            return Err(MediaError::Format(format!(
                "{other:?} images cannot be written to a 1541/1571/1581"
            )))
        }
    };
    if !layout.fits(drive.kind) {
        return Err(MediaError::Format(format!(
            "a {:?} cannot write a {layout:?} image",
            drive.kind
        )));
    }
    for t in 1..=d.tracks {
        for s in 0..d.sectors_in(t) {
            if let Some(data) = d.sector(t, s) {
                drive.write_sector(t, s, data)?;
            }
        }
        progress(t, d.tracks);
    }
    Ok(())
}

/// The real drive as media: an image of `layout` whose sectors come from
/// the drive, a track at a time on first access; writes of changed
/// sectors go straight to the disk.
pub struct RealDriveBackend<B: CbmBus> {
    drive: CbmDrive<B>,
    geometry: D64,
    /// Image byte offset of each track's first sector (index = track).
    starts: Vec<u64>,
    cache: BTreeMap<u8, Vec<[u8; 256]>>,
    /// Sectors that failed to read: (track, sector, DOS error).
    pub errors: Vec<(u8, u8, u8)>,
}

impl<B: CbmBus> RealDriveBackend<B> {
    pub fn new(drive: CbmDrive<B>, layout: Layout) -> Result<Self, MediaError> {
        if !layout.fits(drive.kind) {
            return Err(MediaError::Format(format!(
                "a {:?} cannot hold a {layout:?} disk",
                drive.kind
            )));
        }
        let geometry = layout.blank();
        let mut starts = vec![0u64];
        let mut at = 0u64;
        for t in 1..=geometry.tracks {
            starts.push(at);
            at += u64::from(geometry.sectors_in(t)) * SECTOR_SIZE as u64;
        }
        starts.push(at);
        Ok(Self {
            drive,
            geometry,
            starts,
            cache: BTreeMap::new(),
            errors: Vec::new(),
        })
    }

    /// The track holding image byte `offset`.
    fn track_at(&self, offset: u64) -> u8 {
        let t = self.starts[1..].partition_point(|&s| s <= offset);
        t.clamp(1, usize::from(self.geometry.tracks)) as u8
    }

    fn load(&mut self, track: u8) -> Result<(), MediaError> {
        if self.cache.contains_key(&track) {
            return Ok(());
        }
        let n = self.geometry.sectors_in(track);
        let mut sectors = Vec::with_capacity(usize::from(n));
        for s in 0..n {
            let (data, code) = self.drive.read_sector(track, s)?;
            if code != 0 {
                self.errors.push((track, s, code));
            }
            sectors.push(data);
        }
        self.cache.insert(track, sectors);
        Ok(())
    }

    /// (track, sector, offset in sector) of an image byte.
    fn locate(&self, offset: u64) -> (u8, u8, usize) {
        let t = self.track_at(offset);
        let rel = offset - self.starts[usize::from(t)];
        (
            t,
            (rel / SECTOR_SIZE as u64) as u8,
            (rel % SECTOR_SIZE as u64) as usize,
        )
    }

    pub fn into_drive(self) -> CbmDrive<B> {
        self.drive
    }
}

impl<B: CbmBus> MediaBackend for RealDriveBackend<B> {
    fn len(&self) -> u64 {
        self.starts[usize::from(self.geometry.tracks) + 1]
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|&e| e <= self.len())
            .ok_or(MediaError::OutOfRange)?;
        let mut at = offset;
        while at < end {
            let (t, s, o) = self.locate(at);
            self.load(t)?;
            let n = ((SECTOR_SIZE - o) as u64).min(end - at) as usize;
            let src = &self.cache[&t][usize::from(s)][o..o + n];
            let dst = (at - offset) as usize;
            buf[dst..dst + n].copy_from_slice(src);
            at += n as u64;
        }
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let end = offset
            .checked_add(data.len() as u64)
            .filter(|&e| e <= self.len())
            .ok_or(MediaError::OutOfRange)?;
        let mut at = offset;
        while at < end {
            let (t, s, o) = self.locate(at);
            self.load(t)?;
            let n = ((SECTOR_SIZE - o) as u64).min(end - at) as usize;
            let src = &data[(at - offset) as usize..][..n];
            let cached = self.cache[&t][usize::from(s)];
            if cached[o..o + n] != *src {
                let mut sector = cached;
                sector[o..o + n].copy_from_slice(src);
                self.drive.write_sector(t, s, &sector)?;
                if let Some(c) = self.cache.get_mut(&t) {
                    c[usize::from(s)] = sector;
                }
            }
            at += n as u64;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        // Writes already went to the disk.
        Ok(())
    }
}

/// Settings for a real drive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealDriveOptions {
    /// OpenCBM adapter, e.g. "xum1541" (None = OpenCBM's default).
    pub adapter: Option<String>,
    pub device: u8,
    /// Drive type instead of asking the drive.
    pub kind: Option<DriveKind>,
    /// Layout instead of the drive's natural one (e.g. 40 tracks).
    pub layout: Option<Layout>,
}

/// Open the real drive through OpenCBM.
#[cfg(feature = "opencbm")]
pub fn open_drive(opts: &RealDriveOptions) -> Result<CbmDrive<ffi::OpenCbm>, MediaError> {
    let bus = ffi::OpenCbm::open(opts.adapter.as_deref())?;
    CbmDrive::new(bus, opts.device, opts.kind)
}

/// The real drive as media for an emulated drive slot. Writes go to the
/// disk only when `writable`; otherwise they stay in a copy-on-write
/// overlay (as with image files).
#[cfg(feature = "opencbm")]
pub fn open_media(opts: &RealDriveOptions, writable: bool) -> Result<Media, MediaError> {
    let drive = open_drive(opts)?;
    media_from_drive(drive, opts.layout, writable)
}

/// [`open_media`] for an already opened drive (any bus).
pub fn media_from_drive<B: CbmBus + 'static>(
    drive: CbmDrive<B>,
    layout: Option<Layout>,
    writable: bool,
) -> Result<Media, MediaError> {
    let layout = layout.unwrap_or(Layout::of(drive.kind));
    let name = format!("drive {} (real {:?})", drive.device, drive.kind);
    let backend = RealDriveBackend::new(drive, layout)?;
    let backend: Box<dyn MediaBackend> = if writable {
        Box::new(backend)
    } else {
        Box::new(crate::image::CowOverlay::new(Box::new(backend)))
    };
    Ok(Media {
        name,
        kind: MediaKind::Disk,
        backend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A drive's DOS in software: a buffer channel, U1/U2/B-P and the
    /// error channel, on a D64/D71/D81 image.
    #[derive(Default)]
    struct FakeState {
        disk: Option<D64>,
        kind: i32,
        buffer: Vec<u8>,
        pointer: usize,
        channel_open: bool,
        status: (u8, String),
        talking: bool,
        listening: bool,
        write_protected: bool,
        bad: Vec<(u8, u8)>,
        double_sided: bool,
        commands: Vec<String>,
        sector_reads: usize,
        sector_writes: usize,
    }

    #[derive(Clone, Default)]
    struct FakeBus(Arc<Mutex<FakeState>>);

    impl FakeBus {
        fn with(disk: D64, kind: i32) -> Self {
            let s = FakeState {
                disk: Some(disk),
                kind,
                status: (73, "73,CBM DOS V2.6 1541,00,00".into()),
                buffer: vec![0; 256],
                ..FakeState::default()
            };
            Self(Arc::new(Mutex::new(s)))
        }

        fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.0.lock().unwrap()
        }
    }

    fn ok() -> (u8, String) {
        (0, "00, OK,00,00".into())
    }

    impl CbmBus for FakeBus {
        fn exec_command(&mut self, dev: u8, cmd: &[u8]) -> Result<(), MediaError> {
            assert_eq!(dev, 8);
            let mut st = self.state();
            let text = String::from_utf8(cmd.to_vec()).unwrap();
            st.commands.push(text.clone());
            let args: Vec<usize> = text
                .get(text.find(' ').map_or(text.len(), |i| i + 1)..)
                .unwrap_or("")
                .split(' ')
                .filter_map(|a| a.parse().ok())
                .collect();
            st.status = ok();
            if text == "U0>M1" {
                st.double_sided = true;
            } else if let Some(rest) = text.strip_prefix("B-P ") {
                assert!(rest.starts_with("2 "));
                st.pointer = args[1];
            } else if text.starts_with("U1 ") || text.starts_with("U2 ") {
                assert!(st.channel_open, "no buffer channel");
                assert_eq!(args[0], 2);
                let (t, s) = (args[2] as u8, args[3] as u8);
                if t > 35 && st.kind == 2 && !st.double_sided {
                    st.status = (66, "66,ILLEGAL TRACK OR SECTOR,36,00".into());
                    return Ok(());
                }
                if st.bad.contains(&(t, s)) {
                    st.status = (23, format!("23,READ ERROR,{t:02},{s:02}"));
                    return Ok(());
                }
                if text.starts_with("U1") {
                    st.sector_reads += 1;
                    let data = st.disk.as_ref().unwrap().sector(t, s).map(<[u8]>::to_vec);
                    match data {
                        Some(d) => st.buffer.copy_from_slice(&d),
                        None => st.status = (66, "66,ILLEGAL TRACK OR SECTOR".into()),
                    }
                    st.pointer = 0;
                } else if st.write_protected {
                    st.status = (26, "26,WRITE PROTECT ON,00,00".into());
                } else {
                    st.sector_writes += 1;
                    let buf = st.buffer.clone();
                    st.disk
                        .as_mut()
                        .unwrap()
                        .sector_mut(t, s)
                        .unwrap()
                        .copy_from_slice(&buf);
                }
            } else {
                st.status = (31, "31,SYNTAX ERROR,00,00".into());
            }
            Ok(())
        }

        fn device_status(&mut self, _dev: u8) -> Result<(u8, String), MediaError> {
            let mut st = self.state();
            Ok(std::mem::replace(&mut st.status, ok()))
        }

        fn open(&mut self, _dev: u8, secondary: u8, name: &[u8]) -> Result<(), MediaError> {
            assert_eq!((secondary, name), (2, &b"#"[..]));
            self.state().channel_open = true;
            Ok(())
        }

        fn close(&mut self, _dev: u8, secondary: u8) -> Result<(), MediaError> {
            assert_eq!(secondary, 2);
            self.state().channel_open = false;
            Ok(())
        }

        fn talk(&mut self, _dev: u8, secondary: u8) -> Result<(), MediaError> {
            assert_eq!(secondary, 2);
            self.state().talking = true;
            Ok(())
        }

        fn untalk(&mut self) -> Result<(), MediaError> {
            self.state().talking = false;
            Ok(())
        }

        fn listen(&mut self, _dev: u8, secondary: u8) -> Result<(), MediaError> {
            assert_eq!(secondary, 2);
            self.state().listening = true;
            Ok(())
        }

        fn unlisten(&mut self) -> Result<(), MediaError> {
            self.state().listening = false;
            Ok(())
        }

        fn raw_read(&mut self, buf: &mut [u8]) -> Result<usize, MediaError> {
            let mut st = self.state();
            assert!(st.talking);
            // Hand out at most 100 bytes per call, like a slow bus.
            let n = buf.len().min(256 - st.pointer).min(100);
            let p = st.pointer;
            buf[..n].copy_from_slice(&st.buffer[p..p + n]);
            st.pointer += n;
            Ok(n)
        }

        fn raw_write(&mut self, data: &[u8]) -> Result<usize, MediaError> {
            let mut st = self.state();
            assert!(st.listening);
            let p = st.pointer;
            let n = data.len().min(256 - p);
            st.buffer[p..p + n].copy_from_slice(&data[..n]);
            st.pointer += n;
            Ok(n)
        }

        fn identify(&mut self, _dev: u8) -> Result<(i32, String), MediaError> {
            let k = self.state().kind;
            Ok((k, ["1541", "1570", "1571", "1581"][k as usize].into()))
        }
    }

    fn sample() -> D64 {
        let mut d = D64::blank(b"REAL DRIVE", *b"RD");
        let data: Vec<u8> = (0..9000u32).map(|i| (i * 17) as u8).collect();
        d.write_file(b"PROG", 2, &data).unwrap();
        d
    }

    #[test]
    fn whole_disk_read_and_written_back() {
        let d = sample();
        let bus = FakeBus::with(d.clone(), 0);
        let mut drive = CbmDrive::new(bus.clone(), 8, None).unwrap();
        assert_eq!(drive.kind, DriveKind::C1541);
        let mut tracks = 0;
        let read = read_disk(&mut drive, Layout::D64 { tracks: 35 }, &mut |_, _| {
            tracks += 1
        })
        .unwrap();
        assert_eq!(tracks, 35);
        assert!(read.errors.is_empty());
        assert_eq!(read.image, d.to_bytes());
        assert_eq!(bus.state().sector_reads, 683);
        assert!(bus.state().commands.contains(&"U1 2 0 18 0".to_string()));

        // Write a changed image back.
        let mut changed = d.clone();
        changed.sector_mut(17, 3).unwrap().fill(0xee);
        write_disk(&mut drive, &changed.to_bytes(), &mut |_, _| {}).unwrap();
        assert_eq!(bus.state().disk.as_ref().unwrap(), &changed);
        drop(drive);
        assert!(!bus.state().channel_open, "channel closed on drop");

        // A 1541 refuses D81 layouts.
        let mut drive = CbmDrive::new(FakeBus::with(d, 0), 8, None).unwrap();
        assert!(read_disk(&mut drive, Layout::D81, &mut |_, _| {}).is_err());
    }

    #[test]
    fn bad_sectors_become_error_bytes() {
        let d = sample();
        let bus = FakeBus::with(d.clone(), 0);
        bus.state().bad.push((20, 4));
        let mut drive = CbmDrive::new(bus, 8, None).unwrap();
        let read = read_disk(&mut drive, Layout::D64 { tracks: 35 }, &mut |_, _| {}).unwrap();
        assert_eq!(read.errors, [(20, 4, 23)]);
        let back = D64::parse(&read.image).unwrap();
        assert_eq!(back.error(20, 4), 5);
        assert_eq!(back.error(20, 3), 1);
        assert_eq!(back.sector(20, 4).unwrap(), &[0u8; 256][..]);
        assert_eq!(back.sector(18, 0), d.sector(18, 0));
    }

    #[test]
    fn d71_switches_the_1571_to_double_sided() {
        let mut d = D64::blank_d71(b"SIDES", *b"SD");
        d.sector_mut(53, 1).unwrap().fill(0x71);
        let bus = FakeBus::with(d.clone(), 2);
        let mut drive = CbmDrive::new(bus.clone(), 8, None).unwrap();
        assert_eq!(drive.kind, DriveKind::C1571);
        let read = read_disk(&mut drive, Layout::D71, &mut |_, _| {}).unwrap();
        assert!(read.errors.is_empty(), "{:?}", read.errors);
        assert_eq!(read.image, d.to_bytes());
        let cmds = &bus.state().commands;
        let m1 = cmds.iter().position(|c| c == "U0>M1").unwrap();
        let t36 = cmds.iter().position(|c| c == "U1 2 0 36 0").unwrap();
        assert!(m1 < t36);
    }

    #[test]
    fn backend_reads_tracks_on_demand_and_writes_through() {
        let d = sample();
        let bus = FakeBus::with(d.clone(), 0);
        let drive = CbmDrive::new(bus.clone(), 8, None).unwrap();
        let mut be = RealDriveBackend::new(drive, Layout::D64 { tracks: 35 }).unwrap();
        assert_eq!(be.len(), 174_848);
        // Track 18 sector 0 (the BAM): only track 18 is read.
        let bam = 357 * 256;
        let mut buf = vec![0u8; 300];
        be.read(bam - 10, &mut buf).unwrap();
        assert_eq!(&buf[10..266], d.sector(18, 0).unwrap());
        assert_eq!(&buf[..10], &d.sector(17, 20).unwrap()[246..]);
        assert_eq!(bus.state().sector_reads, 21 + 19);
        // Whole image, as an emulated drive reads it on insert.
        let mut all = vec![0u8; be.len() as usize];
        be.read(0, &mut all).unwrap();
        assert_eq!(all, d.to_bytes());
        assert_eq!(bus.state().sector_reads, 683);
        // Writing the image back unchanged sends nothing; one changed
        // byte sends one sector.
        all[bam as usize + 5] ^= 0xff;
        be.write(0, &all).unwrap();
        assert_eq!(bus.state().sector_writes, 1);
        assert_eq!(
            bus.state().disk.as_ref().unwrap().sector(18, 0).unwrap()[5],
            all[bam as usize + 5]
        );
        assert_eq!(be.read(174_848, &mut [0]), Err(MediaError::OutOfRange));

        // Write-protected disk.
        bus.state().write_protected = true;
        assert_eq!(be.write(0, &[0xaa]), Err(MediaError::WriteProtected));
    }

    #[test]
    fn media_stays_in_an_overlay_unless_writable() {
        let d = sample();
        let bus = FakeBus::with(d.clone(), 0);
        let drive = CbmDrive::new(bus.clone(), 8, None).unwrap();
        let mut m = media_from_drive(drive, None, false).unwrap();
        assert_eq!(m.kind, MediaKind::Disk);
        assert_eq!(m.name, "drive 8 (real C1541)");
        m.backend.write(0, &[1, 2, 3]).unwrap();
        assert_eq!(bus.state().sector_writes, 0);
        let mut b = [0u8; 3];
        m.backend.read(0, &mut b).unwrap();
        assert_eq!(b, [1, 2, 3]);

        // A 1581, writable: the write reaches the disk.
        let d81 = D64::blank_d81(b"D81", *b"81");
        let bus = FakeBus::with(d81, 3);
        let drive = CbmDrive::new(bus.clone(), 8, None).unwrap();
        let mut m = media_from_drive(drive, None, true).unwrap();
        assert_eq!(m.backend.len(), 819_200);
        assert_eq!(bus.state().sector_reads, 0, "nothing read before use");
        m.backend.write(0, &[9]).unwrap();
        assert_eq!(bus.state().sector_reads, 40);
        assert_eq!(bus.state().sector_writes, 1);
        assert_eq!(
            bus.state().disk.as_ref().unwrap().sector(1, 0).unwrap()[0],
            9
        );
    }
}
