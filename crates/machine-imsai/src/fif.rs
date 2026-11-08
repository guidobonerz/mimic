//! IMSAI FIF floppy disk interface (IFM interface master + FIB floppy
//! interface board) on I/O port $FD, with up to four 8-inch drives
//! (IBM 3740 format: 77 tracks of 26 sectors of 128 bytes; images are the
//! raw 256 256 bytes, track by track, as z80pack's IMSAI disks are).
//!
//! The FIF has its own 8080 (the IFM) running firmware: the system's CPU
//! sends it single bytes with OUT $FD and the IFM fetches command strings
//! from system memory by DMA and puts the data and a status byte back the
//! same way. This module models that firmware's behaviour, not its
//! instructions.
//!
//! **Single-byte commands** (high nibble, low nibble = argument):
//! `$0n` execute the command string pointer `n` points at; `$1n` set
//! pointer `n` from the next two bytes (LSB, MSB); `$2d` restore drives
//! `d` (drive bits) over track 0; `$3d` software write protect, `$4d`
//! write enable; anything else does nothing. Pointers start at $0080 for
//! pointer 0 and `n` × $1000 for the others. The CPU waits in the OUT
//! until the IFM takes the byte, which while a disk operation runs is when
//! it ends.
//!
//! **Command string** (7 bytes, 9 with a logical track): command (function
//! in the high nibble, one drive bit in the low nibble), status (must be 0;
//! the IFM writes the result here), track MSB (0), track (0–76), sector
//! (1–26; not for format), buffer address LSB, MSB (read and write only),
//! then for functions 7–A the logical track MSB and LSB. Functions: 0 read
//! all, 1 write, 2 read, 3 format track, 4 verify (read without transfer),
//! 5 write deleted data, 7–A as 1–4 with a logical track. Status 1 is
//! done; errors: $A1 drive not ready, $A2 write protected (drive), $A3
//! write protected (software), $C1 status byte not zero, $C2 no drive, $C3
//! more than one drive, $C4 illegal function, $C5 bad track, $C6 bad
//! sector, $C8 bad logical track.
//!
//! **Bootstrap**: on power-up and RESET, with a disk in drive 0, the IFM
//! puts JMP $0087 at 0 and at $0080 a command string reading track 0
//! sector 1 to 0 followed by a loop that executes it, waits for the status
//! and jumps to 0 (to $0087 again on an error, its code shown on the
//! programmed output): RESET and RUN boot the disk.
//!
//! **Timing** after the firmware's own delays: 6 ms a step, 10 ms for the
//! head to settle after the last one, 16 ms to load the head when no step
//! is needed and the head is up; the head comes up two revolutions after
//! the last operation; the disk turns at 360 rpm, a sector passing in
//! 1/26 of a revolution, so a read or write ends when its sector has gone
//! by and a format takes one revolution from the index hole.
//!
//! **[S]** (this project's choices): read all (function 0) answers done
//! without transferring data (the raw track bytes with gaps and marks do
//! not exist in a sector image); a logical track is accepted and the
//! physical one used; write deleted data writes the sector like write,
//! from the last buffer address; the FIF's interrupt request (a jumper to
//! one of the vectored interrupt lines) is not fitted; the IFM takes a
//! byte the instant it is sent when idle; the sector's place on the track
//! is its number's share of the revolution (gaps are ignored).
//!
//! Sources: IMSAI "Floppy Interface Board Firmware" manual (functional
//! description, theory of operation: power-up bootstrap, scan loop,
//! subroutines) and its source `fifware.asm` (21/26 Oct 1976), both from
//! the z80pack archive (www.icl1900.co.uk/unix4fun/z80pack/ftp/imsai); the
//! MPU-B monitor's floppy routines (`mpu-b-rom.asm`, Bruce Hall's
//! listings). The implementation is this project's own.

use std::collections::VecDeque;

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

/// The FIF's I/O port.
pub const PORT: u8 = 0xfd;
pub const TRACKS: usize = 77;
pub const SECTORS: usize = 26;
pub const SECTOR_BYTES: usize = 128;
pub const IMAGE_BYTES: usize = TRACKS * SECTORS * SECTOR_BYTES;
pub const DRIVES: usize = 4;

