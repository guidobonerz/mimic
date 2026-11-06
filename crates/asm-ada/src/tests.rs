//! The assembler and linker: every instruction word from its disassembly,
//! the directives, macros, includes, local labels, linking across
//! objects, object files, errors, and the step-2 acceptance program.

use std::collections::BTreeMap;
use std::path::Path;

use cpu_ada::isa;

use crate::run::{run, Board, End};
use crate::*;

/// In-memory files for `.include`.
struct Mem(BTreeMap<String, String>);

impl Files for Mem {
    fn read(&self, path: &Path) -> Result<String, String> {
        let key = path.display().to_string().replace('\\', "/");
        self.0
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("{key}: no such file"))
    }
}

fn none() -> Mem {
    Mem(BTreeMap::new())
}

/// Assemble and link one source at 0040:0000; the bytes from there.
fn bytes(src: &str) -> Vec<u8> {
    let img =
        build(&[("t.s", src)], &none(), Options::default()).unwrap_or_else(|e| panic!("{e:?}"));
    img.contiguous(0xff, 1 << 20)
        .map(|(_, b)| b)
        .unwrap_or_default()
}

fn errors(src: &str) -> String {
    build(&[("t.s", src)], &none(), Options::default())
        .expect_err("should fail")
        .join("\n")
}

