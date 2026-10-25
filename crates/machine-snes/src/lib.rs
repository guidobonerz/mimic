//! Super Nintendo Entertainment System / Super Famicom (NTSC and PAL).
//!
//! The 5A22 (a 65C816, `cpu-65816`, with DMA/HDMA, the multiplier and
//! divider, timers and the joypad interface), the S-PPU (`chips-nintendo`),
//! 128 KB work RAM, the sound module (S-SMP + S-DSP, [`apu`]), two
//! controller ports, the cartridge (`devices_cartridge::snes`).
//!
//! **Clocking.** Master clock 21.477 MHz (PAL 21.281 MHz); a line is 1364
//! master clocks (341 dots of 4), a frame 262 lines (PAL 312; one more in
//! the first interlace field; a non-interlaced NTSC odd frame has line 240
//! 4 clocks short). CPU bus cycles take 6, 8 or 12 master clocks by
//! address: internal cycles, I/O at $2000–$3FFF and $4200–$5FFF 6; WRAM,
//! expansion and slow ROM 8; the joypad ports $4000–$41FF 12; banks
//! $80–$FF at $8000+ (and $C0–$FF) are 6 when MEMSEL bit 0 is set. The CPU
//! stops 40 clocks a line for DRAM refresh (at H = 134 dots).
//!
//! **Line events** (dots): 0.5 the NMI flag at the first vertical-blank
//! line (225, or 240 with overscan); 1 hblank ends; 2.5 a V-IRQ; 6 HDMA
//! is set up on line 0; 10 OAM address reload at vblank; 22 the line is
//! drawn (after its HDMA, with the registers as they are then); 32.5
//! automatic joypad read at vblank (busy for 4224 clocks); HTIME + 3.5 an
//! H-IRQ; 274 hblank begins; 278 HDMA for lines 0 to the last picture
//! line.
//!
//! **CPU registers** ($4200–$421F): NMITIMEN, WRIO (bit 7 falling latches
//! the PPU counters), WRMPYA/B (8 × 8 → RDMPY; RDDIV = B), WRDIV/WRDIVB
//! (16 ÷ 8; ÷ 0 gives $FFFF and the dividend), HTIME, VTIME, MDMAEN,
//! HDMAEN, MEMSEL; RDNMI (bit 7 cleared by the read), TIMEUP (likewise),
//! HVBJOY, RDIO, RDDIV, RDMPY, JOY1–4. Results are there at once (the
//! hardware needs 8/16 cycles). The DMA registers at $43x0–$43xF.
//!
//! **Controllers** (`devices_input::snes`): $4016 bit 0 drives the latch
//! of both ports; reads of $4016/$4017 return D0/D1 in bits 0/1 and pulse
//! CLK ($4017 bits 2–4 read 1). With `multitap=on` port 2 holds a
//! Multiplayer 5: pads 2–5 (ports 2–5), WRIO bit 7 selecting pads 2/3
//! (high) or 4/5 on D0/D1. A Super Scope in port 2 latches the counters at
//! dot X + 40 of line Y + 1 when it points at a bright pixel.
//!
//! Models "ntsc" (default) and "pal". Options: `port1`, `port2` = pad
//! (default), mouse, superscope (port 2), none; `multitap=on`; `roms=stub`
//! uses this project's own sound boot loader even when the IPL is there.
//! ROM role "spc700" (64 bytes, user-supplied, optional).
//!
//! Not emulated: the multiplier/divider delay, the PPU's open bus details,
//! mid-line PPU register changes, cartridge coprocessors.
//!
//! Sources: Martin Korth, fullsnes, "SNES I/O Map", "SNES Memory Map",
//! "SNES DMA …", "SNES PPU Interrupts", "SNES Timing …", "SNES Maths
//! Multiply/Divide", "SNES Controllers I/O Ports" (read 2026-10-23). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

mod overlay;

pub mod apu;

use apu::{Apu, ARAM_SPACE, SMP_HZ};
use chips_nintendo::sppu::{self, SPpu};
use cpu_65816::{Bus816, Cpu65816};
use devices_cartridge::snes::SnesCart;
use devices_cartridge::CartSlot;
use devices_input::snes::{pin, NO_AIM};
use devices_input::{SnesController, SnesKind};
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use emu_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::overlay::{Overlay, OverlayInfo};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, ChipInfo, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine, MemSpace,
    MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError, StateReader,
    StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
pub const SPC: DeviceId = DeviceId("spc700");
/// The 65816's 24-bit view.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const CGRAM_SPACE: MemSpaceId = MemSpaceId(2);
pub const OAM_SPACE: MemSpaceId = MemSpaceId(3);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);

pub const MASTER_HZ_NTSC: u64 = 21_477_270;
pub const MASTER_HZ_PAL: u64 = 21_281_370;
const LINE: u32 = 1364;

/// Machine keys: controller 1's buttons B A Select Start Y X L R = 0–7,
/// controller 2's = 8–15.
pub mod keys {
    pub const PAD1: u16 = 0;
    pub const PAD2: u16 = 8;
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "snes",
        name: "Super Nintendo Entertainment System",
        models: &["ntsc", "pal"],
        create: |config| Ok(Box::new(Snes::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Ntsc,
    Pal,
}

impl Model {
    fn master_hz(self) -> u64 {
        match self {
            Model::Ntsc => MASTER_HZ_NTSC,
            Model::Pal => MASTER_HZ_PAL,
        }
    }

    fn lines(self) -> u16 {
        match self {
            Model::Ntsc => 262,
            Model::Pal => 312,
        }
    }
}

fn port_option(config: &MachineConfig, key: &str) -> Result<Option<SnesKind>, ConfigError> {
    Ok(match config.options.get(key).map(String::as_str) {
        None | Some("pad") => Some(SnesKind::Pad),
        Some("mouse") => Some(SnesKind::Mouse),
        Some("superscope") => Some(SnesKind::SuperScope),
        Some("none") => None,
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "{key} option '{other}' (pad, mouse, superscope or none)"
            )))
        }
    })
}

/// One DMA/HDMA channel ($43x0–$43xB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dma {
    dmap: u8,
    bbad: u8,
    a1t: u16,
    a1b: u8,
    das: u16,
    dasb: u8,
    a2a: u16,
    ntrl: u8,
    unused: u8,
    do_transfer: bool,
    terminated: bool,
}

impl Default for Dma {
    fn default() -> Self {
        Self {
            dmap: 0xff,
            bbad: 0xff,
            a1t: 0xffff,
            a1b: 0xff,
            das: 0xffff,
            dasb: 0xff,
            a2a: 0xffff,
            ntrl: 0xff,
            unused: 0xff,
            do_transfer: false,
            terminated: true,
        }
    }
}

