//! Runner for Wolfgang Lorenz' C64 Emulator Test Suite (public domain).
//!
//! Each test program reports its result by writing to `$D7FF` (VICE "debug
//! cart" convention): `$00` = passed, `$FF` = failed. The runner boots the
//! C64, places the program in memory like LOAD would, starts it, collects
//! its text output from KERNAL BSOUT (`$FFD2`) and stops at the `$D7FF`
//! write. Everything goes through the generic `Machine`/`DebugHooks`
//! interface; nothing here knows C64 internals beyond the KERNAL entry
//! points the suite itself uses.

use emu_core::registry::{MachineConfig, Registry};
use emu_core::{BreakpointKind, DebugHooks, DeviceId, Machine, MemSpaceId, Size, StopReason};

const BSOUT: u32 = 0xffd2;
const EXIT_PORT: u32 = 0xd7ff;

/// Frames to let the KERNAL boot before injecting a test (with real ROMs).
pub const BOOT_FRAMES: u32 = 150;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Timeout,
    /// The machine stopped for another reason (jam, fault).
    Stopped(String),
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub verdict: Verdict,
    /// Text the test printed (PETSCII mapped to ASCII).
    pub output: String,
    /// Emulated frames until the verdict.
    pub frames: u32,
}

/// PETSCII as printed by the suite → readable ASCII.
fn petscii(c: u8) -> Option<char> {
    match c {
        0x0d => Some('\n'),
        0x20..=0x40 | 0x5b..=0x5f => Some(c as char),
        0x41..=0x5a => Some((c + 0x20) as char),
        0xc1..=0xda => Some((c - 0x80) as char),
        _ => None,
    }
}

pub fn space(m: &dyn Machine, name: &str) -> MemSpaceId {
    m.memory_spaces()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.id)
        .unwrap_or_else(|| panic!("machine has no '{name}' memory space"))
}

fn poke(m: &mut dyn Machine, space: MemSpaceId, addr: u32, v: u8) {
    let cpu = m.debug_target(DeviceId("cpu")).expect("cpu");
    assert!(cpu.poke(space, addr, Size::Byte, u32::from(v)));
}

fn reg(m: &mut dyn Machine, name: &str) -> u32 {
    m.debug_target(DeviceId("cpu"))
        .and_then(|c| c.regs().get(name))
        .unwrap_or(0)
}

/// Load a PRG like the KERNAL does and type RUN: sets the BASIC program
/// end pointers and puts `RUN⏎` into the keyboard buffer.
pub fn load_and_run(m: &mut dyn Machine, prg: &[u8]) {
    let ram = space(m, "ram");
    let load = u32::from(u16::from_le_bytes([prg[0], prg[1]]));
    let body = &prg[2..];
    for (i, &b) in body.iter().enumerate() {
        poke(m, ram, load + i as u32, b);
    }
    let end = load + body.len() as u32;
    // VARTAB, ARYTAB, STREND: end of the BASIC program.
    for ptr in [0x2d, 0x2f, 0x31] {
        poke(m, ram, ptr, end as u8);
        poke(m, ram, ptr + 1, (end >> 8) as u8);
    }
    // Keyboard buffer: "RUN" + RETURN, count in $C6.
    for (i, &c) in b"RUN\r".iter().enumerate() {
        poke(m, ram, 0x0277 + i as u32, c);
    }
    poke(m, ram, 0xc6, 4);
}

/// Tests that need the real ROMs: `cputiming` calls BASIC floating point,
/// `mmu` and `mmufetch` compare against bytes of the BASIC/KERNAL images.
/// Skipped when running without ROMs.
pub const NEEDS_REAL_ROMS: &[&str] = &["cputiming", "mmu", "mmufetch"];

