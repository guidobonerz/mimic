//! Apple Macintosh 128K, 512K, Plus and SE (Apple Computer, 1984–1987).
//!
//! A 68000 at 7.8336 MHz, RAM shared with the video and sound circuits,
//! the ROM with the Toolbox, a 6522 VIA (keyboard, real-time clock, sound
//! and screen buffer selection, mouse direction), a Z8530 SCC (serial
//! ports, mouse movement), the IWM floppy controller with Sony 3.5"
//! drives ([`drives_other::iwm`]) and on the Plus and SE an NCR 5380 SCSI
//! controller ([`scsi`]). The SE has the Apple Desktop Bus ([`adb`])
//! instead of the keyboard and mouse ports.
//!
//! **Memory map** (24-bit; Inside Macintosh III, "Hardware"; Linux/m68k
//! "Mac Plus hardware" notes, linux-mac68k.sourceforge.net):
//!
//! | Address           | Contents                                         |
//! | ----------------- | ------------------------------------------------ |
//! | $000000–$3FFFFF   | RAM (repeated by its size); ROM while the overlay is on |
//! | $400000–$57FFFF   | ROM (repeated by its size; to $5FFFFF without SCSI) |
//! | $580000–$5FFFFF   | SCSI 5380 (Plus/SE): register at A6–A4, DACK at A9 |
//! | $600000–$7FFFFF   | RAM (where it is while the overlay is on)         |
//! | $800000–$9FFFFF   | SCC reads ($9FFFF8 + 2n)                          |
//! | $A00000–$BFFFFF   | SCC writes ($BFFFF9 + 2n)                         |
//! | $C00000–$DFFFFF   | IWM, odd bytes, register at A12–A9               |
//! | $E00000–$EFFFFF   | VIA, even bytes, register at A12–A9               |
//! | $F00000–$FFFFFF   | phase read; interrupt acknowledge (autovectors)  |
//!
//! The overlay is on after reset so the 68000 finds its reset vectors in
//! ROM; on the 128K/512K/Plus VIA PA4 turns it off, on the SE the first
//! access to the ROM at its own address does (PA4 selects the second
//! internal drive there).
//!
//! **VIA** (Linux/m68k notes; Inside Macintosh III): PA0–2 sound volume,
//! PA3 sound buffer (0 = alternate), PA4 overlay, PA5 floppy head select,
//! PA6 screen buffer (0 = alternate), PA7 SCC wait/request; PB0–2 the
//! real-time clock's data, clock and /enable, PB3 mouse button (0 =
//! down; SE: ADB /INT), PB4/PB5 mouse X2/Y2 (SE: ADB ST0/ST1), PB6
//! horizontal blanking, PB7 sound off; CA1 vertical blanking, CA2 the
//! clock's one-second signal, CB1/CB2 and the shift register the keyboard
//! (SE: the ADB transceiver). The VIA runs on the E clock (CPU ÷ 10) and
//! CPU accesses to it wait for it.
//!
//! **Interrupts**: the VIA drives IPL0 (level 1), the SCC IPL1 (level 2),
//! the programmer's interrupt switch IPL2 (level 4 and up); interrupts
//! are autovectored.
//!
//! **Mouse** (128K/512K/Plus): X1/Y1 to the SCC's DCDA/DCDB (an interrupt
//! on each change), X2/Y2 to VIA PB4/PB5, sampled once a line. The SE
//! reads the same mouse device through ADB.
//!
//! **Sound**: the high byte of the sound buffer word of each line, scaled
//! by the volume and switched off by PB7, played at the E clock rate and
//! resampled to 48 kHz.
//!
//! **Models** (`--model`): `plus` (default; 128 KB ROM, 1 MB RAM, 800K
//! drives, SCSI, M0110A keyboard), `128k` and `512k` (64 KB ROM, 400K
//! drives, M0110 keyboard), `se` (256 KB ROM, ADB, two internal 800K
//! drives). Options: `ram` = 512k/1m/2m/4m (Plus, SE), `serial` =
//! loopback (both ports wired back to themselves), `roms` = stub
//! ([`stub`]).
//!
//! ROMs (user-supplied): `mac128k` (64 KB), `mac512k` (64 KB; the 128K's
//! when missing), `macplus` (128 KB), `macse` (256 KB), or each as its two
//! chips (`-h` the even bytes, `-l` the odd). The first long word of a Mac
//! ROM is its checksum, which the 68000 takes as its first stack pointer.
//!
//! Sources: Inside Macintosh III (1985), "The Macintosh Hardware";
//! Apple, "Guide to the Macintosh Family Hardware" (2nd ed., 1990);
//! Linux/m68k Mac hardware notes; the module documents name theirs. The
//! implementation is this project's own.

#![forbid(unsafe_code)]

pub mod adb;
pub mod keyboard;
pub mod rtc;
pub mod scc;
pub mod scsi;
pub mod stub;
pub mod video;

#[cfg(test)]
mod tests;

use adb::Adb;
use chips_cbm::via::Via;
use cpu_m68k::{Access, Step, TimedBus68, M68000};
use drives_other::iwm::Iwm;
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Size, StateError, StateReader,
    StateWriter, StopReason,
};
use keyboard::Keyboard;
use rtc::Rtc;
use scc::Scc;
use scsi::Scsi;
use video::Screen;

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// RAM and ROM by their own addresses.
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const ROM_SPACE: MemSpaceId = MemSpaceId(2);
pub const SLOT_INTERNAL: SlotId = SlotId(0);
pub const SLOT_EXTERNAL: SlotId = SlotId(1);
/// The SE's second internal drive.
pub const SLOT_INTERNAL2: SlotId = SlotId(2);
/// SCSI ID n is slot `SLOT_SCSI.0 + n`.
pub const SLOT_SCSI: SlotId = SlotId(10);
pub const MOUSE_PORT: PortId = PortId(0);

