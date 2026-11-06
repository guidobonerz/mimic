//! The machine with its built-in Foundry ROM: the step-3 pictures against
//! golden images, the program slot, the debug port, savestates.
//!
//! Regenerate the golden images after an intended change with
//! `EMU_UPDATE_REFERENCE=1 cargo test -p machine-logophor16` and say why
//! in the commit.

use super::*;
use chips_logophor::telegraph_io;
use emu_core::frame::Rgb8;
use std::path::PathBuf;

fn machine() -> Logophor {
    Logophor::new(&MachineConfig::default()).unwrap()
}

fn frames(m: &mut Logophor, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn reference(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/reference/logophor16")
        .join(format!("{name}.png"))
}

fn pixels(m: &Logophor) -> Vec<Rgb8> {
    match m.framebuffer().pixels {
        PixelData::Rgb(p) => p.to_vec(),
        PixelData::Indexed { .. } => unreachable!("RGB frame"),
    }
}

/// Compare the frame with a golden PNG (exact), or write it.
fn golden(m: &Logophor, name: &str) {
    let path = reference(name);
    let px = pixels(m);
    if std::env::var_os("EMU_UPDATE_REFERENCE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(file, lumiere::WIDTH, lumiere::HEIGHT);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        let data: Vec<u8> = px.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
        w.write_image_data(&data).unwrap();
        return;
    }
    let dec =
        png::Decoder::new(std::fs::File::open(&path).unwrap_or_else(|e| {
            panic!("{}: {e} (EMU_UPDATE_REFERENCE=1 writes it)", path.display())
        }));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (lumiere::WIDTH, lumiere::HEIGHT));
    let differ = px
        .iter()
        .zip(buf.chunks(3))
        .filter(|(c, p)| [c.r, c.g, c.b] != [p[0], p[1], p[2]])
        .count();
    assert_eq!(
        differ,
        0,
        "{name}: {differ} pixels differ from {}",
        path.display()
    );
}

#[test]
fn foundry_shows_its_start_picture_and_six_scenes() {
    let mut m = machine();
    // The start picture (2 s), then no card: the demonstration.
    frames(&mut m, 60);
    golden(&m, "foundry-logo");
    frames(&mut m, 145);
    assert_eq!(m.board.lumiere.mode(), 0);
    golden(&m, "foundry-testpicture");
    frames(&mut m, 200);
    assert_eq!(m.board.lumiere.mode(), 4);
    golden(&m, "foundry-text");
    // The Copper scene: a gradient line by line, split at line 300.
    frames(&mut m, 160);
    golden(&m, "foundry-copper");
    // The Blitter scene: a scrolling background, balls with shadows.
    frames(&mut m, 180);
    golden(&m, "foundry-blitter");
    // The sprite scene: 256 sprites over the hardware-scrolled background.
    frames(&mut m, 180);
    golden(&m, "foundry-sprites");
    // The 3D scene: the Stereoscope's brass zeppelin in mode 1.
    frames(&mut m, 200);
    assert_eq!(m.board.lumiere.mode(), 1);
    golden(&m, "foundry-zeppelin");
    assert_eq!(
        String::from_utf8_lossy(m.console()),
        "Foundry (step 9): boot\n"
    );
}

#[test]
fn text_blinks_and_the_cursor_flashes() {
    let mut m = machine();
    frames(&mut m, 405);
    let a = pixels(&m);
    frames(&mut m, 32);
    let b = pixels(&m);
    assert_eq!(m.board.lumiere.mode(), 4);
    let differ: Vec<usize> = (0..a.len()).filter(|i| a[*i] != b[*i]).collect();
    let rows: std::collections::BTreeSet<usize> =
        differ.iter().map(|i| (i / 800 - 4) / 16).collect();
    assert_eq!(
        rows,
        [26usize, 28].into(),
        "only the blinking line and the cursor change"
    );
}

fn assemble(src: &str) -> Vec<u8> {
    let obj = asm_ada::assemble("t.s", src, &asm_ada::HostFiles).unwrap();
    let img = asm_ada::link(&[obj], asm_ada::Options::default()).unwrap();
    img.contiguous(0xff, 1 << 20).unwrap().1
}

struct Mem(Vec<u8>);

