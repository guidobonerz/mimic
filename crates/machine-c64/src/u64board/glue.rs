//! The board and the C64 put together: the board's lines into the C64,
//! its share of every cycle, its picture, its files.

use super::{Elite2, Halt, Kind, Request};
use crate::drive::Drive8;
use crate::{nmi, C64Bus, C64};
use chips_cbm::pla::Bank;
use chips_ultimate::c64port::CartWindow;
use chips_ultimate::overlay::font_from_c64_chargen;
use chips_ultimate::{C64Rom, C64Side, CLOCK_HZ};
use mimic_core::frame::Rgb8;
use mimic_core::host::{lock, HostServices, OpenMode};
use mimic_core::input::MachineKey;
use mimic_core::{Bus, ResetKind, Size, StateError, StateReader, StateWriter};

/// The folder of the board's own files in the frontend's file area.
const FOLDER: &str = "/Board";

/// The HDMI picture's size while no mode is programmed.
const DEFAULT_CANVAS: (usize, usize) = (720, 576);

/// What the board drives into the C64.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Lines {
    /// The 6510 is held for the board's DMA.
    pub stop: bool,
    /// RESET is pulled.
    pub reset: bool,
    /// RESET was released during the board's last run: the C64 starts.
    released: bool,
    /// The board's cartridge logic forces the ULTIMAX map.
    pub ultimax: bool,
    pub nmi: bool,
    /// The firmware asked for NTSC (kept; the VIC-II here is the one the
    /// machine was made with).
    pub ntsc: bool,
    /// `options["roms"] = "firmware"`: the firmware's writes to the ROM
    /// windows reach the C64's ROMs. Otherwise the C64 keeps the
    /// machine's own (configuration, not state).
    pub firmware_roms: bool,
}

/// The board as a part of the machine.
pub(crate) struct BoardPart {
    pub board: Elite2,
    pub lines: Lines,
    /// Fraction of a board clock carried between C64 cycles.
    acc: u64,
    /// The C64's cycles a second.
    cpu_hz: u64,
    /// `options["update"]`: an update file in the file area's root whose
    /// updater starts at power-on in place of the flash's application.
    update: Option<String>,
    /// The overlay's font (8 bytes a character).
    font: Vec<u8>,
    /// The picture with the overlay, made once a frame while it shows
    /// (output, not state).
    frame: Vec<Rgb8>,
    frame_size: (usize, usize),
    scratch: Vec<u8>,
    /// The flash file exists in full (sectors can be written in place).
    flash_file: bool,
    /// The SD card and the USB sticks (not state: media).
    pub storage: super::storage::BoardStorage,
    /// The cable in the Ethernet socket and the router at its end.
    pub net: super::net::Cable,
    /// Without firmware the hint is on the C64's screen (output, not
    /// state).
    hint_shown: bool,
    /// The board's drive B (Commodore 64 Ultimate); drive A is the
    /// machine's drive 8 ([`super::drives`]).
    pub drive_b: Option<Drive8>,
    /// LEDs on the strip plugged into an Elite II's LED connector
    /// (`options["ledstrip"]`, configuration).
    strip_leds: usize,
}

/// LEDs on an Elite II's strip unless `options["ledstrip"]` says
/// otherwise: the firmware's own default for "Strip Length".
pub const STRIP_LEDS: usize = 24;

/// A state's board, loaded into new parts (taken over only when all of
/// the machine has loaded).
pub(crate) struct LoadedBoard {
    board: Elite2,
    lines: Lines,
    acc: u64,
    drive_b: Option<Drive8>,
    /// The router's state (empty: a state without one).
    net: Vec<u8>,
}

/// What the C64's screen says when the board has no firmware (rows of
/// 40 characters at most, from row 8 on).
const NO_FIRMWARE: [&str; 9] = [
    "ULTIMATE 64: NO FIRMWARE IN ITS FLASH,",
    "THE C64 RUNS WITHOUT THE BOARD.",
    "",
    "INSTALL ONE WITH AN UPDATE FILE (.UE2):",
    "PUT IT INTO THE FILE AREA (--UCI-DIR,",
    "DEFAULT .MIMIC/ULTIMATE IN YOUR HOME)",
    "AND START WITH --OPTION UPDATE=NAME.UE2",
    "ONCE. THE FLASH IS THEN KEPT IN",
    "BOARD/ELITE2-FLASH.BIN OF THAT FOLDER.",
];

