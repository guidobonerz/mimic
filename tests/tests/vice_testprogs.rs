//! The VICE test programs on the C64, run like VICE's testbench runs them
//! (`testprogs/testbench/c64-testlist.in`, `testbench.sh`, `x64sc-hooks.sh`,
//! `cmpscreens.c`; https://sourceforge.net/p/vice-emu/code/HEAD/tree/testprogs/).
//!
//!     python tools/fetch_vice_testprogs.py
//!     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata \
//!         cargo test -p emu-tests --release --test vice_testprogs -- --ignored --nocapture
//!
//! `EMU_VICE_FILTER=CIA/` runs only entries whose path contains the text.
//! A stocktaking, not a gate: the report (one line per entry, then a
//! summary per directory) goes to `target/tmp/vice-testprogs.txt`
//! (`vice-testprogs-filtered.txt` with a filter); `EMU_VICE_DUMP=1` also
//! saves our picture of each failed screenshot entry in
//! `target/tmp/vice-testprogs/`.
//!
//! Each list entry is: directory, program, kind (`exitcode`, `screenshot`,
//! `interactive`, `analyzer`), cycle limit, options. Like the testbench's
//! x64sc (`-model c64c`), the machine defaults to a 6526A and an 8580 and
//! the screenshot references of the 8565. The program writes its result
//! to the debug cartridge at `$D7FF` ($00 passed, $FF failed); VICE's
//! `-limitcycles` makes a timeout. Screenshot entries compare the picture
//! at that moment with `references/<program>[-<VIC-II>].png` from the
//! display window's corner (VICE: 32,35 PAL, 32,23 NTSC), colour by colour
//! after mapping both to palette indices (Pepto's PAL and NTSC palettes,
//! https://www.pepto.de/projects/colorvic/).
//!
//! Not run: interactive and analyzer entries, memory expansions (REU,
//! GEO-RAM, +60K, +256K, DQBB, RAMCart, Isepic) and the 6567R56A
//! (`vicii-ntscold`); there is one VIC-II per standard, so `vicii-old` and
//! `vicii-new` only choose the reference picture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use emu_core::frame::{PixelData, VideoStandard};
use emu_core::media::MediaKind;
use emu_core::registry::MachineConfig;
use emu_core::{BreakpointKind, DebugHooks, Machine, StopReason};
use emu_tests::lorenz;

/// Pepto's PAL palette (the testbench's reference screenshots use it).
const PEPTO_PAL: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0xff, 0xff, 0xff],
    [0x68, 0x37, 0x2b],
    [0x70, 0xa4, 0xb2],
    [0x6f, 0x3d, 0x86],
    [0x58, 0x8d, 0x43],
    [0x35, 0x28, 0x79],
    [0xb8, 0xc7, 0x6f],
    [0x6f, 0x4f, 0x25],
    [0x43, 0x39, 0x00],
    [0x9a, 0x67, 0x59],
    [0x44, 0x44, 0x44],
    [0x6c, 0x6c, 0x6c],
    [0x9a, 0xd2, 0x84],
    [0x6c, 0x5e, 0xb5],
    [0x95, 0x95, 0x95],
];

#[derive(Clone, Debug)]
struct Entry {
    /// Directory relative to `testprogs/`.
    dir: String,
    prg: String,
    kind: String,
    cycles: u64,
    options: Vec<String>,
}

impl Entry {
    fn name(&self) -> String {
        let prg = if self.prg.is_empty() {
            self.mounted().unwrap_or_default()
        } else {
            self.prg.clone()
        };
        let opts: Vec<&str> = self
            .options
            .iter()
            .map(String::as_str)
            .filter(|o| !o.starts_with("comment:") && !o.starts_with("warn:"))
            .collect();
        format!("{}{} {} {}", self.dir, prg, self.kind, opts.join(","))
    }

    fn has(&self, opt: &str) -> bool {
        self.options.iter().any(|o| o == opt)
    }

    fn mounted(&self) -> Option<String> {
        self.options
            .iter()
            .find_map(|o| o.strip_prefix("mountcrt:").or(o.strip_prefix("mountd64:")))
            .map(str::to_string)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Pass,
    Fail(String),
    Timeout,
    Skipped(String),
    Error(String),
}

fn parse(list: &str) -> Vec<Entry> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            if f.len() < 4 {
                return None;
            }
            // Directories are relative to testprogs/testbench.
            let dir = f[0].trim();
            let dir = dir
                .strip_prefix("../")
                .map(str::to_string)
                .unwrap_or_else(|| format!("testbench/{}", dir.trim_start_matches("./")));
            let dir = if dir.ends_with('/') {
                dir
            } else {
                format!("{dir}/")
            };
            Some(Entry {
                dir,
                prg: f[1].trim().to_string(),
                kind: f[2].trim().to_string(),
                cycles: f[3].trim().parse().unwrap_or(0),
                options: f[4..].iter().map(|o| o.trim().to_string()).collect(),
            })
        })
        .collect()
}

