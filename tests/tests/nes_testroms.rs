//! NES test ROMs (Phase 12 gate): blargg's suites, nestest and friends
//! from the nes-test-roms collection (https://github.com/christopherpow/
//! nes-test-roms, fetched into `$EMU_TESTDATA_DIR/nes-test-roms` by
//! `tools/fetch-testdata.sh`; never committed).
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test nes_testroms -- --ignored --nocapture
//!
//! Two ways the ROMs report:
//! - the newer ones write $DE $B0 $61 to $6001–$6003, a status to $6000
//!   ($80 running, $81 "press reset", below $80 the result: 0 passed) and
//!   text from $6004;
//! - the 2005 ones keep a result code in zero page ($F8, or $F0 for the
//!   APU and PPU tests of 2005; 1 = passed) and stop.
//!
//! `EMU_NES_FILTER=text` runs the ROMs whose path contains it; pictures of
//! failing ROMs go to the test's temporary folder.

use std::path::{Path, PathBuf};

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, ResetKind, Size, StopReason};

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn root() -> PathBuf {
    let dir = std::env::var_os("EMU_TESTDATA_DIR")
        .expect("EMU_TESTDATA_DIR not set; run tools/fetch-testdata.sh first");
    PathBuf::from(dir).join("nes-test-roms")
}

fn machine(path: &Path) -> Box<dyn Machine> {
    let mut m = emu_machines::registry()
        .create("nes", &MachineConfig::default())
        .unwrap();
    let bytes = std::fs::read(path).unwrap();
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: path.display().to_string(),
            backend: Box::new(Bytes(bytes)),
        })
        .unwrap();
    m
}

fn peek(m: &mut dyn Machine, addr: u32) -> u8 {
    let cpu = m.devices()[0];
    m.debug_target(cpu)
        .unwrap()
        .peek(emu_core::MemSpaceId(0), addr, Size::Byte)
        .unwrap() as u8
}

fn text(m: &mut dyn Machine) -> String {
    let mut s = String::new();
    for a in 0x6004..0x7000 {
        let c = peek(m, a);
        if c == 0 {
            break;
        }
        s.push(c as char);
    }
    s.trim().replace('\n', " / ")
}

fn frames(m: &mut dyn Machine, n: usize) -> Result<(), String> {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            other => return Err(format!("stopped: {other:?}")),
        }
    }
    Ok(())
}

fn snapshot(m: &dyn Machine, path: &Path) {
    let name = path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .replace(['/', '\\', ' '], "_");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nes");
    std::fs::create_dir_all(&dir).unwrap();
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let png = emu_preview::encode_png(&m.framebuffer());
    std::fs::write(dir.join(format!("{parent}-{name}.png")), png).unwrap();
}

/// A ROM with the $6000 protocol: Ok(text) when it passed.
fn run_protocol(path: &Path) -> Result<String, String> {
    let mut m = machine(path);
    let mut reset_at = None;
    for step in 0..900 {
        frames(m.as_mut(), 4)?;
        let signature = [0x6001, 0x6002, 0x6003].map(|a| peek(m.as_mut(), a));
        if signature != [0xde, 0xb0, 0x61] {
            continue;
        }
        match peek(m.as_mut(), 0x6000) {
            0x80 => {}
            0x81 => {
                // Hold reset a moment after the request.
                let at = *reset_at.get_or_insert(step + 3);
                if step == at {
                    m.reset(ResetKind::Soft);
                    reset_at = None;
                }
            }
            0 => return Ok(text(m.as_mut())),
            code => {
                snapshot(m.as_ref(), path);
                return Err(format!("code {code}: {}", text(m.as_mut())));
            }
        }
    }
    snapshot(m.as_ref(), path);
    Err(format!("no result: {}", text(m.as_mut())))
}

/// A 2005 ROM with its result at `addr`.
fn run_result_byte(path: &Path, addr: u32) -> Result<String, String> {
    let mut m = machine(path);
    frames(m.as_mut(), 1500)?;
    match peek(m.as_mut(), addr) {
        1 => Ok(String::new()),
        code => {
            snapshot(m.as_ref(), path);
            Err(format!("result {code}"))
        }
    }
}

