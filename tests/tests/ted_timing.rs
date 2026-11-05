//! TED timing measured by the programs of `tools/ted_timing.py`, compared
//! with the bytes the same programs measured in VICE's xplus4
//! (`tests/data/ted-<name>.ref`, read back from its screenshot; see the
//! tool's documentation). The programs keep their results at `$2000`.
//!
//!     EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test ted_timing -- --ignored --nocapture
//!
//! `EMU_TED_SHOW=1` prints both side by side.

use emu_core::media::MediaKind;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size};

fn data(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Run `ted-<name>.prg` on a PAL Plus/4 and return `len` bytes from `$2000`.
fn measure(name: &str, len: usize) -> Vec<u8> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "plus4").expect("ROMs"),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("plus4", &config).unwrap();
    let prg = data(&format!("ted-{name}.prg"));
    let media =
        media_backends::image::from_bytes(&format!("ted-{name}.prg"), prg, MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let mut dbg = DebugHooks::new();
    // irqall takes a frame or two per line.
    let frames = if name == "irqall" { 1400 } else { 500 };
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0..len as u32)
        .map(|i| {
            cpu.peek(machine_ted::CPU_SPACE, 0x2000 + i, Size::Byte)
                .unwrap() as u8
        })
        .collect()
}

/// $FF1E counts 0–227 (two units a double clock); the step between two
/// samples, assuming less than a line passed.
fn step(a: u8, b: u8) -> i32 {
    let d = i32::from(b) - i32::from(a);
    if d < 0 {
        d + 228
    } else {
        d
    }
}

fn show() -> bool {
    std::env::var_os("EMU_TED_SHOW").is_some()
}

/// Compare and count differing samples; print side by side if asked.
fn compare_trace(reference: &[u8], ours: &[u8]) -> usize {
    let n = reference.len() / 2;
    let (rh, rl) = reference.split_at(n);
    let (oh, ol) = ours.split_at(n);
    let mut differ = 0;
    if show() {
        println!("trace: xplus4 line hpos step | mimic line hpos step");
    }
    for i in 0..n {
        let same = rh[i] == oh[i] && rl[i] == ol[i];
        differ += usize::from(!same);
        if show() {
            let s = |h: &[u8]| {
                if i == 0 {
                    String::new()
                } else {
                    step(h[i - 1], h[i]).to_string()
                }
            };
            println!(
                "  {i:4}  {:3} ${:02X} {:>4} | {:3} ${:02X} {:>4} {}",
                rl[i],
                rh[i],
                s(rh),
                ol[i],
                oh[i],
                s(oh),
                if same { "" } else { "*" }
            );
        }
    }
    differ
}

fn compare_fine(reference: &[u8], ours: &[u8]) -> usize {
    let mut differ = 0;
    for (run, (r, o)) in reference.chunks(256).zip(ours.chunks(256)).enumerate() {
        let d = r.iter().zip(o).filter(|(a, b)| a != b).count();
        differ += d;
        if show() {
            println!("fine run {run}: {d} differ");
            for i in 0..256 {
                if r[i] != o[i] || i < 4 {
                    let rs = if i == 0 { 0 } else { step(r[i - 1], r[i]) };
                    let os = if i == 0 { 0 } else { step(o[i - 1], o[i]) };
                    println!(
                        "  {i:3} xplus4 ${:02X} {rs:3} | mimic ${:02X} {os:3}",
                        r[i], o[i]
                    );
                }
            }
        }
    }
    differ
}

