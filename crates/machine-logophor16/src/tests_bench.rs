//! The quiet stretches (`Logophor::run_quiet`) against the machine run
//! cycle by cycle, and speed measurements.
//!
//! `cargo test --release -p machine-logophor16 bench -- --ignored --nocapture`
//! prints fields a second for the Foundry demo and Panorama, and checks
//! that everything the machine puts out (savestate, picture, sound,
//! console) is the same as with every cycle through `Logophor::cycle`.

use super::*;
use emu_core::input::{Dirs, MachineKey};
use std::time::Instant;

/// FNV-1a, 64 bit.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// Everything the machine puts out (draining its sound).
    fn machine(&mut self, m: &mut Logophor) {
        self.bytes(&m.save_state());
        if let PixelData::Rgb(p) = m.framebuffer().pixels {
            for c in p {
                self.bytes(&[c.r, c.g, c.b]);
            }
        }
        let mut audio = Vec::new();
        m.audio(&mut audio);
        for s in audio {
            self.bytes(&s.to_le_bytes());
        }
        self.bytes(m.console());
        self.bytes(&m.now().0.to_le_bytes());
    }
}

fn config(options: &[(&str, &str)]) -> MachineConfig {
    MachineConfig {
        options: options
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..MachineConfig::default()
    }
}

/// Input along the way: keys, the mouse, the joystick.
fn script(m: &mut Logophor) {
    let now = m.now().0;
    for (k, u) in [0x04u16, 0x05, 0x28, 0x2c].into_iter().enumerate() {
        let at = now + 1_000_000 * k as u64;
        for (at, down) in [(at, true), (at + 400_000, false)] {
            m.input(TimedInput {
                at: Cycle(at),
                target: InputTarget::Keyboard,
                event: InputEvent::Key {
                    machine_key: MachineKey(u),
                    down,
                },
            });
        }
    }
    m.input(TimedInput {
        at: Cycle(now + 123_457),
        target: InputTarget::Port(PortId(0)),
        event: InputEvent::MouseRel {
            dx: 40,
            dy: -13,
            buttons: 1,
        },
    });
    m.input(TimedInput {
        at: Cycle(now + 3_000_001),
        target: InputTarget::Port(PortId(1)),
        event: InputEvent::Digital {
            dirs: Dirs(5),
            buttons: 1,
        },
    });
}

/// `fields` fields with quiet stretches and cycle by cycle side by side,
/// input from `script` at field `input_at`; compared every `every`
/// fields. Returns fields a second of the quiet run and of the other.
fn agree(options: &[(&str, &str)], fields: usize, every: usize, input_at: usize) -> (f64, f64) {
    let mut a = Logophor::new(&config(options)).unwrap();
    let mut b = Logophor::new(&config(options)).unwrap();
    b.quiet_runs = false;
    let mut dbg = DebugHooks::new();
    let (mut ta, mut tb) = (0.0, 0.0);
    for f in 0..fields {
        if f == input_at {
            script(&mut a);
            script(&mut b);
        }
        let t = Instant::now();
        assert_eq!(a.run_frame(&mut dbg), StopReason::FrameDone);
        ta += t.elapsed().as_secs_f64();
        let t = Instant::now();
        assert_eq!(b.run_frame(&mut dbg), StopReason::FrameDone);
        tb += t.elapsed().as_secs_f64();
        if f % every == every - 1 {
            let (mut da, mut db) = (Digest::new(), Digest::new());
            da.machine(&mut a);
            db.machine(&mut b);
            assert!(da.0 == db.0, "{options:?}: field {f} differs");
        }
    }
    (fields as f64 / ta, fields as f64 / tb)
}

/// A short check that runs with the other tests.
#[test]
fn quiet_stretches_change_nothing() {
    agree(&[], 90, 3, 1000);
    agree(&[("card", "system")], 160, 4, 120);
}

#[test]
#[ignore = "benchmark: --ignored --nocapture"]
fn bench_foundry() {
    let (fast, slow) = agree(&[], 1800, 15, usize::MAX);
    println!("foundry: {fast:.1} fields/s ({slow:.1} cycle by cycle)");
}

#[test]
#[ignore = "benchmark: --ignored --nocapture"]
fn bench_panorama() {
    let (fast, slow) = agree(&[("card", "system")], 900, 10, 600);
    println!("panorama: {fast:.1} fields/s ({slow:.1} cycle by cycle)");
}

/// Single cycles, savestates taken mid-line and loaded into new machines,
/// and run_frame after them agree with the machine run cycle by cycle.
#[test]
fn quiet_stretches_after_single_cycles_and_savestates() {
    for options in [&[][..], &[("card", "system")][..]] {
        let mut a = Logophor::new(&config(options)).unwrap();
        let mut b = Logophor::new(&config(options)).unwrap();
        b.quiet_runs = false;
        let mut dbg = DebugHooks::new();
        for _ in 0..40 {
            a.run_frame(&mut dbg);
            b.run_frame(&mut dbg);
        }
        for round in 0..4 {
            for _ in 0..(1000 + 3337 * round) {
                a.step_cycle();
                b.step_cycle();
            }
            let s = a.save_state();
            assert!(s == b.save_state(), "{options:?}: after single cycles");
            let mut c = Logophor::new(&config(options)).unwrap();
            c.load_state(&s).unwrap();
            for _ in 0..3 {
                a.run_frame(&mut dbg);
                b.run_frame(&mut dbg);
                c.run_frame(&mut dbg);
            }
            let (mut da, mut db) = (Digest::new(), Digest::new());
            da.machine(&mut a);
            db.machine(&mut b);
            assert!(da.0 == db.0, "{options:?}: round {round}");
            assert!(c.save_state() == a.save_state(), "{options:?}: loaded");
        }
    }
}