const UNSUPPORTED: &[&str] = &[
    "reu",
    "geo",
    "plus60k",
    "plus256k",
    "dqbb",
    "ramcart",
    "isepic",
    "vicii-ntscold",
];

/// Machine configuration for an entry, or why it is not run.
fn config(base: &MachineConfig, e: &Entry) -> Result<MachineConfig, String> {
    if e.kind != "exitcode" && e.kind != "screenshot" {
        return Err(e.kind.clone());
    }
    let mut c = base.clone();
    c.options.insert("debugcart".into(), "on".into());
    c.options.insert("cia".into(), "6526a".into());
    c.options.insert("sid".into(), "8580".into());
    for o in &e.options {
        if let Some(u) = UNSUPPORTED.iter().find(|u| o.starts_with(*u)) {
            return Err(format!("{u} not emulated"));
        }
        match o.as_str() {
            "vicii-pal" => c.model = Some("pal".into()),
            "vicii-ntsc" => c.model = Some("ntsc".into()),
            "cia-old" => {
                c.options.insert("cia".into(), "6526".into());
            }
            "cia-new" => {
                c.options.insert("cia".into(), "6526a".into());
            }
            "sid-old" => {
                c.options.insert("sid".into(), "6581".into());
            }
            "sid-new" => {
                c.options.insert("sid".into(), "8580".into());
            }
            _ => {}
        }
    }
    if e.prg.is_empty() && e.options.iter().any(|o| o.starts_with("mountd64:")) {
        c.options.insert("autostart".into(), "on".into());
    }
    Ok(c)
}

fn insert(m: &mut dyn Machine, kind: MediaKind, path: &Path) -> Result<(), String> {
    let media = match kind {
        MediaKind::Cartridge => media_backends::image::open_cart(path),
        _ => media_backends::image::open(path),
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == kind)
        .ok_or("no slot")?
        .id;
    m.media_slot(slot)
        .ok_or("no slot")?
        .insert(media)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The debug cartridge's exit code, once written.
fn exit_code(m: &dyn Machine) -> Option<u8> {
    let chip = m.chips().into_iter().find(|c| c.name == "debugcart")?;
    let reg = |n: &str| chip.regs.iter().find(|r| r.name == n).map(|r| r.value);
    (reg("WRITTEN")? != 0).then(|| reg("EXIT").unwrap_or(0) as u8)
}

/// Run one entry: `Ok(None)` timeout, `Ok(Some(code))` exit code.
fn run(m: &mut dyn Machine, e: &Entry, data: &Path) -> Result<Option<u8>, String> {
    let dir = data.join(&e.dir);
    for o in &e.options {
        if let Some(f) = o.strip_prefix("mountcrt:") {
            insert(m, MediaKind::Cartridge, &dir.join(f))?;
        } else if let Some(f) = o
            .strip_prefix("mountd64:")
            .or(o.strip_prefix("mountg64:"))
            .or(o.strip_prefix("mountp64:"))
        {
            insert(m, MediaKind::Disk, &dir.join(f))?;
        }
    }
    let mut dbg = DebugHooks::new();
    if !e.prg.is_empty() {
        let prg = std::fs::read(dir.join(&e.prg)).map_err(|_| "program missing".to_string())?;
        if prg.len() < 3 {
            return Err("program too short".into());
        }
        for _ in 0..lorenz::BOOT_FRAMES {
            m.run_frame(&mut dbg);
        }
        lorenz::load_and_run(m, &prg);
    }
    let cpu = lorenz::space(m, "cpu");
    dbg.add_breakpoint(BreakpointKind::Write {
        space: cpu,
        addr: 0xd7ff,
        len: 1,
    });
    while m.now().0 < e.cycles {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            StopReason::Watchpoint { .. } => {
                if let Some(code) = exit_code(m) {
                    // The picture of the frame being drawn, completed.
                    if e.kind == "screenshot" {
                        dbg = DebugHooks::new();
                        while m.run_frame(&mut dbg) != StopReason::FrameDone {}
                    }
                    return Ok(Some(code));
                }
            }
            // A jammed CPU stays jammed until the cycle limit, as in VICE.
            StopReason::CpuJam { .. } => return Ok(None),
            other => return Err(format!("{other:?}")),
        }
    }
    Ok(None)
}

