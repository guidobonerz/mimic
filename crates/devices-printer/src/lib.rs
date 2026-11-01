//! Printers. [`Printer`] is the paper end, the same on every machine: it
//! takes bytes, renders them with an emulated [`epson`] printer and writes
//! a print job through the host services as raw data (`print-001.prn`)
//! and page pictures (`print-001-1.png`); a frontend can mount a folder
//! that passes each finished page on to a printer of the host
//! (`media_backends::host_print`), so nothing here sees the host. A page
//! ends with a form feed, at the bottom of the paper, or after a pause
//! in the data, as programs seldom end a listing with a form feed. The
//! machine ends differ:
//!
//! - [`CbmPrinter`]: a printer on the Commodore serial (IEC) bus, or the
//!   PET's IEEE-488 bus, as device 4 or 5. The bus behaves as with any
//!   Commodore printer; the secondary addresses are this project's choice
//!   ([`Translation`]), they are not those of one real printer.
//! - Amstrad CPC and Amiga: a Centronics port, wired by the machine, which
//!   calls [`Printer::feed`] on each strobe.
//! - Atari 8-bit: the SIO printer; [`atari_line`] turns its records into
//!   text.
//! - Macintosh: the serial printer port with an ImageWriter.
//!
//! No file is touched without [`Printer::attach`]; the bytes are then lost
//! but the machine prints all the same (the web frontend).

pub mod epson;

pub use epson::Model;

use emu_core::host::{self, HostServices, OpenMode, SharedFiles};
use emu_core::{StateError, StateReader, StateWriter};

use emu_core::registry::MachineConfig;
use epson::Epson;

/// Folder of the file host that takes the print jobs: frontends mount it.
pub const FOLDER: &str = "/Printer";
/// The name frontends mount [`FOLDER`] under.
pub const MOUNT: &str = "Printer";
/// Frames (of about 1/50 s) without data until the page comes out.
const IDLE_FRAMES: u32 = 150;

/// The paper end of every printer port.
#[derive(Default)]
pub struct Printer {
    epson: Epson,
    /// Bytes not yet in the job's raw file.
    raw: Vec<u8>,
    files: Option<SharedFiles>,
    /// The job's number, once it has one.
    job: Option<u32>,
    /// Page in the printer, from 1.
    page: u32,
    idle: u32,
    dirty: bool,
}

/// `bits` (a set bit is ink) as a PNG file.
fn page_png(bits: &[u8], (width, height): (usize, usize), repeat: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, (height * repeat) as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::One);
    let mut writer = encoder.write_header().ok()?;
    // Each row `repeat` times: the picture has square dots.
    let paper: Vec<u8> = bits
        .chunks(width / 8)
        .flat_map(|row| std::iter::repeat_n(row, repeat))
        .flatten()
        .map(|b| !b)
        .collect();
    writer.write_image_data(&paper).ok()?;
    writer.finish().ok()?;
    Some(out)
}

fn raw_path(job: u32) -> String {
    format!("{FOLDER}/print-{job:03}.prn")
}

/// Chunk of a machine's savestate, the first one so that it can be found
/// whatever the rest looks like.
const CHUNK: [u8; 4] = *b"PRNT";

/// The machine options `printer` (on, 4, 5, off; default off) and
/// `printermodel` (fx80, p6, imagewriter): the model wanted, if any.
fn wanted(config: &MachineConfig, default: Model) -> Option<Model> {
    let option = |k| config.options.get(k).map(String::as_str);
    option("printer")
        .filter(|v| *v != "off")
        .map(|_| Model::from_option(option("printermodel")).unwrap_or(default))
}

