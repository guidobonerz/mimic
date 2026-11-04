//! Development aid for the VICE `drive/viavarious` tests: runs one of them
//! on the C64 with a true 1541 and prints, per sub test, the first bytes
//! the drive measured next to the reference from `viaNref.bin`.
//!
//!     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata EMU_VIA_TEST=via1 cargo test -p emu-tests --release --test via_dump -- --ignored --nocapture

use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Size, StopReason};
use emu_tests::lorenz;

#[test]
#[ignore = "development aid"]
fn dump_via_test() {
    let Ok(name) = std::env::var("EMU_VIA_TEST") else {
        return;
    };
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs/drive/viavarious");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let prg = std::fs::read(data.join(format!("{name}.prg"))).unwrap();
    let reference = std::fs::read(data.join(format!("{name}ref.bin"))).unwrap();
    let reference = &reference[2..];
    let tests = reference.len() / 256;
    let mut dbg = DebugHooks::new();
    for _ in 0..lorenz::BOOT_FRAMES {
        m.run_frame(&mut dbg);
    }
    lorenz::load_and_run(m.as_mut(), &prg);
    let cpu = lorenz::space(m.as_ref(), "cpu");
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0xd7ff,
        len: 1,
    });
    let limit: u64 = std::env::var("EMU_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400_000_000);
    while m.now().0 < limit {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            StopReason::Watchpoint { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    let t = m.debug_target(machine_c64::CPU).unwrap();
    let mut all_ok = true;
    for i in 0..tests {
        let ours: Vec<u8> = (0..256)
            .map(|j| {
                t.peek(cpu, 0x8000 + i as u32 * 256 + j, Size::Byte)
                    .unwrap() as u8
            })
            .collect();
        let refs = &reference[i * 256..i * 256 + 256];
        let ok = ours == refs;
        all_ok &= ok;
        let first = ours.iter().zip(refs).position(|(a, b)| a != b);
        println!(
            "test {}: {}{}",
            (b'a' + i as u8) as char,
            if ok { "ok" } else { "DIFF" },
            first.map_or(String::new(), |f| format!(" from byte {f}"))
        );
        if !ok || std::env::var_os("EMU_ALL").is_some() {
            let n: usize = std::env::var("EMU_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(40);
            let from = first.unwrap_or(0).saturating_sub(4);
            println!("  ours {:02x?}", &ours[from..(from + n).min(256)]);
            println!("  ref  {:02x?}", &refs[from..(from + n).min(256)]);
        }
    }
    println!(
        "{name}: {}",
        if all_ok {
            "all sub tests match"
        } else {
            "differences"
        }
    );
}
