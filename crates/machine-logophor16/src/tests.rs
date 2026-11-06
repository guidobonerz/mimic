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
        let f = m.framebuffer();
        let mut enc = png::Encoder::new(file, f.width, f.height);
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
    let f = m.framebuffer();
    assert_eq!(
        (info.width, info.height),
        (f.width, f.height),
        "{name}: picture size"
    );
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
    // The Jacquard scene: a gradient line by line, split at line 300.
    frames(&mut m, 160);
    golden(&m, "foundry-jacquard");
    // The Lithograph scene: a scrolling background, balls with shadows.
    frames(&mut m, 180);
    golden(&m, "foundry-lithograph");
    // The sprite scene: 256 sprites over the hardware-scrolled background.
    frames(&mut m, 180);
    golden(&m, "foundry-sprites");
    // The 3D scene: the Stereoscope's brass zeppelin in mode 1.
    frames(&mut m, 200);
    assert_eq!(m.board.lumiere.mode(), 1);
    golden(&m, "foundry-zeppelin");
    assert_eq!(
        String::from_utf8_lossy(m.console()),
        "Foundry (step 10): boot\n"
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
    fn write(&mut self, o: u64, d: &[u8]) -> Result<(), MediaError> {
        let o = o as usize;
        self.0[o..o + d.len()].copy_from_slice(d);
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
    for _ in 0..(LINE_CYCLES as usize * 10 + 30 * usize::from(CLK)) {
        m.step_cycle();
    }
    let peek = |m: &Logophor, a: u32| DebugTarget::peek(m, CPU_SPACE, a, Size::Word).unwrap();
    assert_eq!(peek(&m, 0xffff_0818), 10, "BEAMY");
    assert_eq!(peek(&m, 0xffff_081a), 100, "BEAMX: 30 x CLOCK cycles");
    assert_eq!(peek(&m, 0xffff_43f4), 0xc016, "DBGID");
    assert_eq!(
        peek(&m, 0xfffb_0000),
        u32::from(u16::from_le_bytes([FOUNDRY[0], FOUNDRY[1]]))
    );
    assert_eq!(peek(&m, 0x0200_0000), 0xffff, "unpopulated");
    assert_eq!(
        m.now(),
        Cycle((10 * u64::from(LINE_CYCLES) + 30 * u64::from(CLK)) * 40)
    );
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

/// A program that sets up mode 0 (colour 2 green), framebuffer and Jacquard
/// DMA with the list at 0005:0000, enables interrupts as `intena` says,
/// and loops.
fn jacquard_program(intena: u16) -> String {
    format!(
        "
        li r1, #$ffff
        mts s3, r1
        li r2, #$1004
        li r1, #$07e0
        swx r1, (r2)          ; palette 2: green
        li r2, #$0020
        li r1, #5
        swx r1, (r2)          ; JAQ1SEG
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
        swx r1, (r2)          ; DMACON: framebuffer, Jacquard
        ei
    stop: bra stop
        "
    )
}

/// A Jacquard list (words) at 0005:0000 and colour 2 over the mode-0
/// picture at 0002:0000.
fn jacquard_setup(m: &mut Logophor, list: &[u16]) {
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
fn jacquard_writes_land_on_their_pixel() {
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
    insert_program(&mut m, assemble(&jacquard_program(0)));
    jacquard_setup(&mut m, &list);
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
fn jacquard_skip_jump_and_stop() {
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
    insert_program(&mut m, assemble(&jacquard_program(0)));
    jacquard_setup(&mut m, &list);
    for (i, w) in list2.iter().enumerate() {
        let at = 0x0005_1000 + 2 * i as u32;
        DebugTarget::poke(&mut m, CPU_SPACE, at, Size::Word, u32::from(*w));
    }
    m.board.babbage.write(babbage::JAQ2SEG, 5);
    m.board.babbage.write(babbage::JAQ2OFS, 0x1000);
    frames(&mut m, 3);
    assert_eq!(m.board.lumiere.palette()[2], 0x001f);
}

#[test]
fn jacquard_irq_and_the_vblank_reach_the_cpu() {
    let list = [0x2000 | 200, 0, 0xa000, 0x1234, 0xc000, 0];
    let mut m = machine();
    insert_program(
        &mut m,
        assemble(&jacquard_program(0x8000 | telegraph::INT_JACQUARD)),
    );
    jacquard_setup(&mut m, &list);
    frames(&mut m, 3);
    assert_eq!(m.board.babbage.peek(babbage::JAQIRQ), 0x1234);
    assert_eq!(m.cpu.cause, 12, "level 4: vector 12");
    assert_eq!(m.cpu.s[0], 0xfffb, "in the ROM's handler");
    // The VBlank alone.
    let mut m = machine();
    insert_program(
        &mut m,
        assemble(&jacquard_program(0x8000 | telegraph::INT_VBLANK)),
    );
    jacquard_setup(&mut m, &[0xc000, 0]);
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
fn a_jacquard_move_into_babbage_needs_copcon() {
    let list = [0x0002 >> 1, 0x8020, 0xc000, 0]; // MOVE DMACON
    let mut m = machine();
    insert_program(
        &mut m,
        assemble(&jacquard_program(0x8000 | telegraph::INT_DMAERR)),
    );
    jacquard_setup(&mut m, &list);
    frames(&mut m, 3);
    assert_ne!(
        m.board.babbage.peek(babbage::DMAERR) & babbage::DMA_JACQUARD,
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
        // 100 visible lines (a whole field would overflow the counter)
        for _ in 0..LINE_CYCLES as usize * 50 {
            m.step_cycle();
        }
        let before = m.cpu.r[3];
        for _ in 0..LINE_CYCLES as usize * 100 {
            m.step_cycle();
        }
        m.cpu.r[3].wrapping_sub(before)
    };
    let (chip, fast) = (count(0x0010), count(0x0080));
    assert!(chip < fast, "chip {chip}, fast {fast}");
    assert!(chip > fast * 3 / 4, "chip {chip}, fast {fast}");
}

#[test]
fn savestate_in_the_jacquard_scene_resumes_identically() {
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

/// A program that fills 64 x 64 bytes at 0003:0000 with the Lithograph (CPU
/// start), with the Lithograph interrupt enabled, then loops counting in R3.
/// `buscon`: BUSCON's value.
fn lithograph_program(buscon: u16) -> String {
    format!(
        "
        li r1, #$ffff
        mts s3, r1
        li r2, #$0006
        li r1, #{buscon}
        swx r1, (r2)          ; BUSCON
        li r2, #$4282
        li r1, #$8008
        swx r1, (r2)          ; INTENA: Lithograph
        li r2, #$0002
        li r1, #$c020
        swx r1, (r2)          ; DMACON: Lithograph
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
        li r1, #64
        swx r1, (r2)          ; LTHDPITCH
        li r2, #$0116
        swx r1, (r2)          ; LTHW
        li r2, #$0118
        swx r1, (r2)          ; LTHH
        li r2, #$011c
        li r1, #$5a
        swx r1, (r2)          ; LTHCOLOR
        li r2, #$0124
        swx r1, (r2)          ; LTHSTART
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
fn the_lithograph_fills_in_free_slots_and_interrupts_when_done() {
    let mut m = machine();
    insert_program(&mut m, assemble(&lithograph_program(0)));
    frames(&mut m, 2);
    let peek = |m: &Logophor, a: u32| DebugTarget::peek(m, CPU_SPACE, a, Size::Byte).unwrap();
    assert_eq!(peek(&m, 0x0003_0000), 0x5a);
    assert_eq!(peek(&m, 0x0003_0000 + 63 * 64 + 63), 0x5a);
    assert_eq!(peek(&m, 0x0003_0000 + 64 * 64), 0, "only 64 rows");
    assert_eq!(m.cpu.cause, 11, "level 3 (Lithograph): vector 11");
    assert_ne!(
        m.board.ints.peek(telegraph::INTREQ) & telegraph::INT_LITHOGRAPH,
        0
    );
}

#[test]
fn buscon_lets_the_cpu_before_the_lithograph() {
    // A long fill (256 x 256) while the CPU stores into chip RAM: with
    // BUSCON bit 0 the CPU gets further in the same time.
    let run = |buscon: u16| {
        let src = lithograph_program(buscon)
            .replace("li r1, #64\n", "li r1, #256\n")
            .replace("li r2, #$4282", "li r2, #$4280"); // no interrupt
        let mut m = machine();
        insert_program(&mut m, assemble(&src));
        for _ in 0..3_000 {
            m.step_cycle();
        }
        (m.cpu.r[3], m.board.babbage.lithograph.busy())
    };
    let ((slow, busy0), (fast, busy1)) = (run(0), run(1));
    assert!(busy0 && busy1, "the fill still runs");
    assert!(fast > slow, "CPU first {fast}, Lithograph first {slow}");
}

#[test]
fn the_jacquard_starts_a_blit_and_waits_for_it() {
    // The Jacquard starts the prepared fill (LITH), then WAIT with the
    // Lithograph bit before setting palette 2: it lands after the fill.
    // Each field: palette 2 green, LITH, WAIT for the Lithograph, blue.
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
    insert_program(&mut m, assemble(&jacquard_program(0)));
    jacquard_setup(&mut m, &list);
    for (r, v) in [
        (chips_logophor::lithograph::LTHCON0, 0x0100),
        (chips_logophor::lithograph::LTHCON1, 1),
        (chips_logophor::lithograph::LTHDSEG, 7),
        (chips_logophor::lithograph::LTHDPITCH, 1024),
        (chips_logophor::lithograph::LTHW, 1024),
        (chips_logophor::lithograph::LTHH, 1024),
        (chips_logophor::lithograph::LTHCOLOR, 0x33),
    ] {
        m.board.babbage.write(r, v);
    }
    m.board
        .babbage
        .write(babbage::DMACON, 0x8000 | babbage::DMA_LITHOGRAPH);
    frames(&mut m, 3);
    let last = 0x0007_0000 + 1023 * 1024 + 1023;
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, last, Size::Byte),
        Some(0x33)
    );
    assert_eq!(m.board.lumiere.palette()[2], 0x001f);
    // The palette changes only after the fill's 1024 x 256 slots (about
    // 103 lines at 2 544 slots a line): line 61 is still green, line 160
    // blue.
    let px = pixels(&m);
    assert_eq!(
        px[61 * 800 + 400],
        lumiere::rgb565(0x07e0),
        "line 61: the fill runs"
    );
    assert_eq!(
        px[160 * 800 + 400],
        lumiere::rgb565(0x001f),
        "line 160: done"
    );
}

#[test]
fn savestate_in_the_lithograph_scene_resumes_identically() {
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
        press(m, ch);
        frames(m, 1);
    }
}

/// Type on `m` while `other` runs along (a field each per key).
fn type_on(m: &mut Logophor, other: &mut Logophor, text: &str) {
    for ch in text.chars() {
        press(m, ch);
        frames(m, 1);
        frames(other, 1);
    }
}

/// A key's press and release (US layout) into the input queue.
fn press(m: &mut Logophor, ch: char) {
    {
        let (usage, shift) = match ch {
            'a'..='z' => (4 + (ch as u8 - b'a'), false),
            'A'..='Z' => (4 + (ch as u8 - b'A'), true),
            '1'..='9' => (0x1e + (ch as u8 - b'1'), false),
            '0' => (0x27, false),
            '\n' => (0x28, false),
            ' ' => (0x2c, false),
            '-' => (0x2d, false),
            '.' => (0x37, false),
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

// ---- step 10: the network ----

/// A hub in memory: what one machine sends reaches the others' inboxes;
/// with a gateway (`ada-asm hub`'s) also DHCP, DNS and the test Internet.
#[derive(Default)]
struct HubState {
    inbox: Vec<std::collections::VecDeque<Vec<u8>>>,
    gateway: Option<(asm_ada::hub::Gateway, Box<dyn asm_ada::hub::Outside + Send>)>,
    /// machine 0's polls of the socket: the gateway's clock
    polls: u64,
}

impl HubState {
    fn broadcast(&mut self, frames: Vec<Vec<u8>>) {
        for f in frames {
            let d = asm_ada::hub::datagram(&f);
            for q in &mut self.inbox {
                q.push_back(d.clone());
            }
        }
    }
}

/// The Internet as the tests see it: `bbs.example` (93.184.216.34) answers
/// on port 23 with a greeting and sends every line that comes back in
/// capitals.
#[derive(Default)]
struct TestInternet {
    names: std::collections::VecDeque<(u32, Option<std::net::Ipv4Addr>)>,
    conns: std::collections::BTreeMap<u32, std::collections::VecDeque<u8>>,
    lines: std::collections::BTreeMap<u32, Vec<u8>>,
    radio: std::collections::BTreeMap<u32, RadioListener>,
}

/// `radio.example` (192.0.2.80), port 80: an Icecast-like station. A
/// request for `/stream` is redirected (302, a long Location like the real
/// stations' CDN links); the redirected request gets ICY headers and then
/// the MP3 test tone again and again, a metadata block with a title every
/// 8 192 bytes.
#[derive(Default)]
struct RadioListener {
    request: Vec<u8>,
    head_sent: bool,
    redirected: bool,
    pos: usize,
    since_meta: usize,
    out: std::collections::VecDeque<u8>,
}

const RADIO: std::net::Ipv4Addr = std::net::Ipv4Addr::new(192, 0, 2, 80);
const RADIO_METAINT: usize = 8192;
static TONE_MP3: &[u8] = include_bytes!("../../../tests/data/logophor16/tone.mp3");

impl RadioListener {
    fn fill(&mut self, want: usize) {
        if self.redirected {
            return; // the first connection ends after the redirect
        }
        if !self.head_sent {
            if !self.request.windows(4).any(|w| w == b"\r\n\r\n") {
                return;
            }
            self.head_sent = true;
            if self.request.starts_with(b"GET /stream ") {
                let target = format!(
                    "http://radio.example/live/stream.mp3?cid={}&token={}",
                    "0".repeat(60),
                    "x".repeat(120)
                );
                self.out.extend(
                    format!(
                        "HTTP/1.1 302 Found\r\nContent-Type: audio/mpeg\r\nLocation: {target}\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                );
                self.redirected = true;
                return;
            }
            self.out.extend(
                b"ICY 200 OK\r\nicy-name: Test Radio\r\ncontent-type: audio/mpeg\r\nicy-metaint: 8192\r\n\r\n",
            );
        }
        while self.out.len() < want {
            if self.since_meta == RADIO_METAINT {
                let mut meta = b"StreamTitle='Brass Band - Steam Waltz';".to_vec();
                meta.resize(meta.len().div_ceil(16) * 16, 0);
                self.out.push_back((meta.len() / 16) as u8);
                self.out.extend(meta);
                self.since_meta = 0;
            }
            self.out.push_back(TONE_MP3[self.pos]);
            self.pos = (self.pos + 1) % TONE_MP3.len();
            self.since_meta += 1;
        }
    }
}

const BBS: std::net::Ipv4Addr = std::net::Ipv4Addr::new(93, 184, 216, 34);

impl asm_ada::hub::Outside for TestInternet {
    fn resolve(&mut self, id: u32, name: &str) -> bool {
        let a = match name {
            "bbs.example" => Some(BBS),
            "radio.example" => Some(RADIO),
            _ => None,
        };
        self.names.push_back((id, a));
        true
    }
    fn resolved(&mut self) -> Option<(u32, Option<std::net::Ipv4Addr>)> {
        self.names.pop_front()
    }
    fn udp_send(&mut self, _: std::net::SocketAddrV4, _: std::net::SocketAddrV4, _: &[u8]) {}
    fn udp_recv(&mut self) -> Option<(std::net::SocketAddrV4, std::net::SocketAddrV4, Vec<u8>)> {
        None
    }
    fn udp_close(&mut self, _: std::net::SocketAddrV4) {}
    fn tcp_connect(&mut self, id: u32, to: std::net::SocketAddrV4) {
        if to == std::net::SocketAddrV4::new(RADIO, 80) {
            self.radio.insert(id, RadioListener::default());
            return;
        }
        if to == std::net::SocketAddrV4::new(BBS, 23) {
            let hello = b"Welcome to the test BBS\r\n".iter().copied().collect();
            self.conns.insert(id, hello);
        }
    }
    fn tcp_connect_state(&mut self, id: u32) -> asm_ada::hub::outside::Connect {
        if self.conns.contains_key(&id) || self.radio.contains_key(&id) {
            asm_ada::hub::outside::Connect::Open
        } else {
            asm_ada::hub::outside::Connect::Failed
        }
    }
    fn tcp_send(&mut self, id: u32, data: &[u8]) -> Option<usize> {
        if let Some(r) = self.radio.get_mut(&id) {
            r.request.extend_from_slice(data);
            return Some(data.len());
        }
        let q = self.conns.get_mut(&id)?;
        let line = self.lines.entry(id).or_default();
        for &b in data {
            line.push(b.to_ascii_uppercase());
            if b == b'\n' {
                q.extend(line.drain(..));
            }
        }
        Some(data.len())
    }
    fn tcp_recv(&mut self, id: u32, buf: &mut [u8]) -> asm_ada::hub::outside::TcpRead {
        use asm_ada::hub::outside::TcpRead;
        if let Some(r) = self.radio.get_mut(&id) {
            // as fast as the gateway takes it, a little at a time
            r.fill(buf.len().min(1024));
            let n = r.out.len().min(buf.len());
            if n == 0 {
                return if r.redirected {
                    TcpRead::Closed
                } else {
                    TcpRead::Nothing
                };
            }
            for (b, c) in buf.iter_mut().zip(r.out.drain(..n)) {
                *b = c;
            }
            return TcpRead::Data(n);
        }
        let Some(q) = self.conns.get_mut(&id) else {
            return TcpRead::Failed;
        };
        let n = q.len().min(buf.len());
        if n == 0 {
            return TcpRead::Nothing;
        }
        for (b, c) in buf.iter_mut().zip(q.drain(..n)) {
            *b = c;
        }
        TcpRead::Data(n)
    }
    fn tcp_shutdown(&mut self, _: u32) {}
    fn tcp_close(&mut self, id: u32) {
        self.radio.remove(&id);
        self.conns.remove(&id);
    }
}

struct HubNet {
    hub: std::sync::Arc<std::sync::Mutex<HubState>>,
    me: usize,
}

impl emu_core::host::NetBackend for HubNet {
    fn connect(&mut self, _: SocketKind, _: &str, _: u16) -> Result<NetHandle, host::NetError> {
        Ok(1)
    }
    fn listen(&mut self, _: u16) -> Result<NetHandle, host::NetError> {
        Err(host::NetError::Unsupported("listen".into()))
    }
    fn accept(&mut self, _: NetHandle) -> Result<Option<NetHandle>, host::NetError> {
        Ok(None)
    }
    fn state(&mut self, _: NetHandle) -> Result<ConnState, host::NetError> {
        Ok(ConnState::Connected)
    }
    fn send(&mut self, _: NetHandle, data: &[u8]) -> Result<usize, host::NetError> {
        if data.get(4) == Some(&0) {
            let mut hub = self.hub.lock().unwrap();
            for (k, q) in hub.inbox.iter_mut().enumerate() {
                if k != self.me {
                    q.push_back(data.to_vec());
                }
            }
            let now = hub.polls * 2 / 5;
            if let Some((gw, inet)) = &mut hub.gateway {
                let out = gw.frame(&data[5..data.len() - 4], now, inet.as_mut());
                hub.broadcast(out);
            }
        }
        Ok(data.len())
    }
    fn recv(&mut self, _: NetHandle, buf: &mut [u8]) -> Result<Recv, host::NetError> {
        let mut hub = self.hub.lock().unwrap();
        if self.me == 0 {
            // about 40 polls a field: 2/5 of a millisecond each
            hub.polls += 1;
            let now = hub.polls * 2 / 5;
            if let Some((gw, inet)) = &mut hub.gateway {
                let out = gw.poll(now, inet.as_mut());
                hub.broadcast(out);
            }
        }
        Ok(Recv::Data(match hub.inbox[self.me].pop_front() {
            Some(d) => {
                buf[..d.len()].copy_from_slice(&d);
                d.len()
            }
            None => 0,
        }))
    }
    fn close(&mut self, _: NetHandle) {}
    fn http_get(&mut self, _: &str) -> Result<host::HttpTicket, host::NetError> {
        Err(host::NetError::Unsupported("http".into()))
    }
    fn http_post(
        &mut self,
        _: &str,
        _: &str,
        _: &[u8],
    ) -> Result<host::HttpTicket, host::NetError> {
        Err(host::NetError::Unsupported("http".into()))
    }
    fn poll_http(
        &mut self,
        _: host::HttpTicket,
    ) -> Option<Result<host::HttpResponse, host::NetError>> {
        None
    }
    fn info(&mut self) -> Result<host::NetInfo, host::NetError> {
        Err(host::NetError::Unsupported("info".into()))
    }
    fn time(&mut self) -> host::HostTime {
        host::HostTime::default()
    }
}

fn chat_machines() -> (Logophor, Logophor) {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("asm/examples/chat.s"),
    )
    .unwrap();
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), assemble(&src))]).unwrap();
    let hub = std::sync::Arc::new(std::sync::Mutex::new(HubState {
        inbox: vec![Default::default(), Default::default()],
        ..HubState::default()
    }));
    let config = MachineConfig {
        options: [("net".into(), "hub:6516".into())].into(),
        ..MachineConfig::default()
    };
    let mut ms = (0..2).map(|me| {
        let mut m = Logophor::new(&config).unwrap();
        m.attach_host(HostServices::new(
            Some(Box::new(HubNet {
                hub: hub.clone(),
                me,
            })),
            None,
        ));
        insert_card(&mut m, img.clone());
        frames(&mut m, 200);
        m
    });
    (ms.next().unwrap(), ms.next().unwrap())
}

fn screen(m: &Logophor) -> String {
    (0..37).map(|r| text_row(m, r) + "\n").collect()
}

#[test]
fn two_machines_chat_over_the_hub() {
    let (mut a, mut b) = chat_machines();
    assert!(a.board.net.link() && b.board.net.link(), "link up");
    assert_eq!(text_row(&a, 0), "Logophor 16 chat over Telegraph's network");
    type_text(&mut a, "ada\n");
    type_text(&mut b, "bab\n");
    type_text(&mut a, "hello from a\n");
    frames(&mut b, 5);
    assert!(screen(&b).contains("ada: hello from a"), "{}", screen(&b));
    type_text(&mut b, "hi\n");
    frames(&mut a, 5);
    assert!(screen(&a).contains("bab: hi"), "{}", screen(&a));
    assert!(
        !screen(&a).contains("ada: hello"),
        "no echo of its own frames"
    );
    // The chat polls: NETIRQ shows the frames, no interrupt is enabled.
    let irq = a.board.net.peek(telegraph_net::NETIRQ);
    assert_eq!(irq & (telegraph_net::IRQ_RX | telegraph_net::IRQ_TX), 3);
    assert_eq!(
        a.board.ints.peek(telegraph::INTREQ) & telegraph::INT_NETWORK,
        0
    );
    golden(&b, "chat");
}

#[test]
fn without_a_hub_the_link_stays_down() {
    let config = MachineConfig {
        options: [
            ("net".into(), "localhost:6516".into()),
            ("mac".into(), "02:00:00:00:00:2a".into()),
        ]
        .into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    frames(&mut m, 20);
    assert!(!m.board.net.link(), "no host services: no network");
    assert_eq!(m.board.net.mac(), [2, 0, 0, 0, 0, 0x2a]);
    for bad in ["net=nohost", "net=:5", "mac=1:2:3"] {
        let (k, v) = bad.split_once('=').unwrap();
        let c = MachineConfig {
            options: [(k.into(), v.into())].into(),
            ..MachineConfig::default()
        };
        assert!(Logophor::new(&c).is_err(), "{bad}");
    }
}

// ---- step 11a: Brass programs on the machine ----

#[test]
fn a_brass_program_boots_from_the_card_and_prints() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../brass/examples/primes");
    let prog =
        brass::build(&brass::DirSources { root }).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), prog.binary)]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 400);
    assert_eq!(text_row(&m, 0), "Brass on the Logophor 16");
    assert!(
        text_row(&m, 1).starts_with("2 3 5 7 11 13"),
        "{}",
        text_row(&m, 1)
    );
    assert_eq!(text_row(&m, 2), "primes below 100: 25");
    assert_eq!(m.board.exit, Some(0), "main returned");
    // After main the monitor (TRAP #0) takes over below the output.
    assert!(text_row(&m, 3).starts_with("Logophor 16 - Foundry emergency monitor"));
}

// ---- step 11b: Boiler ----

/// The system's packages from `system/` with `main` in its place.
fn system_with_main(main: &str) -> Vec<u8> {
    let sys = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("system");
    let mut pkgs = std::collections::BTreeMap::new();
    for e in std::fs::read_dir(&sys).unwrap() {
        let dir = e.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        for f in std::fs::read_dir(&dir).unwrap() {
            let p = f.unwrap().path();
            // sources only (the artwork lives there too)
            if !matches!(p.extension().and_then(|e| e.to_str()), Some("brass" | "s")) {
                continue;
            }
            files.push((
                p.display().to_string(),
                std::fs::read_to_string(&p).unwrap(),
            ));
        }
        files.sort();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        pkgs.insert(name, files);
    }
    pkgs.insert("main".into(), vec![("main.brass".into(), main.to_string())]);
    brass::build(&brass::MemSources(pkgs))
        .unwrap_or_else(|e| panic!("{}", e.join("\n")))
        .binary
}

/// The system (Boiler, Panorama and its main) compiled from `system/`.
fn system_binary() -> Vec<u8> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("system");
    brass::build(&brass::DirSources { root })
        .unwrap_or_else(|e| panic!("{}", e.join("\n")))
        .binary
}