/// The reference picture the testbench would choose.
fn reference(e: &Entry, data: &Path, ntsc: bool) -> Option<PathBuf> {
    let refs = data.join(&e.dir).join("references");
    let test = if e.prg.is_empty() {
        e.mounted()?
    } else {
        e.prg.clone()
    };
    let subtype = match (ntsc, e.has("vicii-old")) {
        (false, false) => "8565",
        (false, true) => "6569",
        (true, false) => "8562",
        (true, true) => "6567",
    };
    let mut names = vec![format!("{test}-{subtype}.png")];
    if ntsc {
        names.push(format!("{test}-ntsc.png"));
    }
    names.push(format!("{test}.png"));
    names
        .into_iter()
        .map(|n| refs.join(n))
        .find(|p| p.is_file())
}

/// Palette index of an RGB colour: nearest Pepto colour.
fn index_of(c: [u8; 3]) -> u8 {
    let d = |p: &[u8; 3]| -> i32 {
        (0..3)
            .map(|i| (i32::from(p[i]) - i32::from(c[i])).pow(2))
            .sum()
    };
    (0..16).min_by_key(|&i| d(&PEPTO_PAL[i])).unwrap() as u8
}

fn load_png(path: &Path) -> Result<(u32, u32, Vec<[u8; 3]>), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(file);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let bpp = info.color_type.samples();
    let px = buf[..info.buffer_size()]
        .chunks(bpp)
        .map(|p| match bpp {
            1 | 2 => [p[0]; 3],
            _ => [p[0], p[1], p[2]],
        })
        .collect();
    Ok((info.width, info.height, px))
}

/// Compare like `cmpscreens`: align both pictures at their offsets and
/// check the overlapping area.
fn compare(m: &dyn Machine, reference: &Path) -> Result<(), String> {
    let (rw, rh, rpx) = load_png(reference)?;
    let ref_ntsc = reference.to_string_lossy().contains("ntsc")
        || reference.to_string_lossy().contains("-8562")
        || reference.to_string_lossy().contains("-6567");
    let (rx, ry) = if ref_ntsc { (32, 23) } else { (32, 35) };
    let frame = m.framebuffer();
    let (ow, oh) = (frame.width as i64, frame.height as i64);
    // Our frame: the display window starts 32 pixels in; PAL on line 35,
    // NTSC on line 10 (the 6567R8 window begins at raster line 41).
    let (ox, oy) = match frame.signal.standard {
        VideoStandard::Ntsc => (32, 10),
        _ => (32, 35),
    };
    let ours = |x: i64, y: i64| -> u8 {
        match frame.pixels {
            // The VIC-II's pixels are its colour numbers.
            PixelData::Indexed { pixels, .. } => pixels[(y * ow + x) as usize] & 0x0f,
            PixelData::Rgb(px) => {
                let c = px[(y * ow + x) as usize];
                index_of([c.r, c.g, c.b])
            }
        }
    };
    let x0 = -(rx.min(ox) as i64);
    let y0 = -(ry.min(oy) as i64);
    let x1 = (rw as i64 - rx as i64).min(ow - ox as i64);
    let y1 = (rh as i64 - ry as i64).min(oh - oy as i64);
    let mut bad = 0;
    let mut first = None;
    for y in y0..y1 {
        for x in x0..x1 {
            let r = index_of(rpx[((y + ry as i64) * rw as i64 + x + rx as i64) as usize]);
            let o = ours(x + ox as i64, y + oy as i64);
            if r != o {
                bad += 1;
                first.get_or_insert((x, y, r, o));
            }
        }
    }
    match first {
        None => Ok(()),
        Some((x, y, r, o)) => Err(format!(
            "{bad} pixels differ, first at {x},{y} (window) ref {r} ours {o}"
        )),
    }
}

/// `EMU_VICE_DUMP`: our picture of a failed screenshot entry, next to
/// the report in `target/tmp/vice-testprogs/`.
fn dump(m: &dyn Machine, e: &Entry) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("vice-testprogs");
    std::fs::create_dir_all(&dir).unwrap();
    let name = e.name().replace(['/', ' ', ',', ':'], "_");
    let frame = m.framebuffer();
    let mut rgb = Vec::with_capacity((frame.width * frame.height * 3) as usize);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let c = frame.rgb_at(x, y).unwrap_or_default();
            rgb.extend([c.r, c.g, c.b]);
        }
    }
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut enc = png::Encoder::new(file, frame.width, frame.height);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