/// B-bus address offsets of a transfer unit.
const UNITS: [&[u8]; 8] = [
    &[0],
    &[0, 1],
    &[0, 0],
    &[0, 0, 1, 1],
    &[0, 1, 2, 3],
    &[0, 1, 0, 1],
    &[0, 0],
    &[0, 0, 1, 1],
];

/// The controller ports (2–5 behind a Multiplayer 5).
struct Ports {
    devices: [Option<Box<dyn PortDevice>>; 5],
    multitap: bool,
}

/// Everything on the buses but the main CPU and the port devices.
#[derive(Clone)]
struct Board {
    model: Model,
    wram: Vec<u8>,
    wram_addr: u32,
    ppu: SPpu,
    apu: Apu,
    cart: Option<SnesCart>,
    open_bus: u8,
    nmitimen: u8,
    wrio: u8,
    wrmpya: u8,
    wrdiv: u16,
    htime: u16,
    vtime: u16,
    memsel: u8,
    rddiv: u16,
    rdmpy: u16,
    nmi_flag: bool,
    irq_flag: bool,
    joy: [u16; 4],
    joy_busy_until: u64,
    strobe: bool,
    dma: [Dma; 8],
    hdmaen: u8,
    /// Master clocks: in the current line, total; the line and frame.
    h: u32,
    clock: u64,
    v: u16,
    frames: u64,
    vblank: bool,
    hblank: bool,
    /// The NTSC odd-frame short line toggles with this.
    odd_frame: bool,
    smp_acc: u64,
    field_done: bool,
    /// Resampling the DSP's 32 kHz to 48 kHz: position, last sample.
    resample_pos: u32,
    last_sample: (i16, i16),
    samples: Vec<i16>,
    /// Debug overlay records (not machine state, not in savestates).
    ov: overlay::Overlays,
}

impl Board {
    fn vblank_line(&self) -> u16 {
        self.ppu.visible_lines() + 1
    }

    fn frame_lines(&self) -> u16 {
        self.model.lines() + u16::from(self.ppu.interlace() && !self.ppu.field)
    }

    fn line_len(&self) -> u32 {
        if self.model == Model::Ntsc && !self.ppu.interlace() && self.odd_frame && self.v == 240 {
            LINE - 4
        } else {
            LINE
        }
    }

    /// Master clocks of a CPU bus cycle at `addr`.
    fn speed(&self, addr: u32) -> u32 {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        let fast = self.memsel & 1 != 0;
        if bank & 0x40 == 0 {
            match a {
                0x0000..=0x1fff => 8,
                0x2000..=0x3fff => 6,
                0x4000..=0x41ff => 12,
                0x4200..=0x5fff => 6,
                0x6000..=0x7fff => 8,
                _ => {
                    if bank & 0x80 != 0 && fast {
                        6
                    } else {
                        8
                    }
                }
            }
        } else if bank & 0x80 != 0 && fast {
            6
        } else {
            8
        }
    }

    fn nmi_line(&self) -> bool {
        self.nmi_flag && self.nmitimen & 0x80 != 0
    }

    fn irq_line(&self) -> bool {
        self.irq_flag
    }

    // ---- time

    /// The next event position after `h` in this line.
    fn next_event(&self, h: u32) -> u32 {
        let hirq = u32::from(self.htime) * 4 + 14;
        let points = [2, 4, 10, 24, 40, 88, 130, 536, 1096, 1104, hirq];
        points
            .into_iter()
            .filter(|&p| p > h)
            .min()
            .unwrap_or(u32::MAX)
            .min(self.line_len())
    }

    /// Let `n` master clocks pass (the CPU's bus cycle, a DMA byte …).
    fn advance(&mut self, mut n: u32, ports: &mut Ports) {
        while n > 0 {
            let p = self.next_event(self.h);
            let step = n.min(p - self.h);
            self.h += step;
            self.clock += u64::from(step);
            n -= step;
            self.smp_acc += u64::from(step) * SMP_HZ;
            let hz = self.model.master_hz();
            if self.smp_acc >= hz * 64 {
                let clocks = self.smp_acc / hz;
                self.smp_acc %= hz;
                self.run_apu(clocks as i64);
            }
            if self.h == p {
                n += self.event(ports);
            }
        }
    }

    fn run_apu(&mut self, clocks: i64) {
        self.apu.run(clocks);
        for (l, r) in std::mem::take(&mut self.apu.smp.samples) {
            // 32 kHz to 48 kHz in units of 1/96000 s: an input sample every
            // 3, an output sample every 2. `resample_pos` is where the next
            // output falls after the previous input sample (0–2); outputs
            // up to this input are interpolated linearly.
            let (pl, pr) = self.last_sample;
            while self.resample_pos < 3 {
                let f = self.resample_pos as i32;
                let lerp =
                    |a: i16, b: i16| ((i32::from(a) * (3 - f) + i32::from(b) * f) / 3) as i16;
                self.samples.push(lerp(pl, l));
                self.samples.push(lerp(pr, r));
                self.resample_pos += 2;
            }
            self.resample_pos -= 3;
            self.last_sample = (l, r);
        }
    }

    /// Catch the sound module up with the master clock.
    fn sync_apu(&mut self) {
        let hz = self.model.master_hz();
        let clocks = self.smp_acc / hz;
        self.smp_acc %= hz;
        self.run_apu(clocks as i64);
    }

    /// Handle the event at the current `h`; returns master clocks the CPU
    /// is held up by it.
    fn event(&mut self, ports: &mut Ports) -> u32 {
        let (h, v) = (self.h, self.v);
        let mut stall = 0;
        if h >= self.line_len() {
            self.new_line();
            return 0;
        }
        let vb = self.vblank_line();
        match h {
            2 if v == vb => {
                self.nmi_flag = true;
                self.field_done = true;
                if self.ov.irq && self.nmitimen & 0x80 != 0 {
                    self.record_event(overlay::Event::Nmi);
                }
            }
            4 => self.hblank = false,
            24 if v == 0 => stall += self.hdma_init(),
            40 if v == vb => self.ppu.start_vblank(),
            88 => {
                if (1..=sppu::LINES as u16).contains(&v) {
                    self.ppu.render_line(v);
                    self.scope_latch(ports);
                }
            }
            130 if v == vb && self.nmitimen & 1 != 0 => self.auto_joypad(ports),
            536 => stall += 40,
            1096 => self.hblank = true,
            1104 if v < vb => stall += self.hdma_line(),
            _ => {}
        }
        // H/V timer interrupts.
        let hirq = u32::from(self.htime) * 4 + 14;
        let was = self.irq_flag;
        match (self.nmitimen >> 4) & 3 {
            1 if h == hirq => self.irq_flag = true,
            2 if h == 10 && v == self.vtime => self.irq_flag = true,
            3 if v == self.vtime
                && (self.htime == 0 && h == 10 || self.htime != 0 && h == hirq) =>
            {
                self.irq_flag = true
            }
            _ => {}
        }
        if self.ov.irq && self.irq_flag && !was {
            self.record_event(overlay::Event::HvIrq);
        }
        stall
    }