/// nestest in its automatic mode (start at $C000 without a PPU): $02 and
/// $03 hold the first failing official and unofficial test (0 = none).
fn run_nestest(path: &Path) -> Result<String, String> {
    let mut m = machine(path);
    // Power up with the cartridge, then start at $C000.
    m.step_cycle();
    for _ in 0..7 {
        m.step_cycle();
    }
    let cpu = m.devices()[0];
    m.debug_target(cpu).unwrap().set_reg("PC", 0xc000);
    // The last test returns with RTS to an empty stack and runs into a JAM.
    for _ in 0..100_000 {
        if matches!(m.step_cycle(), StopReason::CpuJam { .. }) {
            break;
        }
    }
    let (a, b) = (peek(m.as_mut(), 2), peek(m.as_mut(), 3));
    if (a, b) == (0, 0) {
        Ok(String::new())
    } else {
        Err(format!("$02 = {a:#04x}, $03 = {b:#04x}"))
    }
}

/// Every .nes file under `dir` (sorted).
fn roms(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(roms(&p));
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("nes")) {
            out.push(p);
        }
    }
    out.sort();
    out
}

#[derive(Clone, Copy)]
enum Kind {
    Protocol,
    ResultByte(u32),
    Nestest,
}

const SUITES: &[(&str, Kind)] = &[
    ("other/nestest.nes", Kind::Nestest),
    ("instr_test-v5", Kind::Protocol),
    ("instr_misc", Kind::Protocol),
    ("instr_timing", Kind::Protocol),
    ("cpu_interrupts_v2", Kind::Protocol),
    ("cpu_dummy_writes", Kind::Protocol),
    ("cpu_exec_space", Kind::Protocol),
    ("cpu_reset", Kind::Protocol),
    ("ppu_vbl_nmi", Kind::Protocol),
    ("ppu_open_bus", Kind::Protocol),
    ("ppu_read_buffer", Kind::Protocol),
    ("oam_read", Kind::Protocol),
    ("oam_stress", Kind::Protocol),
    ("apu_test", Kind::Protocol),
    ("apu_reset", Kind::Protocol),
    ("sprdma_and_dmc_dma", Kind::Protocol),
    ("mmc3_test_2", Kind::Protocol),
    ("sprite_hit_tests_2005.10.05", Kind::ResultByte(0xf8)),
    ("sprite_overflow_tests", Kind::ResultByte(0xf8)),
    ("vbl_nmi_timing", Kind::ResultByte(0xf8)),
    ("branch_timing_tests", Kind::ResultByte(0xf8)),
    ("blargg_ppu_tests_2005.09.15b", Kind::ResultByte(0xf0)),
    ("blargg_apu_2005.07.30", Kind::ResultByte(0xf0)),
];

/// ROMs expected to fail, with the reason.
const KNOWN: &[(&str, &str)] = &[
    (
        "mmc3_test_2/rom_singles/6-MMC3_alt.nes",
        "tests the MMC3A, but its iNES 1.0 header cannot say so (NES 2.0 submapper 4 selects it)",
    ),
    (
        "blargg_apu_2005.07.30/10.len_halt_timing.nes",
        "open: a halt write in the cycle of the length clock (apu_test passes)",
    ),
    (
        "blargg_apu_2005.07.30/11.len_reload_timing.nes",
        "open: a length reload in the cycle of the length clock (apu_test passes)",
    ),
];

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with nes-test-roms (tools/fetch-testdata.sh)"]
fn nes_test_roms() {
    let root = root();
    let filter = std::env::var("EMU_NES_FILTER").unwrap_or_default();
    let (mut passed, mut failed) = (0, Vec::new());
    for (suite, kind) in SUITES {
        let base = root.join(suite);
        let files = if base.is_file() {
            vec![base.clone()]
        } else {
            roms(&base)
        };
        for path in files {
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .display()
                .to_string()
                .replace('\\', "/");
            if !rel.contains(&filter) {
                continue;
            }
            // Protocol suites also hold the singles of their combined ROM.
            let result = match kind {
                Kind::Protocol => run_protocol(&path),
                Kind::ResultByte(a) => run_result_byte(&path, *a),
                Kind::Nestest => run_nestest(&path),
            };
            match result {
                Ok(t) => {
                    passed += 1;
                    eprintln!("ok    {rel} {t}");
                }
                Err(e) => {
                    if let Some((_, why)) = KNOWN.iter().find(|(k, _)| *k == rel) {
                        eprintln!("known {rel}: {e} ({why})");
                    } else {
                        eprintln!("FAIL  {rel}: {e}");
                        failed.push(rel);
                    }
                }
            }
        }
    }
    eprintln!("{passed} passed, {} failed", failed.len());
    assert!(failed.is_empty(), "failed: {failed:#?}");
}