/// CPU clocks (2 MHz) of one revolution at 360 rpm.
pub const REV: u64 = 333_333;
const STEP: u64 = 12_000;
const SETTLE: u64 = 20_000;
const HEAD_LOAD: u64 = 32_000;
/// The firmware's work around an operation.
const OVERHEAD: u64 = 400;

/// What the system memory looks like to the IFM's DMA.
pub trait Dma {
    fn dma_read(&self, addr: u16) -> u8;
    fn dma_write(&mut self, addr: u16, v: u8);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    ReadAll,
    Write,
    Read,
    Format,
    Verify,
    Fail(u8),
}

#[derive(Clone, Copy, Debug)]
struct Op {
    done: u64,
    drive: usize,
    kind: Kind,
    track: u8,
    sector: u8,
    buf: u16,
    status: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct Drive {
    /// The head's track as the IFM believes it; `None`: restore first.
    track: Option<u8>,
    /// Where the head really is.
    head: u8,
    soft_protect: bool,
}

/// The controller's state (no disk contents: those are [`Disk`]s).
#[derive(Clone, Debug)]
pub struct Fif {
    pointers: [u16; 16],
    /// Bytes from the CPU the IFM has not taken yet.
    queue: VecDeque<u8>,
    /// Setting a pointer: its number and the bytes still to come.
    set: Option<(usize, u8)>,
    drives: [Drive; DRIVES],
    /// The drive whose head is down and the clock it comes up.
    head_down: Option<(usize, u64)>,
    op: Option<Op>,
    /// The last buffer address (BUFA).
    buf: u16,
}

impl Default for Fif {
    fn default() -> Self {
        Self::new()
    }
}

impl Fif {
    pub fn new() -> Self {
        let mut f = Self {
            pointers: [0; 16],
            queue: VecDeque::new(),
            set: None,
            drives: [Drive::default(); DRIVES],
            head_down: None,
            op: None,
            buf: 0,
        };
        f.reset();
        f
    }

    /// The IFM restarts (power-up, RESET): default pointers, every drive
    /// to be restored, nothing running. The heads stay where they are.
    pub fn reset(&mut self) {
        self.pointers = std::array::from_fn(|n| if n == 0 { 0x0080 } else { n as u16 * 0x1000 });
        self.queue.clear();
        self.set = None;
        for d in &mut self.drives {
            d.track = None;
            d.soft_protect = false;
        }
        self.head_down = None;
        self.op = None;
    }

    /// The bootstrap the IFM DMAs into memory at power-up and RESET when
    /// drive 0 holds a disk (`fifware.asm`, SYSCM, relocated to $0080).
    pub fn bootstrap(mem: &mut dyn Dma) {
        const CODE: [u8; 32] = [
            0x21, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, // read, drive 0, track 0, sector 1, to 0
            0x97, // $87 START: SUB A
            0x32, 0x81, 0x00, // STA $0081 (status)
            0xd3, 0xfd, // OUT $FD: execute pointer 0 ($0080)
            0x3a, 0x81, 0x00, // $8D: LDA $0081
            0xb7, // ORA A
            0xca, 0x8d, 0x00, // JZ $008D
            0xfe, 0x01, // CPI 1
            0xca, 0x00, 0x00, // JZ 0
            0x2f, // CMA
            0xd3, 0xff, // OUT $FF: the error in the lamps
            0xc3, 0x87, 0x00, // JMP START
            0x00,
        ];
        for (i, &b) in CODE.iter().enumerate() {
            mem.dma_write(0x80 + i as u16, b);
        }
        for (i, b) in [0xc3, 0x87, 0x00].into_iter().enumerate() {
            mem.dma_write(i as u16, b);
        }
    }

    /// An OUT to the port at clock `t`. Returns the clock the CPU may go on
    /// at: the IFM takes the byte when no operation runs.
    pub fn out(&mut self, v: u8, t: u64) -> u64 {
        self.queue.push_back(v);
        self.op.map_or(t, |op| op.done.max(t))
    }

    /// Work waiting: an operation or bytes not yet taken.
    pub fn pending(&self) -> bool {
        self.op.is_some() || !self.queue.is_empty()
    }

    /// An operation is running on drive `d`.
    pub fn busy(&self, d: usize) -> bool {
        self.op.is_some_and(|op| op.drive == d)
    }