impl emu_core::media::MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), MediaError> {
        let o = o as usize;
        b.copy_from_slice(&self.0[o..o + b.len()]);
        Ok(())
    }
    fn write(&mut self, _o: u64, _d: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn insert_program(m: &mut Logophor, bytes: Vec<u8>) {
    let media = Media {
        name: "t.bin".into(),
        kind: MediaKind::Program,
        backend: Box::new(Mem(bytes)),
    };
    m.media_slot(SLOT_PROGRAM).unwrap().insert(media).unwrap();
}

#[test]
fn program_slot_runs_the_prime_program_on_the_machine() {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("asm/examples/primes.s"),
    )
    .unwrap();
    let mut m = machine();
    insert_program(&mut m, assemble(&src));
    frames(&mut m, 2);
    assert_eq!(m.board.exit, Some(0));
    let out = String::from_utf8_lossy(m.console()).to_string();
    assert!(out.starts_with("2 3 5 7 11 "), "{out}");
    assert!(out.ends_with("991 997 \n"));
}

#[test]
fn a_program_draws_in_mode_0_and_the_pointer_waits_for_the_vblank() {
    // Palette 1 red, a 320 x 240 picture of colour 1 at 0002:0000; the
    // pointer written mid-field shows from the next field.
    let src = "
        li r1, #$ffff
        mts s3, r1
        li r2, #$0002
        li r1, #$c001
        swx r1, (r2)          ; DMACON: framebuffer DMA on
        li r2, #$1002
        li r1, #$f800
        swx r1, (r2)          ; palette 1: red
        li r1, #2
        mts s3, r1
        movi r2, #0
        li r3, #320*240/2
        li r1, #$0101
    fill: swx r1, (r2)+
        addi r3, #-1
        bne fill
        li r1, #$ffff
        mts s3, r1
        li r2, #$0802
        li r1, #2
        swx r1, (r2)          ; FBSEG
        li r2, #$0806
        li r1, #320
        swx r1, (r2)          ; PITCH
        li r2, #$0800
        li r1, #$8000
        swx r1, (r2)          ; display on, mode 0
    stop: bra stop
    ";
    let mut m = machine();
    insert_program(&mut m, assemble(src));
    // The fill takes about a field; the pointer then waits for a VBlank.
    frames(&mut m, 3);
    let px = pixels(&m);
    let red = Rgb8 { r: 255, g: 0, b: 0 };
    assert_eq!(px[300 * 800 + 400], red);
    assert_eq!(px[300 * 800 + 79], Rgb8::default(), "border: palette 0");
    assert_eq!(px[59 * 800 + 400], Rgb8::default());
    assert_eq!(px[539 * 800 + 719], red);
}

#[test]
fn registers_beam_and_debug_port() {
    let mut m = machine();
    // Beam registers follow the line and cycle.
    for _ in 0..(LINE_CYCLES as usize * 10 + 30) {
        m.step_cycle();
    }
    let peek = |m: &Logophor, a: u32| DebugTarget::peek(m, CPU_SPACE, a, Size::Word).unwrap();
    assert_eq!(peek(&m, 0xffff_0818), 10, "BEAMY");
    assert_eq!(peek(&m, 0xffff_081a), 100, "BEAMX: 30 cycles x 10/3");
    assert_eq!(peek(&m, 0xffff_43f4), 0xc016, "DBGID");
    assert_eq!(
        peek(&m, 0xfffb_0000),
        u32::from(u16::from_le_bytes([FOUNDRY[0], FOUNDRY[1]]))
    );
    assert_eq!(peek(&m, 0x0200_0000), 0xffff, "unpopulated");
    assert_eq!(m.now(), Cycle((10 * u64::from(LINE_CYCLES) + 30) * 40));
    let regs = m.chips();
    assert_eq!(regs[0].name, "lumiere");
    assert!(DebugTarget::poke(
        &mut m,
        CPU_SPACE,
        0x0040_0000,
        Size::Word,
        0x1234
    ));
    assert!(
        !DebugTarget::poke(&mut m, CPU_SPACE, 0xfffb_0000, Size::Byte, 0),
        "ROM"
    );
    assert_eq!(m.disassemble(0xfffb_0000).mnemonic, "bra");
}

#[test]
fn savestate_resumes_identically() {
    let mut a = machine();
    frames(&mut a, 30);
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    frames(&mut a, 100);
    frames(&mut b, 100);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(pixels(&a), pixels(&b));
    let before = b.save_state();
    assert!(b.load_state(&state[..state.len() - 3]).is_err());
    assert_eq!(b.save_state(), before, "a failed load changes nothing");
}

#[test]
fn unknown_options_and_models_are_refused() {
    let mut c = MachineConfig::default();
    c.options.insert("ram".into(), "1m".into());
    assert!(Logophor::new(&c).is_err());
    let c = MachineConfig {
        model: Some("17".into()),
        ..MachineConfig::default()
    };
    assert!(Logophor::new(&c).is_err());
}