/// The C64 for the board's blocks during a run.
struct Side<'a, 'b> {
    bus: C64Bus<'a>,
    lines: &'b mut Lines,
}

impl C64Side for Side<'_, '_> {
    fn dma_read(&mut self, addr: u16) -> u8 {
        self.bus.read(u32::from(addr), Size::Byte) as u8
    }

    fn dma_peek(&self, addr: u16) -> u8 {
        self.bus.peek(u32::from(addr), Size::Byte) as u8
    }

    fn dma_write(&mut self, addr: u16, v: u8) {
        self.bus.write(u32::from(addr), Size::Byte, u32::from(v));
    }

    fn set_stopped(&mut self, on: bool) {
        self.lines.stop = on;
    }

    fn stopped(&self) -> bool {
        self.lines.stop
    }

    fn set_reset(&mut self, on: bool) {
        if self.lines.reset && !on {
            self.lines.released = true;
        }
        self.lines.reset = on;
    }

    fn rom_write(&mut self, rom: C64Rom, offset: u16, v: u8) {
        if !self.lines.firmware_roms {
            return;
        }
        let roms = &mut self.bus.mem.roms;
        let o = usize::from(offset);
        match rom {
            C64Rom::Basic => roms.basic[o % roms.basic.len()] = v,
            C64Rom::Kernal => roms.kernal[o % roms.kernal.len()] = v,
            C64Rom::Char => roms.chargen[o % roms.chargen.len()] = v,
        }
    }

    fn rom_read(&self, rom: C64Rom, offset: u16) -> u8 {
        let roms = &self.bus.mem.roms;
        let o = usize::from(offset);
        match rom {
            C64Rom::Basic => roms.basic[o % roms.basic.len()],
            C64Rom::Kernal => roms.kernal[o % roms.kernal.len()],
            C64Rom::Char => roms.chargen[o % roms.chargen.len()],
        }
    }

    fn set_ntsc(&mut self, ntsc: bool) {
        self.lines.ntsc = ntsc;
    }

    fn set_ultimax(&mut self, on: bool) {
        self.lines.ultimax = on;
    }

    fn set_cart_lines(&mut self, game: bool, exrom: bool) {
        // At once, for the DMA of this run and the 6510's next cycle
        // (`C64::cartridge_cycle` keeps them from then on).
        let (game, exrom) = (!game, !exrom);
        if (self.bus.mem.game, self.bus.mem.exrom) != (game, exrom) {
            self.bus.mem.set_cartridge_lines(game, exrom);
        }
    }

    fn cart_window(&self, addr: u16) -> Option<CartWindow> {
        match self.bus.mem.bank(addr) {
            Bank::RomL => Some(CartWindow::RomL),
            Bank::RomH => Some(CartWindow::RomH),
            Bank::Io if (0xde00..=0xdfff).contains(&addr) => Some(CartWindow::Io),
            _ => None,
        }
    }

    fn set_nmi(&mut self, on: bool) {
        self.lines.nmi = on;
    }

    fn dma_read_ram(&mut self, addr: u16) -> u8 {
        self.bus.mem.ram[usize::from(addr)]
    }

    fn dma_peek_ram(&self, addr: u16) -> u8 {
        self.bus.mem.ram[usize::from(addr)]
    }

    fn dma_write_ram(&mut self, addr: u16, v: u8) {
        self.bus.mem.ram[usize::from(addr)] = v;
    }
}

fn read_file(host: &HostServices, path: &str) -> Option<Vec<u8>> {
    let files = host.files.as_ref()?;
    let mut f = lock(files);
    let h = f
        .open(
            path,
            OpenMode {
                read: true,
                ..OpenMode::default()
            },
        )
        .ok()?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 0x1_0000];
    loop {
        match f.read(h, &mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => {
                f.close(h);
                return None;
            }
        }
    }
    f.close(h);
    Some(out)
}