pub const CPU_HZ: u32 = 7_833_600;
/// CPU clocks per E clock (the VIA's clock).
pub const E_CLOCKS: u64 = 10;
/// Half a second: the clock chip's one-second signal changes level.
const HALF_SECOND: u64 = CPU_HZ as u64 / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Mac128k,
    Mac512k,
    Plus,
    Se,
}

impl Model {
    fn code(self) -> u8 {
        match self {
            Model::Mac128k => 0,
            Model::Mac512k => 1,
            Model::Plus => 2,
            Model::Se => 3,
        }
    }

    fn rom_size(self) -> usize {
        match self {
            Model::Mac128k | Model::Mac512k => 0x1_0000,
            Model::Plus => 0x2_0000,
            Model::Se => 0x4_0000,
        }
    }

    fn rom_role(self) -> &'static str {
        match self {
            Model::Mac128k => "mac128k",
            Model::Mac512k => "mac512k",
            Model::Plus => "macplus",
            Model::Se => "macse",
        }
    }

    pub fn scsi(self) -> bool {
        matches!(self, Model::Plus | Model::Se)
    }

    /// 800K drives.
    pub fn double_sided(self) -> bool {
        matches!(self, Model::Plus | Model::Se)
    }
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "mac",
        name: "Apple Macintosh",
        models: &["plus", "128k", "512k", "se"],
        create: |config| Ok(Box::new(Mac::new(config)?)),
    }
}

fn invalid(key: &str, value: &str, choices: &str) -> ConfigError {
    ConfigError::Invalid(format!("{key} option '{value}' ({choices})"))
}

/// The ROM image: one file, or the two chips (even and odd bytes).
fn load_rom(model: Model, config: &MachineConfig) -> Result<Vec<u8>, ConfigError> {
    let role = model.rom_role();
    let size = model.rom_size();
    if let Some(r) = config.roms.get(role) {
        if r.len() != size {
            return Err(ConfigError::Invalid(format!(
                "ROM {role} must be {size} bytes, got {}",
                r.len()
            )));
        }
        return Ok(r.clone());
    }
    let hi = config.roms.get(&format!("{role}-h"));
    let lo = config.roms.get(&format!("{role}-l"));
    match (hi, lo) {
        (Some(h), Some(l)) if h.len() == size / 2 && l.len() == size / 2 => {
            Ok(h.iter().zip(l).flat_map(|(&a, &b)| [a, b]).collect())
        }
        // The 512K runs the 128K's ROM too.
        _ if model == Model::Mac512k => load_rom(Model::Mac128k, config),
        _ => Err(ConfigError::MissingRom(role.into())),
    }
}

/// Everything on the bus but the CPU and the mouse.
struct Board {
    model: Model,
    ram: Vec<u8>,
    rom: Vec<u8>,
    /// SE: the overlay (the others take it from VIA PA4).
    se_overlay: bool,
    via: Via,
    rtc: Rtc,
    kbd: Option<Keyboard>,
    adb: Option<Adb>,
    scc: Scc,
    iwm: Iwm,
    scsi: Option<Scsi>,
    screen: Screen,
    /// CPU clocks since power-on.
    clock: u64,
    frame: u64,
    frame_done: bool,
    /// Mouse lines X2, Y2, button (down) as last sampled.
    mouse_x2: bool,
    mouse_y2: bool,
    mouse_button: bool,
    /// SE: the mouse's Gray-code phases as last seen.
    mouse_phase: [u8; 2],
    /// The programmer's interrupt switch is held.
    nmi: bool,
    /// The current sound sample (the buffer word's high byte).
    sample: u8,
    /// Average of the sound buffer's low bytes (the 400K drives' speed
    /// control), in 1/256.
    pwm_avg: u32,
    audio: Decimator,
    dc: [i32; 2],
    samples: Vec<i16>,
}

impl Board {
    fn ram_size(&self) -> u32 {
        self.ram.len() as u32
    }

    fn overlay(&self) -> bool {
        if self.model == Model::Se {
            self.se_overlay
        } else {
            self.via.port_a_out() & 0x10 != 0
        }
    }

    fn line_pos(&self) -> (u64, u64) {
        let f = self.clock % video::FRAME_CLOCKS;
        (f / video::LINE_CLOCKS, f % video::LINE_CLOCKS)
    }

    /// The ROM at $400000: the 128K/512K's 64 KB repeat up to $5FFFFF;
    /// on the Plus and SE the ROM space ends at $43FFFF (the Plus ROM tells
    /// a Plus from a 512Ke by $440000 not repeating $420000) and the rest
    /// reads as 0.
    fn rom_byte(&self, a: u32) -> u8 {
        if self.model.scsi() && a & 0x1f_ffff >= 0x4_0000 {
            return 0;
        }
        self.rom[(a as usize) & (self.rom.len() - 1)]
    }

    fn ram_byte(&self, a: u32) -> u8 {
        self.ram[(a as usize) & (self.ram.len() - 1)]
    }

    /// The VIA's port A input pins: PA7 (SCC wait/request) high.
    fn pa_pins(&self) -> u8 {
        0xff
    }

    /// The VIA's port B input pins.
    fn pb_pins(&self) -> u8 {
        let mut v = 0xffu8;
        if self.rtc.driving() && !self.rtc.data_out {
            v &= !0x01;
        }
        if self.model == Model::Se {
            if let Some(adb) = &self.adb {
                if !adb.int_line() {
                    v &= !0x08;
                }
            }
        } else {
            if self.mouse_button {
                v &= !0x08;
            }
            if !self.mouse_x2 {
                v &= !0x10;
            }
            if !self.mouse_y2 {
                v &= !0x20;
            }
        }
        let (_, h) = self.line_pos();
        if h < video::ACTIVE_CLOCKS {
            v &= !0x40;
        }
        v
    }

    fn ipl(&self) -> u8 {
        u8::from(self.via.irq()) | u8::from(self.scc.irq()) << 1 | u8::from(self.nmi) << 2
    }

