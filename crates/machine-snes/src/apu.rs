//! The sound module: the S-SMP (SPC700 core, 64 KB sound RAM, the 64-byte
//! boot ROM, three timers, four communication ports) and the S-DSP.
//!
//! **S-SMP I/O** ($F0–$FF; writes also reach the RAM below): $F0 TEST, $F1
//! CONTROL (timer enables — switching one on restarts it —, bits 4/5 clear
//! the input ports, bit 7 maps the boot ROM at $FFC0), $F2/$F3 the DSP
//! register index and data, $F4–$F7 the ports (reads: what the main CPU
//! wrote; writes: what it reads), $F8/$F9 two spare registers, $FA–$FC the
//! timer dividers (1–256), $FD–$FF the 4-bit timer outputs (cleared when
//! read).
//!
//! **Clocking.** 1.024 MHz (24.576 MHz ÷ 24): every bus cycle of the
//! SPC700 is one clock; timers 0/1 count at 8 kHz, timer 2 at 64 kHz, the
//! DSP makes a stereo sample every 32 clocks (32 kHz).
//!
//! **Boot ROM.** The console's 64-byte IPL is user-supplied (ROM role
//! "spc700", 64 bytes). Without it [`STUB_IPL`] runs: this project's own
//! loader for the documented upload protocol (ready signal $AA/$BB, start
//! code $CC, destination address in ports 2/3, data bytes numbered in port
//! 0, a command of 0 jumps to the address) — the same handshake, not the
//! same code.
//!
//! Sources: Martin Korth, fullsnes, "SNES APU SPC700 I/O Ports", "SNES APU
//! Main CPU Communication Port", "SNES APU Low Level Timings" (read
//! 2026-10-23). The implementation is this project's own.

use chips_nintendo::sdsp::SDsp;
use cpu_spc700::{Spc700, SpcBus};
use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};
use emu_core::{DebugTarget, Instr, MemSpaceId, RegisterSet, Size};

/// The SPC700's 64 KB address space (debugger).
pub const ARAM_SPACE: MemSpaceId = MemSpaceId(10);

/// This project's own boot loader (see the module documentation).
#[rustfmt::skip]
pub const STUB_IPL: [u8; 64] = [
    0xcd, 0xef, 0xbd, 0xe8, 0x00, 0xd4, 0x00, 0x1d, 0xd0, 0xfb, 0x8f, 0xaa, 0xf4, 0x8f, 0xbb, 0xf5,
    0xe8, 0xcc, 0x2e, 0xf4, 0xfd, 0xba, 0xf6, 0xda, 0x00, 0xba, 0xf4, 0xc4, 0xf4, 0xad, 0x00, 0xf0,
    0x18, 0x8d, 0x00, 0x64, 0xf4, 0xf0, 0xfc, 0x7e, 0xf4, 0xd0, 0xea, 0xe4, 0xf5, 0xd7, 0x00, 0xdd,
    0xc4, 0xf4, 0xfc, 0xd0, 0xee, 0xab, 0x01, 0x2f, 0xea, 0xdd, 0x1f, 0x00, 0x00, 0x00, 0xc0, 0xff,
];

pub const SMP_HZ: u64 = 1_024_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Timer {
    div: u8,
    stage: u8,
    out: u8,
    on: bool,
}

impl Timer {
    fn tick(&mut self) {
        if !self.on {
            return;
        }
        self.stage = self.stage.wrapping_add(1);
        if self.stage == self.div {
            self.stage = 0;
            self.out = (self.out + 1) & 0x0f;
        }
    }
}

/// Everything the SPC700 sees.
#[derive(Clone, Debug)]
pub struct Smp {
    pub ram: Box<[u8; 0x10000]>,
    ipl: [u8; 64],
    pub dsp: SDsp,
    /// Written by the main CPU, read by the SPC700 ($F4–$F7).
    pub to_apu: [u8; 4],
    /// Written by the SPC700, read by the main CPU ($2140–$2143).
    pub to_cpu: [u8; 4],
    control: u8,
    dsp_addr: u8,
    test: u8,
    aux: [u8; 2],
    timers: [Timer; 3],
    /// Clocks since reset.
    clock: u64,
    /// 32 kHz stereo samples not yet taken.
    pub samples: Vec<(i16, i16)>,
}

impl Smp {
    fn tick(&mut self) {
        self.clock += 1;
        if self.clock.is_multiple_of(16) {
            self.timers[2].tick();
            if self.clock.is_multiple_of(128) {
                self.timers[0].tick();
                self.timers[1].tick();
            }
        }
        if self.clock.is_multiple_of(32) {
            let s = self.dsp.sample(&mut self.ram);
            self.samples.push(s);
        }
    }