    fn overlay_rows(&self) -> (bool, bool) {
        (self.ppu.interlace(), self.ppu.field)
    }

    fn record_event(&mut self, e: overlay::Event) {
        let rows = self.overlay_rows();
        self.ov.record_event(e, self.v, (self.h / 4) as u16, rows);
    }

    fn new_line(&mut self) {
        self.h = 0;
        self.v += 1;
        if self.v >= self.frame_lines() {
            self.v = 0;
            self.frames += 1;
            self.odd_frame = !self.odd_frame;
            self.vblank = false;
            self.nmi_flag = false;
            self.ppu.start_frame();
            self.sync_apu();
        }
        if self.v == self.vblank_line() {
            self.vblank = true;
        }
    }

    // ---- DMA

    fn dma_read_a(&mut self, addr: u32) -> u8 {
        // The A bus cannot reach the B bus or the DMA registers.
        let a = addr as u16;
        if (addr >> 16) as u8 & 0x40 == 0
            && ((0x2100..0x2200).contains(&a) || (0x4300..0x4380).contains(&a))
        {
            return self.open_bus;
        }
        self.mem_read(addr)
    }

    fn dma_write_a(&mut self, addr: u32, v: u8) {
        let a = addr as u16;
        if (addr >> 16) as u8 & 0x40 == 0
            && ((0x2100..0x2200).contains(&a) || (0x4300..0x4380).contains(&a))
        {
            return;
        }
        self.mem_write(addr, v);
    }

    fn transfer(&mut self, ch: usize, a_addr: u32, b: u8) {
        let to_b = self.dma[ch].dmap & 0x80 == 0;
        let wram_b = b == 0x80;
        let a_is_wram = {
            let bank = (a_addr >> 16) as u8;
            bank == 0x7e || bank == 0x7f || bank & 0x40 == 0 && (a_addr as u16) < 0x2000
        };
        if to_b {
            let v = self.dma_read_a(a_addr);
            if !(wram_b && a_is_wram) {
                self.b_write(b, v);
            }
        } else {
            let v = if wram_b && a_is_wram {
                self.open_bus
            } else {
                self.b_read(b)
            };
            self.dma_write_a(a_addr, v);
        }
    }

    /// General-purpose DMA of the channels in `mask`.
    fn gp_dma(&mut self, mask: u8, ports: &mut Ports) {
        self.advance(8, ports);
        for ch in 0..8 {
            if mask & (1 << ch) == 0 {
                continue;
            }
            self.advance(8, ports);
            let unit = UNITS[usize::from(self.dma[ch].dmap & 7)];
            let mut i = 0;
            loop {
                let d = self.dma[ch];
                let a = u32::from(d.a1b) << 16 | u32::from(d.a1t);
                let b = d.bbad.wrapping_add(unit[i % unit.len()]);
                self.transfer(ch, a, b);
                let d = &mut self.dma[ch];
                d.a1t = match (d.dmap >> 3) & 3 {
                    0 => d.a1t.wrapping_add(1),
                    2 => d.a1t.wrapping_sub(1),
                    _ => d.a1t,
                };
                d.das = d.das.wrapping_sub(1);
                i += 1;
                let done = d.das == 0;
                self.advance(8, ports);
                if done {
                    break;
                }
            }
        }
    }

    /// Line 0: every channel starts the frame unfinished and without a
    /// pending transfer; each enabled one loads its first table entry. A
    /// channel enabled later in the frame runs from its A2A and NTRL as
    /// they are (Super Ghouls'n Ghosts enables its HDMA at line 38, where
    /// the playfield begins under the status bar).
    fn hdma_init(&mut self) -> u32 {
        let mut cost = 0;
        for ch in 0..8 {
            let d = &mut self.dma[ch];
            d.terminated = false;
            d.do_transfer = false;
            if self.hdmaen & (1 << ch) == 0 {
                continue;
            }
            d.a2a = d.a1t;
            d.terminated = false;
            cost += 8 + self.hdma_load(ch);
        }
        if cost > 0 {
            cost += 18;
        }
        cost
    }

    /// Read a channel's next line count (and indirect address).
    fn hdma_load(&mut self, ch: usize) -> u32 {
        let d = self.dma[ch];
        let bank = u32::from(d.a1b) << 16;
        let n = self.dma_read_a(bank | u32::from(d.a2a));
        let mut a2a = d.a2a.wrapping_add(1);
        let mut cost = 8;
        let mut das = d.das;
        if d.dmap & 0x40 != 0 {
            let lo = self.dma_read_a(bank | u32::from(a2a));
            let hi = self.dma_read_a(bank | u32::from(a2a.wrapping_add(1)));
            a2a = a2a.wrapping_add(2);
            das = u16::from_le_bytes([lo, hi]);
            cost += 16;
        }
        let d = &mut self.dma[ch];
        d.ntrl = n;
        d.a2a = a2a;
        d.das = das;
        d.terminated = n == 0;
        d.do_transfer = true;
        cost
    }

    /// The HDMA of a line.
    fn hdma_line(&mut self) -> u32 {
        let mut cost = 0;
        for ch in 0..8 {
            if self.hdmaen & (1 << ch) == 0 || self.dma[ch].terminated {
                continue;
            }
            cost += 8;
            if self.dma[ch].do_transfer {
                let unit = UNITS[usize::from(self.dma[ch].dmap & 7)];
                for off in unit {
                    let d = self.dma[ch];
                    let indirect = d.dmap & 0x40 != 0;
                    let a = if indirect {
                        u32::from(d.dasb) << 16 | u32::from(d.das)
                    } else {
                        u32::from(d.a1b) << 16 | u32::from(d.a2a)
                    };
                    let reg = d.bbad.wrapping_add(*off);
                    if self.ov.hdma {
                        let rows = self.overlay_rows();
                        self.ov.record_hdma(self.v + 1, ch, reg, rows);
                    }
                    self.transfer(ch, a, reg);
                    let d = &mut self.dma[ch];
                    if indirect {
                        d.das = d.das.wrapping_add(1);
                    } else {
                        d.a2a = d.a2a.wrapping_add(1);
                    }
                    cost += 8;
                }
            }
            let d = &mut self.dma[ch];
            d.ntrl = d.ntrl.wrapping_sub(1);
            d.do_transfer = d.ntrl & 0x80 != 0;
            if d.ntrl & 0x7f == 0 {
                cost += self.hdma_load(ch);
            }
        }
        if cost > 0 {
            cost += 18;
        }
        cost
    }