    /// The VIA's outputs changed: the clock chip, the drives, ADB.
    fn via_outputs(&mut self) {
        let pb = self.via.port_b_out();
        self.rtc
            .lines(pb & 0x04 == 0, pb & 0x02 != 0, pb & 0x01 != 0);
        let pa = self.via.port_a_out();
        self.iwm.sel = pa & 0x20 != 0;
        if self.model == Model::Se {
            self.iwm.drive_b = pa & 0x10 == 0;
            let now = self.clock;
            if let Some(adb) = &mut self.adb {
                adb.set_state(pb >> 4 & 3, now);
            }
        }
    }

    fn via_read(&mut self, reg: u8) -> u8 {
        let (pa, pb) = (self.pa_pins(), self.pb_pins());
        self.via.read(reg, pa, pb)
    }

    fn via_write(&mut self, reg: u8, v: u8) {
        self.via.write(reg, v);
        if reg == 0x0a && self.via.acr >> 2 & 7 == 7 {
            let now = self.clock;
            if let Some(k) = &mut self.kbd {
                k.command_started(now);
            }
            if let Some(a) = &mut self.adb {
                a.byte_written(now);
            }
        }
        self.via_outputs();
    }

    /// A byte access to the I/O and memory space; `write` is the byte of a
    /// write cycle.
    fn access(&mut self, addr: u32, write: Option<u8>) -> u8 {
        let addr = addr & 0xff_ffff;
        let overlay = self.overlay();
        match addr >> 20 {
            0x0..=0x3 if overlay => self.rom[(addr as usize) & (self.rom.len() - 1)],
            0x0..=0x3 | 0x6 | 0x7 => {
                let i = (addr as usize) & (self.ram.len() - 1);
                match write {
                    Some(v) => {
                        self.ram[i] = v;
                        v
                    }
                    None => self.ram[i],
                }
            }
            0x5 if addr >= 0x58_0000 && self.scsi.is_some() => {
                let reg = (addr >> 4 & 7) as u8;
                let dack = addr & 0x200 != 0;
                let scsi = self.scsi.as_mut().expect("checked");
                match write {
                    Some(v) => {
                        scsi.write(reg, v, dack);
                        v
                    }
                    None => scsi.read(reg, dack),
                }
            }
            0x4 | 0x5 => {
                if self.model == Model::Se {
                    self.se_overlay = false;
                }
                self.rom_byte(addr)
            }
            0x8..=0xb => {
                self.scc.run_to(self.clock);
                let ch = usize::from(addr & 2 == 0);
                let data = addr & 4 != 0;
                match (write, addr < 0xa0_0000) {
                    (None, true) => self.scc.read(ch, data),
                    (Some(v), false) => {
                        self.scc.write(ch, data, v);
                        v
                    }
                    _ => 0xff,
                }
            }
            0xc | 0xd => {
                if addr & 1 == 0 {
                    return 0xff;
                }
                let reg = (addr >> 9 & 15) as u8;
                self.iwm.access(reg, write, self.clock)
            }
            0xe => {
                if addr & 1 != 0 {
                    return 0xff;
                }
                let reg = (addr >> 9 & 15) as u8;
                match write {
                    Some(v) => {
                        self.via_write(reg, v);
                        v
                    }
                    None => self.via_read(reg),
                }
            }
            _ => 0,
        }
    }

    fn read(&mut self, addr: u32, size: Size) -> u32 {
        bytes(addr, size).fold(0, |v, a| v << 8 | u32::from(self.access(a, None)))
    }

    /// Side-effect-free read (golden rule 2).
    fn peek(&self, addr: u32, size: Size) -> u32 {
        bytes(addr, size).fold(0, |v, a| v << 8 | u32::from(self.peek_byte(a)))
    }