fn words(b: &[u8]) -> Vec<u16> {
    b.chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[test]
fn every_instruction_assembles_from_its_disassembly() {
    let pc = 0x8000u16;
    let mut n = 0;
    for w in 0..=0xffffu16 {
        let op = isa::decode(w);
        if let isa::Op::Illegal(_) = op {
            continue;
        }
        let (m, o) = op.text(pc);
        let src = format!(".org $0040{pc:04x}\n {m} {o}\n");
        let objs = assemble("t.s", &src, &none()).unwrap_or_else(|e| panic!("{m} {o}: {e:?}"));
        let img = link(&[objs], Options::default()).unwrap_or_else(|e| panic!("{m} {o}: {e:?}"));
        let got = u16::from_le_bytes([img.blocks[0].1[0], img.blocks[0].1[1]]);
        assert_eq!(got, w, "{m} {o}");
        n += 1;
    }
    assert_eq!(n, 39_381);
}

#[test]
fn pseudo_instructions() {
    let b = words(&bytes(
        "li r1, #$1234\n ret\n retf\n push r3\n pop r4\n bhs .x\n.x: blo .x\n nop\n",
    ));
    let e = |op| isa::encode(&op);
    use isa::{Imm, Mem as M, Op};
    assert_eq!(
        b,
        vec![
            e(Op::Imm {
                f: Imm::Movi,
                rd: 1,
                imm: 0x34
            }),
            e(Op::Imm {
                f: Imm::Lui,
                rd: 1,
                imm: 0x12
            }),
            e(Op::Jr { rs: 11 }),
            e(Op::Jf { rd: 10, rs: 11 }),
            e(Op::Imm {
                f: Imm::Addi,
                rd: 12,
                imm: 0xfe
            }),
            e(Op::Mem {
                f: M::Sw,
                rd: 3,
                rb: 12,
                off: 0
            }),
            e(Op::Mem {
                f: M::Lw,
                rd: 4,
                rb: 12,
                off: 0
            }),
            e(Op::Imm {
                f: Imm::Addi,
                rd: 12,
                imm: 2
            }),
            e(Op::Branch { cond: 2, off: 0 }),
            e(Op::Branch { cond: 3, off: -1 }),
            e(Op::Nop),
        ]
    );
}

#[test]
fn data_directives_and_expressions() {
    let b = bytes(
        "N = 3 * 4\n .word N, -1, $beef\n .byte 1, 'A', \"hi\", <$1234, >$1234\n .asciz \"a\\n\"\n .align 4\n .space 3, $ee\n .ascii \"\\x41\"\n",
    );
    assert_eq!(
        b,
        vec![
            12, 0, 0xff, 0xff, 0xef, 0xbe, 1, b'A', b'h', b'i', 0x34, 0x12, b'a', b'\n', 0, 0,
            0xee, 0xee, 0xee, b'A'
        ]
    );
}

#[test]
fn addresses_far_pointers_and_relocations() {
    // Code at 0040:0000; `data` follows it.
    let b = words(&bytes(
        "start: li r1, #data\n li r2, #seg(data)\n movi r3, #hi(data)\n .far data\n .word data\ndata: .word start\n",
    ));
    let data = 0x0040_0000u32 + 2 * 8;
    assert_eq!(b[0], 0x2100 | (data & 0xff) as u16, "movi lo");
    assert_eq!(b[1], 0x3100 | ((data >> 8) & 0xff) as u16, "lui hi");
    assert_eq!(b[2], 0x2200 | 0x40, "movi lo(seg)");
    assert_eq!(b[3], 0x3200, "lui hi(seg)");
    assert_eq!(b[4], 0x2300 | ((data >> 8) & 0xff) as u16);
    assert_eq!((b[5], b[6]), (0x0040, data as u16), ".far: segment, offset");
    assert_eq!(b[7], data as u16);
    assert_eq!(b[8], 0, "start is 0040:0000");
}

#[test]
fn local_labels_macros_and_includes() {
    let files = Mem(BTreeMap::from([(
        "lib/inc.s".to_string(),
        "TEN = 10\n.macro addn r, n\n addi \\r, #\\n\n.endm\n".to_string(),
    )]));
    let src = ".include \"inc.s\"\nf: bra .l\n.l: addn r1, TEN\ng: bra .l\n.l: .macro twice x\n  nop\n  \\x\n.endm\n twice ret\n";
    let img = build(&[("lib/main.s", src)], &files, Options::default())
        .unwrap_or_else(|e| panic!("{e:?}"));
    let b = words(&img.blocks[0].1);
    use isa::{Imm, Op};
    assert_eq!(
        b[0],
        isa::encode(&Op::Branch { cond: 14, off: 0 }),
        "f.l is the next word"
    );
    assert_eq!(
        b[1],
        isa::encode(&Op::Imm {
            f: Imm::Addi,
            rd: 1,
            imm: 10
        })
    );
    assert_eq!(b[2], isa::encode(&Op::Branch { cond: 14, off: 0 }), "g.l");
    assert_eq!(b[3], isa::encode(&Op::Nop));
    assert_eq!(b[4], isa::encode(&Op::Jr { rs: 11 }));
}

#[test]
fn macro_counter_makes_unique_labels() {
    let b = words(&bytes(".macro wait\nw\\@: bne w\\@\n.endm\n wait\n wait\n"));
    assert_eq!(b, vec![0xa1ff, 0xa1ff]);
}

#[test]
fn linking_objects_with_globals_and_fixed_sections() {
    let a = assemble(
        "a.s",
        ".global main\nmain: bl helper\n li r1, #table\n",
        &none(),
    )
    .unwrap();
    let b = assemble(
        "b.s",
        ".global helper, table\nhelper: ret\n.section rom\n.org $fffb0000\ntable: .word main\n",
        &none(),
    )
    .unwrap();
    // Through the object file format.
    let a = Object::from_bytes(&a.to_bytes()).unwrap();
    let b = Object::from_bytes(&b.to_bytes()).unwrap();
    let img = link(&[a, b], Options { base: 0x0040_0100 }).unwrap();
    assert_eq!(img.symbols["main"], 0x0040_0100);
    assert_eq!(img.symbols["helper"], 0x0040_0106);
    assert_eq!(img.symbols["table"], 0xfffb_0000);
    assert_eq!(img.blocks.len(), 2);
    let code = words(&img.blocks[0].1);
    assert_eq!(code[0], isa::encode(&isa::Op::Bl { off: 2 }));
    assert_eq!(code[1], 0x2100, "lo(table) = 0");
    assert_eq!(words(&img.blocks[1].1), vec![0x0100], "table: off(main)");
    assert!(img.listing.contains("0040:0100"), "{}", img.listing);
    assert!(img.listing.contains("bl helper"));
}

#[test]
fn errors_name_the_line() {
    assert!(errors("nop\n bl nowhere\n").contains("undefined symbol `nowhere`"));
    assert!(errors(" beq far\n .space 300\nfar: nop\n").contains("out of range"));
    assert!(errors(" addi r1, #200\n").contains("t.s:1"));
    assert!(errors(" lw r1, 3(r2)\n").contains("offset 3"));
    assert!(errors(" mts s0, r1\n").contains("jf"));
    assert!(errors("x: nop\nx: nop\n").contains("defined twice"));
    assert!(errors(" frobnicate r1\n").contains("unknown instruction"));
    assert!(errors(".byte 1\n nop\n").contains("odd address"));
    assert!(errors(".org $0040fffe\n nop\n nop\n").contains("segment boundary"));
    assert!(errors(".org $00400000\n nop\n.section b\n.org $00400000\n nop\n").contains("overlap"));
    assert!(errors(" movi r1, #x\nx: nop\n").contains("lo(), hi()"));
}

#[test]
fn board_runs_and_reports_exit() {
    let src = " li r1, #$ffff\n mts s3, r1\n li r2, #$43f0\n movi r1, #'!'\n sbx r1, (r2)\n li r2, #$43f2\n movi r1, #7\n swx r1, (r2)\n";
    let img = build(&[("t.s", src)], &none(), Options::default()).unwrap();
    let mut board = Board::default();
    let (at, b) = img.contiguous(0xff, 1 << 20).unwrap();
    board.load(at, &b).unwrap();
    let r = run(&mut board, at, 1000);
    assert_eq!((r.end, r.output), (End::Exit(7), b"!".to_vec()));
}

/// Step 2's acceptance test: the prime program prints the primes below
/// 1000 on the debug console and exits with 0.
#[test]
fn prime_program_prints_the_primes_below_1000() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../machine-logophor16/asm/examples/primes.s");
    let src = std::fs::read_to_string(&path).unwrap();
    let img = build(&[("primes.s", &src)], &HostFiles, Options::default())
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = Board::default();
    let (at, b) = img.contiguous(0xff, 1 << 20).unwrap();
    assert_eq!(at, img.symbols["start"]);
    board.load(at, &b).unwrap();
    let r = run(&mut board, at, 50_000_000);
    assert_eq!(r.end, End::Exit(0));
    let want: Vec<String> = (2..1000u32)
        .filter(|n| (2..*n).take_while(|d| d * d <= *n).all(|d| n % d != 0))
        .map(|n| n.to_string())
        .collect();
    assert_eq!(String::from_utf8(r.output).unwrap(), want.join(" ") + " \n");
    assert_eq!(want.len(), 168);
    println!(
        "primes below 1000: {} cycles ({:.1} ms at 12 MHz)",
        r.cycles,
        r.cycles as f64 / 12_000.0
    );
}