/// A program that sets up mode 0 (colour 2 green), framebuffer and Copper
/// DMA with the list at 0005:0000, enables interrupts as `intena` says,
/// and loops.
fn copper_program(intena: u16) -> String {
    format!(
        "
        li r1, #$ffff
        mts s3, r1
        li r2, #$1004
        li r1, #$07e0
        swx r1, (r2)          ; palette 2: green
        li r2, #$0020
        li r1, #5
        swx r1, (r2)          ; COP1SEG
        li r2, #$0802
        li r1, #2
        swx r1, (r2)          ; FBSEG
        li r2, #$0806
        li r1, #320
        swx r1, (r2)          ; PITCH
        li r2, #$0800
        li r1, #$8000
        swx r1, (r2)          ; mode 0
        li r2, #$4282
        li r1, #${intena:04x}
        swx r1, (r2)          ; INTENA
        li r2, #$0002
        li r1, #$c011
        swx r1, (r2)          ; DMACON: framebuffer, Copper
        ei
    stop: bra stop
        "
    )
}

/// A Copper list (words) at 0005:0000 and colour 2 over the mode-0
/// picture at 0002:0000.
fn copper_setup(m: &mut Logophor, list: &[u16]) {
    for (i, w) in list.iter().enumerate() {
        let at = 0x0005_0000 + 2 * i as u32;
        assert!(DebugTarget::poke(
            m,
            CPU_SPACE,
            at,
            Size::Word,
            u32::from(*w)
        ));
    }
    for a in 0..320 * 240 {
        DebugTarget::poke(m, CPU_SPACE, 0x0002_0000 + a, Size::Byte, 2);
    }
}

#[test]
fn copper_writes_land_on_their_pixel() {
    // WAIT (100, 400), MOVE palette 2 = red; WAIT (101, 0), MOVE = blue.
    let list = [
        0x2000 | 100,
        400,
        0x1004 >> 1,
        0xf800,
        0x2000 | 101,
        0,
        0x1004 >> 1,
        0x001f,
        0xc000,
        0,
    ];
    let mut m = machine();
    insert_program(&mut m, assemble(&copper_program(0)));
    copper_setup(&mut m, &list);
    frames(&mut m, 3);
    let px = pixels(&m);
    let (red, blue) = (lumiere::rgb565(0xf800), lumiere::rgb565(0x001f));
    assert_eq!(px[99 * 800 + 700], blue, "line 99: the last field's colour");
    assert_eq!(
        px[100 * 800 + 399],
        blue,
        "line 100 up to x 399: the colour before"
    );
    assert_eq!(px[100 * 800 + 400], red, "after WAIT x 400 from pixel 400");
    assert_eq!(px[101 * 800 + 80], blue);
}

#[test]
fn copper_skip_jump_and_stop() {
    // SKIP (0, 0) skips the MOVE to red; JUMP to list 2 sets blue, stops.
    let list = [
        0x4000,
        0,
        0x1004 >> 1,
        0xf800,
        0x6001,
        0,
        0x1004 >> 1,
        0xffff,
    ];
    let list2: [u16; 4] = [0x1004 >> 1, 0x001f, 0xc000, 0];
    let mut m = machine();
    insert_program(&mut m, assemble(&copper_program(0)));
    copper_setup(&mut m, &list);
    for (i, w) in list2.iter().enumerate() {
        let at = 0x0005_1000 + 2 * i as u32;
        DebugTarget::poke(&mut m, CPU_SPACE, at, Size::Word, u32::from(*w));
    }
    m.board.babbage.write(babbage::COP2SEG, 5);
    m.board.babbage.write(babbage::COP2OFS, 0x1000);
    frames(&mut m, 3);
    assert_eq!(m.board.lumiere.palette()[2], 0x001f);
}

