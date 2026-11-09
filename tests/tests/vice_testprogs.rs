//! The VICE test programs on the C64, run like VICE's testbench runs them
//! (`testprogs/testbench/c64-testlist.in`, `testbench.sh`, `x64sc-hooks.sh`,
//! `cmpscreens.c`; https://sourceforge.net/p/vice-emu/code/HEAD/tree/testprogs/).
//!
//!     python tools/fetch_vice_testprogs.py
//!     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata \
//!         cargo test -p emu-tests --release --test vice_testprogs -- --ignored --nocapture
//!
//! `EMU_VICE_FILTER=CIA/` runs only entries whose path contains the text,
//! `EMU_VICE_SKIP=zex128` leaves out those that contain it.
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
//!
//! `vice_testprogs_vic20` runs `vic20-testlist.in` the same way on the
//! VIC-20 (`python tools/fetch_vice_testprogs.py --list vic20`): the
//! program goes into the program slot (the machine injects it after boot
//! and types RUN, like VICE's autostart), `vic20-unexp`/`vic20-8k`/
//! `vic20-32k` set the RAM, the exit code is written to `$910F`. Its
//! screenshot entries are not compared yet (no VIC palette and window
//! mapping for xvic's references); report `vice-testprogs-vic20.txt`.

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

/// The machine a list runs on.
#[derive(Clone, Copy)]
struct Target {
    machine: &'static str,
    list: &'static str,
    /// Where the test programs write their exit code.
    exit_addr: u32,
    /// Report and trend names.
    name: &'static str,
}

const C64: Target = Target {
    machine: "c64",
    list: "testbench/c64-testlist.in",
    exit_addr: 0xd7ff,
    name: "vice-testprogs-c64",
};

const VIC20: Target = Target {
    machine: "vic20",
    list: "testbench/vic20-testlist.in",
    exit_addr: 0x910f,
    name: "vice-testprogs-vic20",
};

const PLUS4: Target = Target {
    machine: "plus4",
    list: "testbench/plus4-testlist.in",
    exit_addr: 0xfdcf,
    name: "vice-testprogs-plus4",
};

const C128: Target = Target {
    machine: "c128",
    list: "testbench/c128-testlist.in",
    exit_addr: 0xd7ff,
    name: "vice-testprogs-c128",
};