    fn peek_byte(&self, addr: u32) -> u8 {
        let addr = addr & 0xff_ffff;
        match addr >> 20 {
            0x0..=0x3 if self.overlay() => self.rom[(addr as usize) & (self.rom.len() - 1)],
            0x0..=0x3 | 0x6 | 0x7 => self.ram_byte(addr),
            0x5 if addr >= 0x58_0000 && self.scsi.is_some() => self
                .scsi
                .as_ref()
                .map_or(0, |s| s.peek((addr >> 4 & 7) as u8)),
            0x4 | 0x5 => self.rom_byte(addr),
            0x8 | 0x9 => self.scc.peek(usize::from(addr & 2 == 0), addr & 4 != 0),
            0xc | 0xd if addr & 1 == 1 => self.iwm.peek(self.clock),
            0xe if addr & 1 == 0 => {
                self.via
                    .peek((addr >> 9 & 15) as u8, self.pa_pins(), self.pb_pins())
            }
            0xa..=0xe => 0xff,
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        let n = size.bytes();
        for (i, a) in bytes(addr, size).enumerate() {
            let shift = 8 * (n - 1 - i as u32);
            self.access(a, Some((v >> shift) as u8));
        }
    }

    /// 68000 RESET: the VIA, the IWM, the SCSI chip reset.
    fn reset_devices(&mut self) {
        self.via.reset();
        self.iwm.reset();
        if let Some(s) = &mut self.scsi {
            s.reset();
        }
        self.se_overlay = true;
        self.via_outputs();
    }

    /// Sample the mouse port once a line.
    fn sample_mouse(&mut self, port: &mut Option<Box<dyn PortDevice>>) {
        let Some(dev) = port else {
            return;
        };
        let now = Cycle(self.clock);
        dev.write_lines(PortLines::default(), now);
        let low = dev.read_lines(now).pulled_low;
        use devices_input::joystick::pin;
        let on = |p: u32| low & p != 0;
        let (x1, x2, y1, y2) = (on(pin::DOWN), on(pin::RIGHT), on(pin::UP), on(pin::LEFT));
        let button = on(pin::FIRE);
        if let Some(adb) = &mut self.adb {
            let phase = |pulse: bool, quad: bool| match (pulse, quad) {
                (false, false) => 0u8,
                (true, false) => 1,
                (true, true) => 2,
                (false, true) => 3,
            };
            let new = [phase(x1, x2), phase(y1, y2)];
            let mut d = [0i32; 2];
            for k in 0..2 {
                d[k] = match new[k].wrapping_sub(self.mouse_phase[k]) & 3 {
                    1 => 1,
                    3 => -1,
                    _ => 0,
                };
            }
            self.mouse_phase = new;
            adb.mouse_move(d[0], d[1], button);
        } else {
            // The Mac's Y2 runs the other way round from the device's
            // quadrature line (the ROM counts down where Y2 follows it).
            self.scc.set_dcd(0, x1);
            self.scc.set_dcd(1, y1);
            self.mouse_x2 = x2;
            self.mouse_y2 = !y2;
            self.mouse_button = button;
        }
    }

    /// The next clock something happens on the board.
    fn next_event(&self) -> u64 {
        let c = self.clock;
        let e = (c / E_CLOCKS + 1) * E_CLOCKS;
        let (_, h) = self.line_pos();
        let line_event = if h < video::ACTIVE_CLOCKS {
            c - h + video::ACTIVE_CLOCKS
        } else {
            c - h + video::LINE_CLOCKS
        };
        let half = (c / HALF_SECOND + 1) * HALF_SECOND;
        let mut t = e.min(line_event).min(half);
        if let Some(k) = &self.kbd {
            t = t.min(k.next_event().max(c + 1));
        }
        if let Some(a) = &self.adb {
            t = t.min(a.next_event().max(c + 1));
        }
        t.min(self.scc.next_event().max(c + 1))
    }

    /// Run everything up to clock `target`.
    fn run_to(&mut self, target: u64, port: &mut Option<Box<dyn PortDevice>>) {
        while self.clock < target {
            let next = self.next_event().min(target);
            self.clock = next;
            let c = self.clock;
            if c.is_multiple_of(E_CLOCKS) {
                self.via.tick();
                self.sound();
            }
            let (line, h) = self.line_pos();
            if h == 0 {
                if line == u64::from(video::HEIGHT) {
                    let pa = self.pa_pins();
                    self.via.set_ca1(false, pa);
                } else if line == 0 {
                    let pa = self.pa_pins();
                    self.via.set_ca1(true, pa);
                    self.frame += 1;
                    self.frame_done = true;
                }
                self.sample_mouse(port);
            } else if h == video::ACTIVE_CLOCKS {
                self.end_of_line(line);
            }
            if c.is_multiple_of(HALF_SECOND) {
                let high = (c / HALF_SECOND).is_multiple_of(2);
                if high {
                    self.rtc.tick_second();
                }
                self.via.set_ca2(high);
            }
            if let Some(k) = &mut self.kbd {
                k.run(c, &mut self.via);
            }
            if let Some(a) = &mut self.adb {
                a.run(c, &mut self.via);
            }
            self.scc.run_to(c);
        }
    }

    /// The shown part of a line has ended: draw it, read the sound word.
    fn end_of_line(&mut self, line: u64) {
        let top = self.ram_size();
        let pa = self.via.port_a_out();
        if line < u64::from(video::HEIGHT) {
            let base = top
                - if pa & 0x40 != 0 {
                    video::MAIN_SCREEN
                } else {
                    video::ALT_SCREEN
                };
            let start = (base + line as u32 * 64) as usize & (self.ram.len() - 1);
            let mut row = [0u8; 64];
            for (i, b) in row.iter_mut().enumerate() {
                *b = self.ram[(start + i) & (self.ram.len() - 1)];
            }
            self.screen.draw_line(line as u32, &row);
        }
        let sound = top
            - if pa & 0x08 != 0 {
                video::MAIN_SOUND
            } else {
                video::ALT_SOUND
            };
        self.sample = self.ram_byte(sound + 2 * line as u32);
        // The low byte drives the 400K drives' motors, which follow its
        // average (the ROM spreads a speed over a pattern of lines).
        if !self.model.double_sided() {
            let byte = self.ram_byte(sound + 2 * line as u32 + 1) & 0x3f;
            let v = u32::from(PWM_WIDTH[usize::from(byte)]) << 8;
            self.pwm_avg = self.pwm_avg - (self.pwm_avg >> 5) + (v >> 5);
            let pwm = self.pwm_avg.min(63 << 8) as u16;
            if self.iwm.drives.iter().any(|d| d.pwm != Some(pwm)) {
                // Bits under the head so far still at the old speed.
                self.iwm.run_to(self.clock);
                for d in &mut self.iwm.drives {
                    d.pwm = Some(pwm);
                }
            }
        }
    }

    /// One E clock of sound.
    fn sound(&mut self) {
        let on = self.via.port_b_out() & 0x80 == 0;
        let vol = i32::from(self.via.port_a_out() & 7);
        let level = if on {
            (i32::from(self.sample) - 128) * vol * 24
        } else {
            0
        };
        if let Some(s) = self.audio.push(level) {
            // First-order high-pass (~20 Hz), like the speaker's coupling.
            let x = i32::from(s) << 16;
            let [px, py] = self.dc;
            let y = x - px + ((i64::from(py) * 65_264) >> 16) as i32;
            self.dc = [x, y];
            self.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
    }
}

/// The pulse width a sound buffer low byte gives the 400K drive's motor
/// (0 slowest … 62): 62 less the number of steps a 6-bit shift register,
/// shifting right with the XOR of bits 0 and 1 fed into bit 5, takes from
/// $0B to the byte (values it never reaches count as 0). This is how the
/// 128K ROM makes the byte for a speed: it steps the register from $0B
/// once per ten units below its fastest speed and mixes the two states
/// around the speed over ten lines, so the motor's speed follows the step
/// count; read from the ROM's behaviour, not from a hardware document.
const PWM_WIDTH: [u8; 64] = pwm_widths();

const fn pwm_widths() -> [u8; 64] {
    let mut t = [0u8; 64];
    let mut x = 0x0bu8;
    let mut i = 0;
    while i < 63 {
        if t[x as usize] == 0 {
            t[x as usize] = 62 - i;
        }
        let fb = (x ^ (x >> 1)) & 1;
        x = (x >> 1) | fb << 5;
        i += 1;
    }
    t
}

/// The byte addresses of an access, most significant first.
fn bytes(addr: u32, size: Size) -> impl Iterator<Item = u32> {
    (0..size.bytes()).map(move |i| addr.wrapping_add(i) & 0xff_ffff)
}

/// Clocks a VPA (E-clock) cycle starting at `at` waits: it ends on an E
/// edge at least one E period after it began.
fn e_clock_waits(at: u64) -> u32 {
    let end = (at + E_CLOCKS).div_ceil(E_CLOCKS) * E_CLOCKS;
    (end - at - 4) as u32
}

/// The 68000's bus for one instruction.
struct MacBus<'a> {
    b: &'a mut Board,
    port: &'a mut Option<Box<dyn PortDevice>>,
    start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for MacBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let mut v = self.b.read(addr, size);
        if let Some(dbg) = self.dbg.as_deref_mut() {
            let addr = addr & 0xff_ffff;
            if dbg.read_armed(CPU_SPACE) {
                v = dbg.on_read(CPU_SPACE, addr, size, v);
                dbg.trace_read(CPU_SPACE, addr, size, v, self.beam);
            }
        }
        v
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.b.peek(addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            let addr = addr & 0xff_ffff;
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v) {
                    return;
                }
            }
        }
        self.b.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