impl Printer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(model: Model) -> Self {
        Self {
            epson: Epson::with_model(model),
            ..Self::default()
        }
    }

    /// The printer the machine's options ask for, `default` the model
    /// usual on the machine.
    pub fn from_config(config: &MachineConfig, default: Model) -> Option<Self> {
        wanted(config, default).map(Self::with_model)
    }

    pub fn save_chunk(&self, w: &mut StateWriter) {
        w.chunk(CHUNK, 1, |w| self.save(w));
    }

    /// Restore from the machine's savestate `s`. One without a printer, or
    /// a damaged one, leaves it as it is.
    pub fn load_chunk(&mut self, s: &[u8]) {
        let Ok((_, mut c)) = StateReader::new(s).chunk(CHUNK, 1) else {
            return;
        };
        let mut fresh = Printer::new();
        if fresh.load(&mut c).is_ok() {
            fresh.files = self.files.take();
            *self = fresh;
        }
    }

    /// Lend the printer the machine's file services.
    pub fn attach(&mut self, host: &HostServices) {
        self.files.clone_from(&host.files);
    }

    /// The next byte from the computer.
    pub fn feed(&mut self, b: u8) {
        self.epson.feed(b);
        self.raw.push(b);
        self.dirty = true;
        self.idle = 0;
        self.take_page();
    }

    /// The emulated printer, for its page.
    pub fn epson(&self) -> &Epson {
        &self.epson
    }

    /// A new frame: a pause in the data puts what came on disk.
    pub fn frame(&mut self) {
        if !self.dirty {
            return;
        }
        self.idle += 1;
        if self.idle >= IDLE_FRAMES {
            self.flush();
        }
    }

    /// Bring out the page in the printer and write the raw data now.
    pub fn flush(&mut self) {
        self.dirty = false;
        self.idle = 0;
        self.write_raw();
        self.epson.eject();
        self.take_page();
    }

    fn take_page(&mut self) {
        if let Some(page) = self.epson.finished.take() {
            self.write_page(&page);
            self.page += 1;
        }
    }

    /// The job's number: the first with no file yet.
    fn job(&mut self) -> Option<u32> {
        if self.job.is_none() {
            let files = self.files.as_ref()?;
            let mut f = host::lock(files);
            let free = (1..10_000).find(|n| f.stat(&raw_path(*n)).is_err())?;
            self.job = Some(free);
        }
        self.job
    }

    fn write_file(&self, path: &str, data: &[u8], append: bool) {
        let Some(files) = &self.files else { return };
        let mode = OpenMode {
            write: true,
            create: true,
            truncate: !append,
            ..OpenMode::default()
        };
        let mut f = host::lock(files);
        if let Ok(h) = f.open(path, mode) {
            if append {
                let size = f.fstat(h).map_or(0, |s| s.size);
                let _ = f.seek(h, size);
            }
            let _ = f.write(h, data);
            f.close(h);
        }
    }

    fn write_raw(&mut self) {
        let raw = std::mem::take(&mut self.raw);
        if let Some(job) = self.job() {
            self.write_file(&raw_path(job), &raw, true);
        }
    }

    fn write_page(&mut self, bits: &[u8]) {
        self.write_raw();
        let Some(job) = self.job() else { return };
        if let Some(png) = page_png(bits, self.epson.size(), self.epson.row_repeat()) {
            let path = format!("{FOLDER}/print-{job:03}-{}.png", self.page + 1);
            self.write_file(&path, &png, false);
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.epson.save(w);
        w.bytes(&self.raw);
        w.bool(self.job.is_some());
        w.u32(self.job.unwrap_or(0));
        w.u32(self.page);
        w.u32(self.idle);
        w.bool(self.dirty);
    }

    /// What was written stays written: a state from before only repeats it.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.epson.load(r)?;
        self.raw = r.bytes()?.to_vec();
        let has_job = r.bool()?;
        let job = r.u32()?;
        self.job = has_job.then_some(job);
        self.page = r.u32()?;
        self.idle = r.u32()?;
        self.dirty = r.bool()?;
        Ok(())
    }
}

/// How an interface treats the bytes of a secondary address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Translation {
    /// PETSCII to ASCII capitals (secondary address 0, `LIST`, `CMD`).
    Upper,
    /// PETSCII to ASCII, shifted letters as capitals (7).
    Lower,
    /// As they come: printer codes and graphics from a program (1).
    Transparent,
}

impl Translation {
    pub fn of_secondary(sa: u8) -> Self {
        match sa {
            1 => Self::Transparent,
            7 => Self::Lower,
            _ => Self::Upper,
        }
    }