const PET: Target = Target {
    machine: "pet",
    list: "testbench/pet-testlist.in",
    exit_addr: 0x8bff,
    name: "vice-testprogs-pet",
};

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
fn config(t: Target, base: &MachineConfig, e: &Entry) -> Result<MachineConfig, String> {
    if e.kind != "exitcode" && e.kind != "screenshot" {
        return Err(e.kind.clone());
    }
    let mut c = base.clone();
    c.options.insert("debugcart".into(), "on".into());
    if t.machine == "plus4" {
        // A 1551 as drive 8: its interface cartridge's TIA is what
        // `Plus4/tcbm` tests (without it $FEC0-$FEFF is open bus).
        if base.roms.contains_key("dos1551") {
            c.options.insert("drive8".into(), "1551".into());
        }
        for o in &e.options {
            if let Some(u) = UNSUPPORTED.iter().find(|u| o.starts_with(*u)) {
                return Err(format!("{u} not emulated"));
            }
        }
        return Ok(c);
    }
    if t.machine == "c128" {
        // x128's defaults: PAL, the 8580 is not what x128 has by default
        // but the entries name the SID they need.
        for o in &e.options {
            if let Some(u) = UNSUPPORTED.iter().find(|u| o.starts_with(*u)) {
                return Err(format!("{u} not emulated"));
            }
            match o.as_str() {
                "vicii-ntsc" | "vicii-ntscold" => {
                    c.video_format = Some(emu_core::frame::VideoStandard::Ntsc);
                }
                "sid-old" => {
                    c.options.insert("sid".into(), "6581".into());
                }
                "sid-new" => {
                    c.options.insert("sid".into(), "8580".into());
                }
                // A D64 goes into a 1541 (c128/burstmode burstcheck0: no
                // fast serial then), a D71 into the default 1571.
                o if o.starts_with("mountd64:") && c.roms.contains_key("dos1541") => {
                    c.options.insert("drive8".into(), "1541".into());
                }
                _ => {}
            }
        }
        return Ok(c);
    }
    if t.machine == "pet" {
        // xpet runs the list as an 8096 (an 8032 with 64 KB more RAM,
        // which no entry uses): our 8032 with its 50 Hz editor.
        c.model = Some("8032".into());
        return Ok(c);
    }
    if t.machine == "vic20" {
        if e.kind == "screenshot" {
            return Err("VIC-20 screenshots not compared".into());
        }
        for o in &e.options {
            if let Some(u) = UNSUPPORTED.iter().find(|u| o.starts_with(*u)) {
                return Err(format!("{u} not emulated"));
            }
            let ram = match o.as_str() {
                "vic20-unexp" => "none",
                "vic20-3k" => "3k",
                "vic20-8k" => "8k",
                "vic20-16k" => "16k",
                "vic20-24k" => "24k",
                "vic20-32k" => "all",
                _ => continue,
            };
            c.options.insert("ram".into(), ram.into());
        }
        return Ok(c);
    }
    c.options.insert("cia".into(), "6526a".into());
    c.options.insert("sid".into(), "8580".into());
    // The 8565/8562 like the testbench's C64C, unless the entry asks for
    // the old type (the reference pictures follow the same choice).
    let old_vic = e.has("vicii-old");
    for o in &e.options {
        if let Some(u) = UNSUPPORTED.iter().find(|u| o.starts_with(*u)) {
            return Err(format!("{u} not emulated"));
        }
        match o.as_str() {
            "vicii-ntsc" => c.video_format = Some(emu_core::frame::VideoStandard::Ntsc),
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
    let ntsc = c.video_format == Some(emu_core::frame::VideoStandard::Ntsc);
    let vic = match (ntsc, old_vic) {
        (false, false) => "8565",
        (false, true) => "6569",
        (true, false) => "8562",
        (true, true) => "6567",
    };
    c.options.insert("vicii".into(), vic.into());
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

/// Type `text` on the keyboard; with `ready` run until BASIC shows another
/// "READY." (or 1500 frames pass), as a LOAD from the drive takes seconds.
fn type_and_wait(m: &mut dyn Machine, dbg: &mut DebugHooks, text: &str, ready_prompt: bool) {
    let ready = |m: &mut dyn Machine| -> usize {
        let cpu = lorenz::space(m, "cpu");
        let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
        let screen: Vec<u8> = (0..1000)
            .map(|i| t.peek(cpu, 0x400 + i, emu_core::Size::Byte).unwrap_or(0) as u8)
            .collect();
        screen
            .windows(6)
            .filter(|w| *w == [18, 5, 1, 4, 25, 46])
            .count()
    };
    let before = ready(m);
    let (events, _) = machine_c64::keyboard::typed_events(text, m.now().0);
    for ev in events {
        m.input(ev);
    }
    if ready_prompt {
        for _ in 0..1500 {
            m.run_frame(dbg);
            if ready(m) > before {
                break;
            }
        }
    }
    // Let the keys go through before the next line.
    for _ in 0..5 {
        m.run_frame(dbg);
    }
}

/// The debug cartridge's exit code, once written.
fn exit_code(m: &dyn Machine) -> Option<u8> {
    let chip = m.chips().into_iter().find(|c| c.name == "debugcart")?;
    let reg = |n: &str| chip.regs.iter().find(|r| r.name == n).map(|r| r.value);
    (reg("WRITTEN")? != 0).then(|| reg("EXIT").unwrap_or(0) as u8)
}

/// Run one entry: `Ok(None)` timeout, `Ok(Some(code))` exit code.
fn run(t: Target, m: &mut dyn Machine, e: &Entry, data: &Path) -> Result<Option<u8>, String> {
    let dir = data.join(&e.dir);
    for o in &e.options {
        if let Some(f) = o.strip_prefix("mountcrt:") {
            insert(m, MediaKind::Cartridge, &dir.join(f))?;
        } else if let Some(f) = o
            .strip_prefix("mountd64:")
            .or(o.strip_prefix("mountg64:"))
            .or(o.strip_prefix("mountd71:"))
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
        if t.machine != "c64" {
            // The machine's own program slot: injected after boot, then RUN.
            let media = media_backends::image::from_bytes(&e.prg, prg.clone(), MediaKind::Program);
            let slot = m
                .media_slots()
                .into_iter()
                .find(|s| s.kind == MediaKind::Program)
                .ok_or("no program slot")?
                .id;
            m.media_slot(slot)
                .ok_or("no program slot")?
                .insert(media)
                .map_err(|err| format!("program: {err}"))?;
        } else {
            for _ in 0..lorenz::BOOT_FRAMES {
                m.run_frame(&mut dbg);
            }
        }
        if t.machine != "c64" {
        } else if e.options.iter().any(|o| o.contains("\"TEST\" as filename")) {
            // The test wants a real LOAD"TEST",8: its reference memory
            // dump holds the typed file name. Without a disk the program
            // goes on a disk of its own as "TEST".
            if !e.options.iter().any(|o| o.starts_with("mountd64:")) {
                let mut image = media::d64::D64::blank(b"TEST", *b"00");
                image
                    .add_prg(b"TEST", &prg)
                    .map_err(|err| format!("TEST disk: {err}"))?;
                let media = media_backends::image::from_bytes(
                    "test.d64",
                    image.to_bytes(),
                    MediaKind::Disk,
                );
                let slot = m
                    .media_slots()
                    .into_iter()
                    .find(|s| s.kind == MediaKind::Disk)
                    .ok_or("no drive")?
                    .id;
                m.media_slot(slot)
                    .ok_or("no drive")?
                    .insert(media)
                    .map_err(|err| format!("TEST disk: {err}"))?;
            }
            type_and_wait(
                m,
                &mut dbg,
                "load\"test\",8
",
                true,
            );
            type_and_wait(
                m, &mut dbg, "run
", false,
            );
        } else {
            lorenz::load_and_run(m, &prg);
        }
    }
    let cpu = lorenz::space(m, "cpu");
    dbg.add_breakpoint(BreakpointKind::Write {
        space: cpu,
        addr: t.exit_addr,
        len: 1,
    });
    while m.now().0 < e.cycles {
        match m.run_frame(&mut dbg) {
            // A write the CPU watchpoint does not see (the C128's Z80
            // writes $D7FF through its I/O space): the register knows.
            StopReason::FrameDone => {
                if let Some(code) = exit_code(m) {
                    return Ok(Some(code));
                }
            }
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

/// Where xplus4's PAL screenshot (384 × 288) has our frame's pixel (0, 0):
/// both show 32 border pixels left of the window, xplus4 two lines fewer
/// above it (measured on the selftest pictures: window at line 40 there,
/// 42 here).
const TED_REF_OFFSET: (i64, i64) = (0, -2);

/// Compare a TED picture: xplus4's palette is not ours, so the colours
/// must match one to one (each reference colour stands for one TED colour
/// and back), over the area both pictures cover.
fn compare_ted(m: &dyn Machine, reference: &Path) -> Result<(), String> {
    let (rw, rh, rpx) = load_png(reference)?;
    let frame = m.framebuffer();
    let PixelData::Indexed { pixels, .. } = frame.pixels else {
        return Err("TED frame not indexed".into());
    };
    let (ow, oh) = (i64::from(frame.width), i64::from(frame.height));
    let (dx, dy) = TED_REF_OFFSET;
    let mut to_ours: BTreeMap<[u8; 3], u8> = BTreeMap::new();
    let mut to_ref: BTreeMap<u8, [u8; 3]> = BTreeMap::new();
    let (mut bad, mut first) = (0, None);
    for y in 0..oh {
        for x in 0..ow {
            let (rx, ry) = (x + dx, y + dy);
            if rx < 0 || ry < 0 || rx >= i64::from(rw) || ry >= i64::from(rh) {
                continue;
            }
            let r = rpx[(ry * i64::from(rw) + rx) as usize];
            let o = pixels[(y * ow + x) as usize] & 0x7f;
            let ok = *to_ours.entry(r).or_insert(o) == o && *to_ref.entry(o).or_insert(r) == r;
            if !ok {
                bad += 1;
                first.get_or_insert((x, y, r, o));
            }
        }
    }
    match first {
        None => Ok(()),
        Some((x, y, r, o)) => Err(format!(
            "{bad} pixels differ, first at {x},{y} (frame) ref {r:02x?} ours ${o:02x}"
        )),
    }
}

/// Lines xpet's screenshot may sit above or below ours. Both show 32
/// pixels of margin left of the text (the same width), but xpet frames
/// the picture vertically its own way (text mode and the CRTC programs'
/// graphics mode land at different heights), so the comparison takes the
/// vertical offset that fits best.
const PET_SEARCH_DY: i64 = 48;

/// Compare a PET picture: one colour on black, so pixels compare as lit
/// or dark, over the area both pictures cover, at the best vertical
/// offset.
fn compare_pet(m: &dyn Machine, reference: &Path) -> Result<(), String> {
    let (rw, rh, rpx) = load_png(reference)?;
    let frame = m.framebuffer();
    let (ow, oh) = (i64::from(frame.width), i64::from(frame.height));
    let (rw, rh) = (i64::from(rw), i64::from(rh));
    let ours: Vec<bool> = (0..oh)
        .flat_map(|y| (0..ow).map(move |x| (x, y)))
        .map(|(x, y)| {
            let o = frame.rgb_at(x as u32, y as u32).unwrap_or_default();
            u32::from(o.r) + u32::from(o.g) + u32::from(o.b) > 192
        })
        .collect();
    let lit: Vec<bool> = rpx
        .iter()
        .map(|r| r.iter().map(|&c| u32::from(c)).sum::<u32>() > 192)
        .collect();
    let differ = |dy: i64| {
        let (mut bad, mut first) = (0, None);
        for y in 0..oh {
            let ry = y + dy;
            if ry < 0 || ry >= rh {
                continue;
            }
            // xpet draws nothing left of x 8 or right of x 695 (in no
            // reference): its window, not the machine's.
            for x in 8..ow.min(rw).min(696) {
                let r = lit[(ry * rw + x) as usize];
                if r != ours[(y * ow + x) as usize] {
                    bad += 1;
                    first.get_or_insert((x, y, r));
                }
            }
        }
        (bad, first)
    };
    let (dy, (bad, first)) = (-PET_SEARCH_DY..=PET_SEARCH_DY)
        .map(|dy| (dy, differ(dy)))
        .min_by_key(|(dy, (bad, _))| (*bad, dy.abs()))
        .unwrap_or((0, (0, None)));
    match first {
        None => Ok(()),
        Some((x, y, lit)) => Err(format!(
            "{bad} pixels differ (best at {dy:+} lines), first at {x},{y} (frame), reference {}",
            if lit { "lit" } else { "dark" }
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

fn judge(t: Target, base: &MachineConfig, e: &Entry, data: &Path) -> Verdict {
    let config = match config(t, base, e) {
        Ok(c) => c,
        Err(why) => return Verdict::Skipped(why),
    };
    if !e.prg.is_empty() && !data.join(&e.dir).join(&e.prg).is_file() {
        return Verdict::Skipped("program not in the repository".into());
    }
    let registry = emu_machines::registry();
    let mut m = match registry.create(t.machine, &config) {
        Ok(m) => m,
        Err(err) => return Verdict::Error(format!("{err:?}")),
    };
    let expect_error = e.has("expect:error");
    let expect_timeout = e.has("expect:timeout");
    let outcome = run(t, m.as_mut(), e, data);
    let result = match outcome {
        Err(why) => Verdict::Error(why),
        // The PET's CRTC programs never write an exit code: the picture
        // at the cycle limit is the result, as in the testbench.
        Ok(None) if t.machine == "pet" && e.kind == "screenshot" => {
            match reference(e, data, false) {
                None => Verdict::Skipped("reference picture missing".into()),
                Some(r) => match compare_pet(m.as_ref(), &r) {
                    Ok(()) => Verdict::Pass,
                    Err(why) => Verdict::Fail(why),
                },
            }
        }
        Ok(None) => Verdict::Timeout,
        Ok(Some(code)) if e.kind == "screenshot" => {
            let ntsc = config.video_format == Some(emu_core::frame::VideoStandard::Ntsc);
            match reference(e, data, ntsc) {
                None => Verdict::Skipped("reference picture missing".into()),
                Some(r) => match if t.machine == "plus4" {
                    compare_ted(m.as_ref(), &r)
                } else if t.machine == "pet" {
                    compare_pet(m.as_ref(), &r)
                } else {
                    compare(m.as_ref(), &r)
                } {
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
    suite(C64);
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR (tools/fetch_vice_testprogs.py --list vic20)"]
fn vice_testprogs_vic20() {
    suite(VIC20);
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR (tools/fetch_vice_testprogs.py --list plus4)"]
fn vice_testprogs_plus4() {
    suite(PLUS4);
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR (tools/fetch_vice_testprogs.py --list c128)"]
fn vice_testprogs_c128() {
    suite(C128);
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR (tools/fetch_vice_testprogs.py --list pet)"]
fn vice_testprogs_pet() {
    suite(PET);
}

fn suite(t: Target) {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let base = MachineConfig {
        roms: media_backends::roms::load(&rom_dir, t.machine).expect("ROMs"),
        ..MachineConfig::default()
    };
    let data = PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
        .join("vice-testprogs");
    let list = std::fs::read_to_string(data.join(t.list))
        .expect("run tools/fetch_vice_testprogs.py (--list vic20 for the VIC-20)");
    let filter = std::env::var("EMU_VICE_FILTER").unwrap_or_default();
    let skip = std::env::var("EMU_VICE_SKIP").unwrap_or_default();
    let entries: Vec<Entry> = parse(&list)
        .into_iter()
        .filter(|e| e.name().contains(&filter))
        .filter(|e| skip.is_empty() || !e.name().contains(&skip))
        .collect();

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Verdict)>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(e) = entries.get(i) else { break };
                let v = judge(t, &base, e, &data);
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
    let report_name = match (t.machine, filter.is_empty()) {
        ("c64", true) => "vice-testprogs.txt".to_string(),
        ("c64", false) => "vice-testprogs-filtered.txt".to_string(),
        (_, true) => format!("{}.txt", t.name),
        (_, false) => format!("{}-filtered.txt", t.name),
    };
    // Trend: every entry that ran (the options tell apart entries of one
    // program).
    let outcomes: Vec<(String, bool)> = results
        .iter()
        .filter(|(_, v)| !matches!(v, Verdict::Skipped(_)))
        .map(|(i, v)| {
            let e = &entries[*i];
            let name = format!("{} {}", e.name(), e.options.join(","));
            (name.trim_end().to_string(), matches!(v, Verdict::Pass))
        })
        .collect();
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let trend = emu_tests::trend::record(tmp, t.name, &outcomes);
    report.push('\n');
    report.push_str(&trend);
    report.push('\n');
    let out = tmp.join(&report_name);
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
    println!("{trend}");
}
