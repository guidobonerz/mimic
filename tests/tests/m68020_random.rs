//! Random 68020 instruction tests against results from another 68020.
//!
//!     EMU_TESTDATA_DIR=… cargo test -p emu-tests --release --test m68020_random -- --ignored --nocapture
//!
//! `tools/m68020_random.py` builds a boot disk with test cases and a
//! program that runs them (see the tool for the layout); booted on an A1200
//! in WinUAE it writes the results back to the disk, and `extract` turns
//! them into the reference `tests/data/m68020_random.ref`. This test loads
//! the same program (`tests/data/m68020_random.bin`, the tool's `--bin`) on
//! a bare 68020 with 16 MB of flat RAM, enters it through the bare entry
//! and compares every case's record: exception vector, D0-D7/A0-A6, A7,
//! 16 bytes from A7 and the changed bytes of the test area. The report goes
//! to `target/tmp/m68020-random.txt`, our results to
//! `target/tmp/m68020-random-mimic.res`; `EMU_M68020_REF=file` takes
//! another reference (e.g. a second seed with its `.bin` in
//! `EMU_M68020_BIN`). A stocktaking, not a gate.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use cpu_m68k::{Model, M68000};
use emu_core::{Bus, Cpu, Size};

const CODE: u32 = 0x10_0000;
const LIMIT: u64 = 400_000_000;

struct Header {
    cases: usize,
    seed: u32,
    case_at: u32,
    case_size: u32,
    results: u32,
    rec_size: u32,
    done: u32,
    insn_at: u32,
    /// The Amiga entry's final loops: written back (green), error (red).
    green: u32,
    fail: u32,
}

fn header(image: &[u8]) -> Header {
    let be32 = |o: usize| u32::from_be_bytes(image[o..o + 4].try_into().unwrap());
    assert_eq!(&image[8..12], b"M020", "not a m68020_random program");
    Header {
        cases: usize::from(u16::from_be_bytes([image[14], image[15]])),
        seed: be32(16),
        case_at: be32(20),
        case_size: be32(24),
        results: be32(28),
        rec_size: be32(32),
        done: be32(40),
        insn_at: be32(52),
        green: be32(60),
        fail: be32(64),
    }
}

/// Flat RAM; 24 address lines like the 68EC020. Writes outside the
/// program's own areas are counted (a case reaching memory the Amiga's OS
/// owns would make the reference worthless).
struct Ram {
    mem: Vec<u8>,
    stray: Vec<u32>,
}

impl Ram {
    fn allowed(addr: u32) -> bool {
        (0x14_3000..0x1A_8000).contains(&addr)
    }