#[test]
fn copper_irq_and_the_vblank_reach_the_cpu() {
    let list = [0x2000 | 200, 0, 0xa000, 0x1234, 0xc000, 0];
    let mut m = machine();
    insert_program(
        &mut m,
        assemble(&copper_program(0x8000 | telegraph::INT_COPPER)),
    );
    copper_setup(&mut m, &list);
    frames(&mut m, 3);
    assert_eq!(m.board.babbage.peek(babbage::COPIRQ), 0x1234);
    assert_eq!(m.cpu.cause, 12, "level 4: vector 12");
    assert_eq!(m.cpu.s[0], 0xfffb, "in the ROM's handler");
    // The VBlank alone.
    let mut m = machine();
    insert_program(
        &mut m,
        assemble(&copper_program(0x8000 | telegraph::INT_VBLANK)),
    );
    copper_setup(&mut m, &[0xc000, 0]);
    frames(&mut m, 2);
    assert_eq!(m.cpu.cause, 12);
    // The ROM's handler (the music player) took and acknowledged it, and
    // the program runs on.
    assert_eq!(
        m.board.ints.peek(telegraph::INTREQ) & telegraph::INT_VBLANK,
        0
    );
    assert_eq!(m.cpu.s[0], 0x0040);
}

#[test]
fn a_copper_move_into_babbage_needs_copcon() {
    let list = [0x0002 >> 1, 0x8020, 0xc000, 0]; // MOVE DMACON
    let mut m = machine();
    insert_program(
        &mut m,
        assemble(&copper_program(0x8000 | telegraph::INT_DMAERR)),
    );
    copper_setup(&mut m, &list);
    frames(&mut m, 3);
    assert_ne!(
        m.board.babbage.peek(babbage::DMAERR) & babbage::DMA_COPPER,
        0
    );
    assert_eq!(
        m.board.babbage.peek(babbage::DMACON) & 0x20,
        0,
        "not written"
    );
    assert_eq!(m.cpu.cause, 12, "DMA-error interrupt");
}

#[test]
fn the_cpu_waits_for_chip_bus_slots_during_the_fixed_dma() {
    // Mode 3 fetches 200 slots on every visible line: stores into chip RAM
    // wait for free slots, stores into fast RAM do not.
    let count = |seg: u32| {
        let src = format!(
            "
            li r1, #$ffff
            mts s3, r1
            li r2, #$0002
            li r1, #$c001
            swx r1, (r2)          ; framebuffer DMA
            li r2, #$0800
            li r1, #$8003
            swx r1, (r2)          ; mode 3
            li r1, #${seg:04x}
            mts s1, r1
            movi r3, #0
        loop: sw r3, 0(r2)
            addi r3, #1
            sw r3, 2(r2)
            sw r3, 4(r2)
            bra loop
            "
        );
        let mut m = machine();
        insert_program(&mut m, assemble(&src));
        frames(&mut m, 1);
        let before = m.cpu.r[3];
        frames(&mut m, 1);
        m.cpu.r[3].wrapping_sub(before)
    };
    let (chip, fast) = (count(0x0010), count(0x0080));
    assert!(chip < fast, "chip {chip}, fast {fast}");
    assert!(chip > fast * 3 / 4, "chip {chip}, fast {fast}");
}

#[test]
fn savestate_in_the_copper_scene_resumes_identically() {
    let mut a = machine();
    frames(&mut a, 555);
    for _ in 0..5000 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    frames(&mut a, 30);
    frames(&mut b, 30);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(pixels(&a), pixels(&b));
}

/// A program that fills 64 x 64 bytes at 0003:0000 with the Blitter (CPU
/// start), with the Blitter interrupt enabled, then loops counting in R3.
/// `buscon`: BUSCON's value.
fn blitter_program(buscon: u16) -> String {
    format!(
        "
        li r1, #$ffff
        mts s3, r1
        li r2, #$0006
        li r1, #{buscon}
        swx r1, (r2)          ; BUSCON
        li r2, #$4282
        li r1, #$8008
        swx r1, (r2)          ; INTENA: Blitter
        li r2, #$0002
        li r1, #$c020
        swx r1, (r2)          ; DMACON: Blitter
        li r2, #$0100
        li r1, #$0100
        swx r1, (r2)          ; BLTCON0: fill
        li r2, #$0102
        li r1, #1
        swx r1, (r2)          ; BLTCON1: 8 bit
        li r2, #$010c
        li r1, #3
        swx r1, (r2)          ; BLTDSEG
        li r2, #$0114
        li r1, #64
        swx r1, (r2)          ; BLTDPITCH
        li r2, #$0116
        swx r1, (r2)          ; BLTW
        li r2, #$0118
        swx r1, (r2)          ; BLTH
        li r2, #$011c
        li r1, #$5a
        swx r1, (r2)          ; BLTCOLOR
        li r2, #$0124
        swx r1, (r2)          ; BLTSTART
        li r1, #$0010
        mts s1, r1
        movi r3, #0
        ei
    count: sw r3, 0(r2)       ; into chip RAM 0010:xxxx
        addi r3, #1
        bra count
        "
    )
}