#[test]
fn boiler_runs_tasks_channels_and_select() {
    let demo = system_with_main(include_str!("boiler_demo.brass"));
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), demo)]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 600);
    let console = String::from_utf8_lossy(m.console()).into_owned();
    assert!(
        console.contains("Boiler: tasks, channels, files\n0 1 4 9 16 \n"),
        "{console}"
    );
    assert!(
        console.contains("both spinners ran: true true"),
        "{console}"
    );
    assert!(
        console.contains("message from a task\nticks true"),
        "{console}"
    );
}

#[test]
fn boiler_reads_the_keyboard() {
    let sys = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("system/boiler");
    let mut pkgs = std::collections::BTreeMap::new();
    let mut boiler_files = Vec::new();
    for e in std::fs::read_dir(&sys).unwrap() {
        let p = e.unwrap().path();
        boiler_files.push((
            p.display().to_string(),
            std::fs::read_to_string(&p).unwrap(),
        ));
    }
    pkgs.insert("boiler".to_string(), boiler_files);
    pkgs.insert(
        "main".to_string(),
        vec![(
            "main.brass".to_string(),
            r#"
package main

import "boiler"

func main() {
    println("type:")
    for {
        c := boiler.Key()
        if c == 13 {
            break
        }
        print(string([]byte{c}))
    }
    println()
    println("done")
}
"#
            .to_string(),
        )],
    );
    let prog =
        brass::build(&brass::MemSources(pkgs)).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), prog.binary)]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 200);
    type_text(&mut m, "Gear 7\n");
    frames(&mut m, 10);
    let console = String::from_utf8_lossy(m.console()).into_owned();
    assert!(console.ends_with("type:\nGear 7\ndone\n"), "{console:?}");
}