impl TimedBus68 for MacBus<'_> {
    fn bus_cycle(&mut self, access: Access, _fc: u8, addr: u32, _size: Size, t: u32) -> u32 {
        let at = self.start + u64::from(t);
        self.b.run_to(at, self.port);
        let addr = addr & 0xff_ffff;
        if access == Access::IntAck {
            return e_clock_waits(at);
        }
        let ram = match addr >> 20 {
            0x0..=0x3 => !self.b.overlay(),
            0x6 | 0x7 => true,
            _ => false,
        };
        if ram {
            let (line, h) = self.b.line_pos();
            let w = video::ram_waits(line, h, self.b.model == Model::Se);
            if w > 0 {
                self.b.run_to(at + u64::from(w), self.port);
            }
            return w;
        }
        if addr >> 20 == 0xe {
            let w = e_clock_waits(at);
            self.b.run_to(at + u64::from(w), self.port);
            return w;
        }
        0
    }

    fn reset_devices(&mut self) {
        self.b.reset_devices();
    }
}

/// Reads through the board without timing (reset vectors).
struct PlainBus<'a>(&'a mut Board);

impl Bus for PlainBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.0.read(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.0.peek(addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        self.0.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

impl TimedBus68 for PlainBus<'_> {
    fn bus_cycle(&mut self, _: Access, _: u8, _: u32, _: Size, _: u32) -> u32 {
        0
    }

    fn reset_devices(&mut self) {
        self.0.reset_devices();
    }
}

/// Peeks for the disassembler.
struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.0.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.0.peek(addr, size)
    }

    fn write(&mut self, _: u32, _: Size, _: u32) {}

    fn addr_bits(&self) -> u8 {
        24
    }
}

pub struct Mac {
    model: Model,
    cpu: M68000,
    board: Board,
    port: Option<Box<dyn PortDevice>>,
    input: InputQueue,
    jam_reported: bool,
    palette: [Rgb8; 2],
}

impl Mac {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("plus") => Model::Plus,
            Some("128k") => Model::Mac128k,
            Some("512k") => Model::Mac512k,
            Some("se") => Model::Se,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let opt = |k: &str| config.options.get(k).map(String::as_str);
        let stub = match opt("roms") {
            None => false,
            Some("stub") => true,
            Some(other) => return Err(invalid("roms", other, "stub")),
        };
        let ram_size: u32 = match model {
            Model::Mac128k => 0x2_0000,
            Model::Mac512k => 0x8_0000,
            Model::Plus | Model::Se => match opt("ram").unwrap_or("1m") {
                "512k" => 0x8_0000,
                "1m" => 0x10_0000,
                "2m" => 0x20_0000,
                "4m" => 0x40_0000,
                other => return Err(invalid("ram", other, "512k, 1m, 2m or 4m")),
            },
        };
        let loopback = match opt("serial") {
            None | Some("none") => false,
            Some("loopback") => true,
            Some(other) => return Err(invalid("serial", other, "none or loopback")),
        };
        let rom = if stub {
            stub::rom(model.rom_size(), ram_size)
        } else {
            load_rom(model, config)?
        };
        let ds = model.double_sided();
        let drives: &[bool] = if model == Model::Se {
            &[true, true, true]
        } else {
            &[ds, ds]
        };
        let mut scc = Scc::new();
        for c in &mut scc.ch {
            c.loopback = loopback;
        }
        let audio = Decimator::new(CPU_HZ / E_CLOCKS as u32, emu_core::SAMPLE_RATE)
            .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?;
        let mut m = Self {
            model,
            cpu: M68000::new(),
            board: Board {
                model,
                ram: vec![0; ram_size as usize],
                rom,
                se_overlay: true,
                via: Via::new(),
                rtc: Rtc::new(model.scsi()),
                kbd: (model != Model::Se).then(|| Keyboard::new(model == Model::Plus)),
                adb: (model == Model::Se).then(Adb::new),
                scc,
                iwm: Iwm::new(drives),
                scsi: model.scsi().then(Scsi::new),
                screen: Screen::new(),
                clock: 0,
                frame: 0,
                frame_done: false,
                mouse_x2: false,
                mouse_y2: false,
                mouse_button: false,
                mouse_phase: [0; 2],
                nmi: false,
                sample: 0x80,
                pwm_avg: 0,
                audio,
                dc: [0; 2],
                samples: Vec::new(),
            },
            // One quadrature step per 1000 clocks at most (128 µs), so
            // sampling once a line (45 µs) sees every phase.
            port: Some(Box::new(devices_input::AmigaMouse::mac(1000))),
            input: InputQueue::new(),
            jam_reported: false,
            palette: [
                Rgb8 {
                    r: 255,
                    g: 255,
                    b: 255,
                },
                Rgb8 { r: 0, g: 0, b: 0 },
            ],
        };
        m.power_on();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn power_on(&mut self) {
        let b = &mut self.board;
        b.ram.fill(0);
        b.scc.reset();
        b.rtc = Rtc::new(self.model.scsi());
        if let Some(k) = &mut b.kbd {
            *k = Keyboard::new(self.model == Model::Plus);
        }
        if let Some(a) = &mut b.adb {
            *a = Adb::new();
        }
        self.reset_cpu();
    }