    /// The screen code of `b` if it is a graphics character here: it then
    /// prints from the character ROM, set `lower` of the two. In the
    /// capitals' set the shifted letters are graphics too (C64
    /// Programmer's Reference Guide, screen codes and PETSCII tables).
    pub fn graphic(self, b: u8) -> Option<(u8, bool)> {
        let letter = matches!(b, 0x61..=0x7a | 0xc1..=0xda);
        let code = match b {
            0x5c | 0x5e | 0x5f => b - 0x40,
            0x60..=0x7f => b - 0x20,
            0xa1..=0xbf => b - 0x40,
            0xc0..=0xfe => b - 0x80,
            0xff => 0x5e,
            _ => return None,
        };
        match self {
            Self::Upper => Some((code, false)),
            Self::Lower if !letter => Some((code, true)),
            _ => None,
        }
    }

    /// `b` for the printer: a character, a line end or nothing.
    pub fn apply(self, b: u8, mut out: impl FnMut(u8)) {
        match (self, b) {
            (Self::Transparent, _) => out(b),
            (_, 0x0d) => {
                out(0x0d);
                out(0x0a);
            }
            (_, 0x08..=0x0c | 0x0e | 0x0f | 0x12 | 0x14 | 0x1b) => out(b),
            (_, 0x20..=0x40 | 0x5b | 0x5d) => out(b),
            (Self::Upper, 0x41..=0x5a) => out(b),
            (Self::Lower, 0x41..=0x5a) => out(b | 0x20),
            (Self::Lower, 0x61..=0x7a | 0xc1..=0xda) => out(b & 0x5f),
            (_, 0xa0) => out(0x20),
            // Without a character ROM: PETSCII graphics have no ASCII.
            (_, 0x5c | 0x5e | 0x5f | 0x60..=0x7f | 0xa1..=0xbf | 0xc0..=0xff) => out(b'?'),
            // Colour and cursor codes do not print.
            _ => {}
        }
    }
}

/// The printer's end of the serial bus's handshake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Iec {
    #[default]
    Idle,
    AtnWaitClk,
    WaitReady,
    WaitClkLow,
    EoiAck,
    BitHigh,
    BitLow,
}

/// A printer interface on a Commodore bus.
pub struct CbmPrinter {
    pub printer: Printer,
    device: u8,
    listening: bool,
    /// The secondary address of the channel in use.
    secondary: u8,
    /// Data goes to the printer (not an OPEN's file name).
    data_phase: bool,
    iec: Iec,
    timer: u32,
    bit: u8,
    byte: u8,
    eoi: bool,
    prev_atn: bool,
    atn: bool,
    /// Character ROM: two sets of eight-byte characters, for PETSCII
    /// graphics. Taken from the machine's ROMs, not state.
    charset: Vec<u8>,
    /// The one serial line a listener pulls.
    pull_data: bool,
    /// IEEE-488: the lines it pulls, and a byte taken until DAV ends.
    pull_ndac: bool,
    pull_nrfd: bool,
    accepted: bool,
}

/// EOI and its acknowledgement, in microseconds (the bus protocol is
/// described at `drives-cbm`'s virtual drive).
const EOI_TIMEOUT: u32 = 200;
const EOI_ACK: u32 = 60;

impl CbmPrinter {
    /// The printer the machine's options ask for: `printer=on` or `4` is
    /// device 4, `5` device 5.
    pub fn from_config(config: &MachineConfig) -> Option<Self> {
        let model = wanted(config, Model::Fx80)?;
        let five = config.options.get("printer").is_some_and(|v| v == "5");
        let mut p = Self::new(if five { 5 } else { 4 });
        p.printer = Printer::with_model(model);
        // Their own ROM, or (C16, Plus/4) the part of the KERNAL at $D000.
        let charset = match (config.roms.get("chargen"), config.roms.get("kernal")) {
            (Some(rom), _) => rom.get(..rom.len().min(0x1000)),
            (None, Some(rom)) => rom.get(0x1000..0x1800),
            _ => None,
        };
        p.charset = charset.unwrap_or_default().to_vec();
        Some(p)
    }