// ---- step 11c: Panorama ----

fn mouse(m: &mut Logophor, dx: i16, dy: i16, buttons: u8) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PortId(0)),
        event: InputEvent::MouseRel { dx, dy, buttons },
    });
    frames(m, 3);
}

/// Move the pointer from `from` to `to` in steps the mouse counters can
/// carry between two ticks.
fn point(m: &mut Logophor, from: (i16, i16), to: (i16, i16)) {
    let (mut x, mut y) = from;
    while (x, y) != to {
        let dx = (to.0 - x).clamp(-100, 100);
        let dy = (to.1 - y).clamp(-100, 100);
        mouse(m, dx, dy, 0);
        x += dx;
        y += dy;
    }
}

/// The skin (tile.rs) as the system card carries it.
static SYSTEM_TILE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tile.raw"));

fn panorama() -> Logophor {
    let img = asm_ada::sdimage::fat32(
        64,
        &[
            ("BOOT/BOILER.BIN".into(), system_binary()),
            ("PANORAMA/TILE.RAW".into(), SYSTEM_TILE.to_vec()),
            ("README.TXT".into(), b"Logophor 16 system card".to_vec()),
            ("DOCS/BRASS.TXT".into(), vec![b'b'; 1234]),
            ("DOCS/GEARS/SPROCKET.DAT".into(), vec![7; 4096]),
        ],
    )
    .unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 400);
    m
}

