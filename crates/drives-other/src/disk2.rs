//! Disk II: the interface card (slot 6) and two 5.25" drives, at the
//! level of the bit stream under the head.
//!
//! **Card I/O** ($C080 + slot·16 + n; Apple, "The DOS Manual"; Sather,
//! "Understanding the Apple II", chapter 9, "The Disk II"):
//!
//! | n   | even (off)          | odd (on)                     |
//! | --- | ------------------- | ---------------------------- |
//! | 0–7 | stepper phase n/2 off | phase n/2 on               |
//! | 8/9 | motor off (after ~1 s) | motor on                  |
//! | A/B | drive 1             | drive 2                      |
//! | C/D | Q6 low (shift)      | Q6 high (load / sense write protect) |
//! | E/F | Q7 low (read)       | Q7 high (write)              |
//!
//! An access to an even address puts the data register on the bus; the
//! register's bit 7 is QA.
//!
//! **Logic state sequencer.** A 256-byte PROM (P6, 341-0028) and a 4-bit
//! state register S3–S0 clocked at 2 MHz control the 8-bit data register.
//! The PROM's address lines, as the card wires them: A7 = S0, A6 = S2,
//! A5 = S3, A4 = /PULSE, A3 = Q7, A2 = Q6, A1 = QA, A0 = S1; its outputs:
//! D7–D4 the next state with the bits reflected (D7 = S0 … D4 = S3) and
//! D3–D0 a command: 0–7 clear, 8/C nothing, 9 shift left bringing in 0, D
//! shift left bringing in 1, A/E shift right bringing in the write-protect
//! sense, B/F load from the data bus (command codes: Sather, table 9.1;
//! the line order: Christer Ericson's notes on the P6 ROM, retro.co.za,
//! checked here by reading GCR nibbles through the user's 341-0028 image,
//! which only this order does). /PULSE — the drive's MC3470 seeing a flux
//! transition — is low for the first step of a bit cell holding a 1. In
//! write mode the register's bit 7 is written as the cell's bit at the end
//! of every bit cell (8 steps).
//!
//! The PROM is user-supplied (role "disk2-p6"). Without it a functional
//! model of what it does stands in: reading shifts every bit into the
//! register while QA is clear; once QA is set the byte stays until a 1
//! arrives, which starts the next byte one cell later (it keeps the CPU
//! a window of at least 4 µs, as the PROM's delayed clear does); writing
//! loads the register from the bus when Q6 goes high and shifts one bit
//! out per cell.
//!
//! **Drive.** 300 rpm, one bit cell every 4 µs (4 CPU cycles). Four
//! stepper magnets, each one pulling the head to the half-track positions
//! where it sits ([`Disk2Drive::step`]: magnet k at quarter tracks 2k mod
//! 8); two adjacent magnets on hold it between them, at a quarter track.
//! The head reaches quarter track 0–159 (`media::a2disk` maps them to
//! tracks). Where no track is recorded the MC3470 amplifies noise:
//! random bits from a 32-bit LFSR. The motor runs on for about a second
//! after the motor-off access.
//!
//! **Disks** (`media::a2disk`): DSK/DO/PO images are encoded into GCR
//! 6-and-2 tracks on insert and decoded back on flush and eject; NIB
//! images (35 tracks of 6656 nibbles) keep their nibbles; WOZ 1/2 keep
//! their bit streams. The disk is media, not state: savestates keep the
//! card and the mechanics.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::a2disk::{self, NibbleDisk, SectorOrder, QUARTER_TRACKS};

/// Highest quarter track the head reaches.
pub const LAST_QUARTER: u8 = (QUARTER_TRACKS - 1) as u8;
/// CPU cycles per bit cell.
pub const CELL_CYCLES: u8 = 4;
/// Cycles the motor runs on after the motor-off access (about 1 s).
pub const MOTOR_RUN_ON: u32 = 1_020_484;
/// A NIB image: 35 tracks of 6656 nibbles.
pub const NIB_TRACK: usize = 6656;
pub const NIB_BYTES: usize = 35 * NIB_TRACK;

enum Format {
    Dsk(SectorOrder),
    Nib,
    Woz,
}

struct Disk {
    media: Media,
    /// The file as read (written back in its own format).
    file: Vec<u8>,
    format: Format,
    nib: NibbleDisk,
    dirty: bool,
    protected: bool,
}

