//! Runner for Kevin Thacker's Arnold "Acid tests" for the Amstrad CPC
//! (assembled by `tools/fetch-testdata.sh` into `.bin` files with an
//! AMSDOS header and a pasmo symbol table, `.sym`).
//!
//! The runner boots the CPC, copies the program to its load address and
//! starts it with `CALL`. The tests print through the library routine
//! `output_char` (character in A): an execution breakpoint there records
//! the text. Where a test waits for a key (`wait_key`, the firmware's
//! KM WAIT CHAR at &BB06) the runner presses SPACE; the library's pause
//! after every 25 lines (`should_wait_key`) is switched off, and the text
//! is not drawn (the breakpoint returns from `output_char` at once). A program is
//! over when it prints "Done." (the library's `run_tests` end) or returns
//! to BASIC.
//!
//! `run_tests` goes through `run_all_tests` before each test's name: a
//! breakpoint there splits the output into tests (a program that never
//! calls it, `itest`, is one test). A result line ends in PASS or OK
//! (passed) or FAIL;
//! a test with any FAIL line fails, one with only passing lines passes,
//! one without result lines (data dumps, visual checks) is information.
//! Everything goes through the generic `Machine`/`DebugHooks` interface
//! plus the CPC's key codes.

use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::registry::{MachineConfig, Registry};
use emu_core::{
    BreakpointKind, Cycle, DebugHooks, DeviceId, Machine, MemSpaceId, Size, StopReason,
};

/// The firmware's KM WAIT CHAR jumpblock entry.
const KM_WAIT_CHAR: u32 = 0xbb06;
/// T-states a SPACE press lasts (two keyboard scans and more).
const KEY_HOLD: u64 = 200_000;

/// An AMSDOS binary: load address, entry point, data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binary {
    pub load: u16,
    pub exec: u16,
    pub data: Vec<u8>,
}

/// Parse a file with a 128-byte AMSDOS header (checksum over bytes 0–66
/// at 67–68; load address 21–22, entry 26–27, length 64–66).
pub fn parse_amsdos(file: &[u8]) -> Option<Binary> {
    let h = file.get(..128)?;
    let sum: u16 = h[..67].iter().map(|&b| u16::from(b)).sum();
    if sum != u16::from_le_bytes([h[67], h[68]]) {
        return None;
    }
    let load = u16::from_le_bytes([h[21], h[22]]);
    let exec = u16::from_le_bytes([h[26], h[27]]);
    let len = usize::from(h[64]) | usize::from(h[65]) << 8 | usize::from(h[66]) << 16;
    let data = file.get(128..128 + len)?.to_vec();
    Some(Binary { load, exec, data })
}

/// A symbol's address from a pasmo symbol table (`name<TAB>EQU 01234H`).
pub fn symbol(table: &str, name: &str) -> Option<u32> {
    table.lines().find_map(|l| {
        let mut f = l.split_whitespace();
        let (n, equ, v) = (f.next()?, f.next()?, f.next()?);
        if n != name || !equ.eq_ignore_ascii_case("equ") {
            return None;
        }
        let v = v.trim_end_matches(['H', 'h']);
        u32::from_str_radix(v, 16).ok()
    })
}

/// Marks the start of a test in the captured text.
const TEST_START: char = '\u{1}';

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// The program printed "Done." (otherwise it timed out or never got
    /// there).
    pub done: bool,
    /// Without text output (visual and audio tests): judged by a person.
    pub manual: bool,
    /// Tests that passed and failed (by their result lines).
    pub passed: u32,
    pub failed: u32,
    /// Failed tests: the test's first line and its first failing lines.
    pub failures: Vec<String>,
    /// Everything the test printed.
    pub text: String,
    pub frames: u32,
}

