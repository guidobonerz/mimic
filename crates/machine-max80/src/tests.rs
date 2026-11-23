use super::*;

fn machine(options: &[(&str, &str)]) -> Max80 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Max80::new(&config).unwrap()
}

fn frames(m: &mut Max80, n: usize) {
    let mut dbg = DebugHooks::default();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn the_stub_loads_its_font_and_writes_its_message() {
    let mut m = machine(&[]);
    frames(&mut m, 10);
    assert!(m.screen_text().contains("NO ROM"), "{}", m.screen_text());
    assert_eq!(m.screen_text().lines().count(), 16);
    assert_eq!(m.board.font[usize::from(b'M') * 8], 0x82);
    assert!(m.pixels.contains(&1));
    // 64 columns are centred: nothing in the first 64 dots of a line.
    assert!(m
        .pixels
        .chunks(video::WIDTH as usize)
        .all(|l| l[..64].iter().all(|&p| p == 0)));
    // 264 lines of 120 characters: 59.97 fields a second.
    assert_eq!(m.field_clocks, 84_480);
}

#[test]
fn the_block_moves_and_its_first_kilobyte_changes() {
    let mut m = machine(&[]);
    let b = &mut m.board;
    // After reset: the block at 0 with the ROM, twice.
    assert_eq!(b.peek(0x0000, 0), 0xf3);
    assert_eq!(b.peek(0x0200, 0), 0xf3);
    b.write(0x0000, 0x55, 0);
    assert_eq!(b.peek(0x0000, 0), 0xf3, "the ROM takes no write");
    // RAM in the window.
    b.write(0x07dc, 0x01, 0);
    b.write(0x0000, 0x55, 0);
    assert_eq!(b.peek(0x0000, 0), 0x55);
    assert_eq!(b.ram[0], 0x55);
    // The second kilobyte of video RAM, then both font kilobytes.
    b.write(0x07dc, 0x02, 0);
    b.write(0x0010, 0x11, 0);
    assert_eq!(b.vram[0x410], 0x11);
    b.write(0x07dc, 0x04, 0);
    b.write(0x0010, 0x22, 0);
    assert_eq!(b.font[0x010], 0x22);
    b.write(0x07dc, 0x06, 0);
    b.write(0x0010, 0x33, 0);
    assert_eq!(b.font[0x410], 0x33);
    // The first video kilobyte at the block's end.
    b.write(0x0c05, 0x44, 0);
    assert_eq!(b.vram[5], 0x44);
    assert_eq!(b.peek(0x0c05, 0), 0x44);
    // The block to $F000: its RAM is A1 at the address without A15.
    b.write(0x07dc, 0xf1, 0);
    assert_eq!(b.io.vlatch, 0xf1);
    assert_eq!(b.ram[0x7dc], 0xf1, "a latch write lands in the RAM below");
    assert_eq!(b.peek(0x0000, 0), 0x55, "plain RAM where the block was");
    b.write(0xf400, 0x66, 0);
    assert_eq!(b.ram[0x7400], 0x66);
    b.write(0xfc05, 0x77, 0);
    assert_eq!(b.vram[5], 0x77);
    // The keyboard reads the matrix; a write goes to the RAM below.
    b.keys[6] = 0x01;
    assert_eq!(b.peek(0xf840, 0), 0x01);
    b.write(0xf840, 0x99, 0);
    assert_eq!(b.peek(0xf840, 0), 0x01);
    assert_eq!(b.ram[0x7840], 0x99);
}

#[test]
fn port_a_banks_the_four_chunks() {
    let mut m = machine(&[]);
    let b = &mut m.board;
    b.write(0x07dc, 0xf1, 0); // the block out of the way
    for (pa, tag) in [(0x00u8, 0xa1u8), (0x40, 0xa2), (0x80, 0xb1), (0xc0, 0xb2)] {
        b.write(0xf7fc, pa, 0);
        b.write(0x9000, tag, 0);
    }
    assert_eq!(b.ram[0x1000], 0xa1);
    assert_eq!(b.ram[CHUNK + 0x1000], 0xa2);
    assert_eq!(b.ram[2 * CHUNK + 0x1000], 0xb1);
    assert_eq!(b.ram[3 * CHUNK + 0x1000], 0xb2);
    // The low half by bits 5-4.
    b.write(0xf7fc, 0x30, 0);
    assert_eq!(b.peek(0x1000, 0), 0xb2);
    assert_eq!(b.peek(0x9000, 0), 0xa1);
    // The block's RAM is A1 whatever is banked in.
    b.write(0xf7fc, 0xc0, 0);
    b.write(0xf500, 0x12, 0);
    assert_eq!(b.ram[0x7500], 0x12);

    // 64 KB: bank B reads $FF and keeps nothing.
    let mut m = machine(&[("ram", "64k")]);
    let b = &mut m.board;
    b.write(0x07dc, 0xf1, 0);
    b.write(0xf7fc, 0x80, 0);
    b.write(0x9000, 0x12, 0);
    assert_eq!(b.peek(0x9000, 0), 0xff);
}

#[test]
fn wait_states_by_place() {
    let m = machine(&[]);
    let b = &m.board;
    assert_eq!(b.waits(0x4000), 0);
    assert_eq!(b.waits(0x0100), 0, "the ROM");
    assert_eq!(b.waits(0x0600), 0, "the block's RAM");
    assert_eq!(b.waits(0x0700), 2, "its slow part");
    assert_eq!(b.waits(0x07d8), 2, "a latch");
    assert_eq!(b.waits(0x07ec), 2, "a device");
    assert_eq!(b.waits(0x07f4), 0, "USTAT");
    assert_eq!(b.waits(0x0c00), 1, "video RAM");
}

#[test]
fn the_tick_interrupts_and_a_beep_sounds() {
    let mut m = machine(&[]);
    frames(&mut m, 2);
    assert!(
        m.board.io.int(),
        "the tick flag stands (interrupts are off)"
    );
    // A beep through the bus, then a tenth of a second of sound.
    let now = m.sched.now().0;
    m.board.write(0x07f8, 0, now);
    let mut out = Vec::new();
    m.audio(&mut out);
    out.clear();
    frames(&mut m, 6);
    m.audio(&mut out);
    let loud = out.iter().filter(|&&s| s != 0).count();
    // Three to four ticks of 16.4 ms at 48 kHz.
    assert!((2300..=3200).contains(&loud), "{loud}");
    // About 1953 Hz: the sign changes 3906 times a second.
    let flips = out
        .windows(2)
        .filter(|w| w[0] != 0 && w[1] != 0 && w[0] != w[1])
        .count();
    assert!((180..=260).contains(&flips), "{flips}");
    assert!(m.board.io.beeps.is_empty());
}

#[test]
fn keys_reach_the_matrix_and_text_is_typed() {
    let mut m = machine(&[]);
    m.type_text("aZ");
    let hold = Max80::key_clocks();
    let mut dbg = DebugHooks::default();
    let mut seen = Vec::new();
    while m.sched.now().0 < hold * 5 {
        m.run_frame(&mut dbg);
        seen.push((m.board.keys[0], m.board.keys[3], m.board.keys[7]));
    }
    assert!(seen.contains(&(0x02, 0, 0)), "a alone");
    assert!(seen.contains(&(0, 0x04, 0x01)), "Z with SHIFT");
    assert_eq!(m.board.keys, [0; 8]);
}

#[test]
fn savestate_round_trip() {
    let mut m = machine(&[]);
    frames(&mut m, 5);
    let state = m.save_state();
    frames(&mut m, 5);
    let (pc, now, pixels) = (m.cpu.pc, m.sched.now(), m.pixels.clone());
    let mut n = machine(&[]);
    n.load_state(&state).unwrap();
    assert_eq!(n.save_state(), state);
    frames(&mut n, 5);
    assert_eq!((n.cpu.pc, n.sched.now()), (pc, now));
    assert_eq!(n.pixels, pixels);
    // Corrupt data is refused, not a panic.
    for cut in [0, 10, state.len() / 2, state.len() - 1] {
        assert!(n.load_state(&state[..cut]).is_err());
    }
    let mut other = machine(&[("ram", "64k")]);
    assert!(other.load_state(&state).is_err());
}

#[test]
fn the_debugger_sees_memory_without_touching_it() {
    let mut m = machine(&[]);
    frames(&mut m, 3);
    // Reading SYSFLG through the debugger leaves the tick standing.
    assert!(m.board.io.int());
    let flags = DebugTarget::peek(&m, CPU_SPACE, 0x07e0, Size::Byte).unwrap();
    assert_eq!(flags & 0x80, 0x80);
    assert!(m.board.io.int());
    // The spaces: RAM as fitted, video and font RAM.
    assert_eq!(
        DebugTarget::peek(&m, FONT_SPACE, u32::from(b'O') * 8, Size::Byte),
        Some(0x7c)
    );
    assert_eq!(DebugTarget::peek(&m, FONT_SPACE, 0x800, Size::Byte), None);
    assert!(DebugTarget::poke(
        &mut m,
        RAM_SPACE,
        0x1_8000,
        Size::Word,
        0x1234
    ));
    assert_eq!(m.board.ram[3 * CHUNK], 0x34);
    assert!(!DebugTarget::poke(&mut m, CPU_SPACE, 0x07e0, Size::Byte, 0));
    assert!(DebugTarget::poke(
        &mut m,
        CPU_SPACE,
        0x5000,
        Size::Byte,
        0x42
    ));
    assert_eq!(m.peek_cpu(0x5000), 0x42);
    assert_eq!(m.disassemble(0x4044).mnemonic, "HALT");
}
