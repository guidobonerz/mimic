//! SNES test ROMs: gilyon's snes-tests (MIT licence,
//! https://github.com/gilyon/snes-tests, release v1.4 fetched into
//! `$EMU_TESTDATA_DIR/snes-tests` by `tools/fetch-testdata.sh`; never
//! committed): `cputest-basic`, `cputest-full` (every 65C816 opcode in all
//! addressing modes, emulation-mode wrapping, decimal mode) and `spctest`
//! (the SPC700 instruction set).
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test snes_tests -- --ignored --nocapture
//!
//! The ROMs write "Success" or "Fail" as text to video RAM word `$0032`
//! (one character per word, the tile number its ASCII code) and the
//! current test number in hex to word `$006E`; the failing test's inputs
//! and expected results are in the release's `tests-*.txt`. With
//! `EMU_ROM_DIR` the user's SPC700 IPL ROM is used, otherwise the
//! machine's own loader.

use std::path::PathBuf;

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Size};

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
    PathBuf::from(dir).join("snes-tests")
}

fn machine(rom: &str) -> Box<dyn Machine> {
    let mut config = MachineConfig::default();
    if let Some(dir) = media_backends::roms::env_dir() {
        if let Ok(roms) = media_backends::roms::load(&dir, "snes") {
            config.roms = roms;
        }
    }
    let mut m = emu_machines::registry().create("snes", &config).unwrap();
    let bytes = std::fs::read(root().join(rom)).unwrap();
    m.media_slot(machine_snes::SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: rom.into(),
            backend: Box::new(Bytes(bytes)),
        })
        .unwrap();
    m
}

/// Text from video RAM: `n` words from word address `word`, low bytes.
fn vram_text(m: &mut dyn Machine, word: u32, n: u32) -> String {
    let cpu = m.devices()[0];
    let t = m.debug_target(cpu).unwrap();
    (0..n)
        .map(|i| {
            t.peek(machine_snes::VRAM_SPACE, (word + i) * 2, Size::Byte)
                .unwrap_or(0) as u8
        })
        .filter(|&c| c != 0)
        .map(|c| c as char)
        .collect()
}

/// Run `rom` until it reports; returns its verdict text and test number.
fn run(rom: &str, max_frames: u32) -> (String, String) {
    let mut m = machine(rom);
    let mut dbg = DebugHooks::new();
    for frame in 0..max_frames {
        m.run_frame(&mut dbg);
        if frame % 30 == 0 {
            let verdict = vram_text(m.as_mut(), 0x32, 7);
            if verdict.starts_with("Success") || verdict.starts_with("Fail") {
                return (verdict, vram_text(m.as_mut(), 0x6e, 4));
            }
        }
    }
    ("timeout".into(), vram_text(m.as_mut(), 0x6e, 4))
}

fn check(rom: &str, max_frames: u32) {
    let (verdict, test) = run(rom, max_frames);
    println!("{rom}: {verdict} (test {test})");
    assert!(
        verdict.starts_with("Success"),
        "{rom}: {verdict} at test ${test} (see tests-*.txt)"
    );
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with snes-tests"]
fn cputest_basic() {
    check("cputest/cputest-basic.sfc", 20_000);
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with snes-tests"]
fn cputest_full() {
    check("cputest/cputest-full.sfc", 40_000);
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with snes-tests"]
fn spctest() {
    check("spctest/spctest.sfc", 40_000);
}