impl Disk {
    fn parse(media: Media) -> Result<Self, (MediaError, Media)> {
        let mut media = media;
        let mut file = vec![0; media.backend.len() as usize];
        if let Err(e) = media.backend.read(0, &mut file) {
            return Err((e, media));
        }
        let name = media.name.to_ascii_lowercase();
        let parsed = if file.starts_with(b"WOZ1") || file.starts_with(b"WOZ2") {
            a2disk::parse_woz(&file).map(|n| (Format::Woz, n))
        } else if file.len() == NIB_BYTES {
            Ok((Format::Nib, nib_tracks(&file)))
        } else if file.len() == a2disk::DSK_BYTES {
            let order = a2disk::guess_order(&name, &file);
            a2disk::encode_dsk(&file, order).map(|n| (Format::Dsk(order), n))
        } else {
            Err(format!(
                "not a 5.25\" disk image ({} bytes; DSK/DO/PO {}, NIB {NIB_BYTES}, WOZ)",
                file.len(),
                a2disk::DSK_BYTES
            ))
        };
        match parsed {
            Ok((format, nib)) => Ok(Self {
                protected: nib.write_protected,
                media,
                file,
                format,
                nib,
                dirty: false,
            }),
            Err(e) => Err((MediaError::Format(e), media)),
        }
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        match self.format {
            Format::Dsk(order) => {
                // Sectors that cannot be decoded keep their old contents.
                let r = a2disk::decode_dsk(&self.nib, order, &mut self.file);
                self.media.backend.write(0, &self.file)?;
                r.map_err(|e| MediaError::Format(e.to_string()))
            }
            Format::Nib => {
                for t in 0..35 {
                    store_nib_track(&self.nib, t, &mut self.file);
                }
                self.media.backend.write(0, &self.file)
            }
            Format::Woz => {
                let out = a2disk::store_woz(&self.file, &self.nib).map_err(MediaError::Format)?;
                if out.len() != self.file.len() {
                    // Grown tracks: the backend writes past its end.
                    self.media.backend.write(0, &out)?;
                } else {
                    self.media.backend.write(0, &out)?;
                }
                self.file = out;
                Ok(())
            }
        }
    }
}

/// The nibbles of a NIB image as bit streams (eight cells each).
fn nib_tracks(file: &[u8]) -> NibbleDisk {
    let tracks: Vec<Vec<u8>> = file.chunks(NIB_TRACK).map(<[u8]>::to_vec).collect();
    let mut quarter = [None; QUARTER_TRACKS];
    for (q, slot) in quarter.iter_mut().enumerate() {
        let t = (q + 1) / 4;
        if t < 35 && q % 4 != 2 {
            *slot = Some(t as u8);
        }
    }
    NibbleDisk {
        bits: vec![(NIB_TRACK * 8) as u32; tracks.len()],
        tracks,
        quarter,
        write_protected: false,
    }
}

/// Track `t` back into NIB nibbles: the bit stream read the way the
/// controller's latch does, starting at a self-sync gap if there is one.
fn store_nib_track(nib: &NibbleDisk, t: usize, file: &mut [u8]) {
    let (Some(track), Some(&bits)) = (nib.tracks.get(t), nib.bits.get(t)) else {
        return;
    };
    if bits == 0 {
        return;
    }
    let bit = |i: u32| {
        let i = i % bits;
        track[(i / 8) as usize] >> (7 - i % 8) & 1
    };
    let mut out = Vec::with_capacity(NIB_TRACK);
    let mut latch = 0u8;
    let mut i = 0u32;
    // Two turns: the first only to fall into step.
    while out.len() < NIB_TRACK && i < 3 * bits {
        latch = latch << 1 | bit(i);
        i += 1;
        if latch & 0x80 != 0 {
            if i > bits {
                out.push(latch);
            }
            latch = 0;
        }
    }
    out.resize(NIB_TRACK, 0xff);
    file[t * NIB_TRACK..(t + 1) * NIB_TRACK].copy_from_slice(&out);
}

/// One 5.25" drive.
pub struct Disk2Drive {
    /// Head position in quarter tracks.
    pub quarter: u8,
    /// Bit cell under the head.
    pub pos: u32,
    /// The motor turns (including its run-on).
    pub spinning: bool,
    disk: Option<Disk>,
}

impl std::fmt::Debug for Disk2Drive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Disk2Drive")
            .field("quarter", &self.quarter)
            .field("pos", &self.pos)
            .field("disk", &self.disk.as_ref().map(|d| &d.media.name))
            .finish()
    }
}

impl Default for Disk2Drive {
    fn default() -> Self {
        Self::new()
    }
}