/// A program in the program slot (fast RAM) with its own vector table:
/// mode 3 (fixed DMA on every line), timer 0 interrupting every 300 µs,
/// the Lithograph filling and the Stereoscope clearing (as `dmacon`
/// allows; restarted from every interrupt), BUSCON `buscon`, the CPU
/// storing into chip RAM and halting between.
fn stress_program(dmacon: u16, buscon: u16) -> Vec<u8> {
    let src = format!(
        "
        .org $00400000
        bra start
        .org $00400040
        .far handler, handler, handler, handler, handler, handler, handler, handler
        .far handler, handler, handler, handler, handler, handler, handler, handler
        .far handler, handler, handler, handler, handler, handler, handler, handler
        .far handler, handler, handler, handler, handler, handler, handler, handler
start:
        li r1, #$0040
        mtsc #6, r1           ; VBR: the table above
        li r1, #$ffff
        mts s3, r1
        li r2, #$0006
        li r1, #{buscon}
        swx r1, (r2)          ; BUSCON
        li r2, #$0800
        li r1, #$8003
        swx r1, (r2)          ; mode 3
        li r2, #$0002
        li r1, #{dmacon}
        swx r1, (r2)          ; DMACON
        li r2, #$4202
        li r1, #300
        swx r1, (r2)          ; T0 latch
        li r2, #$4200
        swx r1, (r2)          ; T0 count
        li r2, #$4204
        li r1, #$000b
        swx r1, (r2)          ; T0: run, reload, interrupt
        li r2, #$4282
        li r1, #$8059
        swx r1, (r2)          ; INTENA: VBlank, Lithograph, Stereoscope, timer
        li r2, #$0100
        li r1, #$0100
        swx r1, (r2)          ; LTHCON0: fill
        li r2, #$0102
        li r1, #1
        swx r1, (r2)          ; LTHCON1: 8 bit
        li r2, #$010c
        li r1, #3
        swx r1, (r2)          ; LTHDSEG
        li r2, #$0114
        li r1, #256
        swx r1, (r2)          ; LTHDPITCH
        li r2, #$0116
        li r1, #200
        swx r1, (r2)          ; LTHW
        li r2, #$0118
        li r1, #40
        swx r1, (r2)          ; LTHH
        li r2, #$011c
        li r1, #$5a
        swx r1, (r2)          ; LTHCOLOR
        li r2, #$0200
        movi r1, #2
        swx r1, (r2)          ; S3DLSEG
        li r2, #$0204
        movi r1, #4
        swx r1, (r2)          ; S3DTSEG
        li r2, #$0208
        li r1, #320
        swx r1, (r2)          ; S3DPITCH
        bl restart
        li r1, #$0010
        mts s1, r1
        movi r3, #0
        movi r4, #0
        movi r6, #31
        ei
loop:   sw r3, 0(r4)          ; chip RAM
        addi r3, #1
        lw r5, 2(r4)
        addi r4, #2
        tst r3, r6
        bne loop
        halt
        sw r3, 4(r4)          ; after the HALT: chip RAM again
        bra loop

restart:
        li r7, #$0124
        swx r7, (r7)          ; LTHSTART
        li r7, #$0210
        swx r7, (r7)          ; S3DSTART
        ret

handler:
        li r7, #$4280
        li r8, #$7fff
        swx r8, (r7)          ; INTREQ: all clear
        mfsc r8, #3
        cmpi r8, #11
        bne done
        bl restart
done:   rti
        "
    );
    let obj = asm_ada::assemble("stress.s", &src, &asm_ada::HostFiles).unwrap();
    let img = asm_ada::link(&[obj], asm_ada::Options::default()).unwrap();
    img.contiguous(0xff, 1 << 20).unwrap().1
}

struct Bytes(Vec<u8>);