    fn io_peek(&self, addr: u16) -> u8 {
        match addr {
            0xf2 => self.dsp_addr,
            0xf3 => self.dsp.read(self.dsp_addr),
            0xf4..=0xf7 => self.to_apu[usize::from(addr - 0xf4)],
            0xf8 | 0xf9 => self.aux[usize::from(addr - 0xf8)],
            0xfd..=0xff => self.timers[usize::from(addr - 0xfd)].out,
            _ => 0,
        }
    }

    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0xf0..=0xff => self.io_peek(addr),
            0xffc0..=0xffff if self.control & 0x80 != 0 => self.ipl[usize::from(addr - 0xffc0)],
            _ => self.ram[usize::from(addr)],
        }
    }

    fn write_io(&mut self, addr: u16, v: u8) {
        match addr {
            0xf0 => self.test = v,
            0xf1 => {
                for (n, t) in self.timers.iter_mut().enumerate() {
                    let on = v & (1 << n) != 0;
                    if on && !t.on {
                        t.stage = 0;
                        t.out = 0;
                    }
                    t.on = on;
                }
                if v & 0x10 != 0 {
                    self.to_apu[0] = 0;
                    self.to_apu[1] = 0;
                }
                if v & 0x20 != 0 {
                    self.to_apu[2] = 0;
                    self.to_apu[3] = 0;
                }
                self.control = v;
            }
            0xf2 => self.dsp_addr = v,
            0xf3 => self.dsp.write(self.dsp_addr, v),
            0xf4..=0xf7 => self.to_cpu[usize::from(addr - 0xf4)] = v,
            0xf8 | 0xf9 => self.aux[usize::from(addr - 0xf8)] = v,
            0xfa..=0xfc => self.timers[usize::from(addr - 0xfa)].div = v,
            _ => {}
        }
    }
}

impl SpcBus for Smp {
    fn read(&mut self, addr: u16) -> u8 {
        let v = self.peek(addr);
        if (0xfd..=0xff).contains(&addr) {
            self.timers[usize::from(addr - 0xfd)].out = 0;
        }
        self.tick();
        v
    }

    fn write(&mut self, addr: u16, v: u8) {
        if (0xf0..=0xff).contains(&addr) {
            self.write_io(addr, v);
        }
        self.ram[usize::from(addr)] = v;
        self.tick();
    }

    fn idle(&mut self) {
        self.tick();
    }

    fn peek(&self, addr: u16) -> u8 {
        Smp::peek(self, addr)
    }
}

/// The sound module with its CPU.
#[derive(Clone, Debug)]
pub struct Apu {
    pub cpu: Spc700,
    pub smp: Smp,
    /// SPC700 clocks owed to catch up with the main CPU.
    owed: i64,
}

impl Apu {
    pub fn new(ipl: Option<&[u8]>) -> Self {
        let mut ipl_rom = STUB_IPL;
        if let Some(r) = ipl {
            ipl_rom.copy_from_slice(&r[..64]);
        }
        let mut ram = Box::new([0u8; 0x10000]);
        // Power-up pattern (fullsnes "APU RAM"): 32 × $00, 32 × $FF.
        for (i, b) in ram.iter_mut().enumerate() {
            *b = if i & 0x20 == 0 { 0x00 } else { 0xff };
        }
        let mut apu = Self {
            cpu: Spc700::new(),
            smp: Smp {
                ram,
                ipl: ipl_rom,
                dsp: SDsp::new(),
                to_apu: [0; 4],
                to_cpu: [0; 4],
                control: 0xb0,
                dsp_addr: 0,
                test: 0x0a,
                aux: [0; 2],
                timers: [Timer::default(); 3],
                clock: 0,
                samples: Vec::new(),
            },
            owed: 0,
        };
        apu.reset();
        apu
    }

    /// The console's reset line also resets the sound module.
    pub fn reset(&mut self) {
        let s = &mut self.smp;
        s.control = 0xb0;
        s.to_apu = [0; 4];
        s.to_cpu = [0; 4];
        for t in &mut s.timers {
            t.on = false;
        }
        s.dsp.reset();
        self.cpu.reset(&mut self.smp);
        self.owed = 0;
    }

    /// Run the SPC700 for `clocks` more clocks (it finishes instructions,
    /// so it may run a little ahead).
    pub fn run(&mut self, clocks: i64) {
        self.owed += clocks;
        while self.owed > 0 {
            self.owed -= i64::from(self.cpu.step(&mut self.smp));
        }
    }
}

const STATE_TAG: [u8; 4] = *b"SSMP";

impl Savestate for Apu {
    fn save(&self, w: &mut StateWriter) {
        let s = &self.smp;
        w.chunk(STATE_TAG, 1, |w| {
            w.raw(&s.ram[..]);
            w.raw(&s.to_apu);
            w.raw(&s.to_cpu);
            for v in [s.control, s.dsp_addr, s.test, s.aux[0], s.aux[1]] {
                w.u8(v);
            }
            for t in &s.timers {
                w.u8(t.div);
                w.u8(t.stage);
                w.u8(t.out);
                w.bool(t.on);
            }
            w.u64(s.clock);
            w.u64(self.owed as u64);
        });
        self.cpu.save(w);
        s.dsp.save(w);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut a = self.clone();
        let s = &mut a.smp;
        c.raw_into(&mut s.ram[..])?;
        c.raw_into(&mut s.to_apu)?;
        c.raw_into(&mut s.to_cpu)?;
        s.control = c.u8()?;
        s.dsp_addr = c.u8()?;
        s.test = c.u8()?;
        s.aux = [c.u8()?, c.u8()?];
        for t in &mut s.timers {
            t.div = c.u8()?;
            t.stage = c.u8()?;
            t.out = c.u8()? & 0x0f;
            t.on = c.bool()?;
        }
        s.clock = c.u64()?;
        a.owed = c.u64()? as i64;
        s.samples.clear();
        a.cpu.load(r)?;
        a.smp.dsp.load(r)?;
        *self = a;
        Ok(())
    }
}

impl DebugTarget for Apu {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != ARAM_SPACE {
            return None;
        }
        Some((0..size.bytes()).fold(0, |v, i| {
            v | u32::from(self.smp.peek(addr.wrapping_add(i) as u16)) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != ARAM_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            self.smp.ram[(addr.wrapping_add(i) & 0xffff) as usize] = (v >> (8 * i)) as u8;
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        cpu_spc700::disassemble(&self.smp, addr as u16)
    }
}