impl Disk2Drive {
    pub fn new() -> Self {
        Self {
            quarter: 0,
            pos: 0,
            spinning: false,
            disk: None,
        }
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// The stored track under the head, if one is recorded there.
    fn track(&self) -> Option<usize> {
        let d = self.disk.as_ref()?;
        let t = usize::from(d.nib.quarter[usize::from(self.quarter)]?);
        (d.nib.bits.get(t).copied().unwrap_or(0) > 0).then_some(t)
    }

    fn track_bits(&self) -> u32 {
        match (self.track(), &self.disk) {
            (Some(t), Some(d)) => d.nib.bits[t],
            _ => a2disk::TRACK_BITS,
        }
    }

    /// The magnets in `phases` (bit k = phase k on) pull the head.
    pub fn step(&mut self, phases: u8) {
        let cur = i32::from(self.quarter);
        let (mut sum, mut n) = (0, 0);
        for k in 0..4 {
            if phases & (1 << k) == 0 {
                continue;
            }
            let d = (2 * k - cur).rem_euclid(8);
            let delta = if d <= 4 { d } else { d - 8 };
            // The magnet opposite the head pulls both ways at once.
            if delta.abs() == 4 {
                continue;
            }
            sum += delta;
            n += 1;
        }
        if n == 0 {
            return;
        }
        let old_bits = self.track_bits();
        let q = (cur + sum / n).clamp(0, i32::from(LAST_QUARTER)) as u8;
        if q != self.quarter {
            self.quarter = q;
            // The disk keeps turning: the same angle on the new track.
            let new_bits = self.track_bits();
            self.pos = (u64::from(self.pos) * u64::from(new_bits) / u64::from(old_bits.max(1)))
                as u32
                % new_bits.max(1);
        }
    }

    /// One bit cell: the bit under the head (written first if `write` is
    /// given and the disk is not protected); `noise` stands in where
    /// nothing is recorded.
    fn cell(&mut self, write: Option<bool>, noise: bool) -> bool {
        let bits = self.track_bits();
        let t = self.track();
        let pos = self.pos;
        self.pos = (self.pos + 1) % bits.max(1);
        let (Some(t), Some(d)) = (t, &mut self.disk) else {
            return noise;
        };
        let byte = &mut d.nib.tracks[t][(pos / 8) as usize];
        let mask = 0x80 >> (pos % 8);
        if let (Some(b), false) = (write, d.protected) {
            if b {
                *byte |= mask;
            } else {
                *byte &= !mask;
            }
            d.dirty = true;
        }
        *byte & mask != 0
    }

    pub fn protected(&self) -> bool {
        self.disk.as_ref().is_some_and(|d| d.protected)
    }

    fn save(&self, w: &mut StateWriter) {
        w.u8(self.quarter);
        w.u32(self.pos);
        w.bool(self.spinning);
    }

    fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        let quarter = c.u8()?;
        let pos = c.u32()?;
        let spinning = c.bool()?;
        if quarter > LAST_QUARTER {
            return Err(StateError::Invalid("disk II head position"));
        }
        self.quarter = quarter;
        self.spinning = spinning;
        self.pos = pos % self.track_bits().max(1);
        Ok(())
    }
}

impl MediaSlot for Disk2Drive {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let disk = Disk::parse(media).map_err(|(e, _)| e)?;
        let _ = self.eject();
        self.disk = Some(disk);
        self.pos %= self.track_bits().max(1);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let mut d = self.disk.take()?;
        let _ = d.write_back();
        Some(d.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disk {
            d.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.disk {
            Some(d) => {
                d.write_back()?;
                d.media.backend.commit()
            }
            None => Ok(()),
        }
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.spinning,
            error: false,
            track: Some(self.quarter / 4 + 1),
        }
    }
}

/// The interface card with its two drives.
pub struct Disk2 {
    pub drives: [Disk2Drive; 2],
    /// The P6 sequencer PROM (`None`: the functional model).
    p6: Option<[u8; 256]>,
    /// Stepper magnets (bit k = phase k).
    pub phases: u8,
    pub motor_on: bool,
    /// Cycles of motor run-on left after the motor-off access.
    run_on: u32,
    /// Drive 2 selected.
    pub drive2: bool,
    pub q6: bool,
    pub q7: bool,
    /// The data register and the last value the CPU wrote to the card.
    pub latch: u8,
    bus: u8,
    /// Sequencer state, the step within the bit cell (0–7) and the
    /// current cell's bit.
    state: u8,
    step: u8,
    bit: bool,
    /// Functional model: a 1 arrived while QA was set.
    pending: bool,
    lfsr: u32,
}