    fn reset_cpu(&mut self) {
        self.board.reset_devices();
        self.board.scc.reset();
        self.cpu.reset_timed(&mut PlainBus(&mut self.board));
    }

    fn beam_pos(&self) -> BeamPos {
        let (line, h) = self.board.line_pos();
        BeamPos {
            frame: self.board.frame,
            line: line as u16,
            cycle: h as u16,
            hpos: (2 * h).min(u64::from(u16::MAX)) as u16,
        }
    }

    /// One instruction. Returns `Some` if emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(Cycle(self.board.clock)) {
            self.apply_input(ev);
        }
        let start = self.board.clock;
        let beam = self.beam_pos();
        let step = {
            let mut bus = MacBus {
                b: &mut self.board,
                port: &mut self.port,
                start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let clocks = match step {
            Step::Done { clocks } | Step::Stopped { clocks } => clocks,
            Step::Halted => 4,
        };
        self.board.run_to(start + u64::from(clocks), &mut self.port);
        self.cpu.set_ipl(self.board.ipl());
        if step == Step::Halted {
            if !self.jam_reported {
                self.jam_reported = true;
                return Some(StopReason::CpuJam {
                    device: CPU,
                    pc: self.cpu.pc(),
                });
            }
        } else {
            self.jam_reported = false;
        }
        let entered = self.cpu.take_interrupt();
        let frame = std::mem::take(&mut self.board.frame_done);
        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            let next_pc = self.cpu.pc();
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, self.beam_pos());
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
            if dbg.raster_armed() {
                let b = self.beam_pos();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.board.frame_done |= frame;
                return Some(stop);
            }
        }
        frame.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                let k = machine_key.0 as u8;
                if let Some(kbd) = &mut self.board.kbd {
                    kbd.key(k, down);
                }
                if let Some(adb) = &mut self.board.adb {
                    adb.key(k, down);
                }
            }
            (InputTarget::Port(p), event) if p == MOUSE_PORT => {
                // The 128K/512K/Plus ROM moves the cursor a pixel for every
                // second quadrature phase (an X1/Y1 edge): two phases per
                // host pixel. ADB counts every phase.
                let event = match event {
                    InputEvent::MouseRel { dx, dy, buttons } if self.model != Model::Se => {
                        InputEvent::MouseRel {
                            dx: dx.saturating_mul(2),
                            dy: dy.saturating_mul(2),
                            buttons,
                        }
                    }
                    e => e,
                };
                if let Some(dev) = &mut self.port {
                    dev.input(&event);
                }
            }
            (InputTarget::Panel, InputEvent::Control { id, value }) => match id.0 {
                0 if value == 1 => Machine::reset(self, ResetKind::Hard),
                1 if value == 1 => Machine::reset(self, ResetKind::Soft),
                2 => self.board.nmi = value != 0,
                _ => {}
            },
            _ => {}
        }
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            match f.space {
                CPU_SPACE => self.board.write(f.addr, f.size, f.value),
                RAM_SPACE => {
                    let n = f.size.bytes();
                    for i in 0..n {
                        let a = f.addr.wrapping_add(i) as usize & (self.board.ram.len() - 1);
                        self.board.ram[a] = (f.value >> (8 * (n - 1 - i))) as u8;
                    }
                }
                _ => {}
            }
        }
    }

    /// RAM as stored (tests and tools).
    pub fn ram(&self) -> &[u8] {
        &self.board.ram
    }

    pub fn cpu(&self) -> &M68000 {
        &self.cpu
    }

    /// The main screen buffer's address.
    pub fn screen_base(&self) -> u32 {
        self.board.ram_size() - video::MAIN_SCREEN
    }

    pub fn scc(&mut self) -> &mut Scc {
        &mut self.board.scc
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: video::PIXEL_HZ,
            active_pixels: video::WIDTH,
            total_pixels: 2 * video::LINE_CLOCKS as u32,
            lines_per_field: video::LINES as u32,
            standard: VideoStandard::Ntsc,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        }
    }
}