#[test]
fn the_blitter_fills_in_free_slots_and_interrupts_when_done() {
    let mut m = machine();
    insert_program(&mut m, assemble(&blitter_program(0)));
    frames(&mut m, 2);
    let peek = |m: &Logophor, a: u32| DebugTarget::peek(m, CPU_SPACE, a, Size::Byte).unwrap();
    assert_eq!(peek(&m, 0x0003_0000), 0x5a);
    assert_eq!(peek(&m, 0x0003_0000 + 63 * 64 + 63), 0x5a);
    assert_eq!(peek(&m, 0x0003_0000 + 64 * 64), 0, "only 64 rows");
    assert_eq!(m.cpu.cause, 11, "level 3 (Blitter): vector 11");
    assert_ne!(
        m.board.ints.peek(telegraph::INTREQ) & telegraph::INT_BLITTER,
        0
    );
}

#[test]
fn buscon_lets_the_cpu_before_the_blitter() {
    // A long fill (256 x 256) while the CPU stores into chip RAM: with
    // BUSCON bit 0 the CPU gets further in the same time.
    let run = |buscon: u16| {
        let src = blitter_program(buscon)
            .replace("li r1, #64\n", "li r1, #256\n")
            .replace("li r2, #$4282", "li r2, #$4280"); // no interrupt
        let mut m = machine();
        insert_program(&mut m, assemble(&src));
        for _ in 0..3_000 {
            m.step_cycle();
        }
        (m.cpu.r[3], m.board.babbage.blitter.busy())
    };
    let ((slow, busy0), (fast, busy1)) = (run(0), run(1));
    assert!(busy0 && busy1, "the fill still runs");
    assert!(fast > slow, "CPU first {fast}, Blitter first {slow}");
}

#[test]
fn the_copper_starts_a_blit_and_waits_for_it() {
    // The Copper starts the prepared fill (BLIT), then WAIT with the
    // Blitter bit before setting palette 2: it lands after the fill.
    // Each field: palette 2 green, BLIT, WAIT for the Blitter, blue.
    let list = [
        0x1004 >> 1,
        0x07e0,
        0x8000,
        0,
        0x2000,
        0x8000,
        0x1004 >> 1,
        0x001f,
        0xc000,
        0,
    ];
    let mut m = machine();
    insert_program(&mut m, assemble(&copper_program(0)));
    copper_setup(&mut m, &list);
    for (r, v) in [
        (chips_logophor::blitter::BLTCON0, 0x0100),
        (chips_logophor::blitter::BLTCON1, 1),
        (chips_logophor::blitter::BLTDSEG, 7),
        (chips_logophor::blitter::BLTDPITCH, 256),
        (chips_logophor::blitter::BLTW, 256),
        (chips_logophor::blitter::BLTH, 1024),
        (chips_logophor::blitter::BLTCOLOR, 0x33),
    ] {
        m.board.babbage.write(r, v);
    }
    m.board
        .babbage
        .write(babbage::DMACON, 0x8000 | babbage::DMA_BLITTER);
    frames(&mut m, 3);
    let last = 0x0007_0000 + 1023 * 256 + 255;
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, last, Size::Byte),
        Some(0x33)
    );
    assert_eq!(m.board.lumiere.palette()[2], 0x001f);
    // The palette changes only after the fill's 1024 x 64 slots (about 77
    // lines): line 61 is still green, line 120 blue.
    let px = pixels(&m);
    assert_eq!(
        px[61 * 800 + 400],
        lumiere::rgb565(0x07e0),
        "line 61: the fill runs"
    );
    assert_eq!(
        px[120 * 800 + 400],
        lumiere::rgb565(0x001f),
        "line 120: done"
    );
}

#[test]
fn savestate_in_the_blitter_scene_resumes_identically() {
    let mut a = machine();
    frames(&mut a, 745);
    for _ in 0..3000 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    frames(&mut a, 20);
    frames(&mut b, 20);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(pixels(&a), pixels(&b));
}

#[test]
fn savestate_in_the_sprite_scene_resumes_identically() {
    let mut a = machine();
    frames(&mut a, 925);
    for _ in 0..3000 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    frames(&mut a, 20);
    frames(&mut b, 20);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(pixels(&a), pixels(&b));
}