    /// Print a graphics character from the character ROM.
    fn graphic(&mut self, code: u8, lower: bool) -> bool {
        let set = self.charset.len() / 2;
        let start = usize::from(code) * 8 + if lower { set } else { 0 };
        let Some(rows) = self.charset.get(start..start + 8) else {
            return false;
        };
        let bytes = self.printer.epson().model().glyph_bytes(rows);
        bytes.into_iter().for_each(|b| self.printer.feed(b));
        true
    }

    pub fn save_chunk(&self, w: &mut StateWriter) {
        w.chunk(CHUNK, 1, |w| self.save(w));
    }

    /// Restore from the machine's savestate `s`; see [`Printer::load_chunk`].
    pub fn load_chunk(&mut self, s: &[u8]) {
        let Ok((_, mut c)) = StateReader::new(s).chunk(CHUNK, 1) else {
            return;
        };
        let mut fresh = Self::new(self.device);
        if fresh.load(&mut c).is_ok() {
            fresh.printer.files = self.printer.files.take();
            fresh.charset = std::mem::take(&mut self.charset);
            *self = fresh;
        }
    }

    /// A printer as device `device` (4 or 5).
    pub fn new(device: u8) -> Self {
        Self {
            printer: Printer::new(),
            device,
            listening: false,
            secondary: 0,
            data_phase: false,
            iec: Iec::Idle,
            timer: 0,
            bit: 0,
            byte: 0,
            eoi: false,
            prev_atn: false,
            atn: false,
            charset: Vec::new(),
            pull_data: false,
            pull_ndac: false,
            pull_nrfd: false,
            accepted: false,
        }
    }

    /// RESET lets go of the bus; paper and job stay.
    pub fn reset(&mut self) {
        self.listening = false;
        self.data_phase = false;
        self.iec = Iec::Idle;
        self.atn = false;
        self.pull_data = false;
        self.pull_ndac = false;
        self.pull_nrfd = false;
        self.accepted = false;
    }

    /// A command under ATN.
    fn command(&mut self, b: u8) {
        match b {
            0x3f => self.listening = false,
            0x20..=0x3e => {
                self.listening = b & 0x1f == self.device;
                self.secondary = 0;
                self.data_phase = self.listening;
            }
            // TALK of anyone: this device has nothing to say.
            0x40..=0x5f => self.listening = false,
            0x60..=0x6f if self.listening => {
                self.secondary = b & 0x0f;
                self.data_phase = true;
            }
            // OPEN: its file name follows and is not printed.
            0xf0..=0xff if self.listening => {
                self.secondary = b & 0x0f;
                self.data_phase = false;
            }
            0xe0..=0xef if self.listening => self.data_phase = false,
            _ => {}
        }
    }

    fn received(&mut self, b: u8) {
        if self.atn {
            self.command(b);
        } else if self.listening && self.data_phase {
            let translation = Translation::of_secondary(self.secondary);
            if let Some((code, lower)) = translation.graphic(b) {
                if self.graphic(code, lower) {
                    return;
                }
            }
            let printer = &mut self.printer;
            translation.apply(b, |c| printer.feed(c));
        }
    }

    /// Does the printer pull DATA on the serial bus?
    pub fn iec_data(&self) -> bool {
        self.pull_data
    }