    /// Work up to clock `t`: finish an operation that is due, then take the
    /// bytes the CPU sent.
    pub fn run(&mut self, t: u64, mem: &mut dyn Dma, disks: &mut [Disk; DRIVES]) {
        if self.op.is_none() && self.queue.is_empty() {
            return;
        }
        loop {
            if let Some(op) = self.op {
                if t < op.done {
                    return;
                }
                self.op = None;
                self.finish(op, mem, disks);
            }
            let Some(v) = self.queue.pop_front() else {
                return;
            };
            self.take(v, t, mem, disks);
        }
    }

    fn take(&mut self, v: u8, t: u64, mem: &mut dyn Dma, disks: &[Disk; DRIVES]) {
        if let Some((n, left)) = self.set {
            let p = &mut self.pointers[n];
            if left == 2 {
                *p = *p & 0xff00 | u16::from(v);
                self.set = Some((n, 1));
            } else {
                *p = *p & 0x00ff | u16::from(v) << 8;
                self.set = None;
            }
            return;
        }
        let arg = usize::from(v & 0x0f);
        match v >> 4 {
            0 => self.start(self.pointers[arg], t, mem, disks),
            1 => self.set = Some((arg, 2)),
            2..=4 => {
                for (d, drive) in self.drives.iter_mut().enumerate() {
                    if arg >> d & 1 != 0 {
                        match v >> 4 {
                            2 => drive.track = None,
                            3 => drive.soft_protect = true,
                            _ => drive.soft_protect = false,
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Check a command string at `at` and start its operation (or its
    /// error), as the firmware's ACT1 does.
    fn start(&mut self, at: u16, t: u64, mem: &mut dyn Dma, disks: &[Disk; DRIVES]) {
        let byte = |i: u16| mem.dma_read(at.wrapping_add(i));
        let status = at.wrapping_add(1);
        let fail = |code: u8| Op {
            done: t + OVERHEAD,
            drive: DRIVES,
            kind: Kind::Fail(code),
            track: 0,
            sector: 0,
            buf: 0,
            status,
        };
        let command = byte(0);
        let check = || -> Result<Op, u8> {
            if byte(1) != 0 {
                return Err(0xc1);
            }
            if byte(2) != 0 {
                return Err(0xc5);
            }
            let drives = command & 0x0f;
            if drives == 0 {
                return Err(0xc2);
            }
            if drives.count_ones() > 1 {
                return Err(0xc3);
            }
            let drive = drives.trailing_zeros() as usize;
            let track = byte(3);
            if usize::from(track) >= TRACKS {
                return Err(0xc5);
            }
            let mut func = command & 0xf0;
            let mut next = 4u16;
            let mut sector = 0;
            let mut buf = self.buf;
            if func == 0 {
                sector = byte(next); // read all: the delay
                buf = u16::from(byte(next + 1)) | u16::from(byte(next + 2)) << 8;
            } else {
                if func >= 0xa1 {
                    return Err(0xc4);
                }
                if func >= 0x60 {
                    func = func.wrapping_add(0xa0);
                    if func == 0 {
                        return Err(0xc4);
                    }
                }
                if func != 0x30 {
                    sector = byte(next);
                    next += 1;
                    if sector == 0 || usize::from(sector) > SECTORS {
                        return Err(0xc6);
                    }
                    if func < 0x21 {
                        buf = u16::from(byte(next)) | u16::from(byte(next + 1)) << 8;
                        next += 2;
                    }
                }
                if command >= 0x6f && (byte(next) != 0 || usize::from(byte(next + 1)) >= TRACKS) {
                    return Err(0xc8);
                }
            }
            if disks[drive].media.is_none() {
                return Err(0xa1);
            }
            if command & 0x10 != 0 {
                if disks[drive].protected {
                    return Err(0xa2);
                }
                if self.drives[drive].soft_protect {
                    return Err(0xa3);
                }
            }
            let kind = match func {
                0x00 => Kind::ReadAll,
                0x10 | 0x50 => Kind::Write,
                0x20 => Kind::Read,
                0x30 => Kind::Format,
                _ => Kind::Verify,
            };
            Ok(Op {
                done: 0,
                drive,
                kind,
                track,
                sector,
                buf,
                status,
            })
        };
        let op = match check() {
            Err(code) => fail(code),
            Ok(mut op) => {
                self.buf = op.buf;
                op.done = self.timing(&op, t);
                op
            }
        };
        self.op = Some(op);
    }

    /// When an operation started at `t` ends; moves the head.
    fn timing(&mut self, op: &Op, t: u64) -> u64 {
        let d = op.drive;
        let drive = &mut self.drives[d];
        let mut at = t + OVERHEAD;
        if drive.track.is_none() {
            // Restore: out over track 0 and back in.
            at += (u64::from(drive.head) + 2) * STEP;
            drive.head = 0;
            drive.track = Some(0);
        }
        let steps = u64::from(drive.head.abs_diff(op.track));
        drive.head = op.track;
        drive.track = Some(op.track);
        let loaded = self
            .head_down
            .is_some_and(|(hd, until)| hd == d && until > at);
        at += if steps > 0 {
            steps * STEP + SETTLE
        } else if loaded {
            0
        } else {
            HEAD_LOAD
        };
        let sector_time = REV / SECTORS as u64;
        let done = match op.kind {
            Kind::Format | Kind::ReadAll => {
                // From the index hole, one revolution.
                at + (REV - at % REV) % REV + REV
            }
            _ => {
                let start = (u64::from(op.sector) - 1) * sector_time;
                at + (start + REV - at % REV) % REV + sector_time
            }
        };
        self.head_down = Some((d, done + 2 * REV));
        done
    }

    fn finish(&mut self, op: Op, mem: &mut dyn Dma, disks: &mut [Disk; DRIVES]) {
        let code = match op.kind {
            Kind::Fail(code) => code,
            Kind::ReadAll | Kind::Verify => 1,
            Kind::Read => {
                let off = offset(op.track, op.sector);
                for i in 0..SECTOR_BYTES {
                    mem.dma_write(op.buf.wrapping_add(i as u16), disks[op.drive].data[off + i]);
                }
                1
            }
            Kind::Write => {
                let data: Vec<u8> = (0..SECTOR_BYTES)
                    .map(|i| mem.dma_read(op.buf.wrapping_add(i as u16)))
                    .collect();
                disks[op.drive].store(offset(op.track, op.sector), &data);
                1
            }
            Kind::Format => {
                // Every sector of the track zero (the firmware's FRMT).
                let off = offset(op.track, 1);
                disks[op.drive].store(off, &[0; SECTORS * SECTOR_BYTES]);
                1
            }
        };
        mem.dma_write(op.status, code);
    }

    /// A drive's status for the footer: an operation on it runs.
    pub fn drive_status(&self, d: usize) -> DriveStatus {
        DriveStatus {
            active: self.busy(d),
            error: false,
            track: Some(self.drives[d].head + 1),
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        for p in self.pointers {
            w.u16(p);
        }
        w.bytes(&self.queue.iter().copied().collect::<Vec<u8>>());
        w.u8(self.set.map_or(0xff, |(n, _)| n as u8));
        w.u8(self.set.map_or(0, |(_, left)| left));
        for d in &self.drives {
            w.u8(d.track.unwrap_or(0xff));
            w.u8(d.head);
            w.bool(d.soft_protect);
        }
        w.u8(self.head_down.map_or(0xff, |(d, _)| d as u8));
        w.u64(self.head_down.map_or(0, |(_, until)| until));
        match self.op {
            None => w.u8(0xff),
            Some(op) => {
                w.u8(match op.kind {
                    Kind::ReadAll => 0,
                    Kind::Write => 1,
                    Kind::Read => 2,
                    Kind::Format => 3,
                    Kind::Verify => 4,
                    Kind::Fail(_) => 5,
                });
                w.u8(match op.kind {
                    Kind::Fail(code) => code,
                    _ => 0,
                });
                w.u64(op.done);
                w.u8(op.drive as u8);
                w.u8(op.track);
                w.u8(op.sector);
                w.u16(op.buf);
                w.u16(op.status);
            }
        }
        w.u16(self.buf);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut f = Fif::new();
        for p in &mut f.pointers {
            *p = r.u16()?;
        }
        f.queue = r.bytes()?.iter().copied().collect();
        let n = r.u8()?;
        let left = r.u8()?;
        f.set = match (n, left) {
            (0xff, _) => None,
            (n @ 0..=15, l @ (1 | 2)) => Some((usize::from(n), l)),
            _ => return Err(StateError::Invalid("FIF pointer set")),
        };
        for d in &mut f.drives {
            let track = r.u8()?;
            d.track = (track != 0xff).then_some(track);
            d.head = r.u8()?;
            d.soft_protect = r.bool()?;
            if usize::from(d.head) >= TRACKS || d.track.is_some_and(|t| usize::from(t) >= TRACKS) {
                return Err(StateError::Invalid("FIF track"));
            }
        }
        let hd = r.u8()?;
        let until = r.u64()?;
        f.head_down = match hd {
            0xff => None,
            d if usize::from(d) < DRIVES => Some((usize::from(d), until)),
            _ => return Err(StateError::Invalid("FIF head")),
        };
        let kind = r.u8()?;
        if kind != 0xff {
            let code = r.u8()?;
            let done = r.u64()?;
            let drive = usize::from(r.u8()?);
            let track = r.u8()?;
            let sector = r.u8()?;
            let buf = r.u16()?;
            let status = r.u16()?;
            let kind = match kind {
                0 => Kind::ReadAll,
                1 => Kind::Write,
                2 => Kind::Read,
                3 => Kind::Format,
                4 => Kind::Verify,
                5 => Kind::Fail(code),
                _ => return Err(StateError::Invalid("FIF operation")),
            };
            let fail = matches!(kind, Kind::Fail(_));
            if (!fail && drive >= DRIVES)
                || usize::from(track) >= TRACKS
                || usize::from(sector) > SECTORS
                || (!fail && sector == 0 && !matches!(kind, Kind::Format | Kind::ReadAll))
            {
                return Err(StateError::Invalid("FIF operation"));
            }
            f.op = Some(Op {
                done,
                drive,
                kind,
                track,
                sector,
                buf,
                status,
            });
        }
        f.buf = r.u16()?;
        *self = f;
        Ok(())
    }
}

/// Offset of `sector` (1–26) of `track` in an image.
fn offset(track: u8, sector: u8) -> usize {
    (usize::from(track) * SECTORS + usize::from(sector) - 1) * SECTOR_BYTES
}

/// A drive's disk: the image's bytes, written through to the backend.
#[derive(Default)]
pub struct Disk {
    pub media: Option<Media>,
    data: Vec<u8>,
    /// The drive's write-protect: the slot's, or a backend that refused
    /// a write.
    protected: bool,
    /// The slot's last error writing back (shown, not fatal).
    write_error: bool,
}

impl Disk {
    fn store(&mut self, off: usize, bytes: &[u8]) {
        self.data[off..off + bytes.len()].copy_from_slice(bytes);
        if let Some(m) = &mut self.media {
            self.write_error = m.backend.write(off as u64, bytes).is_err();
        }
    }

    pub fn write_error(&self) -> bool {
        self.write_error
    }
}

impl MediaSlot for Disk {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        if media.backend.len() != IMAGE_BYTES as u64 {
            return Err(MediaError::Format(format!(
                "an IMSAI 8-inch disk image has {IMAGE_BYTES} bytes (77 tracks of 26 sectors of 128 bytes), this one {}",
                media.backend.len()
            )));
        }
        let mut data = vec![0; IMAGE_BYTES];
        media.backend.read(0, &mut data)?;
        self.data = data;
        self.media = Some(media);
        self.write_error = false;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.data.clear();
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A writable image in memory.
    pub(crate) struct MemoryBackend {
        pub data: Vec<u8>,
    }

    impl emu_core::media::MediaBackend for MemoryBackend {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(
                self.data
                    .get(o..o + buf.len())
                    .ok_or(MediaError::OutOfRange)?,
            );
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            self.data
                .get_mut(o..o + data.len())
                .ok_or(MediaError::OutOfRange)?
                .copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    struct Ram(Vec<u8>);
    impl Dma for Ram {
        fn dma_read(&self, addr: u16) -> u8 {
            self.0[usize::from(addr)]
        }
        fn dma_write(&mut self, addr: u16, v: u8) {
            self.0[usize::from(addr)] = v;
        }
    }

    fn disk_with(f: impl Fn(usize) -> u8) -> Disk {
        let mut d = Disk::default();
        d.insert(Media {
            name: "a.dsk".into(),
            kind: MediaKind::Disk,
            backend: Box::new(MemoryBackend {
                data: (0..IMAGE_BYTES).map(f).collect(),
            }),
        })
        .unwrap();
        d
    }

    fn disks() -> [Disk; DRIVES] {
        [
            disk_with(|i| (i / SECTOR_BYTES) as u8 ^ i as u8),
            Disk::default(),
            Disk::default(),
            Disk::default(),
        ]
    }

    /// Run the controller until its operation is over; the clock it ended.
    fn settle(f: &mut Fif, mut t: u64, ram: &mut Ram, d: &mut [Disk; DRIVES]) -> u64 {
        while f.op.is_some() || !f.queue.is_empty() {
            t += 100;
            f.run(t, ram, d);
        }
        t
    }

    #[test]
    fn the_bootstrap_reads_sector_1_and_jumps_to_0() {
        let mut ram = Ram(vec![0; 0x1_0000]);
        Fif::bootstrap(&mut ram);
        assert_eq!(&ram.0[0..3], &[0xc3, 0x87, 0x00]);
        assert_eq!(&ram.0[0x80..0x87], &[0x21, 0, 0, 0, 1, 0, 0]);
        // The loop's OUT executes pointer 0 = $0080.
        let mut f = Fif::new();
        let mut d = disks();
        f.out(0x00, 0);
        let t = settle(&mut f, 0, &mut ram, &mut d);
        assert_eq!(ram.0[0x81], 1, "status: done");
        assert_eq!(&ram.0[0..128], &d[0].data[0..128], "sector 1 at 0");
        // Restore from track 0 and a head load, then sector 1 from where the
        // disk stood: within a revolution and a bit.
        assert!(
            t < OVERHEAD + 2 * STEP + HEAD_LOAD + REV + REV / 26 + 200,
            "{t}"
        );
    }

    #[test]
    fn pointers_read_write_and_errors() {
        let mut ram = Ram(vec![0; 0x1_0000]);
        let mut f = Fif::new();
        let mut d = disks();
        // Pointer 3 to $2000: write track 5 sector 7 from $3000.
        f.out(0x13, 0);
        f.out(0x00, 0);
        f.out(0x20, 0);
        ram.0[0x2000..0x2007].copy_from_slice(&[0x11, 0, 0, 5, 7, 0x00, 0x30]);
        for i in 0..128 {
            ram.0[0x3000 + i] = 0xa5 ^ i as u8;
        }
        f.out(0x03, 0);
        let t = settle(&mut f, 0, &mut ram, &mut d);
        assert_eq!(ram.0[0x2001], 1);
        let off = offset(5, 7);
        assert_eq!(&d[0].data[off..off + 128], &ram.0[0x3000..0x3080]);
        // Read it back elsewhere, through pointer 0 ($0080).
        ram.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 5, 7, 0x00, 0x40]);
        f.out(0x00, t);
        let t = settle(&mut f, t, &mut ram, &mut d);
        assert_eq!(&ram.0[0x4000..0x4080], &ram.0[0x3000..0x3080]);
        // Errors: status not zero, no drive, two drives, bad track, bad
        // sector, a drive without a disk, illegal function, protection.
        let cases: [([u8; 7], u8); 8] = [
            ([0x21, 9, 0, 0, 1, 0, 0], 0xc1),
            ([0x20, 0, 0, 0, 1, 0, 0], 0xc2),
            ([0x23, 0, 0, 0, 1, 0, 0], 0xc3),
            ([0x21, 0, 0, 77, 1, 0, 0], 0xc5),
            ([0x21, 0, 0, 0, 27, 0, 0], 0xc6),
            ([0x22, 0, 0, 0, 1, 0, 0], 0xa1),
            ([0x61, 0, 0, 0, 1, 0, 0], 0xc4),
            ([0xb1, 0, 0, 0, 1, 0, 0], 0xc4),
        ];
        let mut t = t;
        for (cmd, code) in cases {
            ram.0[0x80..0x87].copy_from_slice(&cmd);
            f.out(0x00, t);
            t = settle(&mut f, t, &mut ram, &mut d);
            assert_eq!(ram.0[0x81], code, "{cmd:02x?}");
        }
        // Software protect drive 0: writes fail with $A3, enable again.
        f.out(0x31, t);
        ram.0[0x80..0x87].copy_from_slice(&[0x11, 0, 0, 0, 1, 0, 0x30]);
        f.out(0x00, t);
        t = settle(&mut f, t, &mut ram, &mut d);
        assert_eq!(ram.0[0x81], 0xa3);
        f.out(0x41, t);
        ram.0[0x81] = 0;
        f.out(0x00, t);
        t = settle(&mut f, t, &mut ram, &mut d);
        assert_eq!(ram.0[0x81], 1);
        // The slot's write protect: $A2.
        d[0].write_protect(true);
        ram.0[0x81] = 0;
        f.out(0x00, t);
        settle(&mut f, t, &mut ram, &mut d);
        assert_eq!(ram.0[0x81], 0xa2);
    }

    #[test]
    fn timing_follows_the_head_and_the_disk() {
        let mut ram = Ram(vec![0; 0x1_0000]);
        let mut f = Fif::new();
        let mut d = disks();
        // First access: restore, then track 10 sector 1.
        ram.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 10, 1, 0, 0x10]);
        f.out(0x00, 0);
        let t1 = settle(&mut f, 0, &mut ram, &mut d);
        assert!(t1 >= OVERHEAD + 2 * STEP + 10 * STEP + SETTLE, "{t1}");
        // Sector 2 has just gone by: a revolution. Sector 4 comes after
        // two sectors' time.
        ram.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 10, 2, 0, 0x10]);
        f.out(0x00, t1);
        let t2 = settle(&mut f, t1, &mut ram, &mut d);
        assert!(t2 - t1 > REV - REV / 26, "{}", t2 - t1);
        ram.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 10, 5, 0, 0x10]);
        f.out(0x00, t2);
        let t2 = settle(&mut f, t2, &mut ram, &mut d) - t2;
        assert!(t2 <= 3 * (REV / 26) + OVERHEAD + 300, "{t2}");
        let t2 = t1 + t2;
        // An OUT while busy waits for the end.
        ram.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 40, 1, 0, 0x10]);
        f.out(0x00, t2);
        f.run(t2, &mut ram, &mut d);
        let end = f.op.unwrap().done;
        assert_eq!(f.out(0x40, t2 + 5), end);
        assert!(f.busy(0));
        // Format: from the index hole, one revolution; sectors zeroed.
        settle(&mut f, t2, &mut ram, &mut d);
        ram.0[0x80..0x84].copy_from_slice(&[0x31, 0, 0, 3]);
        f.out(0x00, end);
        let t3 = settle(&mut f, end, &mut ram, &mut d);
        assert_eq!(ram.0[0x81], 1);
        assert!(d[0].data[offset(3, 1)..offset(4, 1)]
            .iter()
            .all(|&b| b == 0));
        assert!(t3 - end >= REV, "{}", t3 - end);
    }