    fn put(&mut self, addr: u32, v: u8) {
        let a = addr & 0xff_ffff;
        if !Self::allowed(a) && self.stray.len() < 100 {
            self.stray.push(a);
        }
        self.mem[a as usize] = v;
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        (0..nbytes(size)).fold(0, |v, i| {
            v << 8 | u32::from(self.mem[(addr.wrapping_add(i) & 0xff_ffff) as usize])
        })
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        let n = nbytes(size);
        for i in 0..n {
            self.put(addr.wrapping_add(i), (v >> (8 * (n - 1 - i))) as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

fn nbytes(size: Size) -> u32 {
    match size {
        Size::Byte => 1,
        Size::Word => 2,
        Size::Long => 4,
    }
}

/// One case's record (see the tool).
#[derive(PartialEq, Eq)]
struct Record<'a>(&'a [u8]);

impl Record<'_> {
    fn u16(&self, o: usize) -> u16 {
        u16::from_be_bytes([self.0[o], self.0[o + 1]])
    }
    fn u32(&self, o: usize) -> u32 {
        u32::from_be_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn vector(&self) -> u16 {
        self.u16(0)
    }
    fn reg(&self, i: usize) -> u32 {
        self.u32(4 + 4 * i)
    }
    fn sp(&self) -> u32 {
        self.u32(64)
    }
    fn stack(&self) -> &[u8] {
        &self.0[68..84]
    }
    fn diffs(&self) -> Vec<(u16, u8)> {
        let n = usize::from(self.u16(2)).min(24);
        (0..n)
            .map(|i| (self.u16(84 + 4 * i), self.0[86 + 4 * i]))
            .collect()
    }
    fn changed(&self) -> u16 {
        self.u16(2)
    }
}

const REG_NAMES: [&str; 15] = [
    "D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7", "A0", "A1", "A2", "A3", "A4", "A5", "A6",
];

fn vector_name(v: u16) -> String {
    match v {
        4 => "ILLEGAL".into(),
        5 => "zero divide".into(),
        6 => "CHK".into(),
        7 => "TRAPV/TRAPcc".into(),
        8 => "privilege".into(),
        10 => "line A".into(),
        11 => "line F".into(),
        14 => "format error".into(),
        32..=47 => format!("TRAP #{}", v - 32),
        _ => format!("vector {v}"),
    }
}

fn data_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
}

/// Run the program on a bare 68020; returns the memory afterwards.
fn run(image: &[u8], h: &Header) -> Ram {
    let mut ram = Ram {
        mem: vec![0; 1 << 24],
        stray: Vec::new(),
    };
    ram.mem[CODE as usize..CODE as usize + image.len()].copy_from_slice(image);
    let mut cpu = M68000::with_model(Model::Mc68020);
    cpu.set_sr(0x2700);
    let entry = CODE + 4;
    let queue = [
        ram.peek(entry, Size::Word) as u16,
        ram.peek(entry + 2, Size::Word) as u16,
    ];
    cpu.set_pc_and_queue(entry, queue);
    let mut steps = 0u64;
    while !cpu.is_stopped() {
        cpu.tick(&mut ram);
        steps += 1;
        assert!(
            !cpu.is_halted(),
            "68020 halted (double fault) after {steps} steps"
        );
        assert!(steps < LIMIT, "no end after {steps} instructions");
    }
    assert_eq!(ram.peek(h.done, Size::Word), 1, "program did not finish");
    ram
}

#[test]
#[ignore = "runs a few million instructions; reference from WinUAE in tests/data"]
fn random_68020_matches_the_reference() {
    let bin = std::env::var_os("EMU_M68020_BIN")
        .map_or_else(|| data_path("m68020_random.bin"), PathBuf::from);
    let image = std::fs::read(&bin).expect("program image (tools/m68020_random.py build … --bin)");
    let h = header(&image);
    let ram = run(&image, &h);
    let rec_size = h.rec_size as usize;
    let ours = &ram.mem[h.results as usize..h.results as usize + h.cases * rec_size];
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(tmp.join("m68020-random-mimic.res"), ours).unwrap();
    assert!(
        ram.stray.is_empty(),
        "writes outside the test areas: {:x?}",
        ram.stray
    );

    let ref_path = std::env::var_os("EMU_M68020_REF")
        .map_or_else(|| data_path("m68020_random.ref"), PathBuf::from);
    let Ok(reference) = std::fs::read(&ref_path) else {
        eprintln!(
            "no reference at {} yet: ran {} cases (seed {}), results in target/tmp/m68020-random-mimic.res",
            ref_path.display(),
            h.cases,
            h.seed
        );
        return;
    };
    assert_eq!(
        reference.len(),
        ours.len(),
        "reference for another case count"
    );

    let mut report = String::new();
    let mut by_mnemonic: BTreeMap<String, [usize; 2]> = BTreeMap::new();
    let mut outcomes = Vec::new();
    for i in 0..h.cases {
        let theirs = Record(&reference[i * rec_size..(i + 1) * rec_size]);
        let mine = Record(&ours[i * rec_size..(i + 1) * rec_size]);
        // The case: instruction words and initial registers.
        let case = h.case_at as usize - CODE as usize + i * h.case_size as usize;
        let nwords = usize::from(u16::from_be_bytes([image[case], image[case + 1]]));
        let mut code = Ram {
            mem: vec![0; 1 << 24],
            stray: Vec::new(),
        };
        let at = h.insn_at as usize;
        code.mem[at..at + 2 * nwords].copy_from_slice(&image[case + 2..case + 2 + 2 * nwords]);
        let cpu = M68000::with_model(Model::Mc68020);
        let instr = cpu.disassemble(&code, h.insn_at);
        let mnemonic = instr.mnemonic.clone();
        let pass = theirs == mine;
        by_mnemonic.entry(mnemonic.clone()).or_default()[usize::from(!pass)] += 1;
        outcomes.push((format!("{i:04} {mnemonic}"), pass));
        if pass {
            continue;
        }
        let words: Vec<String> = image[case + 2..case + 2 + 2 * nwords]
            .chunks(2)
            .map(|w| format!("{:02X}{:02X}", w[0], w[1]))
            .collect();
        let _ = writeln!(report, "case {i:04}: {instr}    [{}]", words.join(" "));
        let init = |r: usize| {
            u32::from_be_bytes(
                image[case + 24 + 4 * r..case + 28 + 4 * r]
                    .try_into()
                    .unwrap(),
            )
        };
        let sr = u16::from_be_bytes([image[case + 84], image[case + 85]]);
        let regs: Vec<String> = (0..15)
            .map(|r| format!("{}={:08X}", REG_NAMES[r], init(r)))
            .collect();
        let _ = writeln!(report, "  before: SR={sr:04X} {}", regs.join(" "));
        if theirs.vector() != mine.vector() {
            let _ = writeln!(
                report,
                "  exception: expected {}, got {}",
                vector_name(theirs.vector()),
                vector_name(mine.vector())
            );
        }
        for (r, name) in REG_NAMES.iter().enumerate() {
            if theirs.reg(r) != mine.reg(r) {
                let _ = writeln!(
                    report,
                    "  {name}: expected {:08X}, got {:08X}",
                    theirs.reg(r),
                    mine.reg(r)
                );
            }
        }
        if theirs.sp() != mine.sp() {
            let _ = writeln!(
                report,
                "  A7: expected {:08X}, got {:08X}",
                theirs.sp(),
                mine.sp()
            );
        }
        if theirs.stack() != mine.stack() {
            let _ = writeln!(
                report,
                "  stack (frame: SR, PC, format/vector …): expected {:02X?}, got {:02X?}",
                theirs.stack(),
                mine.stack()
            );
        }
        if theirs.changed() != mine.changed() || theirs.diffs() != mine.diffs() {
            let fmt = |r: &Record| {
                r.diffs()
                    .iter()
                    .map(|(o, v)| format!("+{o:03X}={v:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let _ = writeln!(
                report,
                "  memory ({} / {} bytes changed): expected {}, got {}",
                theirs.changed(),
                mine.changed(),
                fmt(&theirs),
                fmt(&mine)
            );
        }
    }
    let failed = outcomes.iter().filter(|(_, p)| !p).count();
    let mut summary = String::from("\nmnemonic        pass  fail\n");
    for (m, [p, f]) in &by_mnemonic {
        let _ = writeln!(summary, "{m:<14} {p:>5} {f:>5}");
    }
    let total = format!(
        "68020 random tests (seed {}): {}/{} passed ({:.1} %), {failed} failed",
        h.seed,
        h.cases - failed,
        h.cases,
        100.0 * (h.cases - failed) as f64 / h.cases as f64
    );
    let trend = emu_tests::trend::record(tmp, &format!("m68020-random-seed{}", h.seed), &outcomes);
    let out = tmp.join("m68020-random.txt");
    std::fs::write(&out, format!("{report}{summary}\n{total}\n{trend}\n")).unwrap();
    println!("{summary}");
    println!("{total}; report: {}", out.display());
    println!("{trend}");
}

/// The disk itself on mimic's A1200 (`EMU_M68020_ADF`, built by the tool;
/// Kickstart 3.1 from `EMU_ROM_DIR`): it boots, runs every case, writes the
/// results back to the disk, and they equal the bare run's. A check of the
/// disk before it goes to another machine.
#[test]
#[ignore = "needs EMU_ROM_DIR (Kickstart 3.1) and EMU_M68020_ADF"]
fn random_68020_disk_runs_on_the_a1200() {
    use emu_core::machine::Machine;
    use emu_core::media::{Media, MediaKind, SlotId};
    use emu_core::registry::MachineConfig;
    use emu_core::{DebugHooks, StopReason};

    let adf = std::fs::read(std::env::var_os("EMU_M68020_ADF").expect("EMU_M68020_ADF")).unwrap();
    let image = &adf[1024..];
    let h = header(image);
    let res_off = u32::from_be_bytes(image[36..40].try_into().unwrap()) as usize;
    let rec = h.rec_size as usize;
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "amiga").expect("ROMs"),
        model: Some("1200".into()),
        ..MachineConfig::default()
    };
    config.options.insert("kickstart".into(), "3.1".into());
    let mut m: Box<dyn Machine> = emu_machines::registry().create("amiga", &config).unwrap();
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: "m68020.adf".into(),
            kind: MediaKind::Disk,
            backend: Box::new(media_backends::image::MemoryBackend { data: adf.clone() }),
        })
        .unwrap();
    let mut dbg = DebugHooks::new();
    let done = |m: &mut Box<dyn Machine>| {
        let t = m.debug_target(machine_amiga::CPU).unwrap();
        t.peek(machine_amiga::CPU_SPACE, h.done, Size::Word) == Some(1)
    };
    let mut frames = 0;
    while !done(&mut m) {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        frames += 1;
        assert!(frames < 6000, "the program did not finish");
    }
    eprintln!("cases done after {frames} frames");
    // The write-back: trackdisk writes the track buffer at CMD_UPDATE.
    // Wait for the green (or red) loop at the end of the Amiga entry.
    let (green, fail) = (h.green, h.fail);
    let mut waited = 0;
    loop {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        waited += 1;
        let pc = m
            .debug_target(machine_amiga::CPU)
            .unwrap()
            .regs()
            .get("PC")
            .unwrap_or(0);
        assert!(!(fail..fail + 10).contains(&pc), "disk error (red screen)");
        if (green..green + 10).contains(&pc) {
            break;
        }
        assert!(waited < 6000, "the write-back did not finish (PC {pc:06X})");
    }
    eprintln!("written back after {waited} more frames");
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let in_ram: Vec<u8> = (0..h.cases * rec)
        .map(|i| {
            t.peek(machine_amiga::CPU_SPACE, h.results + i as u32, Size::Byte)
                .unwrap() as u8
        })
        .collect();
    let slot = m.media_slot(SlotId(0)).unwrap();
    slot.flush().unwrap();
    let mut media = slot.eject().expect("disk");
    let mut on_disk = vec![0; h.cases * rec];
    media.backend.read(res_off as u64, &mut on_disk).unwrap();
    let mut written = vec![0; adf.len()];
    media.backend.read(0, &mut written).unwrap();
    std::fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("m68020-random-a1200.adf"),
        written,
    )
    .unwrap();
    let bare = run(image, &h);
    let bare = &bare.mem[h.results as usize..h.results as usize + h.cases * rec];
    let differ: Vec<usize> = (0..h.cases)
        .filter(|i| in_ram[i * rec..(i + 1) * rec] != bare[i * rec..(i + 1) * rec])
        .collect();
    assert!(
        differ.is_empty(),
        "A1200 and bare 68020 differ in cases {differ:?}"
    );
    assert!(on_disk == in_ram, "the results did not reach the disk");
}
