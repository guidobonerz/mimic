//! A bare board to run Ada programs before the Cogitatron 16 exists as a
//! machine: CPU, 4 MiB chip RAM + 28 MiB fast RAM, the ROM area (writable
//! here, so programs can be placed there), and the debug port at
//! FFFF:43F0 (SPEC.md 10.5): DBGOUT prints, DBGEXIT ends the run, DBGID
//! reads $C016. Everything else in segment FFFF reads 0.

use cpu_ada::Ada;
use emu_core::{Bus, Size};

const RAM_SEGMENTS: u32 = 0x200;
const ROM_FIRST: u32 = 0xfffb;
const ROM_SIZE: usize = 0x4_0000;
pub const DBGOUT: u32 = 0xffff_43f0;
pub const DBGEXIT: u32 = 0xffff_43f2;
pub const DBGID: u32 = 0xffff_43f4;

pub struct Board {
    ram: Vec<u8>,
    rom: Vec<u8>,
    pub output: Vec<u8>,
    pub exit: Option<u16>,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            ram: vec![0; (RAM_SEGMENTS as usize) << 16],
            rom: vec![0xff; ROM_SIZE],
            output: Vec::new(),
            exit: None,
        }
    }
}

impl Board {
    fn cell(&self, a: u32) -> Option<u8> {
        let seg = a >> 16;
        if seg < RAM_SEGMENTS {
            Some(self.ram[a as usize])
        } else if (ROM_FIRST..ROM_FIRST + 4).contains(&seg) {
            Some(self.rom[(a - (ROM_FIRST << 16)) as usize])
        } else {
            None
        }
    }

    fn cell_mut(&mut self, a: u32) -> Option<&mut u8> {
        let seg = a >> 16;
        if seg < RAM_SEGMENTS {
            Some(&mut self.ram[a as usize])
        } else if (ROM_FIRST..ROM_FIRST + 4).contains(&seg) {
            Some(&mut self.rom[(a - (ROM_FIRST << 16)) as usize])
        } else {
            None
        }
    }

    /// Put bytes at linear `at` (RAM or ROM area).
    pub fn load(&mut self, at: u32, bytes: &[u8]) -> Result<(), String> {
        for (i, b) in bytes.iter().enumerate() {
            let a = at.wrapping_add(i as u32);
            *self
                .cell_mut(a)
                .ok_or_else(|| format!("${a:08x} is neither RAM nor ROM"))? = *b;
        }
        Ok(())
    }

    pub fn byte(&self, a: u32) -> u8 {
        self.cell(a).unwrap_or(0xff)
    }

    fn io_read(&self, a: u32) -> u32 {
        if a & !1 == DBGID {
            0xc016
        } else {
            0
        }
    }
}

impl Bus for Board {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        if addr >> 16 == 0xffff {
            let v = self.io_read(addr);
            return if size == Size::Byte { v & 0xff } else { v };
        }
        match size {
            Size::Byte => u32::from(self.byte(addr)),
            _ => u32::from(u16::from_le_bytes([self.byte(addr), self.byte(addr | 1)])),
        }
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if addr >> 16 == 0xffff {
            match addr & !1 {
                DBGOUT => self.output.push(v as u8),
                DBGEXIT => self.exit = Some(if size == Size::Byte { v & 0xff } else { v } as u16),
                _ => {}
            }
            return;
        }
        match size {
            Size::Byte => {
                if let Some(c) = self.cell_mut(addr) {
                    *c = v as u8;
                }
            }
            _ => {
                for (i, b) in (v as u16).to_le_bytes().into_iter().enumerate() {
                    if let Some(c) = self.cell_mut((addr & !1) | i as u32) {
                        *c = b;
                    }
                }
            }
        }
    }

    fn addr_bits(&self) -> u8 {
        32
    }
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The program wrote DBGEXIT.
    Exit(u16),
    /// The cycle limit was reached.
    Limit,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub end: End,
    pub output: Vec<u8>,
    pub cycles: u64,
    pub cpu: Ada,
}

/// Run from `entry` (linear) in supervisor mode until DBGEXIT or
/// `max_cycles`. The stack (S2:R12) starts at the top of chip RAM.
pub fn run(board: &mut Board, entry: u32, max_cycles: u64) -> Run {
    let mut cpu = Ada::new();
    cpu.s[0] = (entry >> 16) as u16;
    cpu.pc = entry as u16;
    cpu.s[1] = cpu.s[0];
    cpu.s[2] = 0x003f;
    cpu.r[12] = 0xfffe;
    let mut cycles = 0;
    while cycles < max_cycles {
        cycles += u64::from(cpu.step(board));
        if let Some(code) = board.exit {
            return Run {
                end: End::Exit(code),
                output: board.output.clone(),
                cycles,
                cpu,
            };
        }
    }
    Run {
        end: End::Limit,
        output: board.output.clone(),
        cycles,
        cpu,
    }
}