/// Write `data` at `pos` of a file (made if it is not there). Done?
fn write_file(host: &HostServices, path: &str, pos: u64, data: &[u8]) -> bool {
    let Some(files) = host.files.as_ref() else {
        return false;
    };
    let mut f = lock(files);
    let _ = f.mkdir(FOLDER);
    let Ok(h) = f.open(
        path,
        OpenMode {
            read: true,
            write: true,
            create: true,
            ..OpenMode::default()
        },
    ) else {
        return false;
    };
    let mut done = f.seek(h, pos).is_ok();
    let mut rest = data;
    while done && !rest.is_empty() {
        match f.write(h, rest) {
            Ok(n) if n > 0 => rest = &rest[n..],
            _ => done = false,
        }
    }
    f.close(h);
    done
}

impl BoardPart {
    /// The board's LEDs as they are lit ([`mimic_core::Machine::lights`]).
    ///
    /// Elite II: the strip on the LED connector, as many LEDs as it has
    /// (the controller always sends 84 colours; a strip takes as many as
    /// it has LEDs). Commodore 64 Ultimate: what its firmware calls the
    /// "Case Lights", in the controller's order 30 along the bottom, 30
    /// along the top and a strip of 24 at the keyboard (top shown first),
    /// and the "Keyboard Lights", the Blingboard's 71 LEDs in the order
    /// their controller sends them (where each lies under the keyboard is
    /// not known).
    pub fn lights(&self) -> Vec<mimic_core::LightGroup> {
        use mimic_core::LightGroup;
        let leds = self.board.misc.led.leds();
        match self.board.kind() {
            Kind::Elite2 if self.strip_leds == 0 => Vec::new(),
            Kind::Elite2 => vec![LightGroup::from_light(
                "LED STRIP",
                &leds[..self.strip_leds.min(leds.len())],
            )],
            Kind::C64Ultimate => vec![
                LightGroup::from_light("CASE LIGHTS TOP", &leds[30..60]),
                LightGroup::from_light("CASE LIGHTS BOTTOM", &leds[..30]),
                LightGroup::from_light("CASE LIGHTS STRIP", &leds[60..]),
                LightGroup::from_light("KEYBOARD LIGHTS", &self.board.misc.bling_led.leds()),
            ],
        }
    }

    pub fn new(
        kind: Kind,
        cpu_hz: u64,
        update: Option<String>,
        firmware_roms: bool,
        chargen: &[u8],
        net: super::net::Cable,
        strip_leds: usize,
    ) -> Self {
        Self {
            strip_leds,
            board: Elite2::new(kind),
            lines: Lines {
                firmware_roms,
                ..Lines::default()
            },
            acc: 0,
            cpu_hz,
            update,
            font: font_from_c64_chargen(chargen),
            frame: Vec::new(),
            frame_size: (0, 0),
            scratch: Vec::new(),
            flash_file: false,
            storage: Default::default(),
            net,
            hint_shown: false,
            drive_b: (kind == Kind::C64Ultimate).then(|| super::drives::new_drive_b(cpu_hz as u32)),
        }
    }