    // ---- controllers

    fn port_lines(&self, ports: &Ports, n: usize) -> (bool, bool) {
        let now = Cycle(self.clock);
        let bit = |i: usize| -> bool {
            ports.devices[i]
                .as_ref()
                .is_some_and(|d| d.read_lines(now).pulled_low & pin::D0 != 0)
        };
        if n == 1 && ports.multitap {
            if self.wrio & 0x80 != 0 {
                (bit(1), bit(2))
            } else {
                (bit(3), bit(4))
            }
        } else {
            (bit(n), false)
        }
    }

    fn clock_port(&mut self, ports: &mut Ports, n: usize) {
        let now = Cycle(self.clock);
        let out = if self.strobe { pin::OUT } else { 0 };
        let devices: &[usize] = if n == 1 && ports.multitap {
            if self.wrio & 0x80 != 0 {
                &[1, 2]
            } else {
                &[3, 4]
            }
        } else if n == 0 {
            &[0]
        } else {
            &[1]
        };
        for &i in devices {
            if let Some(d) = ports.devices[i].as_mut() {
                for bits in [out | pin::CLK, out] {
                    d.write_lines(
                        PortLines {
                            pulled_low: bits,
                            analog: [0; 2],
                        },
                        now,
                    );
                }
            }
        }
    }

    fn set_strobe(&mut self, on: bool, ports: &mut Ports) {
        self.strobe = on;
        let now = Cycle(self.clock);
        for d in ports.devices.iter_mut().flatten() {
            d.write_lines(
                PortLines {
                    pulled_low: if on { pin::OUT } else { 0 },
                    analog: [0; 2],
                },
                now,
            );
        }
    }

    fn read_port(&mut self, ports: &mut Ports, n: usize) -> u8 {
        let (d0, d1) = self.port_lines(ports, n);
        self.clock_port(ports, n);
        u8::from(d0) | u8::from(d1) << 1
    }

    fn auto_joypad(&mut self, ports: &mut Ports) {
        self.set_strobe(true, ports);
        self.set_strobe(false, ports);
        self.joy = [0; 4];
        for _ in 0..16 {
            for n in 0..2 {
                let (d0, d1) = self.port_lines(ports, n);
                self.clock_port(ports, n);
                self.joy[n] = self.joy[n] << 1 | u16::from(d0);
                self.joy[n + 2] = self.joy[n + 2] << 1 | u16::from(d1);
            }
        }
        self.joy_busy_until = self.clock + 4224;
    }

    /// A Super Scope in port 2 latches the counters when the line it aims
    /// at is drawn, if the point is bright.
    fn scope_latch(&mut self, ports: &Ports) {
        let Some(d) = ports.devices[1].as_ref() else {
            return;
        };
        let l = d.read_lines(Cycle(self.clock));
        if l.pulled_low & pin::PHOTODIODE == 0 || l.pulled_low & pin::TRIGGER == 0 {
            return;
        }
        let [x, y] = l.analog;
        if x == NO_AIM || y == NO_AIM || self.v != y + 1 || self.wrio & 0x80 == 0 {
            return;
        }
        let row = self.ppu.row(y + 1);
        let px = self.ppu.frame()[row * sppu::WIDTH + usize::from(x) * 2];
        // The gun does not see red: blue and green count.
        if u32::from(px.g) + u32::from(px.b) >= 0x100 {
            self.ppu.latch(x + 40, self.v);
        }
    }

    // ---- memory

    fn mem_peek(&self, addr: u32) -> u8 {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        if bank == 0x7e || bank == 0x7f {
            return self.wram[(addr & 0x1ffff) as usize];
        }
        if bank & 0x40 == 0 && a < 0x8000 {
            return match a {
                0x0000..=0x1fff => self.wram[usize::from(a)],
                0x2100..=0x21ff => self.b_peek(a as u8),
                0x4210..=0x421f => self.cpu_reg_peek(a),
                0x4300..=0x437f => self.dma_reg(a),
                _ => self
                    .cart
                    .as_ref()
                    .and_then(|c| c.peek(addr))
                    .unwrap_or(self.open_bus),
            };
        }
        self.cart
            .as_ref()
            .and_then(|c| c.peek(addr))
            .unwrap_or(self.open_bus)
    }

    /// A memory read without the port side (DMA, CPU).
    fn mem_read(&mut self, addr: u32) -> u8 {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        if bank & 0x40 == 0 && a < 0x8000 {
            match a {
                0x2100..=0x21ff => return self.b_read(a as u8),
                0x4210..=0x421f => return self.cpu_reg_read(a),
                _ => {}
            }
        }
        if let Some(c) = self.cart.as_mut() {
            if !(bank == 0x7e || bank == 0x7f || bank & 0x40 == 0 && a < 0x2000) {
                if let Some(v) = c.read(addr) {
                    return v;
                }
            }
        }
        self.mem_peek(addr)
    }

    fn mem_write(&mut self, addr: u32, v: u8) {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        if bank == 0x7e || bank == 0x7f {
            self.wram[(addr & 0x1ffff) as usize] = v;
            return;
        }
        if bank & 0x40 == 0 && a < 0x8000 {
            match a {
                0x0000..=0x1fff => self.wram[usize::from(a)] = v,
                0x2100..=0x21ff => self.b_write(a as u8, v),
                0x4300..=0x437f => self.dma_reg_write(a, v),
                _ => {
                    if let Some(c) = self.cart.as_mut() {
                        c.write(addr, v);
                    }
                }
            }
            return;
        }
        if let Some(c) = self.cart.as_mut() {
            c.write(addr, v);
        }
    }

    fn b_peek(&self, b: u8) -> u8 {
        match b {
            0x34..=0x3f => self.ppu.peek(b),
            0x40..=0x7f => self.apu.smp.to_cpu[usize::from(b & 3)],
            0x80 => self.wram[self.wram_addr as usize],
            _ => self.open_bus,
        }
    }

    fn b_read(&mut self, b: u8) -> u8 {
        match b {
            0x34..=0x3f => {
                let (h, v) = ((self.h / 4) as u16, self.v);
                self.ppu.read(b, h, v, self.wrio & 0x80 != 0)
            }
            0x40..=0x7f => {
                self.sync_apu();
                self.apu.smp.to_cpu[usize::from(b & 3)]
            }
            0x80 => {
                let v = self.wram[self.wram_addr as usize];
                self.wram_addr = (self.wram_addr + 1) & 0x1ffff;
                v
            }
            _ => self.open_bus,
        }
    }