impl Machine for Mac {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.board.frame_done) {
            return StopReason::FrameDone;
        }
        let b = self.beam_pos();
        if b.line == 0 && b.cycle < 64 {
            self.apply_frame_freezes(dbg);
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        match self.instruction(None) {
            Some(stop @ StopReason::CpuJam { .. }) => stop,
            _ => StopReason::StepDone,
        }
    }

    /// Hard: power cycle. Soft: the programmer's reset switch.
    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            self.power_on();
        } else {
            self.reset_cpu();
        }
        self.input.clear();
        self.board.frame_done = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: video::WIDTH,
            height: video::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.board.screen.pixels,
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let bits = |n: usize| (usize::BITS - (n - 1).leading_zeros()) as u8;
        vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 24,
                big_endian: true,
            },
            MemSpace {
                id: RAM_SPACE,
                name: "ram",
                device: CPU,
                addr_bits: bits(self.board.ram.len()),
                big_endian: true,
            },
            MemSpace {
                id: ROM_SPACE,
                name: "rom",
                device: CPU,
                addr_bits: bits(self.board.rom.len()),
                big_endian: true,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let b = &self.board;
        const VIA: [&str; 16] = [
            "ORB", "ORA", "DDRB", "DDRA", "T1CL", "T1CH", "T1LL", "T1LH", "T2CL", "T2CH", "SR",
            "ACR", "PCR", "IFR", "IER", "ORA_NH",
        ];
        let (line, h) = b.line_pos();
        let mut chips = vec![
            ChipInfo {
                name: "via",
                regs: VIA
                    .iter()
                    .enumerate()
                    .map(|(i, r)| ChipReg {
                        name: r,
                        addr: Some(0xefe1fe + 0x200 * i as u32),
                        value: u32::from(b.via.peek(i as u8, b.pa_pins(), b.pb_pins())),
                        bits: 8,
                    })
                    .collect(),
            },
            ChipInfo {
                name: "video",
                regs: vec![
                    reg("LINE", line as u32, 9),
                    reg("CLOCK", h as u32, 9),
                    reg("OVERLAY", u32::from(b.overlay()), 1),
                    reg("PAGE2", u32::from(b.via.port_a_out() & 0x40 == 0), 1),
                ],
            },
            ChipInfo {
                name: "iwm",
                regs: vec![
                    reg("LINES", u32::from(b.iwm.lines), 8),
                    reg("MODE", u32::from(b.iwm.mode), 5),
                    reg("DATA", u32::from(b.iwm.data), 8),
                    reg("SEL", u32::from(b.iwm.sel), 1),
                    reg("TRACK", u32::from(b.iwm.drives[0].track), 7),
                ],
            },
            ChipInfo {
                name: "scc",
                regs: vec![
                    reg("RR3", u32::from(b.scc.pending()), 8),
                    reg("WR9", u32::from(b.scc.ch[0].wr[9]), 8),
                    reg("A_WR1", u32::from(b.scc.ch[0].wr[1]), 8),
                    reg("B_WR1", u32::from(b.scc.ch[1].wr[1]), 8),
                    reg("A_WR15", u32::from(b.scc.ch[0].wr[15]), 8),
                    reg("B_WR15", u32::from(b.scc.ch[1].wr[15]), 8),
                ],
            },
            ChipInfo {
                name: "rtc",
                regs: vec![
                    reg("SECONDS", b.rtc.seconds, 32),
                    reg("WP", u32::from(b.rtc.write_protect), 1),
                ],
            },
        ];
        if let Some(s) = &b.scsi {
            chips.push(ChipInfo {
                name: "scsi",
                regs: (0..8)
                    .map(|r| ChipReg {
                        name: ["CSD", "ICR", "MR", "TCR", "CSR", "BSR", "IDR", "RPI"][r],
                        addr: Some(0x58_0000 + 0x10 * r as u32),
                        value: u32::from(s.peek(r as u8)),
                        bits: 8,
                    })
                    .collect(),
            });
        }
        chips
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        Cycle(self.board.clock)
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
            emu_core::ControlInfo::button(2, "INTERRUPT", self.board.nmi),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        vec![PortInfo {
            id: MOUSE_PORT,
            kind: PortKind::MacMouse,
            name: if self.model == Model::Se {
                "ADB mouse"
            } else {
                "mouse"
            },
            connected: self.port.is_some(),
        }]
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        if port != MOUSE_PORT {
            return Err(PortError::NoSuchPort(port));
        }
        if dev.kind() != PortKind::MacMouse {
            return Err(PortError::Incompatible {
                port: PortKind::MacMouse,
                device: dev.kind(),
            });
        }
        if self.port.is_some() {
            return Err(PortError::Occupied(port));
        }
        self.port = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        if port == MOUSE_PORT {
            self.port.take()
        } else {
            None
        }
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let b = &self.board;
        let mut v = vec![
            MediaSlotInfo {
                id: SLOT_INTERNAL,
                name: "internal",
                kind: MediaKind::Disk,
                inserted: b.iwm.drives[0].has_disk(),
            },
            MediaSlotInfo {
                id: SLOT_EXTERNAL,
                name: "external",
                kind: MediaKind::Disk,
                inserted: b.iwm.drives[1].has_disk(),
            },
        ];
        if b.iwm.drives.len() > 2 {
            v.insert(
                1,
                MediaSlotInfo {
                    id: SLOT_INTERNAL2,
                    name: "internal 2",
                    kind: MediaKind::Disk,
                    inserted: b.iwm.drives[2].has_disk(),
                },
            );
        }
        if let Some(s) = &b.scsi {
            const NAMES: [&str; scsi::TARGETS] = [
                "SCSI 0", "SCSI 1", "SCSI 2", "SCSI 3", "SCSI 4", "SCSI 5", "SCSI 6",
            ];
            for (i, d) in s.disks.iter().enumerate() {
                v.push(MediaSlotInfo {
                    id: SlotId(SLOT_SCSI.0 + i as u8),
                    name: NAMES[i],
                    kind: MediaKind::Disk,
                    inserted: d.has_disk(),
                });
            }
        }
        v
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        let b = &mut self.board;
        match slot {
            SLOT_INTERNAL => Some(&mut b.iwm.drives[0] as &mut dyn MediaSlot),
            SLOT_EXTERNAL => Some(&mut b.iwm.drives[1] as &mut dyn MediaSlot),
            SLOT_INTERNAL2 => b.iwm.drives.get_mut(2).map(|d| d as &mut dyn MediaSlot),
            SlotId(n) if n >= SLOT_SCSI.0 => {
                let s = b.scsi.as_mut()?;
                s.disks
                    .get_mut(usize::from(n - SLOT_SCSI.0))
                    .map(|d| d as &mut dyn MediaSlot)
            }
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"MAC ", 1, |w| {
            w.u8(self.model.code());
            w.u32(b.ram.len() as u32);
            w.raw(&b.ram);
            w.bool(b.se_overlay);
            w.u64(b.clock);
            w.u64(b.frame);
            w.bool(b.frame_done);
            w.bool(b.mouse_x2);
            w.bool(b.mouse_y2);
            w.bool(b.mouse_button);
            w.u8(b.mouse_phase[0]);
            w.u8(b.mouse_phase[1]);
            w.bool(b.nmi);
            w.u8(b.sample);
            w.u32(b.pwm_avg);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
            w.raw(&b.screen.pixels);
        });
        self.cpu.save(&mut w);
        b.via.save(&mut w);
        b.rtc.save(&mut w);
        if let Some(k) = &b.kbd {
            k.save(&mut w);
        }
        if let Some(a) = &b.adb {
            a.save(&mut w);
        }
        b.scc.save(&mut w);
        b.iwm.save(&mut w);
        if let Some(s) = &b.scsi {
            s.save(&mut w);
        }
        self.input.save(&mut w);
        w.chunk(*b"MACM", 1, |w| {
            w.bytes(
                &self
                    .port
                    .as_ref()
                    .map(|d| d.save_state())
                    .unwrap_or_default(),
            );
        });
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first (the drives' and SCSI
    /// disks' media stay).
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"MAC ", 1)?;
        if c.u8()? != self.model.code() {
            return Err(StateError::Invalid("mac model differs"));
        }
        if c.u32()? as usize != self.board.ram.len() {
            return Err(StateError::Invalid("mac RAM size differs"));
        }
        let ram = c.raw(self.board.ram.len())?.to_vec();
        let se_overlay = c.bool()?;
        let clock = c.u64()?;
        let frame = c.u64()?;
        let frame_done = c.bool()?;
        let mouse_x2 = c.bool()?;
        let mouse_y2 = c.bool()?;
        let mouse_button = c.bool()?;
        let mouse_phase = [c.u8()? & 3, c.u8()? & 3];
        let nmi = c.bool()?;
        let sample = c.u8()?;
        let pwm_avg = c.u32()?.min(63 << 8);
        let mut audio = self.board.audio;
        audio.load(&mut c)?;
        let dc = [c.u32()? as i32, c.u32()? as i32];
        let pixels = c.raw(self.board.screen.pixels.len())?.to_vec();
        let mut cpu = self.cpu.clone();
        cpu.load(&mut r)?;
        let mut via = self.board.via.clone();
        via.load(&mut r)?;
        let mut rtc = self.board.rtc.clone();
        rtc.load(&mut r)?;
        let mut kbd = self.board.kbd.clone();
        if let Some(k) = &mut kbd {
            k.load(&mut r)?;
        }
        let mut adb = self.board.adb.clone();
        if let Some(a) = &mut adb {
            a.load(&mut r)?;
        }
        let mut scc = self.board.scc.clone();
        scc.load(&mut r)?;
        // The IWM and the SCSI bus load in place (their media stay);
        // check them on scratch copies first.
        let iwm_at = r.clone();
        let ds: Vec<bool> = self
            .board
            .iwm
            .drives
            .iter()
            .map(|d| d.double_sided)
            .collect();
        Iwm::new(&ds).load(&mut r)?;
        let scsi_at = r.clone();
        if self.board.scsi.is_some() {
            Scsi::new().load(&mut r)?;
        }
        let mut input = self.input.clone();
        input.load(&mut r)?;
        let (_, mut pc) = r.chunk(*b"MACM", 1)?;
        let port_state = pc.bytes()?.to_vec();
        if self.port.is_none() != port_state.is_empty() {
            return Err(StateError::Invalid("mouse port device differs"));
        }
        if let Some(d) = &mut self.port {
            d.load_state(&port_state)?;
        }
        self.board.iwm.load(&mut iwm_at.clone())?;
        if let Some(s) = &mut self.board.scsi {
            s.load(&mut scsi_at.clone())?;
        }
        let b = &mut self.board;
        b.ram = ram;
        b.se_overlay = se_overlay;
        b.clock = clock;
        b.frame = frame;
        b.frame_done = frame_done;
        b.mouse_x2 = mouse_x2;
        b.mouse_y2 = mouse_y2;
        b.mouse_button = mouse_button;
        b.mouse_phase = mouse_phase;
        b.nmi = nmi;
        b.sample = sample;
        b.pwm_avg = pwm_avg;
        b.audio = audio;
        b.dc = dc;
        b.screen.pixels = pixels;
        b.samples.clear();
        b.via = via;
        b.rtc = rtc;
        b.kbd = kbd;
        b.adb = adb;
        b.scc = scc;
        self.cpu = cpu;
        self.input = input;
        Ok(())
    }
}

impl DebugTarget for Mac {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    /// Big-endian like the 68000.
    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let b = &self.board;
        let byte = |a: u32| -> Option<u8> {
            match space {
                CPU_SPACE => Some(b.peek_byte(a)),
                RAM_SPACE => b.ram.get(a as usize).copied(),
                ROM_SPACE => b.rom.get(a as usize).copied(),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0u32, |v, i| {
            Some(v << 8 | u32::from(byte(addr.wrapping_add(i))?))
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        let n = size.bytes();
        for i in 0..n {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * (n - 1 - i))) as u8;
            match space {
                CPU_SPACE => {
                    let a = a & 0xff_ffff;
                    let ram = match a >> 20 {
                        0x0..=0x3 => !self.board.overlay(),
                        0x6 | 0x7 => true,
                        _ => false,
                    };
                    if !ram {
                        return false;
                    }
                    let len = self.board.ram.len();
                    self.board.ram[a as usize & (len - 1)] = byte;
                }
                RAM_SPACE => match self.board.ram.get_mut(a as usize) {
                    Some(r) => *r = byte,
                    None => return false,
                },
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.board), addr)
    }
}