    fn name(&self) -> &'static str {
        match self.board.kind() {
            Kind::Elite2 => "elite2",
            Kind::C64Ultimate => "c64u",
        }
    }

    fn flash_path(&self) -> String {
        format!("{FOLDER}/{}-flash.bin", self.name())
    }

    /// The WiFi module's software version, as last flashed.
    fn module_path(&self) -> String {
        format!("{FOLDER}/{}-esp32.ver", self.name())
    }

    fn registers_path(&self) -> String {
        format!("{FOLDER}/{}-flash.reg", self.name())
    }

    /// The overlay shows: its picture is the machine's.
    pub fn picture(&self) -> Option<(&[Rgb8], usize, usize)> {
        (self.board.overlay.active() && !self.frame.is_empty()).then_some((
            &self.frame[..],
            self.frame_size.0,
            self.frame_size.1,
        ))
    }

    /// The C64 does not run: the board holds it.
    pub fn holds(&self) -> bool {
        self.lines.stop || self.lines.reset
    }

    /// A key of the C64's keyboard, which the board reads too. `false`:
    /// the board's menu has the keyboard and the C64 does not get the
    /// key going down.
    pub fn key(&mut self, key: MachineKey, down: bool) -> bool {
        if key.0 < 64 {
            // The board's matrix is the C64's: column (CIA 1 port A line)
            // times eight plus row (port B line).
            self.board
                .u64io
                .set_key((key.0 % 8) as u8, (key.0 / 8) as u8, down);
        }
        !down || !self.board.overlay.owns_keyboard()
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"U64B", 3, |w| {
            w.u8(self.board.kind().fpga_type());
            for b in [
                self.lines.stop,
                self.lines.reset,
                self.lines.released,
                self.lines.ultimax,
                self.lines.nmi,
                self.lines.ntsc,
            ] {
                w.bool(b);
            }
            w.u64(self.acc);
            self.board.save(w);
            // Version 2: drive B (empty: none).
            let mut b = StateWriter::new();
            if let Some(d) = &self.drive_b {
                d.save(&mut b);
            }
            w.bytes(&b.into_bytes());
            // Version 3: the router at the Ethernet cable's end.
            let mut n = StateWriter::new();
            self.net.save(&mut n);
            w.bytes(&n.into_bytes());
        });
    }

    /// A state's board, loaded into a new one (the state is taken over
    /// only when all of the machine has loaded).
    pub fn loaded(&self, s: &[u8]) -> Result<LoadedBoard, StateError> {
        let (version, mut c) = StateReader::new(s).chunk(*b"U64B", 3)?;
        if c.u8()? != self.board.kind().fpga_type() {
            return Err(StateError::Invalid("the state is another board's"));
        }
        let lines = Lines {
            stop: c.bool()?,
            reset: c.bool()?,
            released: c.bool()?,
            ultimax: c.bool()?,
            nmi: c.bool()?,
            ntsc: c.bool()?,
            firmware_roms: self.lines.firmware_roms,
        };
        let acc = c.u64()?;
        if acc >= self.cpu_hz {
            return Err(StateError::Invalid("board clock fraction"));
        }
        let mut board = Elite2::new(self.board.kind());
        board.load(&mut c)?;
        let drive_b = match &self.drive_b {
            Some(_) => {
                let mut d = super::drives::new_drive_b(self.cpu_hz as u32);
                if version >= 2 {
                    d.load(&mut StateReader::new(c.bytes()?))?;
                }
                Some(d)
            }
            None => None,
        };
        let net = if version >= 3 {
            c.bytes()?.to_vec()
        } else {
            Vec::new()
        };
        Ok(LoadedBoard {
            board,
            lines,
            acc,
            drive_b,
            net,
        })
    }

    pub fn take_loaded(&mut self, l: LoadedBoard) {
        self.board = l.board;
        self.lines = l.lines;
        self.acc = l.acc;
        if l.drive_b.is_some() {
            self.drive_b = l.drive_b;
        }
        // The router's host sockets do not outlive a state: what does
        // not load leaves it as it is, and the firmware's connections
        // time out as with a cable pulled for a moment.
        let _ = self.net.load(&mut StateReader::new(&l.net));
    }
}