    fn b_write(&mut self, b: u8, v: u8) {
        match b {
            0x00..=0x33 => self.ppu.write(b, v),
            0x40..=0x7f => {
                self.sync_apu();
                self.apu.smp.to_apu[usize::from(b & 3)] = v;
            }
            0x80 => {
                self.wram[self.wram_addr as usize] = v;
                self.wram_addr = (self.wram_addr + 1) & 0x1ffff;
            }
            0x81 => self.wram_addr = (self.wram_addr & 0x1ff00) | u32::from(v),
            0x82 => self.wram_addr = (self.wram_addr & 0x100ff) | u32::from(v) << 8,
            0x83 => self.wram_addr = (self.wram_addr & 0x0ffff) | u32::from(v & 1) << 16,
            _ => {}
        }
    }

    fn cpu_reg_peek(&self, a: u16) -> u8 {
        let ob = self.open_bus;
        match a {
            0x4210 => u8::from(self.nmi_flag) << 7 | (ob & 0x70) | 2,
            0x4211 => u8::from(self.irq_flag) << 7 | (ob & 0x7f),
            0x4212 => {
                u8::from(self.vblank) << 7
                    | u8::from(self.hblank) << 6
                    | (ob & 0x3e)
                    | u8::from(self.clock < self.joy_busy_until)
            }
            0x4213 => self.wrio,
            0x4214 => self.rddiv as u8,
            0x4215 => (self.rddiv >> 8) as u8,
            0x4216 => self.rdmpy as u8,
            0x4217 => (self.rdmpy >> 8) as u8,
            0x4218..=0x421f => {
                let j = self.joy[usize::from((a - 0x4218) / 2)];
                if a & 1 == 0 {
                    j as u8
                } else {
                    (j >> 8) as u8
                }
            }
            _ => ob,
        }
    }

    fn cpu_reg_read(&mut self, a: u16) -> u8 {
        let v = self.cpu_reg_peek(a);
        match a {
            0x4210 => self.nmi_flag = false,
            0x4211 => self.irq_flag = false,
            _ => {}
        }
        v
    }

    fn dma_reg(&self, a: u16) -> u8 {
        let d = &self.dma[usize::from((a >> 4) & 7)];
        match a & 0x0f {
            0 => d.dmap,
            1 => d.bbad,
            2 => d.a1t as u8,
            3 => (d.a1t >> 8) as u8,
            4 => d.a1b,
            5 => d.das as u8,
            6 => (d.das >> 8) as u8,
            7 => d.dasb,
            8 => d.a2a as u8,
            9 => (d.a2a >> 8) as u8,
            0xa => d.ntrl,
            0xb | 0xf => d.unused,
            _ => self.open_bus,
        }
    }

    fn dma_reg_write(&mut self, a: u16, v: u8) {
        let d = &mut self.dma[usize::from((a >> 4) & 7)];
        match a & 0x0f {
            0 => d.dmap = v,
            1 => d.bbad = v,
            2 => d.a1t = (d.a1t & 0xff00) | u16::from(v),
            3 => d.a1t = (d.a1t & 0x00ff) | u16::from(v) << 8,
            4 => d.a1b = v,
            5 => d.das = (d.das & 0xff00) | u16::from(v),
            6 => d.das = (d.das & 0x00ff) | u16::from(v) << 8,
            7 => d.dasb = v,
            8 => d.a2a = (d.a2a & 0xff00) | u16::from(v),
            9 => d.a2a = (d.a2a & 0x00ff) | u16::from(v) << 8,
            0xa => d.ntrl = v,
            0xb | 0xf => d.unused = v,
            _ => {}
        }
    }

    /// $4016 and $4200–$420D writes; returns a GP-DMA mask to run.
    fn cpu_reg_write(&mut self, a: u16, v: u8, ports: &mut Ports) -> u8 {
        match a {
            0x4016 => self.set_strobe(v & 1 != 0, ports),
            0x4200 => {
                self.nmitimen = v;
                if v & 0x30 == 0 {
                    self.irq_flag = false;
                }
            }
            0x4201 => {
                if self.wrio & 0x80 != 0 && v & 0x80 == 0 {
                    self.ppu.latch((self.h / 4) as u16, self.v);
                }
                self.wrio = v;
            }
            0x4202 => self.wrmpya = v,
            0x4203 => {
                self.rdmpy = u16::from(self.wrmpya) * u16::from(v);
                self.rddiv = u16::from(v);
            }
            0x4204 => self.wrdiv = (self.wrdiv & 0xff00) | u16::from(v),
            0x4205 => self.wrdiv = (self.wrdiv & 0x00ff) | u16::from(v) << 8,
            0x4206 => {
                if v == 0 {
                    self.rddiv = 0xffff;
                    self.rdmpy = self.wrdiv;
                } else {
                    self.rddiv = self.wrdiv / u16::from(v);
                    self.rdmpy = self.wrdiv % u16::from(v);
                }
            }
            0x4207 => self.htime = (self.htime & 0x100) | u16::from(v),
            0x4208 => self.htime = (self.htime & 0xff) | u16::from(v & 1) << 8,
            0x4209 => self.vtime = (self.vtime & 0x100) | u16::from(v),
            0x420a => self.vtime = (self.vtime & 0xff) | u16::from(v & 1) << 8,
            0x420b => return v,
            0x420c => self.hdmaen = v,
            0x420d => self.memsel = v,
            _ => {}
        }
        0
    }
}

/// The 65816's bus.
struct SnesBus<'a> {
    b: &'a mut Board,
    ports: &'a mut Ports,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus816 for SnesBus<'_> {
    fn read(&mut self, addr: u32) -> u8 {
        let speed = self.b.speed(addr);
        self.b.advance(speed, self.ports);
        let a = addr as u16;
        let sys = (addr >> 16) as u8 & 0x40 == 0;
        let v = if sys && (a == 0x4016 || a == 0x4017) {
            let n = usize::from(a & 1);
            let bits = self.b.read_port(self.ports, n);
            let fixed = if n == 1 { 0x1c } else { 0 };
            (self.b.open_bus & 0xe0) | fixed | bits
        } else {
            self.b.mem_read(addr)
        };
        let v = match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let seen = dbg.on_read(CPU_SPACE, addr, Size::Byte, u32::from(v)) as u8;
                dbg.trace_read(CPU_SPACE, addr, Size::Byte, u32::from(seen), self.beam);
                seen
            }
            _ => v,
        };
        self.b.open_bus = v;
        v
    }

    fn write(&mut self, addr: u32, v: u8) {
        let speed = self.b.speed(addr);
        self.b.advance(speed, self.ports);
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, Size::Byte, u32::from(v), self.beam);
                if !dbg.on_write(CPU_SPACE, addr, Size::Byte, u32::from(v)) {
                    return;
                }
            }
        }
        self.b.open_bus = v;
        let a = addr as u16;
        if (addr >> 16) as u8 & 0x40 == 0 && (a == 0x4016 || (0x4200..=0x420d).contains(&a)) {
            let dma = self.b.cpu_reg_write(a, v, self.ports);
            if dma != 0 {
                self.b.gp_dma(dma, self.ports);
            }
        } else {
            self.b.mem_write(addr, v);
        }
    }

    fn idle(&mut self, _addr: u32) {
        self.b.advance(6, self.ports);
    }

    fn peek(&self, addr: u32) -> u8 {
        self.b.mem_peek(addr)
    }

    fn nmi(&mut self) -> bool {
        self.b.nmi_line()
    }

    fn irq(&mut self) -> bool {
        self.b.irq_line()
    }
}