fn stereoscope_program(dmacon: u16) -> String {
    format!(
        "
        li r1, #$ffff
        mts s3, r1
        li r2, #$4282
        li r1, #$8010
        swx r1, (r2)          ; INTENA: Stereoscope
        li r2, #$0002
        li r1, #{dmacon}
        swx r1, (r2)          ; DMACON
        li r2, #$0200
        movi r1, #2
        swx r1, (r2)          ; S3DLSEG: list at 0002:0000
        li r2, #$0204
        movi r1, #3
        swx r1, (r2)          ; S3DTSEG: target at 0003:0000
        li r2, #$0208
        li r1, #320
        swx r1, (r2)          ; S3DPITCH
        li r2, #$0210
        swx r1, (r2)          ; S3DSTART
        ei
    idle: bra idle
        "
    )
}

/// CLEAR 7 over the whole 320 x 240 target, then END.
fn stereoscope_list(m: &mut Logophor) {
    for (k, d) in [0x0a00_0007u32, 0, 0].iter().enumerate() {
        for (i, b) in d.to_le_bytes().iter().enumerate() {
            DebugTarget::poke(
                m,
                CPU_SPACE,
                0x2_0000 + 4 * k as u32 + i as u32,
                Size::Byte,
                u32::from(*b),
            );
        }
    }
}

#[test]
fn the_stereoscope_runs_a_list_in_free_slots_and_interrupts() {
    let mut m = machine();
    stereoscope_list(&mut m);
    insert_program(&mut m, assemble(&stereoscope_program(0xc040)));
    for _ in 0..3_000 {
        m.step_cycle();
    }
    let peek = |m: &Logophor, a: u32| DebugTarget::peek(m, CPU_SPACE, a, Size::Byte).unwrap();
    assert!(m.board.babbage.stereo.busy(), "76 800 slots to go");
    assert_eq!(peek(&m, 0x3_0000 + 320 * 240 - 1), 7, "drawn at the start");
    frames(&mut m, 2);
    assert!(!m.board.babbage.stereo.busy());
    assert_eq!(m.cpu.cause, 11, "level 3 (Stereoscope): vector 11");
    // Without its DMA bit the unit gets no slots.
    let mut m = machine();
    stereoscope_list(&mut m);
    insert_program(&mut m, assemble(&stereoscope_program(0xc000)));
    frames(&mut m, 3);
    assert!(m.board.babbage.stereo.busy());
    assert_eq!(
        m.board.ints.peek(telegraph::INTREQ) & telegraph::INT_STEREOSCOPE,
        0
    );
}

#[test]
fn savestate_in_the_3d_scene_resumes_identically() {
    let mut a = machine();
    frames(&mut a, 1125);
    for _ in 0..3000 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    frames(&mut a, 20);
    frames(&mut b, 20);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(pixels(&a), pixels(&b));
}

fn calliope_program(dmacon: u16) -> String {
    format!(
        "
        li r1, #$ffff
        mts s3, r1
        li r2, #$4282
        li r1, #$8020
        swx r1, (r2)          ; INTENA: Calliope
        li r2, #$0002
        li r1, #{dmacon}
        swx r1, (r2)          ; DMACON
        li r2, #$3100
        movi r1, #127
        swx r1, (r2)          ; AUDMASTER
        li r2, #$3000
        movi r1, #2
        swx r1, (r2)          ; AUDSEG: 0002:0000
        li r2, #$3006
        movi r1, #4
        swx r1, (r2)          ; AUDLENL
        li r2, #$3008
        li r1, #250
        swx r1, (r2)          ; AUDPER: 16 kHz
        li r2, #$300a
        movi r1, #127
        swx r1, (r2)          ; AUDVOL
        li r2, #$300c
        movi r1, #16
        swx r1, (r2)          ; AUDPAN
        li r2, #$3016
        movi r1, #$41
        swx r1, (r2)          ; AUDCTL: play once, interrupt
        ei
    idle: bra idle
        "
    )
}

#[test]
fn calliope_plays_from_chip_ram_and_interrupts_at_the_end() {
    let run = |dmacon: u16| {
        let mut m = machine();
        for (i, b) in [0x40u8, 0xc0, 0x20, 0x10].iter().enumerate() {
            DebugTarget::poke(
                &mut m,
                CPU_SPACE,
                0x2_0000 + i as u32,
                Size::Byte,
                u32::from(*b),
            );
        }
        insert_program(&mut m, assemble(&calliope_program(dmacon)));
        frames(&mut m, 2);
        let mut out = Vec::new();
        let format = m.audio(&mut out);
        assert_eq!((format.sample_rate, format.channels), (48_000, 2));
        (m, out)
    };
    let (m, out) = run(0xc004);
    let left: Vec<i16> = out.iter().step_by(2).copied().collect();
    for s in [0x4000, -0x4000, 0x2000, 0x1000] {
        assert_eq!(
            left.iter().filter(|x| **x == s).count(),
            3,
            "{s:x}: three output samples"
        );
    }
    assert_eq!(m.cpu.cause, 14, "level 6 (Calliope): vector 14");
    assert_eq!(m.board.calliope.peek(0x3102), 1, "AUDIRQ: channel 0");
    // Without its DMA the channel gets no data.
    let (m, out) = run(0xc000);
    assert!(out.iter().all(|s| *s == 0));
    assert_eq!(m.cpu.s[0], 0x0040, "no interrupt");
}