/// Verdicts per test of the captured text (tests start with
/// [`TEST_START`]).
pub fn verdicts(text: &str) -> (u32, u32, Vec<String>) {
    let (mut passed, mut failed, mut failures) = (0, 0, Vec::new());
    for test in text.split(TEST_START).skip(1) {
        let lines: Vec<&str> = test.lines().map(str::trim).collect();
        let bad: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| l.ends_with("FAIL"))
            .collect();
        let good = lines
            .iter()
            .any(|l| l.ends_with("PASS") || l.ends_with("OK"));
        if !bad.is_empty() {
            failed += 1;
            let mut msg = lines.first().copied().unwrap_or("").to_string();
            for l in bad.iter().take(3) {
                if *l != msg {
                    msg.push_str(" | ");
                    msg.push_str(l);
                }
            }
            if bad.len() > 3 {
                msg.push_str(&format!(" | … {} failing lines", bad.len()));
            }
            failures.push(msg);
        } else if good {
            passed += 1;
        }
    }
    (passed, failed, failures)
}

fn cpu_space(m: &dyn Machine) -> MemSpaceId {
    m.memory_spaces()
        .into_iter()
        .find(|s| s.name == "cpu")
        .map(|s| s.id)
        .expect("cpu memory space")
}

fn press_space(m: &mut dyn Machine, at: u64) {
    let key = machine_cpc::keyboard::keys::SPACE;
    for (t, down) in [(at, true), (at + KEY_HOLD, false)] {
        m.input(TimedInput {
            at: Cycle(t),
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: key,
                down,
            },
        });
    }
}