struct PeekBus<'a>(&'a Board);

impl Bus816 for PeekBus<'_> {
    fn read(&mut self, addr: u32) -> u8 {
        self.0.mem_peek(addr)
    }
    fn write(&mut self, _addr: u32, _v: u8) {}
    fn idle(&mut self, _addr: u32) {}
    fn peek(&self, addr: u32) -> u8 {
        self.0.mem_peek(addr)
    }
}

pub struct Snes {
    model: Model,
    cpu: Cpu65816,
    board: Board,
    ports: Ports,
    cart: CartSlot<SnesCart>,
    ipl: Option<Vec<u8>>,
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
}

impl Snes {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("ntsc") => Model::Ntsc,
            Some("pal") => Model::Pal,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let multitap = match config.options.get("multitap").map(String::as_str) {
            None | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "multitap option '{other}' (on or off)"
                )))
            }
        };
        let ipl = match config.options.get("roms").map(String::as_str) {
            Some("stub") => None,
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => match config.roms.get("spc700") {
                Some(r) if r.len() == 64 => Some(r.clone()),
                Some(r) => {
                    return Err(ConfigError::Invalid(format!(
                        "spc700 boot ROM must be 64 bytes, got {}",
                        r.len()
                    )))
                }
                None => None,
            },
        };
        let k1 = port_option(config, "port1")?;
        let k2 = port_option(config, "port2")?;
        if k1 == Some(SnesKind::SuperScope) {
            return Err(ConfigError::Invalid(
                "the Super Scope works in port 2 only".into(),
            ));
        }
        if multitap && k2 != Some(SnesKind::Pad) {
            return Err(ConfigError::Invalid("the Multiplayer 5 takes pads".into()));
        }
        let dev = |k: Option<SnesKind>| -> Option<Box<dyn PortDevice>> {
            k.map(|k| Box::new(SnesController::new(k)) as Box<dyn PortDevice>)
        };
        let extra = multitap.then_some(SnesKind::Pad);
        let devices = [dev(k1), dev(k2), dev(extra), dev(extra), dev(extra)];
        let board = Board {
            model,
            wram: vec![0x55; 0x20000],
            wram_addr: 0,
            ppu: SPpu::new(model == Model::Pal),
            apu: Apu::new(ipl.as_deref()),
            cart: None,
            open_bus: 0,
            nmitimen: 0,
            wrio: 0xff,
            wrmpya: 0xff,
            wrdiv: 0xffff,
            htime: 0x1ff,
            vtime: 0x1ff,
            memsel: 0,
            rddiv: 0,
            rdmpy: 0,
            nmi_flag: false,
            irq_flag: false,
            joy: [0; 4],
            joy_busy_until: 0,
            strobe: false,
            dma: [Dma::default(); 8],
            hdmaen: 0,
            h: 0,
            clock: 0,
            v: 0,
            frames: 0,
            vblank: false,
            hblank: false,
            odd_frame: false,
            smp_acc: 0,
            field_done: false,
            resample_pos: 0,
            last_sample: (0, 0),
            samples: Vec::new(),
            ov: overlay::Overlays::default(),
        };
        let mut m = Self {
            model,
            cpu: Cpu65816::new(),
            board,
            ports: Ports { devices, multitap },
            cart: devices_cartridge::snes::slot(),
            ipl,
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
        };
        m.cpu.reset();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let b = &self.board;
        BeamPos {
            frame: b.frames,
            line: b.v,
            cycle: (b.h / 4) as u16,
            hpos: overlay::dot_column((b.h / 4) as u16),
        }
    }

    /// One instruction. Returns `Some` if emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let before = self.board.clock;
        let entered = if self.board.cart.is_some() {
            let beam = self.beam_pos();
            let mut bus = SnesBus {
                b: &mut self.board,
                ports: &mut self.ports,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step(&mut bus).1
        } else {
            self.board.advance(LINE, &mut self.ports);
            None
        };
        self.sched.advance(self.board.clock - before);
        let field = std::mem::take(&mut self.board.field_done);
        if field && self.board.ov.any() {
            self.board.ppu.overlay_end_frame();
            self.board.ov.end_frame();
        }
        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            let next_pc = self.cpu.pc24();
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
                self.field_done_pending |= field;
                return Some(stop);
            }
        }
        field.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.devices.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
            }
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    let (port, k) = match machine_key.0 {
                        k @ 0..=7 => (0, k),
                        k @ 8..=15 => (1, k - keys::PAD2),
                        _ => return,
                    };
                    if let Some(dev) = &mut self.ports.devices[port] {
                        dev.input(&InputEvent::Key {
                            machine_key: MachineKey(k),
                            down,
                        });
                    }
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value: 1 } = ev.event {
                    match id.0 {
                        0 => self.reset(ResetKind::Hard),
                        1 => self.reset(ResetKind::Soft),
                        _ => {}
                    }
                }
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        let standard = match self.model {
            Model::Ntsc => VideoStandard::Ntsc,
            Model::Pal => VideoStandard::Pal,
        };
        let field = if !self.board.ppu.interlace() {
            FieldParity::Progressive
        } else if self.board.ppu.field {
            FieldParity::Odd
        } else {
            FieldParity::Even
        };
        SignalInfo {
            pixel_clock_hz: (self.model.master_hz() / 2) as u32,
            active_pixels: sppu::WIDTH as u32,
            total_pixels: 682,
            lines_per_field: u32::from(self.model.lines()),
            standard,
            output: OutputType::Composite,
            field,
        }
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Snes, addr: u32) -> u8 {
    m.board.mem_peek(addr)
}

/// The PPU (tests and tools).
pub fn ppu(m: &Snes) -> &SPpu {
    &m.board.ppu
}