impl std::fmt::Debug for Disk2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Disk2")
            .field("drives", &self.drives)
            .field("phases", &self.phases)
            .field("motor_on", &self.motor_on)
            .field("latch", &self.latch)
            .finish()
    }
}

impl Disk2 {
    /// A card with the P6 PROM (256 bytes) or, without one, the
    /// functional sequencer.
    pub fn new(p6: Option<&[u8]>) -> Self {
        Self {
            drives: [Disk2Drive::new(), Disk2Drive::new()],
            p6: p6.and_then(|p| p.try_into().ok()),
            phases: 0,
            motor_on: false,
            run_on: 0,
            drive2: false,
            q6: false,
            q7: false,
            latch: 0,
            bus: 0,
            state: 0,
            step: 0,
            bit: false,
            pending: false,
            lfsr: 0x1234_5678,
        }
    }

    /// Power-on/reset: the card's switches off (the disk stays).
    pub fn reset(&mut self) {
        self.phases = 0;
        self.motor_on = false;
        self.run_on = 0;
        self.drive2 = false;
        self.q6 = false;
        self.q7 = false;
        for d in &mut self.drives {
            d.spinning = false;
        }
    }

    pub fn has_p6(&self) -> bool {
        self.p6.is_some()
    }

    fn drive(&self) -> &Disk2Drive {
        &self.drives[usize::from(self.drive2)]
    }

    fn drive_mut(&mut self) -> &mut Disk2Drive {
        &mut self.drives[usize::from(self.drive2)]
    }

    /// The motor of the selected drive turns.
    pub fn spinning(&self) -> bool {
        self.motor_on || self.run_on > 0
    }

    /// An access to card register `n` (0–15); `write` is the value of a
    /// write cycle. Returns what an even address puts on the bus (`None`
    /// for odd addresses: the bus floats).
    pub fn access(&mut self, n: u8, write: Option<u8>) -> Option<u8> {
        let on = n & 1 != 0;
        match n & 0x0f {
            0..=7 => {
                let bit = 1 << (n >> 1);
                if on {
                    self.phases |= bit;
                } else {
                    self.phases &= !bit;
                }
                if self.spinning() {
                    let phases = self.phases;
                    self.drive_mut().step(phases);
                }
            }
            8 => {
                if self.motor_on {
                    self.run_on = MOTOR_RUN_ON;
                }
                self.motor_on = false;
            }
            9 => {
                self.motor_on = true;
                self.run_on = 0;
            }
            0x0a | 0x0b => {
                let d2 = on;
                if d2 != self.drive2 {
                    let spin = self.spinning();
                    self.drives[usize::from(self.drive2)].spinning = false;
                    self.drive2 = d2;
                    self.drive_mut().spinning = spin;
                }
            }
            0x0c | 0x0d => self.q6 = on,
            _ => self.q7 = on,
        }
        if let Some(v) = write {
            self.bus = v;
        }
        // The functional model loads on the access that sets load mode.
        if self.p6.is_none() && self.q6 && self.q7 && write.is_some() {
            self.latch = self.bus;
            self.step = 0;
        }
        (!on).then_some(if self.q6 && !self.q7 {
            // Sense write protect: the PROM shifts it into bit 7.
            if self.p6.is_none() {
                (self.latch >> 1) | if self.drive().protected() { 0x80 } else { 0 }
            } else {
                self.latch
            }
        } else {
            self.latch
        })
    }

    /// What an even register shows, without side effects.
    pub fn peek(&self) -> u8 {
        self.latch
    }

    fn noise(&mut self) -> bool {
        // 32-bit Galois LFSR (taps 32, 22, 2, 1).
        let out = self.lfsr & 1 != 0;
        self.lfsr >>= 1;
        if out {
            self.lfsr ^= 0x8020_0003;
        }
        out
    }

    /// One CPU cycle (two sequencer steps).
    pub fn tick(&mut self) {
        if self.run_on > 0 {
            self.run_on -= 1;
        }
        let spinning = self.spinning();
        self.drive_mut().spinning = spinning;
        if !spinning {
            return;
        }
        for _ in 0..2 {
            if self.step == 0 {
                let noise = self.noise();
                let write = (self.q7).then_some(self.latch & 0x80 != 0);
                // Reading: the cell's bit; writing happens at the cell end.
                if write.is_none() {
                    self.bit = self.drive_mut().cell(None, noise);
                }
            }
            if self.p6.is_some() {
                self.sequencer_step();
            } else if self.step == 7 {
                self.functional_cell();
            }
            self.step = (self.step + 1) % 8;
            if self.step == 0 && self.q7 {
                let b = self.latch & 0x80 != 0;
                let noise = self.noise();
                self.drive_mut().cell(Some(b), noise);
            }
        }
    }