#[test]
fn panorama_shows_its_desktop() {
    let m = panorama();
    golden(&m, "panorama-desktop");
}

fn click(m: &mut Logophor) {
    mouse(m, 0, 0, 1);
    mouse(m, 0, 0, 0);
}

#[test]
fn engine_room_runs_commands() {
    let mut m = panorama();
    // the menu's "Engine Room"
    point(&mut m, (512, 384), (60, 72));
    click(&mut m);
    frames(&mut m, 30);
    type_text(&mut m, "ls\n");
    type_text(&mut m, "ps\n");
    type_text(&mut m, "mem\n");
    frames(&mut m, 30);
    golden(&m, "panorama-engine-room");
}

#[test]
fn archive_browses_directories_in_columns() {
    let mut m = panorama();
    // the menu's "Archive", then DOCS, GEARS and SPROCKET.DAT
    point(&mut m, (512, 384), (60, 52));
    click(&mut m);
    frames(&mut m, 40);
    let row = |r: i16| 40 + 22 + 24 + 9 + 18 * r;
    point(&mut m, (60, 52), (226, row(3)));
    click(&mut m);
    frames(&mut m, 30);
    point(&mut m, (226, row(3)), (394, row(1)));
    click(&mut m);
    frames(&mut m, 30);
    point(&mut m, (394, row(1)), (562, row(0)));
    click(&mut m);
    frames(&mut m, 30);
    golden(&m, "panorama-archive");
}