fn judge(base: &MachineConfig, e: &Entry, data: &Path) -> Verdict {
    let config = match config(base, e) {
        Ok(c) => c,
        Err(why) => return Verdict::Skipped(why),
    };
    if !e.prg.is_empty() && !data.join(&e.dir).join(&e.prg).is_file() {
        return Verdict::Skipped("program not in the repository".into());
    }
    let registry = emu_machines::registry();
    let mut m = match registry.create("c64", &config) {
        Ok(m) => m,
        Err(err) => return Verdict::Error(format!("{err:?}")),
    };
    let expect_error = e.has("expect:error");
    let expect_timeout = e.has("expect:timeout");
    let outcome = run(m.as_mut(), e, data);
    let result = match outcome {
        Err(why) => Verdict::Error(why),
        Ok(None) => Verdict::Timeout,
        Ok(Some(code)) if e.kind == "screenshot" => {
            let ntsc = config.model.as_deref() == Some("ntsc");
            match reference(e, data, ntsc) {
                None => Verdict::Skipped("reference picture missing".into()),
                Some(r) => match compare(m.as_ref(), &r) {
                    Ok(()) => Verdict::Pass,
                    Err(why) => Verdict::Fail(format!("exit ${code:02x}, {why}")),
                },
            }
        }
        Ok(Some(0)) => Verdict::Pass,
        Ok(Some(code)) => Verdict::Fail(format!("exit ${code:02x}")),
    };
    if e.kind == "screenshot" && std::env::var_os("EMU_VICE_DUMP").is_some() {
        if let Verdict::Fail(_) = result {
            dump(m.as_ref(), e);
        }
    }
    match (expect_error, expect_timeout, result) {
        (true, _, Verdict::Pass) => Verdict::Fail("expected an error".into()),
        (true, _, Verdict::Fail(_) | Verdict::Timeout) => Verdict::Pass,
        (_, true, Verdict::Timeout) => Verdict::Pass,
        (_, true, Verdict::Pass | Verdict::Fail(_)) => Verdict::Fail("expected a timeout".into()),
        (_, _, r) => r,
    }
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR (tools/fetch_vice_testprogs.py)"]
fn vice_testprogs_c64() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let base = MachineConfig {
        roms: media_backends::roms::load(&rom_dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    let data = PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
        .join("vice-testprogs");
    let list = std::fs::read_to_string(data.join("testbench/c64-testlist.in"))
        .expect("run tools/fetch_vice_testprogs.py");
    let filter = std::env::var("EMU_VICE_FILTER").unwrap_or_default();
    let entries: Vec<Entry> = parse(&list)
        .into_iter()
        .filter(|e| e.name().contains(&filter))
        .collect();

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Verdict)>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(e) = entries.get(i) else { break };
                let v = judge(&base, e, &data);
                results.lock().unwrap().push((i, v));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);

    // Summary per directory (two levels, e.g. "VICII/border").
    let mut groups: BTreeMap<String, [usize; 4]> = BTreeMap::new();
    let mut report = String::new();
    for (i, v) in &results {
        let e = &entries[*i];
        let group: String = e.dir.split('/').take(2).collect::<Vec<_>>().join("/");
        let g = groups.entry(group).or_default();
        let (col, text) = match v {
            Verdict::Pass => (0, "pass".to_string()),
            Verdict::Fail(w) => (1, format!("FAIL {w}")),
            Verdict::Timeout => (1, "FAIL timeout".to_string()),
            Verdict::Error(w) => (2, format!("ERROR {w}")),
            Verdict::Skipped(w) => (3, format!("skipped ({w})")),
        };
        g[col] += 1;
        report.push_str(&format!("{:<70} {text}\n", e.name()));
    }
    let mut total = [0; 4];
    report.push_str("\ndirectory                                  pass  fail error  skip\n");
    for (g, n) in &groups {
        report.push_str(&format!(
            "{g:<40} {:>6} {:>5} {:>5} {:>5}\n",
            n[0], n[1], n[2], n[3]
        ));
        for k in 0..4 {
            total[k] += n[k];
        }
    }
    report.push_str(&format!(
        "{:<40} {:>6} {:>5} {:>5} {:>5}\n",
        "total", total[0], total[1], total[2], total[3]
    ));
    let report_name = if filter.is_empty() {
        "vice-testprogs.txt"
    } else {
        "vice-testprogs-filtered.txt"
    };
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(report_name);
    std::fs::write(&out, &report).unwrap();
    let run = total[0] + total[1] + total[2];
    println!(
        "VICE testprogs: {}/{} passed ({:.1} %), {} failed, {} errors, {} skipped; report: {}",
        total[0],
        run,
        100.0 * total[0] as f64 / run.max(1) as f64,
        total[1],
        total[2],
        total[3],
        out.display()
    );
}