    #[test]
    fn images_of_another_size_are_refused_and_writes_reach_the_backend() {
        let mut d = Disk::default();
        let r = d.insert(Media {
            name: "x.dsk".into(),
            kind: MediaKind::Disk,
            backend: Box::new(MemoryBackend {
                data: vec![0; 1000],
            }),
        });
        assert!(matches!(r, Err(MediaError::Format(_))));
        let mut d = disks();
        d[0].store(offset(76, 26), &[7; 128]);
        let m = d[0].eject().unwrap();
        let mut back = vec![0; 128];
        let mut backend = m.backend;
        backend.read(offset(76, 26) as u64, &mut back).unwrap();
        assert_eq!(back, vec![7; 128]);
    }

    #[test]
    fn savestate_round_trip() {
        let mut ram = Ram(vec![0; 0x1_0000]);
        let mut f = Fif::new();
        let mut d = disks();
        f.out(0x12, 0);
        f.out(0x34, 0);
        f.out(0x12, 0);
        ram.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 3, 4, 0, 0x50]);
        f.out(0x00, 0);
        f.run(0, &mut ram, &mut d);
        assert!(f.op.is_some());
        let mut w = StateWriter::new();
        f.save(&mut w);
        let data = w.into_bytes();
        let mut g = Fif::new();
        g.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(g.pointers[2], 0x1234);
        let a = settle(&mut f, 0, &mut ram, &mut d);
        let mut ram2 = Ram(vec![0; 0x1_0000]);
        ram2.0[0x80..0x87].copy_from_slice(&[0x21, 0, 0, 3, 4, 0, 0x50]);
        let b = settle(&mut g, 0, &mut ram2, &mut d);
        assert_eq!(a, b);
        assert_eq!(ram2.0[0x81], 1);
        assert!(Fif::new().load(&mut StateReader::new(&data[..20])).is_err());
    }
}