/// Press the button at `from`, move to `to` with it held, let go.
fn drag(m: &mut Logophor, from: (i16, i16), to: (i16, i16)) {
    mouse(m, 0, 0, 1);
    let (mut x, mut y) = from;
    while (x, y) != to {
        let dx = (to.0 - x).clamp(-60, 60);
        let dy = (to.1 - y).clamp(-60, 60);
        mouse(m, dx, dy, 1);
        x += dx;
        y += dy;
    }
    mouse(m, 0, 0, 0);
}

#[test]
fn windows_move_minimise_and_the_theme_turns_to_the_oscilloscope() {
    let mut m = panorama();
    // Info from the dock, Engine Room from the menu
    point(&mut m, (512, 384), (994, 28));
    click(&mut m);
    frames(&mut m, 30);
    point(&mut m, (994, 28), (60, 72));
    click(&mut m);
    frames(&mut m, 30);
    // Info's title bar (behind Engine Room) to the lower right
    point(&mut m, (60, 72), (300, 50));
    drag(&mut m, (300, 50), (420, 380));
    frames(&mut m, 20);
    // Engine Room minimised: a tile at the bottom
    point(&mut m, (420, 380), (225, 71));
    click(&mut m);
    frames(&mut m, 20);
    golden(&m, "panorama-windows");
    // Theme: the submenu, torn off, then Oscilloscope
    point(&mut m, (225, 71), (60, 112));
    click(&mut m);
    frames(&mut m, 20);
    point(&mut m, (60, 112), (200, 110));
    drag(&mut m, (200, 110), (500, 220));
    frames(&mut m, 20);
    point(&mut m, (500, 220), (500, 262));
    click(&mut m);
    frames(&mut m, 60);
    golden(&m, "panorama-scope");
}