    /// One step of the PROM sequencer.
    fn sequencer_step(&mut self) {
        let Some(p6) = &self.p6 else {
            return;
        };
        let pulse = self.bit && self.step == 0 && !self.q7;
        let s = self.state;
        let addr = (s & 1) << 7
            | (s >> 2 & 1) << 6
            | (s >> 3 & 1) << 5
            | u8::from(!pulse) << 4
            | u8::from(self.q7) << 3
            | u8::from(self.q6) << 2
            | (self.latch >> 7) << 1
            | (s >> 1 & 1);
        let out = p6[usize::from(addr)];
        self.state = (out >> 7 & 1) | (out >> 5 & 2) | (out >> 3 & 4) | (out >> 1 & 8);
        match out & 0x0f {
            0..=7 => self.latch = 0,
            9 => self.latch <<= 1,
            0x0d => self.latch = self.latch << 1 | 1,
            0x0a | 0x0e => {
                let wp = u8::from(self.drive().protected());
                self.latch = self.latch >> 1 | wp << 7;
            }
            0x0b | 0x0f => self.latch = self.bus,
            _ => {}
        }
    }

    /// The functional sequencer, once per bit cell.
    fn functional_cell(&mut self) {
        if self.q7 {
            // Writing: the bit went out at the cell start; shift on.
            self.latch <<= 1;
            return;
        }
        if self.q6 {
            return;
        }
        let bit = u8::from(self.bit);
        if std::mem::take(&mut self.pending) {
            self.latch = 2 | bit;
        } else if self.latch & 0x80 != 0 {
            self.pending = bit == 1;
        } else {
            self.latch = self.latch << 1 | bit;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"DSK2", 1, |w| {
            for v in [self.phases, self.latch, self.bus, self.state, self.step] {
                w.u8(v);
            }
            for v in [
                self.motor_on,
                self.drive2,
                self.q6,
                self.q7,
                self.bit,
                self.pending,
            ] {
                w.bool(v);
            }
            w.u32(self.run_on);
            w.u32(self.lfsr);
            for d in &self.drives {
                d.save(w);
            }
        });
    }

    /// Atomic: on error the card is unchanged. The disks stay.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"DSK2", 1)?;
        let phases = c.u8()?;
        let latch = c.u8()?;
        let bus = c.u8()?;
        let state = c.u8()?;
        let step = c.u8()?;
        let motor_on = c.bool()?;
        let drive2 = c.bool()?;
        let q6 = c.bool()?;
        let q7 = c.bool()?;
        let bit = c.bool()?;
        let pending = c.bool()?;
        let run_on = c.u32()?;
        let lfsr = c.u32()?;
        if phases > 15 || state > 15 || step > 7 || lfsr == 0 {
            return Err(StateError::Invalid("disk II card"));
        }
        let mut heads = [(0u8, 0u32, false); 2];
        for h in &mut heads {
            let mut d = Disk2Drive::new();
            d.load(&mut c)?;
            *h = (d.quarter, d.pos, d.spinning);
        }
        for (d, (q, pos, spin)) in self.drives.iter_mut().zip(heads) {
            d.quarter = q;
            d.spinning = spin;
            d.pos = pos % d.track_bits().max(1);
        }
        self.phases = phases;
        self.latch = latch;
        self.bus = bus;
        self.state = state;
        self.step = step;
        self.motor_on = motor_on;
        self.drive2 = drive2;
        self.q6 = q6;
        self.q7 = q7;
        self.bit = bit;
        self.pending = pending;
        self.run_on = run_on;
        self.lfsr = lfsr;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;

    struct Mem(Vec<u8>);

    impl MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            if self.0.len() < o + data.len() {
                self.0.resize(o + data.len(), 0);
            }
            self.0[o..o + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn image() -> Vec<u8> {
        (0..a2disk::DSK_BYTES)
            .map(|i| (i * 7 + i / 256) as u8)
            .collect()
    }

    fn media(data: Vec<u8>) -> Media {
        Media {
            name: "test.dsk".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(data)),
        }
    }