fn run_fields(m: &mut dyn Machine, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

/// `crtctest` asks for the CRTC type with KM WAIT CHAR and then ignores
/// the key: `LD A,'3'` follows the call (noted in the ACE emulator's test
/// results, which patch it). Put `digit` there instead.
pub fn patch_crtc_type(program: &mut Binary, digit: u8) -> bool {
    let pattern = [0xcd, 0x06, 0xbb, 0x3e, b'3'];
    match program.data.windows(5).position(|w| w == pattern) {
        Some(i) => {
            program.data[i + 4] = digit;
            true
        }
        None => false,
    }
}

/// `cpctest`'s "GA I/O decode test" enters the library's `io_decode`
/// without loading BC with its restore routine (`ga_io_restore_extra`, which
/// is defined but never used), so it calls whatever BC holds, 0 after the
/// previous test, and resets the machine. On entering `ga_io_decode` the
/// runner loads BC as intended.
const ENTRY_FIXES: [(&str, &str, &str); 1] = [("ga_io_decode", "BC", "ga_io_restore_extra")];

/// Run one test program; `crtc_type` is the digit `crtctest` gets.
pub fn run(
    registry: &Registry,
    config: &MachineConfig,
    bin: &[u8],
    symbols: &str,
    crtc_type: u8,
    max_frames: u32,
) -> Result<Outcome, String> {
    let mut program = parse_amsdos(bin).ok_or("no valid AMSDOS header")?;
    patch_crtc_type(&mut program, crtc_type);
    let mut m = registry.create("cpc", config).map_err(|e| e.to_string())?;
    run_fields(m.as_mut(), 150);
    let space = cpu_space(m.as_ref());
    {
        let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
        for (i, &b) in program.data.iter().enumerate() {
            let addr = u32::from(program.load) + i as u32;
            if !cpu.poke(space, addr & 0xffff, Size::Byte, u32::from(b)) {
                return Err(format!("cannot write {addr:#06x}"));
            }
        }
        // No "press a key" every 25 lines.
        if let Some(a) = symbol(symbols, "should_wait_key") {
            cpu.poke(space, a, Size::Byte, 0);
        }
    }
    let (events, _) =
        machine_cpc::keyboard::typed_events(&format!("call &{:x}\r", program.exec), m.now().0);
    // The program starts when BASIC takes the RETURN; the breakpoints go in
    // as RETURN goes down (BASIC is waiting in KM WAIT CHAR already and
    // calls it no more), so the test's first "press a key" is seen.
    let enter = events
        .iter()
        .filter(|e| matches!(e.event, InputEvent::Key { down: true, .. }))
        .map(|e| e.at.0)
        .max()
        .unwrap_or(0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    let mut frames = 0;
    while m.now().0 < enter {
        m.run_frame(&mut dbg);
        frames += 1;
    }
    let Some(output) = symbol(symbols, "output_char") else {
        // Visual and audio tests print nothing to check.
        return Ok(Outcome {
            manual: true,
            ..Outcome::default()
        });
    };
    let out_bp = dbg.add_breakpoint(BreakpointKind::Exec {
        space,
        addr: output,
    });
    let mut wait_bps = vec![dbg.add_breakpoint(BreakpointKind::Exec {
        space,
        addr: KM_WAIT_CHAR,
    })];
    if let Some(w) = symbol(symbols, "wait_key") {
        wait_bps.push(dbg.add_breakpoint(BreakpointKind::Exec { space, addr: w }));
    }
    let test_bp = symbol(symbols, "run_all_tests")
        .map(|addr| dbg.add_breakpoint(BreakpointKind::Exec { space, addr }));
    // At the entry the return address into BASIC is on the stack; reaching
    // it with that stack level ends the program (the firmware's far calls
    // return through the same code at deeper levels).
    let entry_bp = dbg.add_breakpoint(BreakpointKind::Exec {
        space,
        addr: u32::from(program.exec),
    });
    let mut return_bp = None;
    let fixes: Vec<_> = ENTRY_FIXES
        .iter()
        .filter_map(|&(entry, reg, value)| {
            let addr = symbol(symbols, entry)?;
            let value = symbol(symbols, value)?;
            Some((
                dbg.add_breakpoint(BreakpointKind::Exec { space, addr }),
                reg,
                value,
            ))
        })
        .collect();
    // Diagnosis: EMU_CPC_BREAK=<hex> stops at that address once and
    // records the registers and the stack.
    let diag_bp = std::env::var("EMU_CPC_BREAK")
        .ok()
        .and_then(|v| u32::from_str_radix(&v, 16).ok())
        .map(|addr| dbg.add_breakpoint(BreakpointKind::Exec { space, addr }));
    let mut text = String::new();
    // A program that never runs `run_all_tests` is one test.
    text.push(TEST_START);
    let mut done = false;
    while frames < max_frames && !done {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => frames += 1,
            StopReason::Breakpoint(id) if id == out_bp => {
                let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
                let regs = cpu.regs();
                let a = regs.get("A").unwrap_or(0) as u8;
                match a {
                    13 => text.push('\n'),
                    0x20..=0x7e => text.push(a as char),
                    _ => {}
                }
                done = text.contains("Done.");
                // The text is recorded: skip drawing it (scrolling the
                // screen takes most of a test's time) by returning to the
                // caller at once.
                let sp = regs.get("SP").unwrap_or(0);
                let ret = cpu.peek(space, sp, Size::Word).unwrap_or(0);
                cpu.set_reg("SP", (sp + 2) & 0xffff);
                cpu.set_reg("PC", ret);
            }
            StopReason::Breakpoint(id) if Some(id) == test_bp => text.push(TEST_START),
            StopReason::Breakpoint(id) if id == entry_bp => {
                let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
                let sp = cpu.regs().get("SP").unwrap_or(0);
                let ret = cpu.peek(space, sp, Size::Word).unwrap_or(0);
                dbg.remove_breakpoint(id);
                let bp = dbg.add_breakpoint(BreakpointKind::Exec { space, addr: ret });
                return_bp = Some((bp, (sp + 2) & 0xffff));
            }
            StopReason::Breakpoint(id) if return_bp.is_some_and(|r| r.0 == id) => {
                let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
                let sp = cpu.regs().get("SP").unwrap_or(0);
                done = return_bp.is_some_and(|r| r.1 == sp);
            }
            StopReason::Breakpoint(id) if fixes.iter().any(|f| f.0 == id) => {
                let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
                for &(_, reg, value) in fixes.iter().filter(|f| f.0 == id) {
                    cpu.set_reg(reg, value);
                }
            }
            StopReason::Breakpoint(id) if Some(id) == diag_bp => {
                let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
                let regs = cpu.regs();
                let sp = regs.get("SP").unwrap_or(0);
                let stack: Vec<String> = (0..8)
                    .map(|k| {
                        format!(
                            "{:04X}",
                            cpu.peek(space, (sp + 2 * k) & 0xffff, Size::Word)
                                .unwrap_or(0)
                        )
                    })
                    .collect();
                text.push_str(&format!(
                    "
[break: {:?} stack {}]",
                    regs,
                    stack.join(" ")
                ));
                dbg.remove_breakpoint(id);
            }
            StopReason::Breakpoint(id) if wait_bps.contains(&id) => {
                let now = m.now().0;
                press_space(m.as_mut(), now);
            }
            other => {
                text.push_str(&format!("\n[stopped: {other:?}]"));
                break;
            }
        }
    }
    if !done {
        // Where it was when time ran out.
        let cpu = m.debug_target(DeviceId("cpu")).ok_or("no cpu")?;
        let regs = cpu.regs();
        let pc = regs.get("PC").unwrap_or(0);
        let sp = regs.get("SP").unwrap_or(0);
        let ret = cpu.peek(space, sp, Size::Word).unwrap_or(0);
        let stack: Vec<String> = (0..8)
            .map(|k| {
                format!(
                    "{:04X}",
                    cpu.peek(space, (sp + 2 * k) & 0xffff, Size::Word)
                        .unwrap_or(0)
                )
            })
            .collect();
        let mut code = Vec::new();
        let mut at = pc.saturating_sub(6);
        for _ in 0..6 {
            let i = cpu.disassemble(at);
            code.push(format!("{at:04X} {} {}", i.mnemonic, i.operand));
            at += (i.bytes.len() as u32).max(1);
        }
        text.push_str(&format!(
            "
[timeout at PC {pc:04X}, SP {sp:04X} -> {ret:04X}; stack {}; {}; {:?}]",
            stack.join(" "),
            code.join(" | "),
            regs
        ));
    }
    let (passed, failed, failures) = verdicts(&text);
    Ok(Outcome {
        manual: false,
        done,
        passed,
        failed,
        failures,
        text,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amsdos_header_and_symbols() {
        let mut f = vec![0u8; 128];
        f[18] = 2;
        f[21..23].copy_from_slice(&0x4000u16.to_le_bytes());
        f[26..28].copy_from_slice(&0x4002u16.to_le_bytes());
        f[64] = 3;
        let sum: u16 = f[..67].iter().map(|&b| u16::from(b)).sum();
        f[67..69].copy_from_slice(&sum.to_le_bytes());
        f.extend_from_slice(&[1, 2, 3]);
        let b = parse_amsdos(&f).unwrap();
        assert_eq!((b.load, b.exec, b.data), (0x4000, 0x4002, vec![1, 2, 3]));
        f[0] ^= 1;
        assert!(parse_amsdos(&f).is_none(), "bad checksum");
        let sym = "output_char\tEQU 052F9H\nwait_key\tEQU 055E8H\n";
        assert_eq!(symbol(sym, "output_char"), Some(0x52f9));
        assert_eq!(symbol(sym, "missing"), None);
    }

    #[test]
    fn verdict_lines() {
        let text = "intro\n\u{1}bit set-PASS\n\u{1}latch-\n got 5E expected 7E - FAIL\n got 00 - OK\n\u{1}DATA\n00 01\n\u{1}x- got 00 - OK\n";
        let (p, f, l) = verdicts(text);
        assert_eq!((p, f), (2, 1));
        assert_eq!(l, vec!["latch- | got 5E expected 7E - FAIL"]);
    }
}