#[test]
fn the_system_card_boots_into_panorama() {
    let config = MachineConfig {
        options: [("card".into(), "system".into())].into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    frames(&mut m, 400);
    golden(&m, "panorama-desktop");
    let bad = MachineConfig {
        options: [("card".into(), "rom".into())].into(),
        ..MachineConfig::default()
    };
    assert!(Logophor::new(&bad).is_err());
}

// ---- step 11d: the network ----

/// Machines booting Panorama from the system card, on an in-memory hub
/// with the gateway; machine k has the MAC 02:4C:31:36:00:0(k+1).
fn net_machines(n: usize) -> Vec<Logophor> {
    net_machines_with(n, None)
}

/// Machines on the hub; with `card`, that card instead of the system card.
fn net_machines_with(n: usize, card: Option<Vec<u8>>) -> Vec<Logophor> {
    let hub = std::sync::Arc::new(std::sync::Mutex::new(HubState {
        inbox: vec![Default::default(); n],
        gateway: Some((
            asm_ada::hub::Gateway::new(),
            Box::new(TestInternet::default()),
        )),
        polls: 0,
    }));
    let mut ms: Vec<Logophor> = (0..n)
        .map(|me| {
            let mut options: std::collections::BTreeMap<String, String> = [
                ("net".into(), "hub:6516".into()),
                ("mac".into(), format!("02:4c:31:36:00:{:02x}", me + 1)),
            ]
            .into();
            if card.is_none() {
                options.insert("card".into(), "system".into());
            }
            let config = MachineConfig {
                options: options.into_iter().collect(),
                ..MachineConfig::default()
            };
            let mut m = Logophor::new(&config).unwrap();
            if let Some(img) = &card {
                insert_card(&mut m, img.clone());
            }
            m.attach_host(HostServices::new(
                Some(Box::new(HubNet {
                    hub: hub.clone(),
                    me,
                })),
                None,
            ));
            m
        })
        .collect();
    // boot side by side, then give DHCP its time
    for _ in 0..600 {
        for m in &mut ms {
            frames(m, 1);
        }
    }
    ms
}

#[test]
fn engine_room_shows_the_address_pings_and_looks_up_names() {
    let mut ms = net_machines(1);
    let m = &mut ms[0];
    point(m, (512, 384), (60, 72));
    click(m);
    frames(m, 30);
    type_text(m, "ifconfig\n");
    frames(m, 20);
    type_text(m, "ping hub\n");
    frames(m, 200);
    type_text(m, "host logophor-0001\n");
    frames(m, 120);
    type_text(m, "host nowhere.example\n");
    frames(m, 200);
    golden(m, "net-engine-room");
}

#[test]
fn teleprinter_calls_another_logophor() {
    let mut ms = net_machines(2);
    let (a, b) = ms.split_at_mut(1);
    let (a, b) = (&mut a[0], &mut b[0]);
    // the menu's Teleprinter on both; B waits for a call on port 23
    for m in [&mut *a, &mut *b] {
        point(m, (512, 384), (60, 92));
        click(m);
        frames(m, 30);
    }
    type_on(b, a, "listen 23\n");
    type_on(a, b, "logophor-0002 23\n");
    for _ in 0..200 {
        frames(a, 1);
        frames(b, 1);
    }
    type_on(a, b, "Hello from 1\n");
    for _ in 0..120 {
        frames(a, 1);
        frames(b, 1);
    }
    type_on(b, a, "Hello from 2\n");
    for _ in 0..120 {
        frames(a, 1);
        frames(b, 1);
    }
    golden(a, "net-teleprinter-1");
    golden(b, "net-teleprinter-2");
}

#[test]
fn teleprinter_reaches_the_internet_through_the_hub() {
    let mut ms = net_machines(1);
    let m = &mut ms[0];
    point(m, (512, 384), (60, 92));
    click(m);
    frames(m, 30);
    type_text(m, "bbs.example 23\n");
    frames(m, 200);
    type_text(m, "brass and steam\n");
    frames(m, 120);
    golden(m, "net-teleprinter-bbs");
}

#[test]
fn mode_5_runs_the_fields_with_xga_timing() {
    // Palette 1 green, mode 5 from 0002:0000 (whatever RAM holds there).
    let src = "
        li r1, #$ffff
        mts s3, r1
        li r2, #$0002
        li r1, #$c001
        swx r1, (r2)          ; DMACON: framebuffer DMA on
        li r2, #$1002
        li r1, #$07e0
        swx r1, (r2)          ; palette 1: green
        li r2, #$0802
        li r1, #2
        swx r1, (r2)          ; FBSEG
        li r2, #$0806
        li r1, #1024
        swx r1, (r2)          ; PITCH
        li r2, #$080a
        li r1, #1
        swx r1, (r2)          ; BORDER: palette 1
        li r2, #$0800
        li r1, #$8005
        swx r1, (r2)          ; display on, mode 5
    stop: bra stop
    ";
    let mut m = machine();
    insert_program(&mut m, assemble(src));
    frames(&mut m, 3);
    let f = m.framebuffer();
    assert_eq!((f.width, f.height), (1024, 768));
    assert_eq!(f.signal.lines_per_field, 806);
    // A field: 806 lines of 249 CPU cycles (40 master ticks a cycle).
    let t0 = m.now().0;
    frames(&mut m, 1);
    assert_eq!(m.now().0 - t0, 806 * u64::from(XGA.line_cycles) * 40);
    // A savestate taken in an XGA field resumes the same.
    let state = m.save_state();
    frames(&mut m, 2);
    let (after, t) = (pixels(&m), m.now());
    let mut n = machine();
    n.load_state(&state).unwrap();
    frames(&mut n, 2);
    assert_eq!(pixels(&n), after);
    assert_eq!(n.now(), t);
}

// ---- step 11e: games ----

#[test]
fn steamworks_deals_levels_that_can_be_solved() {
    let prog = system_with_main(
        r#"
package main

import "steamworks"

func main() {
    var b steamworks.Board
    var lit [steamworks.H][steamworks.W]bool
    solved, before := 0, 0
    for n := 1; n <= 20; n++ {
        b.Deal(n)
        if b.Steam(&lit) {
            before++
        }
        b.Solve()
        if b.Steam(&lit) {
            solved++
        }
    }
    println("solved", solved, "before", before)
    // a quarter turn more on one piece that is not round
    b.Deal(3)
    b.Solve()
    for y := range steamworks.H {
        for x := range steamworks.W {
            p := b.Pipes[y][x]
            if p != 5 && p != 10 && p != 15 {
                b.Click(x, y)
                println("one turn off", b.Steam(&lit))
                return
            }
        }
    }
}
"#,
    );
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), prog)]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 300);
    let console = String::from_utf8_lossy(m.console()).into_owned();
    assert!(
        console.contains("solved 20 before 0\none turn off false\n"),
        "{console}"
    );
}

#[test]
fn steamworks_shows_its_board_and_turns_pieces() {
    let mut m = panorama();
    // the dock's eighth tile
    point(&mut m, (512, 384), (994, 340));
    click(&mut m);
    frames(&mut m, 60);
    // turn the top left piece twice and the one below once
    let at = |x: i16, y: i16| (190 + 12 + 16 + 48 * x + 24, 40 + 22 + 24 + 48 * y + 24);
    point(&mut m, (994, 340), at(0, 0));
    click(&mut m);
    frames(&mut m, 20);
    click(&mut m);
    frames(&mut m, 20);
    point(&mut m, at(0, 0), at(0, 1));
    click(&mut m);
    frames(&mut m, 40);
    golden(&m, "steamworks");
}

#[test]
fn pathfinder_levels_can_all_be_solved_by_its_rules() {
    let prog = system_with_main(
        r#"
package main

import "pathfinder"

// Depth first over every tower the player can reach and every direction.
func solve(g *pathfinder.Game, depth int) bool {
    if depth > 16 {
        return false
    }
    for i := range pathfinder.Size {
        v := g.Board[i]
        if v < 2 || v > 4 {
            continue
        }
        for d := range 4 {
            g.Cursor = i
            g.Choose()
            if g.State != pathfinder.Dir {
                break
            }
            g.Tip(d)
            if g.State == pathfinder.Solved {
                return true
            }
            if g.Board[i] == v {
                g.Cancel()
                continue
            }
            if solve(g, depth+1) {
                return true
            }
            g.Undo()
        }
    }
    return false
}

func main() {
    var g pathfinder.Game
    for n := range pathfinder.Levels {
        g.Load(n)
        print(solve(&g, 0), " ")
    }
    println()
    g.Load(0)
    g.Cursor = 0
    g.Choose()
    println(g.Status)
}
"#,
    );
    let img = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), prog)]).unwrap();
    let mut m = machine();
    insert_card(&mut m, img);
    frames(&mut m, 1500);
    let console = String::from_utf8_lossy(m.console()).into_owned();
    assert!(
        console.contains(
            "true true true true true true true true true true 
No tower here
"
        ),
        "{console}"
    );
}