    /// Read `n` disk bytes the way RWTS does: wait for QA, take the byte.
    fn read_nibbles(card: &mut Disk2, n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cycles = 0;
        while out.len() < n && cycles < 2_000_000 {
            // LDA $C0EC,X ; BPL: 7 cycles per poll.
            let v = card.access(0x0c, None).unwrap_or(0);
            for _ in 0..7 {
                card.tick();
            }
            cycles += 7;
            if v & 0x80 != 0 {
                out.push(v);
                // The byte is taken; the next poll must see a new one.
                while card.access(0x0c, None).unwrap_or(0) == v && cycles < 2_000_000 {
                    for _ in 0..3 {
                        card.tick();
                    }
                    cycles += 3;
                    if card.latch & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
        out
    }

    fn reads_address_fields(p6: Option<&[u8]>) {
        let mut card = Disk2::new(p6);
        card.drives[0].insert(media(image())).unwrap();
        card.access(0x09, None); // motor on
        card.access(0x0e, None); // read mode
        let nibbles = read_nibbles(&mut card, 8000);
        let found = nibbles
            .windows(3)
            .filter(|w| w == &[0xd5, 0xaa, 0x96])
            .count();
        assert!(found >= 16, "address fields found: {found}");
    }

    #[test]
    fn functional_sequencer_reads_address_fields() {
        reads_address_fields(None);
    }

    /// With the user's P6 PROM (`EMU_ROM_DIR`, file 341-0028 or
    /// 342-0028 in `apple2/`).
    #[test]
    #[ignore = "needs EMU_ROM_DIR with the Disk II P6 PROM"]
    fn prom_sequencer_reads_address_fields() {
        let dir = std::path::PathBuf::from(std::env::var_os("EMU_ROM_DIR").expect("EMU_ROM_DIR"));
        let p6 = std::fs::read_dir(dir.join("apple2"))
            .expect("apple2 folder")
            .filter_map(Result::ok)
            .find(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                (n.starts_with("3410028") || n.starts_with("3420028") || n.starts_with("341-0028"))
                    && e.metadata().map(|m| m.len() == 256).unwrap_or(false)
            })
            .map(|e| std::fs::read(e.path()).unwrap())
            .expect("P6 PROM");
        reads_address_fields(Some(&p6));
    }

    #[test]
    fn stepper_moves_half_and_quarter_tracks() {
        let mut d = Disk2Drive::new();
        // Phase 1 on: two quarter tracks out; with phase 2 as well, between.
        d.step(0b0010);
        assert_eq!(d.quarter, 2);
        d.step(0b0110);
        assert_eq!(d.quarter, 3);
        d.step(0b0100);
        assert_eq!(d.quarter, 4, "track 1");
        // Back: phase 1 pulls inwards again.
        d.step(0b0010);
        assert_eq!(d.quarter, 2);
        d.step(0b0001);
        assert_eq!(d.quarter, 0);
        d.step(0b1000);
        assert_eq!(d.quarter, 0, "stops at track 0");
    }

    #[test]
    fn write_protect_sense_and_flush() {
        let mut card = Disk2::new(None);
        card.drives[0].insert(media(image())).unwrap();
        card.drives[0].write_protect(true);
        card.access(0x0d, None);
        assert_eq!(card.access(0x0e, None).unwrap() & 0x80, 0x80);
        card.drives[0].write_protect(false);
        card.access(0x0d, None);
        assert_eq!(card.access(0x0e, None).unwrap() & 0x80, 0);
        assert!(card.drives[0].flush().is_ok());
    }

    #[test]
    fn written_bits_reach_the_track() {
        let mut card = Disk2::new(None);
        card.drives[0].insert(media(image())).unwrap();
        card.access(0x09, None);
        card.access(0x0f, None); // Q7 high
        card.access(0x0d, Some(0xd5)); // Q6 high: load
        for _ in 0..32 {
            card.tick();
        }
        let d = card.drives[0].disk.as_ref().unwrap();
        assert!(d.dirty);
    }

    #[test]
    fn savestate_round_trip() {
        let mut card = Disk2::new(None);
        card.access(0x09, None);
        card.access(0x03, None);
        for _ in 0..1000 {
            card.tick();
        }
        let mut w = StateWriter::new();
        card.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Disk2::new(None);
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other.phases, card.phases);
        assert_eq!(other.drives[0].quarter, card.drives[0].quarter);
        assert!(other.load(&mut StateReader::new(&bytes[..4])).is_err());
    }
}