#[test]
fn foundry_plays_music_and_resumes_it_from_a_savestate() {
    let mut a = machine();
    frames(&mut a, 825);
    let mut out = Vec::new();
    a.audio(&mut out);
    let rms = (out.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / out.len() as f64).sqrt();
    assert!(rms > 1000.0, "music: rms {rms}");
    for _ in 0..3000 {
        a.step_cycle();
    }
    a.audio(&mut Vec::new());
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    frames(&mut a, 30);
    frames(&mut b, 30);
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    a.audio(&mut sa);
    b.audio(&mut sb);
    // 30 fields of 1/60.09 s: about 23 960 stereo samples.
    assert!((47_800..=48_000).contains(&sa.len()), "{}", sa.len());
    assert_eq!(sa, sb);
    assert_eq!(a.save_state(), b.save_state());
}

// ---- step 9: Telegraph, the boot loader, the monitor ----

fn insert_card(m: &mut Logophor, img: Vec<u8>) {
    let media = Media {
        name: "card.img".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(img)),
    };
    m.media_slot(SLOT_SD).unwrap().insert(media).unwrap();
}

/// The text screen's row `r` (100 cells at 0001:1000) as a string.
fn text_row(m: &Logophor, r: u32) -> String {
    (0..100)
        .map(|c| {
            let a = 0x1_1000 + 2 * (r * 100 + c);
            DebugTarget::peek(m, CPU_SPACE, a, Size::Byte).unwrap() as u8 as char
        })
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn sysvar(m: &Logophor, off: u32) -> u16 {
    DebugTarget::peek(m, CPU_SPACE, 0x003e_0000 + off, Size::Word).unwrap() as u16
}

/// Type `text` (US layout) a key a field.
fn type_text(m: &mut Logophor, text: &str) {
    for ch in text.chars() {
        let (usage, shift) = match ch {
            'a'..='z' => (4 + (ch as u8 - b'a'), false),
            'A'..='Z' => (4 + (ch as u8 - b'A'), true),
            '1'..='9' => (0x1e + (ch as u8 - b'1'), false),
            '0' => (0x27, false),
            '\n' => (0x28, false),
            ' ' => (0x2c, false),
            ':' => (0x33, true),
            '?' => (0x38, true),
            _ => panic!("type {ch:?}"),
        };
        let mut key = |u: u8, down: bool| {
            let at = m.now();
            m.input(TimedInput {
                at,
                target: InputTarget::Keyboard,
                event: InputEvent::Key {
                    machine_key: emu_core::input::MachineKey(u16::from(u)),
                    down,
                },
            });
        };
        if shift {
            key(0xe1, true);
        }
        key(usage, true);
        key(usage, false);
        if shift {
            key(0xe1, false);
        }
        frames(m, 1);
    }
}

/// A boot program: TRAP #4 prints a line on the console, then the word at
/// the end of the 70 000-byte file (0041:116E, past the first segment
/// and many clusters) goes to DBGEXIT.
fn boot_file() -> Vec<u8> {
    let mut b = assemble(
        "
        li r1, #msg
        trap #4
        li r1, #$0041
        mts s3, r1
        li r2, #$116e
        lwx r1, (r2)
        li r2, #$ffff
        mts s3, r2
        li r2, #$43f2
        swx r1, (r2)
    idle: bra idle
    msg: .asciz \"Hello from the SD card\\n\"
        ",
    );
    b.resize(70_000, 0);
    b[69_998..].copy_from_slice(&0x5aa5u16.to_le_bytes());
    b
}

#[test]
fn foundry_boots_boiler_from_a_fat32_card() {
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), boot_file())]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 200);
    assert_eq!(m.board.exit, Some(0x5aa5), "the whole file loaded");
    assert_eq!(m.cpu.s[0], 0x0040);
    assert_eq!(text_row(&m, 0), "Hello from the SD card");
    assert_eq!(m.board.lumiere.mode(), 4, "TRAP #4 set the console up");
    // DMA slots: the SD controller used Telegraph's share of the fixed DMA.
    assert_eq!(m.board.sd.peek(telegraph_io::SDSTAT) & 2, 0, "idle");
}