/// The address in a `nnnn SYS addr` BASIC line at the start of a PRG.
fn sys_address(prg: &[u8]) -> Option<u32> {
    let line = prg.get(2..)?;
    let at = line.iter().position(|&b| b == 0x9e)?;
    let digits: String = line[at + 1..]
        .iter()
        .take_while(|b| b.is_ascii_digit() || **b == b' ')
        .filter(|b| b.is_ascii_digit())
        .map(|&b| b as char)
        .collect();
    digits.parse().ok()
}

/// Without ROMs: place the program and jump to its SYS address directly,
/// as BASIC would, from an instruction boundary.
fn load_and_sys(m: &mut dyn Machine, dbg: &mut DebugHooks, prg: &[u8]) {
    let ram = space(m, "ram");
    let load = u32::from(u16::from_le_bytes([prg[0], prg[1]]));
    for (i, &b) in prg[2..].iter().enumerate() {
        poke(m, ram, load + i as u32, b);
    }
    let start = sys_address(prg).expect("PRG starts with a SYS line");
    dbg.set_instr_step(true);
    while m.run_frame(dbg) != StopReason::StepDone {}
    let cpu = m.debug_target(DeviceId("cpu")).expect("cpu");
    cpu.set_reg("PC", start);
    cpu.set_reg("S", 0xf6);
    // SYS runs with interrupts enabled, like after BASIC's RUN.
    let p = cpu.regs().get("P").unwrap_or(0);
    cpu.set_reg("P", p & !0x04);
}

/// Run one test program. With `options["roms"] = "stub"` in `config` the
/// program is started directly; otherwise the real KERNAL boots and RUNs it.
pub fn run(registry: &Registry, config: &MachineConfig, prg: &[u8], max_frames: u32) -> Outcome {
    let mut m = registry.create("c64", config).expect("c64");
    let mut dbg = DebugHooks::new();
    if config.options.get("roms").is_some_and(|r| r == "stub") {
        m.run_frame(&mut dbg);
        load_and_sys(m.as_mut(), &mut dbg, prg);
    } else {
        for _ in 0..BOOT_FRAMES {
            m.run_frame(&mut dbg);
        }
        load_and_run(m.as_mut(), prg);
    }

    let cpu_space = space(m.as_ref(), "cpu");
    let bsout = dbg.add_breakpoint(BreakpointKind::Exec {
        space: cpu_space,
        addr: BSOUT,
    });
    dbg.add_breakpoint(BreakpointKind::Write {
        space: cpu_space,
        addr: EXIT_PORT,
        len: 1,
    });

    let mut output = String::new();
    let mut frames = 0;
    while frames < max_frames {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => frames += 1,
            StopReason::Breakpoint(id) if id == bsout => {
                if let Some(c) = petscii(reg(m.as_mut(), "A") as u8) {
                    output.push(c);
                }
            }
            StopReason::Watchpoint { addr, .. } if addr == EXIT_PORT => {
                // The suite does LDA #code / STA $D7FF.
                let verdict = if reg(m.as_mut(), "A") == 0 {
                    Verdict::Pass
                } else {
                    Verdict::Fail
                };
                return Outcome {
                    verdict,
                    output,
                    frames,
                };
            }
            other => {
                return Outcome {
                    verdict: Verdict::Stopped(format!("{other:?}")),
                    output,
                    frames,
                }
            }
        }
    }
    Outcome {
        verdict: Verdict::Timeout,
        output,
        frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sys_address_from_basic_stub() {
        // 2016 SYS2062
        let prg = [
            0x01, 0x08, 0x0b, 0x08, 0xe0, 0x07, 0x9e, 0x32, 0x30, 0x36, 0x32, 0x00, 0x00, 0x00,
        ];
        assert_eq!(sys_address(&prg), Some(2062));
        assert_eq!(sys_address(&[0x01, 0x08, 0x00]), None);
    }

    #[test]
    fn petscii_mapping() {
        let s: String = b"LDAB - OK\r".iter().filter_map(|&c| petscii(c)).collect();
        assert_eq!(s, "ldab - ok\n");
    }
}
