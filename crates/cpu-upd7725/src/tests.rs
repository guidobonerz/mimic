use super::*;

/// A ROM image of `prog` (the rest of the program NOPs) and `data`.
fn image(prog: &[u32], data: &[u16]) -> Vec<u8> {
    let mut rom = vec![0; ROM_BYTES];
    for (i, op) in prog.iter().enumerate() {
        rom[i * 3..i * 3 + 3].copy_from_slice(&op.to_le_bytes()[..3]);
    }
    for (i, w) in data.iter().enumerate() {
        let at = PROGRAM_WORDS * 3 + i * 2;
        rom[at..at + 2].copy_from_slice(&w.to_le_bytes());
    }
    rom
}

fn ld(dst: u32, v: u16) -> u32 {
    0xc0_0000 | u32::from(v) << 6 | dst
}

fn op(pselect: u32, alu: u32, asl: u32, src: u32, dst: u32) -> u32 {
    pselect << 20 | alu << 16 | asl << 15 | src << 4 | dst
}

fn jp(brch: u32, na: u32) -> u32 {
    0x80_0000 | brch << 13 | na << 2
}

#[test]
fn multiplier_follows_one_instruction_later() {
    // LD K,$4000; LD L,$C000 (-0.5); NOP; NOP
    let rom = image(&[ld(10, 0x4000), ld(13, 0xc000), 0, 0], &[]);
    let mut d = Upd7725::new(&rom).unwrap();
    d.step();
    d.step();
    // -0.5 × 0.5 = -0.25: M = $E000, N = 0.
    assert_eq!((d.m, d.n), (0xe000, 0));
    d.k = 3;
    d.l = 5;
    d.step();
    assert_eq!((d.m, d.n), (0, 30));
}

#[test]
fn alu_carry_comes_from_the_other_accumulator() {
    // A = $FFFF; B = 0; ADD A,IDB (1) from bus K=1; ADC B,IDB from K=0
    let rom = image(
        &[
            ld(1, 0xffff),
            ld(2, 0),
            ld(10, 1),
            op(1, 5, 0, 13, 0),
            ld(10, 0),
            op(1, 7, 1, 13, 0),
        ],
        &[],
    );
    let mut d = Upd7725::new(&rom).unwrap();
    for _ in 0..6 {
        d.step();
    }
    assert_eq!(d.a, 0);
    assert!(d.fa.c && d.fa.z);
    assert_eq!(d.b, 1);
    assert!(!d.fb.c);
}

#[test]
fn overflow_sets_ov1_and_s1() {
    // A = $7FFF; INC A → $8000 overflows: OV0, OV1, S1 (true sign +).
    let rom = image(&[ld(1, 0x7fff), op(0, 9, 0, 0, 0), op(0, 8, 0, 0, 0)], &[]);
    let mut d = Upd7725::new(&rom).unwrap();
    d.step();
    d.step();
    assert_eq!(d.a, 0x8000);
    assert!(d.fa.ov0 && d.fa.ov1 && !d.fa.s1 && d.fa.s0);
    // SGN saturates to the true sign.
    assert_eq!(d.source(7), 0x7fff);
    // DEC undoes it.
    d.step();
    assert_eq!(d.a, 0x7fff);
    assert!(d.fa.ov0 && !d.fa.ov1);
}

#[test]
fn data_rom_through_rp_and_call_return() {
    // LD RP,1; CALL 4; JMP 3 (stays); -; 4: OP MOV A,RO | RPDEC; RT MOV B,RO
    let rom = image(
        &[
            ld(5, 1),
            jp(0x140, 4),
            0,
            jp(0x100, 3),
            op(0, 0, 0, 6, 1) | 0x100,
            0x40_0000 | op(0, 0, 0, 6, 2),
        ],
        &[0x1111, 0x2222],
    );
    let mut d = Upd7725::new(&rom).unwrap();
    for _ in 0..5 {
        d.step();
    }
    assert_eq!((d.a, d.b, d.rp, d.pc), (0x2222, 0x1111, 0, 3));
}

#[test]
fn host_moves_dr_as_two_bytes() {
    // LD DR,$1234 sets RQM; the host reads low, high.
    let rom = image(&[ld(6, 0x1234), jp(0x0be, 1)], &[]);
    let mut d = Upd7725::new(&rom).unwrap();
    d.step();
    assert_eq!(d.read_sr() & 0x80, 0x80);
    assert_eq!(d.read_dr(), 0x34);
    assert_eq!(d.read_sr() & 0x90, 0x90);
    assert_eq!(d.read_dr(), 0x12);
    assert_eq!(d.read_sr() & 0x90, 0);
    d.write_dr(0xcd);
    d.write_dr(0xab);
    assert_eq!(d.dr, 0xabcd);
}