impl emu_core::media::MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), MediaError> {
        b.copy_from_slice(&self.0[o as usize..o as usize + b.len()]);
        Ok(())
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The Lithograph or the Stereoscope taking slots alone or together, with
/// and without the CPU first, interrupts and HALT: the same as cycle by
/// cycle.
#[test]
fn quiet_stretches_with_the_lithograph_and_the_stereoscope() {
    for dmacon in [0xc021u16, 0xc041, 0xc061] {
        for buscon in [0u16, 1] {
            let prog = stress_program(dmacon, buscon);
            let mut ms = [machine(), machine()];
            ms[1].quiet_runs = false;
            for m in &mut ms {
                // the Stereoscope's list: CLEAR 7 over 320 x 240, END
                for (k, d) in [0x0a00_0007u32, 0, 0].iter().enumerate() {
                    for (i, b) in d.to_le_bytes().iter().enumerate() {
                        let a = 0x2_0000 + 4 * k as u32 + i as u32;
                        DebugTarget::poke(m, CPU_SPACE, a, Size::Byte, u32::from(*b));
                    }
                }
                let media = Media {
                    name: "stress.bin".into(),
                    kind: MediaKind::Program,
                    backend: Box::new(Bytes(prog.clone())),
                };
                m.media_slot(SLOT_PROGRAM).unwrap().insert(media).unwrap();
            }
            let mut dbg = DebugHooks::new();
            for f in 0..12 {
                let [a, b] = &mut ms;
                a.run_frame(&mut dbg);
                b.run_frame(&mut dbg);
                let (mut da, mut db) = (Digest::new(), Digest::new());
                da.machine(a);
                db.machine(b);
                assert!(
                    da.0 == db.0,
                    "DMACON {dmacon:04x} BUSCON {buscon}: field {f}"
                );
            }
            assert!(ms[0].cpu.r[3] > 100, "the program runs");
        }
    }
}

fn machine() -> Logophor {
    Logophor::new(&MachineConfig::default()).unwrap()
}

#[test]
#[ignore = "benchmark"]
fn bench_cpu_alone() {
    // the Foundry's VBlank poll loop, in fast RAM at 0040:0000; then the
    // same with a RAM word instead of INTREQ
    cpu_alone([0x9120u16, 0x2304, 0x013e, 0xa0fc], 0x4280, 0xffff);
    cpu_alone([0x9120u16, 0x2304, 0x013e, 0xa0fc], 0x4280, 0x0050);
    // addi r5, #1 / addi r6, #-1 / mov r7, r5 / bra back
    cpu_alone([0x1501, 0x16ff, 0x0758, 0xa0fc], 0, 0);
}

fn cpu_alone(prog: [u16; 4], r2: u16, s3: u16) {
    let mut m = machine();
    for (i, w) in prog.iter().enumerate() {
        m.board.write_byte(0x40_0000 + 2 * i as u32, *w as u8);
        m.board
            .write_byte(0x40_0001 + 2 * i as u32, (*w >> 8) as u8);
    }
    m.cpu.s[0] = 0x40;
    m.cpu.pc = 0;
    m.cpu.s[3] = s3;
    m.cpu.r[2] = r2;
    let t = Instant::now();
    let n = 100_000_000u64;
    for _ in 0..n {
        m.cpu.tick(&mut m.board);
    }
    let dt = t.elapsed().as_secs_f64();
    println!(
        "cpu alone: {:.2} ns a cycle, pc {:x}",
        dt * 1e9 / n as f64,
        m.cpu.pc
    );
}

#[test]
#[ignore = "benchmark"]
fn bench_machine_poll_loop() {
    // the poll loop of `bench_cpu_alone` as a program, display off
    for (r2, s3) in [(0x4280u16, 0xffffu16), (0x0050, 0x0050)] {
        let src = format!(
            "
            li r1, #${s3:04x}
            mts s3, r1
            li r2, #${r2:04x}
        loop: lwx r1, (r2)
            movi r3, #4
            tst r1, r3
            beq loop
            bra loop
            "
        );
        let obj = asm_ada::assemble("p.s", &src, &asm_ada::HostFiles).unwrap();
        let img = asm_ada::link(&[obj], asm_ada::Options::default()).unwrap();
        let prog = img.contiguous(0xff, 1 << 20).unwrap().1;
        let mut m = machine();
        let media = Media {
            name: "p.bin".into(),
            kind: MediaKind::Program,
            backend: Box::new(Bytes(prog)),
        };
        m.media_slot(SLOT_PROGRAM).unwrap().insert(media).unwrap();
        let mut dbg = DebugHooks::new();
        m.run_frame(&mut dbg);
        let t = Instant::now();
        for _ in 0..300 {
            m.run_frame(&mut dbg);
        }
        let dt = t.elapsed().as_secs_f64();
        println!(
            "poll loop: {:.1} fields/s, {:.2} ns a cycle",
            300.0 / dt,
            dt * 1e9 / (300.0 * 628.0 * 318.0)
        );
    }
}

#[test]
#[ignore = "benchmark: --ignored --nocapture"]
fn bench_savestate_in_panorama() {
    let mut m = Logophor::new(&config(&[("card", "system")])).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..900 {
        m.run_frame(&mut dbg);
    }
    let t = std::time::Instant::now();
    let mut len = 0;
    for _ in 0..20 {
        len = m.save_state().len();
    }
    let save = t.elapsed() / 20;
    let a = m.save_state();
    m.run_frame(&mut dbg);
    let b = m.save_state();
    let t = std::time::Instant::now();
    let d = emu_core::rewind::diff(&b, &a);
    eprintln!(
        "state {len} bytes, save {save:?}, diff {:?} ({} bytes), segments touched {}",
        t.elapsed(),
        d.len(),
        m.board.touched.iter().filter(|t| **t).count()
    );
}