#[test]
fn pathfinder_shows_level_1_and_takes_the_start_tower() {
    let mut m = panorama();
    // the dock's seventh tile
    point(&mut m, (512, 384), (994, 288));
    click(&mut m);
    frames(&mut m, 60);
    type_text(
        &mut m, "
",
    );
    frames(&mut m, 30);
    golden(&m, "pathfinder");
}

/// Keys (USB HID usages) down or up at once.
fn keys(m: &mut Logophor, usages: &[u8], down: bool) {
    for &u in usages {
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: emu_core::input::MachineKey(u16::from(u)),
                down,
            },
        });
    }
}

#[test]
fn panorama_keeps_a_hidden_game_for_the_oscilloscope() {
    let mut m = panorama();
    point(&mut m, (512, 384), (60, 72));
    click(&mut m);
    frames(&mut m, 40);
    let chord = [0xe1, 0xe2, 0x2b, 0x12];
    // in the brass theme the chord does nothing
    let before = pixels(&m);
    keys(&mut m, &chord, true);
    frames(&mut m, 20);
    keys(&mut m, &chord, false);
    frames(&mut m, 40);
    // (the keys typed a capital O into Engine Room, nothing else)
    type_text(&mut m, "\n");
    frames(&mut m, 20);
    assert_ne!(pixels(&m), before);
    type_text(&mut m, "theme scope\n");
    frames(&mut m, 60);
    keys(&mut m, &chord, true);
    frames(&mut m, 20);
    keys(&mut m, &chord, false);
    frames(&mut m, 60);
    // a short flight: thrust, a turn, three shots
    keys(&mut m, &[0x52], true);
    frames(&mut m, 30);
    keys(&mut m, &[0x52], false);
    keys(&mut m, &[0x4f], true);
    frames(&mut m, 12);
    keys(&mut m, &[0x4f], false);
    for _ in 0..3 {
        keys(&mut m, &[0x2c], true);
        frames(&mut m, 4);
        keys(&mut m, &[0x2c], false);
        frames(&mut m, 8);
    }
    frames(&mut m, 10);
    golden(&m, "hidden-game");
}

#[test]
fn the_radio_plays_a_station_through_phonograph() {
    let card = asm_ada::sdimage::fat32(
        64,
        &[
            ("BOOT/BOILER.BIN".into(), system_binary()),
            ("PANORAMA/TILE.RAW".into(), SYSTEM_TILE.to_vec()),
            (
                "RADIO/STATIONS.TXT".into(),
                b"Test Radio|http://radio.example/stream\nNowhere|http://nowhere.example/\n"
                    .to_vec(),
            ),
        ],
    )
    .unwrap();
    let mut ms = net_machines_with(1, Some(card));
    let m = &mut ms[0];
    // the dock's sixth tile, then the first station
    point(m, (512, 384), (994, 236));
    click(m);
    frames(m, 60);
    point(m, (994, 236), (230, 40 + 22 + 28 + 10));
    click(m);
    frames(m, 600);
    let reg = |m: &Logophor, off: u32| {
        DebugTarget::peek(m, CPU_SPACE, 0xffff_0000 + off, Size::Word).unwrap()
    };
    assert_eq!(reg(m, 0x3202) & 1, 1, "Phonograph on");
    assert_eq!(reg(m, 0x3210), 4410, "44.1 kHz");
    assert!(
        (reg(m, 0x321c) << 16 | reg(m, 0x321e)) > 50,
        "frames decoded"
    );
    // the tone reaches the machine's sound
    let mut out = Vec::new();
    m.audio(&mut out);
    frames(m, 60);
    out.clear();
    m.audio(&mut out);
    let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(peak > 1000, "audible: peak {peak}");
    // real time: 38.3 frames a second at 44.1 kHz, the ring kept full
    let f0 = reg(m, 0x321e);
    frames(m, 60);
    let per_second = reg(m, 0x321e).wrapping_sub(f0);
    let fill = (reg(m, 0x320a) + 16384 - reg(m, 0x320c)) % 16384;
    assert!(per_second >= 37, "{per_second} frames a second");
    assert!(fill > 8192, "ring {fill} bytes");
    assert_eq!(reg(m, 0x320e) & 2, 0, "not starved");
    golden(m, "radio");
}