/// Master clocks since power-on (tests).
pub fn master_clocks(m: &Snes) -> u64 {
    m.board.clock
}

impl Machine for Snes {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        self.board.ov.fresh = false;
        for f in dbg.per_frame_freezes() {
            if f.space == CPU_SPACE {
                for i in 0..f.size.bytes() {
                    let a = f.addr + i;
                    if (a >> 17) == 0x7e >> 1 {
                        self.board.wram[(a & 0x1ffff) as usize] = (f.value >> (8 * i)) as u8;
                    }
                }
            }
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.board.ov.fresh = false;
        self.instruction(None);
        StopReason::StepDone
    }

    /// Hard: power cycle. Soft: the RESET button (CPU, PPU registers, the
    /// sound module and the CPU's I/O registers reset; RAM stays).
    fn reset(&mut self, kind: ResetKind) {
        let b = &mut self.board;
        if kind == ResetKind::Hard {
            b.wram.fill(0x55);
            b.ppu = SPpu::new(self.model == Model::Pal);
            b.ppu.set_overlays(&b.ov.ids);
            b.apu = Apu::new(self.ipl.as_deref());
            b.cart = self.cart.cart().cloned();
            b.h = 0;
            b.v = 0;
            b.dma = [Dma::default(); 8];
        } else {
            b.ppu.reset();
            b.apu.reset();
        }
        b.nmitimen = 0;
        b.hdmaen = 0;
        b.memsel = 0;
        b.wrio = 0xff;
        b.nmi_flag = false;
        b.irq_flag = false;
        self.cpu = Cpu65816::new();
        self.cpu.reset();
        self.input.clear();
        self.field_done_pending = false;
    }

    /// The S-PPU's views: `tiles` (VRAM as 2/4/8 bpp or Mode 7 tiles),
    /// `screen` (one BG tilemap; Mode 7's 128 × 128 map), `sprites` (OAM)
    /// and `palette` (CGRAM). Option `addr` is a byte address in the
    /// `vram` space.
    fn gfx_views(&self) -> Vec<GfxViewInfo> {
        sppu::gfx::views("S-PPU")
    }