#[test]
fn without_a_bootable_card_foundry_says_why_and_starts_over_on_a_change() {
    // A card without the file, then one that is no FAT32 volume.
    let img = asm_ada::sdimage::fat32(64, &[("README.TXT".into(), b"hi".to_vec())]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 405);
    assert_eq!(sysvar(&m, 4), 3, "no BOOT/BOILER.BIN");
    assert_eq!(m.board.lumiere.mode(), 4);
    assert_eq!(
        text_row(&m, 33).trim(),
        "No BOOT/BOILER.BIN on the SD card."
    );
    let mut m = machine();
    insert_card(&mut m, vec![0x5a; 1 << 20]);
    frames(&mut m, 200);
    assert_eq!(sysvar(&m, 4), 2, "no FAT32 volume");
    // No card: the demonstration; a card inserted makes Foundry start over
    // and boot it.
    let mut m = machine();
    frames(&mut m, 300);
    assert_eq!(sysvar(&m, 4), 1);
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), boot_file())]).unwrap();
    insert_card(&mut m, img);
    frames(&mut m, 700);
    assert_eq!(m.board.exit, Some(0x5aa5));
}

#[test]
fn a_key_at_the_start_picture_enters_the_monitor() {
    let mut m = machine();
    frames(&mut m, 30);
    type_text(&mut m, " ");
    frames(&mut m, 20);
    assert_eq!(m.board.lumiere.mode(), 4);
    assert!(text_row(&m, 0).starts_with("Logophor 16 - Foundry emergency monitor"));
    assert_eq!(text_row(&m, 1), ">", "the key was forgotten");
    type_text(&mut m, "e 2:10 41 42 43\n");
    type_text(&mut m, "m 2:10 1\n");
    assert_eq!(
        text_row(&m, 3),
        "0002:0010  41 42 43 00 00 00 00 00 00 00 00 00 00 00 00 00  ABC............."
    );
    type_text(&mut m, "d fffb:0 1\n");
    assert!(
        text_row(&m, 5).starts_with("FFFB:0000"),
        "{}",
        text_row(&m, 5)
    );
    assert!(text_row(&m, 5).contains("bra"), "{}", text_row(&m, 5));
    type_text(&mut m, "Q\n");
    assert_eq!(text_row(&m, 7), "?");
    golden(&m, "foundry-monitor");
}

#[test]
fn an_illegal_instruction_shows_the_registers_in_the_monitor() {
    let mut m = machine();
    insert_program(&mut m, assemble("li r7, #$1234\n .word $ffff\n"));
    frames(&mut m, 5);
    assert_eq!(text_row(&m, 1), "Exception $01 at 0040:0004");
    assert!(
        text_row(&m, 3).contains("R7   =1234"),
        "{}",
        text_row(&m, 3)
    );
    assert!(
        text_row(&m, 4).contains("PC   =0004"),
        "{}",
        text_row(&m, 4)
    );
}

#[test]
fn telegraph_options_timers_clock_and_ports() {
    let config = MachineConfig {
        options: [
            ("rtc".into(), "1000".into()),
            ("port1".into(), "mouse".into()),
        ]
        .into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    assert_eq!(m.board.timers.seconds(), 1000);
    assert_eq!(m.board.input.peek(telegraph_io::PORTCFG), 1 | 1 << 2);
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PortId(1)),
        event: InputEvent::MouseRel {
            dx: 5,
            dy: -2,
            buttons: 1,
        },
    });
    frames(&mut m, 61);
    assert_eq!(m.board.timers.seconds(), 1001, "a second of 60.09 fields");
    assert_eq!(
        (
            m.board.input.peek(telegraph_io::MOUSE1X),
            m.board.input.peek(telegraph_io::MOUSE1Y)
        ),
        (5, 0xfffe)
    );
    assert_ne!(
        m.board.ints.peek(telegraph::INTREQ) & telegraph::INT_PORTS,
        0
    );
    let bad = MachineConfig {
        options: [("port0".into(), "lightgun".into())].into(),
        ..MachineConfig::default()
    };
    assert!(Logophor::new(&bad).is_err());
    assert_eq!(m.ports().len(), 2);
    assert_eq!(m.media_slots()[1].name, "SD card");
}