/// The radio on the real Internet through the hub's gateway and the host's
/// sockets (needs a network connection; the first built-in station).
#[test]
#[ignore]
fn the_radio_plays_a_real_station() {
    let hub = std::sync::Arc::new(std::sync::Mutex::new(HubState {
        inbox: vec![Default::default()],
        gateway: Some((
            asm_ada::hub::Gateway::new(),
            Box::new(asm_ada::hub::HostOutside::new()),
        )),
        polls: 0,
    }));
    let config = MachineConfig {
        options: [
            ("card".into(), "system".into()),
            ("net".into(), "hub:6516".into()),
        ]
        .into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    m.attach_host(HostServices::new(
        Some(Box::new(HubNet {
            hub: hub.clone(),
            me: 0,
        })),
        None,
    ));
    frames(&mut m, 600);
    point(&mut m, (512, 384), (994, 236));
    click(&mut m);
    frames(&mut m, 60);
    point(&mut m, (994, 236), (230, 40 + 22 + 28 + 10));
    click(&mut m);
    let reg = |m: &Logophor, off: u32| {
        DebugTarget::peek(m, CPU_SPACE, 0xffff_0000 + off, Size::Word).unwrap()
    };
    // up to 30 s of the machine's time for DNS, the redirect and the stream
    let mut frames_decoded = 0;
    for _ in 0..30 {
        frames(&mut m, 60);
        frames_decoded = reg(&m, 0x321c) << 16 | reg(&m, 0x321e);
        let log = hub.lock().unwrap().gateway.as_mut().unwrap().0.take_log();
        for l in log {
            eprintln!("hub: {l}");
        }
        eprintln!(
            "PHCON {:04x} PHSTAT {:04x} rate {} kbps {} frames {}",
            reg(&m, 0x3202),
            reg(&m, 0x320e),
            reg(&m, 0x3210),
            reg(&m, 0x3214),
            frames_decoded
        );
        if frames_decoded > 200 {
            break;
        }
    }
    assert!(frames_decoded > 200, "decoded {frames_decoded} frames");
    assert_eq!(reg(&m, 0x3210), 4800, "48 kHz (the ARD streams)");
}

// ---- net=host: the gateway in the machine ----

/// The host's network services as `net=host` sees them, in front of the
/// test Internet: connections by name or number.
#[derive(Default)]
struct TestHost {
    inet: TestInternet,
    next: NetHandle,
    /// the names the machine's connections went out by
    asked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl emu_core::host::NetBackend for TestHost {
    fn connect(
        &mut self,
        kind: SocketKind,
        name: &str,
        port: u16,
    ) -> Result<NetHandle, host::NetError> {
        use asm_ada::hub::Outside;
        if kind != SocketKind::Tcp {
            return Err(host::NetError::Unsupported("udp".into()));
        }
        self.asked.lock().unwrap().push(format!("{name}:{port}"));
        let ip = match name {
            "bbs.example" => BBS,
            "radio.example" => RADIO,
            n => n
                .parse()
                .map_err(|_| host::NetError::Resolve(n.to_string()))?,
        };
        self.next += 1;
        self.inet
            .tcp_connect(self.next, std::net::SocketAddrV4::new(ip, port));
        Ok(self.next)
    }
    fn listen(&mut self, _: u16) -> Result<NetHandle, host::NetError> {
        Err(host::NetError::Unsupported("listen".into()))
    }
    fn accept(&mut self, _: NetHandle) -> Result<Option<NetHandle>, host::NetError> {
        Ok(None)
    }
    fn state(&mut self, h: NetHandle) -> Result<ConnState, host::NetError> {
        use asm_ada::hub::{outside::Connect, Outside};
        Ok(match self.inet.tcp_connect_state(h) {
            Connect::Open => ConnState::Connected,
            Connect::Pending => ConnState::Connecting,
            Connect::Failed => ConnState::Failed(host::NetError::Connect("refused".into())),
        })
    }
    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, host::NetError> {
        use asm_ada::hub::Outside;
        self.inet.tcp_send(h, data).ok_or(host::NetError::BadHandle)
    }
    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, host::NetError> {
        use asm_ada::hub::{outside::TcpRead, Outside};
        match self.inet.tcp_recv(h, buf) {
            TcpRead::Data(n) => Ok(Recv::Data(n)),
            TcpRead::Nothing => Ok(Recv::Data(0)),
            TcpRead::Closed => Ok(Recv::Closed),
            TcpRead::Failed => Err(host::NetError::BadHandle),
        }
    }
    fn close(&mut self, h: NetHandle) {
        use asm_ada::hub::Outside;
        self.inet.tcp_close(h);
    }
    fn http_get(&mut self, _: &str) -> Result<host::HttpTicket, host::NetError> {
        Err(host::NetError::Unsupported("http".into()))
    }
    fn http_post(
        &mut self,
        _: &str,
        _: &str,
        _: &[u8],
    ) -> Result<host::HttpTicket, host::NetError> {
        Err(host::NetError::Unsupported("http".into()))
    }
    fn poll_http(
        &mut self,
        _: host::HttpTicket,
    ) -> Option<Result<host::HttpResponse, host::NetError>> {
        None
    }
    fn info(&mut self) -> Result<host::NetInfo, host::NetError> {
        Err(host::NetError::Unsupported("info".into()))
    }
    fn time(&mut self) -> host::HostTime {
        host::HostTime::default()
    }
}

#[test]
fn net_host_plays_the_radio_without_a_hub() {
    let card = asm_ada::sdimage::fat32(
        64,
        &[
            ("BOOT/BOILER.BIN".into(), system_binary()),
            ("PANORAMA/TILE.RAW".into(), SYSTEM_TILE.to_vec()),
            (
                "RADIO/STATIONS.TXT".into(),
                b"Test Radio|http://radio.example/stream\n".to_vec(),
            ),
        ],
    )
    .unwrap();
    let config = MachineConfig {
        options: [("net".into(), "host".into())].into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    insert_card(&mut m, card);
    let test_host = TestHost::default();
    let asked = test_host.asked.clone();
    m.attach_host(HostServices::new(Some(Box::new(test_host)), None));
    frames(&mut m, 600);
    // the dock's sixth tile, then the first station
    point(&mut m, (512, 384), (994, 236));
    click(&mut m);
    frames(&mut m, 60);
    point(&mut m, (994, 236), (230, 40 + 22 + 28 + 10));
    click(&mut m);
    frames(&mut m, 600);
    let reg = |m: &Logophor, off: u32| {
        DebugTarget::peek(m, CPU_SPACE, 0xffff_0000 + off, Size::Word).unwrap()
    };
    assert_eq!(reg(&m, 0x3210), 4410, "44.1 kHz");
    let f0 = reg(&m, 0x321e);
    frames(&mut m, 60);
    let per_second = reg(&m, 0x321e).wrapping_sub(f0);
    assert!(per_second >= 37, "{per_second} frames a second");
    assert_eq!(reg(&m, 0x320e) & 2, 0, "not starved");
    // DHCP and DNS stayed in the machine: the host saw the station by
    // name, twice (the redirect), and nothing else
    assert_eq!(
        *asked.lock().unwrap(),
        ["radio.example:80", "radio.example:80"]
    );
}

#[test]
fn closing_windows_keeps_panorama_running() {
    let mut m = panorama();
    // every app in the dock (Info, Archive, Engine Room, Teleprinter,
    // Radio, Pathfinder, Steamworks), each opened and closed with its
    // button three times
    for tile in 1..8 {
        let ty = 28 + 52 * (tile - 1);
        for round in 0..3 {
            point(&mut m, (512, 384), (994, ty));
            click(&mut m);
            frames(&mut m, 60);
            // its close button (the first window opens at 190, 40; the
            // right end by the application's width)
            let (cx, cy) = ([520, 712, 688, 688, 720, 568, 616][tile as usize - 1], 49);
            point(&mut m, (994, ty), (cx, cy));
            click(&mut m);
            frames(&mut m, 60);
            point(&mut m, (cx, cy), (512, 384));
            let c = String::from_utf8_lossy(m.console()).to_string();
            assert!(!c.contains("panic"), "tile {tile} round {round}: {c}");
            assert_eq!(
                m.framebuffer().width,
                1024,
                "tile {tile} round {round}: Panorama"
            );
        }
    }
}

#[test]
fn maximised_windows_grow_or_keep_their_content_in_the_middle() {
    let mut m = panorama();
    // Engine Room (opens at 190, 40, 512 wide): its maximise button
    point(&mut m, (512, 384), (60, 72));
    click(&mut m);
    frames(&mut m, 30);
    point(&mut m, (60, 72), (671, 51));
    click(&mut m);
    frames(&mut m, 60);
    type_text(&mut m, "ls\n");
    type_text(&mut m, "mem\n");
    frames(&mut m, 30);
    golden(&m, "panorama-maximised");
    // back to its size (the button is at the right end now)
    point(&mut m, (671, 51), (937, 15));
    click(&mut m);
    frames(&mut m, 60);
    // Pathfinder maximised: the board stays in the middle
    point(&mut m, (937, 15), (994, 288));
    click(&mut m);
    frames(&mut m, 60);
    golden(&m, "panorama-restored");
    point(&mut m, (994, 288), (579, 75));
    click(&mut m);
    frames(&mut m, 60);
    golden(&m, "panorama-maximised-game");
}