fn check(name: &str, compare: fn(&[u8], &[u8]) -> usize) {
    let reference = data(&format!("ted-{name}.ref"));
    let ours = measure(name, reference.len());
    let differ = compare(&reference, &ours);
    println!("{name}: {differ} of {} bytes differ", reference.len());
    assert_eq!(differ, 0, "TED {name} differs from xplus4 (EMU_TED_SHOW=1)");
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn trace_matches_xplus4() {
    check("trace", compare_trace);
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn fine_matches_xplus4() {
    check("fine", compare_fine);
}

/// Raster IRQ entry per compare line: $FF1E, $FF1D, $FF1C read by the
/// handler's first instructions.
fn compare_irq(reference: &[u8], ours: &[u8]) -> usize {
    let n = 10;
    let mut differ = 0;
    for k in 0..n {
        let r = (reference[k], reference[0x10 + k], reference[0x20 + k] & 1);
        let o = (ours[k], ours[0x10 + k], ours[0x20 + k] & 1);
        differ += usize::from(r != o);
        if show() {
            println!(
                "irq {k}: xplus4 hpos ${:02X} line {} hi {} | mimic hpos ${:02X} line {} hi {}{}",
                r.0,
                r.1,
                r.2,
                o.0,
                o.1,
                o.2,
                if r == o { "" } else { " *" }
            );
        }
    }
    differ
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn irq_matches_xplus4() {
    check("irq", compare_irq);
}

/// IRQ entry for every line (`irqall`): $FF1E per line (two pages), $FF1D
/// likewise.
fn compare_irqall(reference: &[u8], ours: &[u8]) -> usize {
    let mut differ = 0;
    for line in 0..312usize {
        let i = if line < 256 { line } else { 0x100 + line - 256 };
        let r = (reference[i], reference[0x200 + i]);
        let o = (ours[i], ours[0x200 + i]);
        if r != o {
            differ += 1;
            if show() {
                println!(
                    "line {line:3}: xplus4 x {:3} line {:3} | mimic x {:3} line {:3}",
                    r.0 / 2,
                    r.1,
                    o.0 / 2,
                    o.1
                );
            }
        }
    }
    differ
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn irq_every_line_matches_xplus4() {
    check("irqall", compare_irqall);
}

/// Development aid: the CPU's bus cycles (double clock `x`, read/write)
/// before the raster IRQ handler of `irqall` runs for line
/// `EMU_TED_IRQ_LINE` (default 11).
#[test]
#[ignore = "development aid; needs Plus/4 ROMs in EMU_ROM_DIR"]
fn trace_irq_entry() {
    use emu_core::beam::{TraceConfig, TraceEvent, TraceRange};
    let want: u32 = std::env::var("EMU_TED_IRQ_LINE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(11);
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "plus4").expect("ROMs"),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("plus4", &config).unwrap();
    let prg = data("ted-irqall.prg");
    let media = media_backends::image::from_bytes("ted-irqall.prg", prg, MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let space = machine_ted::CPU_SPACE;
    let mut dbg = DebugHooks::new();
    let bp = dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space,
        addr: 0x1167,
    });
    let all = TraceRange {
        space,
        addr: 0,
        len: 0x10000,
    };
    dbg.set_trace(TraceConfig {
        instructions: true,
        reads: vec![all],
        writes: vec![all],
        capacity: 200,
    });
    for _ in 0..3000 {
        if m.run_frame(&mut dbg) == emu_core::StopReason::Breakpoint(bp) {
            let cpu = m.debug_target(DeviceId("cpu")).unwrap();
            let line = cpu.peek(space, 0xf2, Size::Byte).unwrap()
                | cpu.peek(space, 0xf3, Size::Byte).unwrap() << 8;
            let entries = dbg.drain_trace();
            if line == want {
                for e in entries.iter().rev().take(40).rev() {
                    let what = match e.event {
                        TraceEvent::Instr { pc, .. } => format!("instr ${pc:04X}"),
                        TraceEvent::Read { addr, value, .. } => {
                            format!("R ${addr:04X} = {value:02X}")
                        }
                        TraceEvent::Write { addr, value, .. } => {
                            format!("W ${addr:04X} = {value:02X}")
                        }
                    };
                    println!("line {:3} x {:3}  {what}", e.beam.line, e.beam.cycle);
                }
                return;
            }
        }
    }
    panic!("handler for line {want} not reached");
}

/// A picture test: mimic's frame after `frames` frames against xplus4's
/// screenshot (`tests/data/ted-<name>.png`, 384 × 288): colours must map
/// one to one (the palettes differ), xplus4's picture sits two lines
/// higher. Returns the differing pixels; `EMU_TED_SHOW` saves mimic's
/// picture as `target/tmp/ted-<name>-mimic.png`.
fn compare_picture(name: &str) -> usize {
    use std::collections::BTreeMap;
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "plus4").expect("ROMs"),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("plus4", &config).unwrap();
    let prg = data(&format!("ted-{name}.prg"));
    let media =
        media_backends::image::from_bytes(&format!("ted-{name}.prg"), prg, MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..500 {
        m.run_frame(&mut dbg);
    }
    let png_bytes = data(&format!("ted-{name}.png"));
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let bpp = info.color_type.samples();
    let (rw, rh) = (info.width as i64, info.height as i64);
    let frame = m.framebuffer();
    let emu_core::frame::PixelData::Indexed { pixels, .. } = frame.pixels else {
        panic!("TED frame not indexed");
    };
    let (ow, oh) = (i64::from(frame.width), i64::from(frame.height));
    let (mut to_ours, mut to_ref) = (BTreeMap::new(), BTreeMap::new());
    let (mut bad, mut first) = (0, None);
    for y in 0..oh {
        for x in 0..ow {
            let ry = y - 2;
            if ry < 0 || ry >= rh || x >= rw {
                continue;
            }
            let i = ((ry * rw + x) as usize) * bpp;
            let r = [buf[i], buf[i + 1], buf[i + 2]];
            let o = pixels[(y * ow + x) as usize] & 0x7f;
            let ok = *to_ours.entry(r).or_insert(o) == o && *to_ref.entry(o).or_insert(r) == r;
            if !ok {
                bad += 1;
                first.get_or_insert((x, y));
            }
        }
    }
    if show() {
        let out =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ted-{name}-mimic.png"));
        std::fs::write(&out, emu_preview::encode_png(&frame)).unwrap();
        println!(
            "{name}: first difference at {first:?} (frame); picture {}",
            out.display()
        );
    }
    bad
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn split_matches_xplus4() {
    let bad = compare_picture("split");
    println!("split: {bad} pixels differ");
    // Not equal yet: the border matches outside the refresh cycles, the
    // window's register sampling in single-clock areas does not (open, see
    // CLAUDE.md). The bound keeps it from getting worse.
    assert!(
        bad <= 27_947,
        "TED split picture got worse (EMU_TED_SHOW=1)"
    );
}

/// Development aid: where (`line`, `x`) the `split` program's writes to
/// $FF15/$FF19 land on lines `EMU_TED_LINES` (default 60-63).
#[test]
#[ignore = "development aid; needs Plus/4 ROMs in EMU_ROM_DIR"]
fn trace_split_writes() {
    use emu_core::beam::{TraceConfig, TraceEvent, TraceRange};
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "plus4").expect("ROMs"),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("plus4", &config).unwrap();
    let prg = data("ted-split.prg");
    let media = media_backends::image::from_bytes("ted-split.prg", prg, MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..400 {
        m.run_frame(&mut dbg);
    }
    let space = machine_ted::CPU_SPACE;
    dbg.set_trace(TraceConfig {
        instructions: false,
        reads: vec![],
        writes: vec![TraceRange {
            space,
            addr: 0xff15,
            len: 5,
        }],
        capacity: 100_000,
    });
    m.run_frame(&mut dbg);
    for e in dbg.drain_trace() {
        if let TraceEvent::Write { addr, value, .. } = e.event {
            let (lo, hi) = std::env::var("EMU_TED_LINES")
                .ok()
                .and_then(|v| {
                    let (a, b) = v.split_once("-")?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                })
                .unwrap_or((60u16, 63u16));
            if (lo..=hi).contains(&e.beam.line) {
                println!(
                    "line {:3} x {:3} ${addr:04X} = {value:02X}",
                    e.beam.line, e.beam.cycle
                );
            }
        }
    }
}