impl C64 {
    /// The board's share of a C64 cycle.
    pub(crate) fn board_cycle(&mut self) {
        let Some(mut part) = self.board.take() else {
            return;
        };
        part.acc += CLOCK_HZ;
        let clocks = part.acc / part.cpu_hz;
        part.acc %= part.cpu_hz;
        let request = {
            let mut side = Side {
                bus: C64Bus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    // The board answers for its own cartridge itself
                    // (`C64Side::cart_window`); the port has no other.
                    cart: None,
                    uci: None,
                    reu: None,
                },
                lines: &mut part.lines,
            };
            part.net.advance(part.board.now());
            part.board
                .run(clocks, &mut side, &mut part.storage, &mut part.net)
        };
        self.ints.set_nmi(nmi::BOARD, part.lines.nmi);
        let released = std::mem::take(&mut part.lines.released);
        self.board = Some(part);
        if released {
            self.reset_c64(ResetKind::Soft);
        }
        match request {
            Some(Request::Reboot) => self.board_power_on(),
            Some(Request::PowerOff) | None => {}
        }
    }

    /// One access of the board's I/O bus as its firmware makes it (tests):
    /// `Some(v)` writes, `None` reads.
    #[cfg(test)]
    pub(crate) fn board_io(&mut self, addr: u32, v: Option<u8>) -> u8 {
        let Some(mut part) = self.board.take() else {
            return 0;
        };
        let out = {
            let mut side = Side {
                bus: C64Bus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    cart: None,
                    uci: None,
                    reu: None,
                },
                lines: &mut part.lines,
            };
            part.board.io_access(addr, v, &mut side)
        };
        self.ints.set_nmi(nmi::BOARD, part.lines.nmi);
        let released = std::mem::take(&mut part.lines.released);
        self.board = Some(part);
        if released {
            self.reset_c64(ResetKind::Soft);
        }
        out
    }

    /// Once a frame: changed flash sectors go to their file, and the
    /// overlay's picture is made.
    pub(crate) fn board_frame(&mut self) {
        self.board_drives_frame();
        let host = self.device_host();
        let Some(part) = &mut self.board else {
            return;
        };
        // Written media go to their files once the board is quiet. (A
        // failure stays in the backend as unsaved changes, which the
        // frontend's save reports.)
        let _ = part.storage.frame();
        if part.board.flash.is_dirty() {
            let path = part.flash_path();
            let dirty = part.board.flash.take_dirty();
            let image = part.board.flash.image();
            if part.flash_file {
                for s in dirty {
                    let at = s as usize * 0x1000;
                    write_file(&host, &path, at as u64, &image[at..at + 0x1000]);
                }
            } else {
                part.flash_file = write_file(&host, &path, 0, image);
            }
            write_file(
                &host,
                &part.registers_path(),
                0,
                &part.board.flash.nv_registers(),
            );
        }
        // An updater flashed the WiFi module: its version is kept.
        if let Some((major, minor)) = part.board.wifi.take_identity_changed() {
            let mut v = major.to_le_bytes().to_vec();
            v.extend_from_slice(&minor.to_le_bytes());
            write_file(&host, &part.module_path(), 0, &v);
        }
        if !part.board.overlay.active() {
            part.frame.clear();
            return;
        }
        // The HDMI picture: the VIC-II's stretched over it, the overlay
        // on top. (The board crops and scales by fixed ratios; here the
        // whole frame is stretched.)
        let (w, h) = part.board.overlay.canvas_size().unwrap_or(DEFAULT_CANVAS);
        let t = self.io.vic.timing();
        let (sw, sh) = (t.width as usize, t.height as usize);
        let src = self.io.vic.pixels();
        if w == 0 || h == 0 || src.len() < sw * sh {
            part.frame.clear();
            return;
        }
        part.scratch.resize(w * h * 3, 0);
        for y in 0..h {
            let row = &src[(y * sh / h) * sw..][..sw];
            let out = &mut part.scratch[y * w * 3..][..w * 3];
            for x in 0..w {
                let c = self.palette[usize::from(row[x * sw / w]) & 15];
                out[x * 3] = c.r;
                out[x * 3 + 1] = c.g;
                out[x * 3 + 2] = c.b;
            }
        }
        part.board
            .overlay
            .render(&part.font, &mut part.scratch, w, h);
        part.frame.clear();
        part.frame
            .extend(part.scratch.as_chunks::<3>().0.iter().map(|p| Rgb8 {
                r: p[0],
                g: p[1],
                b: p[2],
            }));
        part.frame_size = (w, h);
    }

    /// The frontend's file area arrived: the flash and the font from
    /// their files, then power-on.
    pub(crate) fn board_attach(&mut self) {
        let host = self.device_host();
        if let Some(part) = &mut self.board {
            part.net.attach_host(host.clone());
            if let Some(image) = read_file(&host, &part.flash_path()) {
                part.board.flash.load_image(&image);
                part.flash_file = image.len() == part.board.flash.image().len();
            }
            if let Some(reg) = read_file(&host, &part.registers_path()) {
                if let Ok(reg) = <[u8; 4]>::try_from(&reg[..]) {
                    part.board.flash.set_nv_registers(reg);
                }
            }
            if let Some(v) = read_file(&host, &part.module_path()) {
                if let Ok(v) = <[u8; 4]>::try_from(&v[..]) {
                    part.board.wifi.set_identity(
                        u16::from_le_bytes([v[0], v[1]]),
                        u16::from_le_bytes([v[2], v[3]]),
                    );
                }
            }
            // The overlay's own font, if the user has it.
            if let Some(font) = read_file(&host, &format!("{FOLDER}/chars.bin")) {
                if font.len() >= 1024 {
                    part.font = font;
                }
            }
            if let Some(net) = &host.net {
                let secs = lock(net).time().unix_secs;
                part.board
                    .misc
                    .set_epoch(secs.clamp(0, i64::from(u32::MAX)) as u32);
            }
        }
        self.board_power_on();
    }

    /// The board's power-on: its processor starts from the flash, or
    /// with the update file's updater.
    pub(crate) fn board_power_on(&mut self) {
        let host = self.device_host();
        let Some(mut part) = self.board.take() else {
            return;
        };
        // The update file is run once, as "Run Update" would: the next
        // power-on starts what its updater wrote.
        let update = part
            .update
            .as_ref()
            .and_then(|name| read_file(&host, &format!("/{name}")));
        if update.is_some() {
            part.update = None;
        }
        part.lines = Lines {
            firmware_roms: part.lines.firmware_roms,
            ..Lines::default()
        };
        part.acc = 0;
        {
            let mut side = Side {
                bus: C64Bus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    // The board answers for its own cartridge itself
                    // (`C64Side::cart_window`); the port has no other.
                    cart: None,
                    uci: None,
                    reu: None,
                },
                lines: &mut part.lines,
            };
            part.board.power_on(update.as_deref(), &mut side);
            part.net.power_on();
        }
        part.lines.released = false;
        // No firmware: the board stands still and lets the C64 go, which
        // then says so on its screen.
        if part.board.halt() == Halt::NoFirmware {
            part.lines.reset = false;
            part.lines.stop = false;
            part.lines.ultimax = false;
        }
        part.hint_shown = false;
        self.board = Some(part);
    }

    /// Once BASIC has booted: the no-firmware hint on the C64's screen.
    pub(crate) fn board_hint(&mut self) {
        let Some(part) = &mut self.board else {
            return;
        };
        if part.hint_shown || part.board.halt() != Halt::NoFirmware {
            return;
        }
        part.hint_shown = true;
        for (row, line) in NO_FIRMWARE.iter().enumerate() {
            let at = 0x0400 + (8 + row) * 40;
            for (i, c) in line.bytes().take(40).enumerate() {
                // Screen codes of the upper-case set: letters 1-26,
                // the rest as in ASCII.
                let code = match c {
                    b'A'..=b'Z' => c - b'@',
                    b'@' => 0,
                    _ => c,
                };
                self.mem.ram[at + i] = code;
                self.mem.color[(at - 0x0400 + i) & 0x3ff] = 7;
            }
        }
    }

    /// The board for the debugger's chip list: whether its processor
    /// runs, where it is, the interrupt line, the C64's lines.
    pub(crate) fn board_chip(&self) -> Option<mimic_core::ChipInfo> {
        let part = self.board.as_ref()?;
        let b = &part.board;
        let reg = |name, value: u32, bits| mimic_core::machine::ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let halt = match b.halt() {
            Halt::Running => 0,
            Halt::NoFirmware => 1,
            Halt::PowerOff => 2,
            Halt::Illegal(_) => 3,
        };
        Some(mimic_core::ChipInfo {
            name: "board",
            regs: vec![
                // 0 runs, 1 no firmware in the flash, 2 switched off.
                reg("HALT", halt, 8),
                reg("PC", b.cpu.pc(), 32),
                reg("IRQ", u32::from(b.cpu.irq()), 1),
                reg("C64STOP", u32::from(part.lines.stop), 1),
                reg("C64RESET", u32::from(part.lines.reset), 1),
                reg("ULTIMAX", u32::from(part.lines.ultimax), 1),
                reg("OVERLAY", u32::from(b.overlay.active()), 1),
            ],
        })
    }
}