    /// One microsecond on the serial bus; `true` = line low.
    pub fn iec_tick(&mut self, atn: bool, clk: bool, data: bool) {
        if atn && !self.prev_atn {
            // Every device answers ATN by pulling DATA and listens.
            self.atn = true;
            self.pull_data = true;
            self.iec = Iec::AtnWaitClk;
            self.timer = 0;
        } else if !atn && self.prev_atn {
            self.atn = false;
            if self.listening {
                self.iec = Iec::WaitReady;
            } else {
                self.pull_data = false;
                self.iec = Iec::Idle;
            }
            self.timer = 0;
        }
        self.prev_atn = atn;
        self.timer = self.timer.saturating_add(1);
        match self.iec {
            Iec::Idle => {}
            Iec::AtnWaitClk => {
                if clk {
                    self.iec = Iec::WaitReady;
                    self.timer = 0;
                }
            }
            Iec::WaitReady => {
                if !clk {
                    self.pull_data = false;
                    self.eoi = false;
                    self.iec = Iec::WaitClkLow;
                    self.timer = 0;
                }
            }
            Iec::WaitClkLow => {
                if clk {
                    self.bit = 0;
                    self.byte = 0;
                    self.iec = Iec::BitHigh;
                } else if !self.eoi && self.timer >= EOI_TIMEOUT {
                    self.eoi = true;
                    self.pull_data = true;
                    self.iec = Iec::EoiAck;
                    self.timer = 0;
                }
            }
            Iec::EoiAck => {
                if self.timer >= EOI_ACK {
                    self.pull_data = false;
                    self.iec = Iec::WaitClkLow;
                    self.timer = 0;
                }
            }
            Iec::BitHigh => {
                if !clk {
                    if !data {
                        self.byte |= 1 << self.bit;
                    }
                    self.bit += 1;
                    self.iec = Iec::BitLow;
                }
            }
            Iec::BitLow => {
                if clk {
                    if self.bit == 8 {
                        self.pull_data = true;
                        self.iec = Iec::WaitReady;
                        self.timer = 0;
                        self.received(self.byte);
                    } else {
                        self.iec = Iec::BitHigh;
                    }
                }
            }
        }
    }

    /// NDAC and NRFD as this device pulls them on the IEEE-488 bus.
    pub fn ieee_lines(&self) -> (bool, bool) {
        (self.pull_ndac, self.pull_nrfd)
    }

    /// One microsecond on the IEEE-488 bus.
    pub fn ieee_tick(&mut self, atn: bool, dav: bool, data: u8) {
        if atn != self.prev_atn {
            self.prev_atn = atn;
            self.atn = atn;
            self.accepted = false;
            self.pull_ndac = atn || self.listening;
            self.pull_nrfd = false;
            return;
        }
        if self.accepted {
            if !dav {
                self.accepted = false;
                self.pull_ndac = true;
                self.pull_nrfd = false;
            }
        } else if self.pull_ndac && dav {
            self.pull_nrfd = true;
            self.pull_ndac = false;
            self.accepted = true;
            self.received(data);
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.printer.save(w);
        w.u8(self.device);
        w.bool(self.listening);
        w.u8(self.secondary);
        w.bool(self.data_phase);
        w.u8(self.iec as u8);
        w.u32(self.timer);
        w.u8(self.bit);
        w.u8(self.byte);
        w.bool(self.eoi);
        w.bool(self.prev_atn);
        w.bool(self.atn);
        w.bool(self.pull_data);
        w.bool(self.pull_ndac);
        w.bool(self.pull_nrfd);
        w.bool(self.accepted);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.printer.load(r)?;
        self.device = r.u8()?;
        self.listening = r.bool()?;
        self.secondary = r.u8()?;
        self.data_phase = r.bool()?;
        self.iec = match r.u8()? {
            0 => Iec::Idle,
            1 => Iec::AtnWaitClk,
            2 => Iec::WaitReady,
            3 => Iec::WaitClkLow,
            4 => Iec::EoiAck,
            5 => Iec::BitHigh,
            6 => Iec::BitLow,
            _ => return Err(StateError::Invalid("printer bus state")),
        };
        self.timer = r.u32()?;
        self.bit = r.u8()?.min(8);
        self.byte = r.u8()?;
        self.eoi = r.bool()?;
        self.prev_atn = r.bool()?;
        self.atn = r.bool()?;
        self.pull_data = r.bool()?;
        self.pull_ndac = r.bool()?;
        self.pull_nrfd = r.bool()?;
        self.accepted = r.bool()?;
        Ok(())
    }
}

/// The text of an Atari printer record: up to its EOL ($9B), which ends
/// the line; the inverse-video bit is dropped.
pub fn atari_line(record: &[u8], mut out: impl FnMut(u8)) {
    for &b in record {
        if b == 0x9b {
            out(0x0d);
            out(0x0a);
            return;
        }
        match b & 0x7f {
            c @ 0x20..=0x7e => out(c),
            _ => out(b'?'),
        }
    }
}

#[cfg(test)]
mod tests;