    fn gfx_view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let ctx = sppu::gfx::GfxContext {
            vram_space: VRAM_SPACE,
            cgram_space: CGRAM_SPACE,
            oam_space: OAM_SPACE,
        };
        self.board.ppu.gfx_view(id, &ctx, opts)
    }

    /// `sprites` (boxes per object as the sprite scan found them line by
    /// line), `hdma` (bands on lines whose registers HDMA rewrote) and
    /// `irq` (H/V-IRQ and NMI marks).
    fn overlays(&self) -> Vec<OverlayInfo> {
        overlay::infos()
    }

    fn set_overlays(&mut self, ids: &[&str]) {
        self.board.ov.set(ids);
        self.board.ppu.set_overlays(ids);
    }

    fn overlay(&self, id: &str) -> Option<Overlay> {
        let last = self.board.ov.fresh;
        match id {
            "sprites" => self.board.ppu.overlay(id, last),
            _ => self.board.ov.overlay(id, last),
        }
    }

    /// Lines 1–239 as the frame is delivered: a row each, or in interlace
    /// the row of the current field in the 478-row picture.
    fn frame_row(&self, line: u16) -> Option<u32> {
        self.board.ppu.frame_row(line)
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: sppu::WIDTH as u32,
            height: self.board.ppu.height() as u32,
            pixels: PixelData::Rgb(self.board.ppu.frame()),
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 2,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let space = |id, name, device, addr_bits| MemSpace {
            id,
            name,
            device,
            addr_bits,
            big_endian: false,
        };
        vec![
            space(CPU_SPACE, "cpu", CPU, 24),
            space(VRAM_SPACE, "vram", CPU, 16),
            space(CGRAM_SPACE, "cgram", CPU, 9),
            space(OAM_SPACE, "oam", CPU, 10),
            space(ARAM_SPACE, "aram", SPC, 16),
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let p = &b.ppu;
        vec![
            ChipInfo {
                name: "cpu-io",
                regs: vec![
                    reg("NMITIMEN", u32::from(b.nmitimen), 8),
                    reg("HTIME", u32::from(b.htime), 9),
                    reg("VTIME", u32::from(b.vtime), 9),
                    reg("HDMAEN", u32::from(b.hdmaen), 8),
                    reg("MEMSEL", u32::from(b.memsel), 1),
                    reg("RDNMI", u32::from(b.nmi_flag), 1),
                    reg("TIMEUP", u32::from(b.irq_flag), 1),
                    reg("H", b.h / 4, 9),
                    reg("V", u32::from(b.v), 9),
                ],
            },
            ChipInfo {
                name: "ppu",
                regs: p
                    .debug_regs()
                    .into_iter()
                    .map(|(name, value, bits)| reg(name, value, bits))
                    .collect(),
            },
            ChipInfo {
                name: "apu-ports",
                regs: (0..4)
                    .map(|i| {
                        const N: [&str; 4] = ["APUIO0", "APUIO1", "APUIO2", "APUIO3"];
                        reg(N[i], u32::from(b.apu.smp.to_cpu[i]), 8)
                    })
                    .collect(),
            },
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU, SPC]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU {
            Some(self as &mut dyn DebugTarget)
        } else if dev == SPC {
            Some(&mut self.board.apu as &mut dyn DebugTarget)
        } else {
            None
        }
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        const NAMES: [&str; 5] = [
            "controller 1",
            "controller 2",
            "controller 3",
            "controller 4",
            "controller 5",
        ];
        let n = if self.ports.multitap { 5 } else { 2 };
        (0..n)
            .map(|i| PortInfo {
                id: PortId(i as u8),
                kind: PortKind::SnesControl,
                name: NAMES[i],
                connected: self.ports.devices[i].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let n = if self.ports.multitap { 5 } else { 2 };
        let i = usize::from(port.0);
        if i >= n {
            return Err(PortError::NoSuchPort(port));
        }
        if dev.kind() != PortKind::SnesControl {
            return Err(PortError::Incompatible {
                port: PortKind::SnesControl,
                device: dev.kind(),
            });
        }
        if self.ports.devices[i].is_some() {
            return Err(PortError::Occupied(port));
        }
        self.ports.devices[i] = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        self.ports.devices.get_mut(usize::from(port.0))?.take()
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        vec![MediaSlotInfo {
            id: SLOT_CARTRIDGE,
            name: "cartridge",
            kind: MediaKind::Cartridge,
            inserted: self.cart.is_inserted(),
        }]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        (slot == SLOT_CARTRIDGE).then_some(&mut self.cart as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"SNSM", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.ports.multitap);
            w.bool(self.field_done_pending);
            w.bytes(&b.wram);
            w.u32(b.wram_addr);
            for v in [b.open_bus, b.nmitimen, b.wrio, b.wrmpya, b.memsel, b.hdmaen] {
                w.u8(v);
            }
            for v in [b.wrdiv, b.htime, b.vtime, b.rddiv, b.rdmpy, b.v] {
                w.u16(v);
            }
            for j in b.joy {
                w.u16(j);
            }
            for v in [
                b.nmi_flag,
                b.irq_flag,
                b.strobe,
                b.vblank,
                b.hblank,
                b.odd_frame,
                b.field_done,
            ] {
                w.bool(v);
            }
            w.u64(b.joy_busy_until);
            w.u32(b.h);
            w.u64(b.clock);
            w.u64(b.frames);
            w.u64(b.smp_acc);
            w.u32(b.resample_pos);
            w.u16(b.last_sample.0 as u16);
            w.u16(b.last_sample.1 as u16);
            for d in &b.dma {
                for v in [d.dmap, d.bbad, d.a1b, d.dasb, d.ntrl, d.unused] {
                    w.u8(v);
                }
                for v in [d.a1t, d.das, d.a2a] {
                    w.u16(v);
                }
                w.bool(d.do_transfer);
                w.bool(d.terminated);
            }
            w.bool(b.cart.is_some());
        });
        self.cpu.save(&mut w);
        b.ppu.save(&mut w);
        b.apu.save(&mut w);
        if let Some(c) = &b.cart {
            c.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for d in &self.ports.devices {
            w.chunk(*b"SNSD", 1, |w| {
                w.bytes(&d.as_ref().map(|d| d.save_state()).unwrap_or_default());
            });
        }
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"SNSM", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        if c.bool()? != self.ports.multitap {
            return Err(StateError::Invalid("multitap differs"));
        }
        let field_done_pending = c.bool()?;
        let mut b = self.board.clone();
        let wram = c.bytes()?;
        if wram.len() != b.wram.len() {
            return Err(StateError::Invalid("WRAM size"));
        }
        b.wram.copy_from_slice(wram);
        b.wram_addr = c.u32()? & 0x1ffff;
        for v in [
            &mut b.open_bus,
            &mut b.nmitimen,
            &mut b.wrio,
            &mut b.wrmpya,
            &mut b.memsel,
            &mut b.hdmaen,
        ] {
            *v = c.u8()?;
        }
        for v in [
            &mut b.wrdiv,
            &mut b.htime,
            &mut b.vtime,
            &mut b.rddiv,
            &mut b.rdmpy,
            &mut b.v,
        ] {
            *v = c.u16()?;
        }
        for j in &mut b.joy {
            *j = c.u16()?;
        }
        for v in [
            &mut b.nmi_flag,
            &mut b.irq_flag,
            &mut b.strobe,
            &mut b.vblank,
            &mut b.hblank,
            &mut b.odd_frame,
            &mut b.field_done,
        ] {
            *v = c.bool()?;
        }
        b.joy_busy_until = c.u64()?;
        b.h = c.u32()?;
        b.clock = c.u64()?;
        b.frames = c.u64()?;
        b.smp_acc = c.u64()?;
        b.resample_pos = c.u32()? % 3;
        b.last_sample = (c.u16()? as i16, c.u16()? as i16);
        if b.h > LINE || b.v > 313 {
            return Err(StateError::Invalid("beam position"));
        }
        for d in &mut b.dma {
            for v in [
                &mut d.dmap,
                &mut d.bbad,
                &mut d.a1b,
                &mut d.dasb,
                &mut d.ntrl,
                &mut d.unused,
            ] {
                *v = c.u8()?;
            }
            for v in [&mut d.a1t, &mut d.das, &mut d.a2a] {
                *v = c.u16()?;
            }
            d.do_transfer = c.bool()?;
            d.terminated = c.bool()?;
        }
        if c.bool()? != b.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        b.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        b.ppu.load(&mut r)?;
        b.apu.load(&mut r)?;
        if let Some(cart) = &mut b.cart {
            cart.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut states = Vec::new();
        for _ in 0..5 {
            let (_, mut c) = r.chunk(*b"SNSD", 1)?;
            states.push(c.bytes()?.to_vec());
        }
        for (dev, st) in self.ports.devices.iter().zip(&states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("controllers differ"));
            }
        }
        for (dev, st) in self.ports.devices.iter_mut().zip(&states) {
            if let Some(d) = dev {
                d.load_state(st)?;
            }
        }
        self.cpu = cpu;
        self.board = b;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Snes {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let b = &self.board;
        let byte = |a: u32| -> Option<u32> {
            Some(u32::from(match space {
                CPU_SPACE => b.mem_peek(a & 0xff_ffff),
                VRAM_SPACE => {
                    let w = b.ppu.vram[((a >> 1) & 0x7fff) as usize];
                    if a & 1 == 0 {
                        w as u8
                    } else {
                        (w >> 8) as u8
                    }
                }
                CGRAM_SPACE => {
                    let w = b.ppu.cgram[((a >> 1) & 0xff) as usize];
                    if a & 1 == 0 {
                        w as u8
                    } else {
                        (w >> 8) as u8
                    }
                }
                OAM_SPACE => b.ppu.oam[(a as usize) % 544],
                _ => return None,
            }))
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            let b = &mut self.board;
            match space {
                CPU_SPACE => {
                    let bank = (a >> 16) as u8;
                    if bank == 0x7e || bank == 0x7f {
                        b.wram[(a & 0x1ffff) as usize] = byte;
                    } else if bank & 0x40 == 0 && (a as u16) < 0x2000 {
                        b.wram[(a & 0x1fff) as usize] = byte;
                    } else {
                        return false;
                    }
                }
                VRAM_SPACE => {
                    let w = &mut b.ppu.vram[((a >> 1) & 0x7fff) as usize];
                    *w = if a & 1 == 0 {
                        (*w & 0xff00) | u16::from(byte)
                    } else {
                        (*w & 0x00ff) | u16::from(byte) << 8
                    };
                }
                CGRAM_SPACE => {
                    let w = &mut b.ppu.cgram[((a >> 1) & 0xff) as usize];
                    *w = if a & 1 == 0 {
                        (*w & 0xff00) | u16::from(byte)
                    } else {
                        (*w & 0x00ff) | u16::from(byte & 0x7f) << 8
                    };
                }
                OAM_SPACE => b.ppu.oam[(a as usize) % 544] = byte,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        let m8 = self.cpu.p & cpu_65816::M != 0;
        let x8 = self.cpu.p & cpu_65816::X != 0;
        cpu_65816::disassemble(&PeekBus(&self.board), addr, m8, x8)
    }
}

#[cfg(test)]
mod tests;