#[test]
fn dp_steps_and_xors() {
    // LD DP,$1F; OP DPINC | M3 → DP = $20 ^ ... low nibble wraps, DPH ^= 3.
    let rom = image(&[ld(4, 0x1f), 1 << 13 | 3 << 9], &[]);
    let mut d = Upd7725::new(&rom).unwrap();
    d.step();
    d.step();
    assert_eq!(d.dp, 0x20);
}

// ---- the real DSP-1 program (user-supplied, EMU_ROM_DIR/snes/dsp1b.rom)

fn dsp1() -> Option<Upd7725> {
    let dir = std::env::var_os("EMU_ROM_DIR")?;
    let dir = std::path::PathBuf::from(dir);
    let rom = ["snes/dsp1b.rom", "dsp1b.rom", "snes/dsp1.rom", "dsp1.rom"]
        .iter()
        .find_map(|p| std::fs::read(dir.join(p)).ok())?;
    Upd7725::new(&rom).ok()
}

struct Host(Upd7725);

impl Host {
    /// Wait for RQM; first let the DSP run as long as a host CPU's next
    /// access takes (it switches DR to 16 bits right after the command).
    fn wait(&mut self) {
        for _ in 0..8 {
            self.0.step();
        }
        for _ in 0..200_000 {
            if self.0.read_sr() & 0x80 != 0 {
                return;
            }
            self.0.step();
        }
        panic!("DSP never asked for a transfer (PC ${:03X})", self.0.pc);
    }

    fn command(&mut self, cmd: u8, params: &[u16], results: usize) -> Vec<u16> {
        self.wait();
        self.0.write_dr(cmd);
        for &p in params {
            for b in p.to_le_bytes() {
                self.wait();
                self.0.write_dr(b);
            }
        }
        (0..results)
            .map(|_| {
                let lo = {
                    self.wait();
                    self.0.read_dr()
                };
                self.wait();
                u16::from_le_bytes([lo, self.0.read_dr()])
            })
            .collect()
    }
}

#[test]
fn dsp1_commands() {
    let Some(d) = dsp1() else {
        eprintln!("EMU_ROM_DIR with snes/dsp1b.rom not set; skipped");
        return;
    };
    let mut h = Host(d);
    let close = |a: u16, b: u16, tol: i32| (i32::from(a as i16) - i32::from(b as i16)).abs() <= tol;

    // 00: multiply, K × I / $8000.
    let r = h.command(0x00, &[0x4000, 0x4000], 1);
    assert_eq!(r, [0x2000], "multiply");
    let r = h.command(0x00, &[0x8000, 0x4000], 1);
    assert_eq!(r, [0xc000], "multiply negative");

    // 04: triangle; angle $4000 = 90°, radius $7FFF → sin = R, cos = 0.
    let r = h.command(0x04, &[0x4000, 0x7fff], 2);
    eprintln!("triangle 90: {r:04X?}");
    assert!(
        close(r[0], 0x7fff, 2) && close(r[1], 0, 2),
        "triangle {r:04X?}"
    );
    // 30°: sin 0.5, cos 0.866.
    let r = h.command(0x04, &[0x1555, 0x7fff], 2);
    eprintln!("triangle 30: {r:04X?}");
    assert!(
        close(r[0], 0x4000, 8) && close(r[1], 0x6eda, 8),
        "triangle {r:04X?}"
    );

    // 28: distance √(x² + y² + z²).
    let r = h.command(0x28, &[3000, 4000, 0], 1);
    eprintln!("distance: {r:?}");
    assert!(close(r[0], 5000, 2), "distance {r:?}");

    // 10: inverse of 0.5 × 2^0 = 2 = 0.5 × 2^2.
    let r = h.command(0x10, &[0x4000, 0], 2);
    eprintln!("inverse: {r:04X?}");
    let value = f64::from(r[0] as i16) / 32768.0 * 2f64.powi(i32::from(r[1] as i16));
    assert!((value - 2.0).abs() < 0.001, "inverse {r:04X?} = {value}");
}

#[test]
#[ignore]
fn dump_dsp1() {
    let d = dsp1().unwrap();
    let from: u16 = std::env::var("DUMP_FROM")
        .ok()
        .and_then(|v| u16::from_str_radix(&v, 16).ok())
        .unwrap_or(0);
    for a in from..from + 96 {
        let (m, o, _) = disassemble(d.program_word(a));
        println!("{a:03X} {:06X} {m} {o}", d.program_word(a));
    }
}
