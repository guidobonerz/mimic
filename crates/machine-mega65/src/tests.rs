//! The MEGA65 with the stub ROM: banking, the I/O personalities, colour
//! RAM, the ROM protection, speeds, raster interrupts, savestates.

use super::*;
use crate::vic::Personality;

fn stub() -> Mega65 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    Mega65::new(&config).unwrap()
}

fn frames(m: &mut Mega65, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// An unmapped CPU access.
fn cpu(a: u16) -> u32 {
    UNMAPPED | u32::from(a)
}

#[test]
fn the_stub_rom_boots_through_the_c64_kernal_vector() {
    let mut m = stub();
    frames(&mut m, 2);
    assert_eq!(m.cpu.pc, stub::DONE);
    assert_eq!(m.screen_text(0x0800, 5), "MIMIC");
    assert_eq!(m.board.vic.personality, Personality::Mega65);
    assert_eq!(m.board.vic.regs[0x20], 2);
    assert_eq!(
        m.framebuffer().rgb_at(0, 0),
        Some(Rgb8 { r: 255, g: 0, b: 0 }),
        "the border colour"
    );
}

#[test]
fn fields_are_50_hz_of_the_master_clock() {
    let mut m = stub();
    frames(&mut m, 1);
    let start = m.now().0;
    frames(&mut m, 10);
    assert_eq!((m.now().0 - start) / 10, u64::from(LINE_CLOCKS * LINES));
}

#[test]
fn c64_banking_follows_the_port() {
    let mut m = stub();
    let b = &mut m.board;
    b.chip[0x2a000] = 0xba;
    b.chip[0x2e000] = 0x4b;
    b.chip[0x2d000] = 0xc4;
    b.chip[0xa000] = 0x11;
    // Port inputs pulled up: BASIC, KERNAL, I/O in.
    assert_eq!(b.peek(cpu(0xa000)), 0xba);
    assert_eq!(b.peek(cpu(0xe000)), 0x4b);
    // DDR all out, $36: BASIC out.
    b.write(cpu(0x0000), 0x2f);
    b.write(cpu(0x0001), 0x36);
    assert_eq!(b.peek(cpu(0xa000)), 0x11);
    assert_eq!(b.peek(cpu(0xe000)), 0x4b);
    // $33: the character ROM at $D000.
    b.write(cpu(0x0001), 0x33);
    assert_eq!(b.peek(cpu(0xd000)), 0xc4);
    // $30: all RAM.
    b.write(cpu(0x0001), 0x30);
    b.chip[0xe000] = 0x22;
    assert_eq!(b.peek(cpu(0xe000)), 0x22);
    // Writes under ROM go to RAM.
    b.write(cpu(0x0001), 0x37);
    b.write(cpu(0xa000), 0x33);
    assert_eq!(b.chip[0xa000], 0x33);
    assert_eq!(b.chip[0x2a000], 0xba);
}

#[test]
fn d030_maps_the_c65_rom() {
    let mut m = stub();
    let b = &mut m.board;
    b.chip[0x38000] = 0x38;
    b.chip[0x3a000] = 0x3a;
    b.chip[0x2c000] = 0x2c;
    b.chip[0x3e000] = 0x3e;
    // In the C64 personality $D030 is the C128's 2 MHz bit.
    b.write(cpu(0xd030), 0xb8);
    assert!(!b.vic.c128_fast && b.vic.regs[0x30] == 0);
    b.write(cpu(0xd02f), 0xa5);
    b.write(cpu(0xd02f), 0x96);
    assert_eq!(b.vic.personality, Personality::C65);
    b.write(cpu(0xd030), 0xb8);
    assert_eq!(
        [0x8000, 0xa000, 0xc000, 0xe000].map(|a| b.peek(cpu(a))),
        [0x38, 0x3a, 0x2c, 0x3e]
    );
    // Back to the C64 personality: the bits stay in force.
    b.write(cpu(0xd02f), 0x00);
    assert_eq!(b.vic.personality, Personality::C64);
    assert_eq!(b.peek(cpu(0x8000)), 0x38);
}

#[test]
fn the_rom_area_is_write_protected_and_mapped_writes_reach_ram() {
    let mut m = stub();
    let b = &mut m.board;
    let old = b.chip[0x2e000];
    b.write(0x2e000, !old);
    assert_eq!(b.chip[0x2e000], old);
    b.rom_protect = false;
    b.write(0x2e000, !old);
    assert_eq!(b.chip[0x2e000], !old);
    // Banks 4 and 5 are RAM.
    b.write(0x5_ffff, 0x77);
    assert_eq!(b.peek(0x5_ffff), 0x77);
    assert_eq!(b.peek(0x6_0000), 0, "nothing above 384 KB yet");
}

#[test]
fn colour_ram_is_mirrored_at_1f800_and_cram2k_covers_the_cias() {
    let mut m = stub();
    let b = &mut m.board;
    b.write(cpu(0xd800), 0x0e);
    assert_eq!(b.peek(0x0ff8_0000), 0x0e);
    assert_eq!(b.chip[0x1f800], 0x0e);
    b.write(0x1_f801, 0x05);
    assert_eq!(b.peek(cpu(0xd801)), 0x05);
    b.write(0x0ff8_0400, 0x09);
    // CRAM2K in the C65/MEGA65 personality: $DC00 is colour RAM $400.
    b.write(cpu(0xd02f), 0x47);
    b.write(cpu(0xd02f), 0x53);
    b.write(cpu(0xd030), 0x01);
    assert_eq!(b.peek(cpu(0xdc00)), 0x09);
    b.write(cpu(0xd030), 0x00);
    assert_ne!(b.peek(cpu(0xdc00)), 0x09, "the CIA again");
}

#[test]
fn the_io_personalities_are_reachable_by_their_28_bit_addresses() {
    let mut m = stub();
    let b = &mut m.board;
    // The palette is in the C65/MEGA65 personality only.
    b.write(0x0ffd_3100 + 6, 0x0f);
    assert_eq!(b.vic.palette[0][0][6], 0x0f);
    b.write(0x0ffd_0100 + 6, 0x99);
    assert_eq!(
        b.vic.palette[0][0][6], 0x0f,
        "C64: $D106 is a VIC-II mirror"
    );
    // KEY knocks in a row.
    b.write(cpu(0xd02f), 0x45);
    b.write(cpu(0xd02f), 0x54);
    assert_eq!(b.vic.personality, Personality::Ethernet);
    b.write(cpu(0xd02f), 0x47);
    assert_eq!(b.vic.personality, Personality::C64);
}

#[test]
fn speeds_follow_fast_vfast_c128_and_the_port() {
    let mut m = stub();
    let b = &mut m.board;
    assert_eq!(b.speed(), Speed::Mhz40, "as the hypervisor leaves it");
    b.write(cpu(0xd02f), 0x47);
    b.write(cpu(0xd02f), 0x53);
    b.write(cpu(0xd054), 0x00);
    assert_eq!(b.speed(), Speed::Mhz3_5);
    b.write(cpu(0xd031), 0x00);
    assert_eq!(b.speed(), Speed::Mhz1);
    b.write(cpu(0xd02f), 0x00);
    b.write(cpu(0xd030), 0x01);
    assert_eq!(b.speed(), Speed::Mhz2);
    b.write(cpu(0x0000), 65);
    assert_eq!(b.speed(), Speed::Mhz40);
    b.write(cpu(0x0000), 64);
    assert_eq!(b.speed(), Speed::Mhz2);
}

#[test]
fn cpu_cycles_a_frame_follow_the_speed() {
    let mut m = stub();
    // Count CPU ticks over one field at each speed (the stub sits in its
    // loop: JMP takes 3 cycles at every speed).
    let mut per_field = |speed: Speed| {
        frames(&mut m, 1);
        m.cpu.set_speed(speed);
        let regs = &mut m.board.vic.regs;
        let (d031, d054) = match speed {
            Speed::Mhz40 => (0x40, 0x40),
            Speed::Mhz3_5 => (0x40, 0),
            _ => (0, 0),
        };
        regs[0x31] = d031;
        regs[0x54] = d054;
        frames(&mut m, 1);
        let mut n = 0u64;
        while m.step(None).is_none() {
            n += 1;
        }
        n + 1
    };
    assert_eq!(per_field(Speed::Mhz1), u64::from(63 * LINES));
    assert_eq!(per_field(Speed::Mhz3_5), u64::from(227 * LINES));
    assert_eq!(per_field(Speed::Mhz40), u64::from(LINE_CLOCKS * LINES));
}

#[test]
fn raster_compare_raises_the_vic_irq() {
    let mut m = stub();
    let b = &mut m.board;
    b.write(cpu(0xd012), 100);
    b.write(cpu(0xd01a), 1);
    for line in 0..LINES as u16 {
        b.vic.line(line);
    }
    assert!(b.vic.irq());
    assert_eq!(b.peek(cpu(0xd019)) & 0x81, 0x81);
    b.write(cpu(0xd019), 1);
    assert!(!b.vic.irq());
}

#[test]
fn savestates_are_deterministic() {
    let mut m = stub();
    frames(&mut m, 3);
    m.board.chip[0x4_0000] = 0x5a;
    let state = m.save_state();
    frames(&mut m, 5);
    let later = (m.now(), m.cpu.clone(), m.board.chip.clone());
    let mut n = stub();
    n.load_state(&state).unwrap();
    assert_eq!(n.board.chip[0x4_0000], 0x5a);
    frames(&mut n, 5);
    assert_eq!((n.now(), n.cpu.clone(), n.board.chip.clone()), later);
    assert!(n.load_state(&state[..state.len() / 2]).is_err());
}

#[test]
fn the_debugger_sees_the_cpu_view_and_the_physical_space() {
    let mut m = stub();
    frames(&mut m, 1);
    let t = m.debug_target(CPU).unwrap();
    assert_eq!(t.peek(CPU_SPACE, 0xfffc, Size::Word), Some(0xe000));
    assert_eq!(t.peek(PHYS_SPACE, 0x2fffc, Size::Word), Some(0xe000));
    assert!(t.poke(PHYS_SPACE, 0x4_1234, Size::Byte, 0x42));
    assert_eq!(t.peek(PHYS_SPACE, 0x4_1234, Size::Byte), Some(0x42));
    assert_eq!(t.disassemble(0xe000).to_string(), "SEI");
}

/// Put `list` at `$0700` and start it with `trigger_reg` ($D700 or $D705).
fn dma_job(m: &mut Mega65, list: &[u8], trigger_reg: u16) {
    m.board.chip[0x700..0x700 + list.len()].copy_from_slice(list);
    let b = &mut m.board;
    b.write(cpu(0xd02f), 0x47);
    b.write(cpu(0xd02f), 0x53);
    // The tests' F018 lists have 11 bytes.
    b.write(cpu(0xd703), 0x00);
    b.write(cpu(0xd702), 0x00);
    b.write(cpu(0xd701), 0x07);
    b.write(cpu(trigger_reg), 0x00);
    let t = b.dma.take().unwrap();
    let map = m.cpu.map;
    let mut pc = m.cpu.pc;
    dma::run(&mut m.board, t, &map, &mut pc);
}

#[test]
fn dma_fills_and_copies_after_the_guides_examples() {
    let mut m = stub();
    // F018 (11 bytes): FILL+CHAIN 2000 x $20 at $0800, then FILL colour
    // RAM at $1F800 with 1.
    dma_job(
        &mut m,
        &[
            0x07, 0xd0, 0x07, 0x20, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, //
            0x03, 0xd0, 0x07, 0x01, 0x00, 0x00, 0x00, 0xf8, 0x01, 0x00, 0x00,
        ],
        0xd700,
    );
    assert!(m.board.chip[0x800..0x800 + 2000].iter().all(|&v| v == 0x20));
    assert_eq!(m.board.chip[0x800 + 2000], 0);
    assert_eq!(m.board.colour[1999], 1, "through the $1F800 mirror");
    // Copy the character set from $2D000 to $5000.
    m.board.chip[0x2d000..0x2e000].fill(0x3c);
    dma_job(
        &mut m,
        &[
            0x00, 0x00, 0x10, 0x00, 0xd0, 0x02, 0x00, 0x50, 0x00, 0x00, 0x00,
        ],
        0xd700,
    );
    assert!(m.board.chip[0x5000..0x6000].iter().all(|&v| v == 0x3c));
}

#[test]
fn enhanced_dma_with_megabyte_options_and_hold_reaches_the_io_area() {
    let mut m = stub();
    m.board.chip[0x900..0x904].copy_from_slice(&[1, 2, 3, 4]);
    // $81 $FF: destination megabyte $FF; copy 4 bytes from $0900 to
    // $FFD3020 with HOLD (bank $D | $10): the border colour gets 4.
    dma_job(
        &mut m,
        &[
            0x81, 0xff, 0x00, 0x00, 0x04, 0x00, 0x00, 0x09, 0x00, 0x20, 0x30, 0x1d, 0x00, 0x00,
        ],
        0xd705,
    );
    assert_eq!(m.board.vic.regs[0x20], 4);
    assert_eq!(m.board.devices[0x104], 0, "$D702 cleared the megabyte");
}

#[test]
fn inline_dma_runs_the_list_after_the_instruction() {
    // A program at $2000: STA $D707, an inline FILL of 10 bytes with $AA
    // at $3000, then INC $D020.
    let mut m = stub();
    let prog = [
        0x8d, 0x07, 0xd7, // STA $D707
        0x00, 0x03, 0x0a, 0x00, 0xaa, 0x00, 0x00, 0x00, 0x30, 0x00, 0x00, 0x00, 0x00, //
        0xee, 0x20, 0xd0, // INC $D020
        0x4c, 0x13, 0x20, // JMP *
    ];
    m.board.chip[0x2000..0x2000 + prog.len()].copy_from_slice(&prog);
    m.board.write(cpu(0xd02f), 0x47);
    m.board.write(cpu(0xd02f), 0x53);
    m.board.vic.regs[0x20] = 0;
    m.cpu.pc = 0x2000;
    frames(&mut m, 1);
    assert!(m.board.chip[0x3000..0x300a].iter().all(|&v| v == 0xaa));
    assert_eq!(m.board.chip[0x300a], 0);
    assert_eq!(m.board.vic.regs[0x20], 1, "the CPU went on after the list");
    assert_eq!(m.cpu.pc, 0x2013);
}

#[test]
#[ignore = "timing probe"]
fn zz_speed_probe() {
    let mut m = stub();
    let t = std::time::Instant::now();
    frames(&mut m, 200);
    eprintln!("200 fields of the stub: {:?}", t.elapsed());
    m.board.vic.regs[0x31] = 0;
    let t = std::time::Instant::now();
    frames(&mut m, 200);
    eprintln!("200 fields at 1 MHz: {:?}", t.elapsed());
}

/// A stub machine with the MEGA65 personality open, the screen at $0400,
/// the character ROM's row 0 of character 1 set to $80 (a dot at the left).
fn video() -> Mega65 {
    let mut m = stub();
    let b = &mut m.board;
    b.write(cpu(0xd02f), 0x47);
    b.write(cpu(0xd02f), 0x53);
    // The stub ROM is $FF: start from an empty character ROM.
    b.charrom.fill(0);
    b.charrom[8] = 0x80;
    b.vic.regs[0x21] = 6;
    b.vic.regs[0x20] = 14;
    m
}

/// Render visible line `y`.
fn render_line(m: &Mega65, y: u16) -> Vec<Rgb8> {
    let mut out = vec![Rgb8::default(); vic::WIDTH];
    render::line(&m.board, y, true, &mut out);
    out
}

fn rom_colour(i: usize) -> Rgb8 {
    vic::Vic::new().colours(0)[i]
}

#[test]
fn hot_registers_set_the_display_geometry() {
    let mut m = video();
    let v = &m.board.vic;
    assert_eq!((v.reg16(0x48), v.reg16(0x4a)), (88, 488));
    assert_eq!((v.border_left, v.border_right), (40, 40));
    assert_eq!(
        (v.reg16(0x4c), v.reg16(0x4e)),
        (40, 88),
        "XSCROLL 0, YSCROLL 3"
    );
    assert_eq!((v.regs[0x58], v.regs[0x5e], v.regs[0x7b]), (40, 40, 24));
    assert_eq!(v.addr24(0x60), 0x0400);
    assert_eq!(v.addr24(0x68) & 0xffff, 0x1000);
    // 24 rows, 38 columns.
    m.board.write(cpu(0xd011), 0x13);
    m.board.write(cpu(0xd016), 0xc0);
    let v = &m.board.vic;
    assert_eq!((v.reg16(0x48), v.reg16(0x4a)), (96, 480));
    assert_eq!((v.border_left, v.border_right), (54, 57));
    // H640 and V400: 80 columns, 50 rows.
    m.board.write(cpu(0xd031), 0x88);
    let v = &m.board.vic;
    assert_eq!((v.regs[0x58], v.regs[0x5e], v.regs[0x7b]), (80, 80, 49));
    // HOTREG clear: no update until it is set again.
    m.board.write(cpu(0xd05d), 0x00);
    m.board.write(cpu(0xd018), 0x25);
    assert_eq!(m.board.vic.addr24(0x60), 0x0400);
    m.board.write(cpu(0xd05d), 0x80);
    assert_eq!(m.board.vic.addr24(0x60), 0x0800);
    // The VIC bank from CIA 2: bank 1 ($4000).
    m.board.write(cpu(0xdd02), 0x03);
    m.board.write(cpu(0xdd00), 0x02);
    assert_eq!(m.board.vic.addr24(0x60), 0x4800);
}

#[test]
fn text_pixels_in_40_and_80_columns() {
    let mut m = video();
    m.board.chip[0x0400] = 1;
    m.board.colour[0] = 1;
    let l = render_line(&m, 88);
    assert_eq!(l[0], rom_colour(14), "border");
    assert_eq!(
        (l[40], l[41], l[42]),
        (rom_colour(1), rom_colour(1), rom_colour(6))
    );
    // Row 0's second pixel row is empty; line 89 is still pixel row 0.
    assert_eq!(render_line(&m, 89)[40], rom_colour(1));
    assert_eq!(render_line(&m, 90)[40], rom_colour(6));
    // H640: one physical pixel a character pixel; V400: a line a row.
    m.board.write(cpu(0xd031), 0x88);
    let l = render_line(&m, 88);
    assert_eq!((l[40], l[41]), (rom_colour(1), rom_colour(6)));
    assert_eq!(render_line(&m, 89)[40], rom_colour(6));
    // The display blanked: border only.
    m.board.write(cpu(0xd011), 0x0b);
    assert_eq!(render_line(&m, 200)[300], rom_colour(14));
}

#[test]
fn vic_iii_attributes_reverse_and_underline() {
    let mut m = video();
    m.board.chip[0x0400] = 1;
    m.board.write(cpu(0xd031), 0x20);
    m.board.colour[0] = 0x20 | 1; // reverse, white
    let l = render_line(&m, 88);
    assert_eq!((l[40], l[42]), (rom_colour(6), rom_colour(1)), "reversed");
    m.board.colour[0] = 0x80 | 1; // underline: pixel row 7 all set
    assert_eq!(render_line(&m, 88 + 14)[46], rom_colour(1));
}

#[test]
fn multicolour_extended_background_and_bitmap() {
    let mut m = video();
    let b = &mut m.board;
    b.charrom[8] = 0b0110_1100; // pairs 01 10 11 00
    b.chip[0x0400] = 1;
    b.colour[0] = 0x08 | 5;
    b.vic.regs[0x22] = 2;
    b.vic.regs[0x23] = 7;
    b.write(cpu(0xd016), 0xd8);
    let l = render_line(&m, 88);
    assert_eq!([l[40], l[44], l[48], l[52]], [2, 7, 5, 6].map(rom_colour));
    // ECM: code bits 6-7 pick the background ($D022 for 01).
    let b = &mut m.board;
    b.write(cpu(0xd016), 0xc8);
    b.write(cpu(0xd011), 0x5b);
    b.chip[0x0400] = 0x41;
    b.charrom[8] = 0x80;
    b.colour[0] = 1;
    let l = render_line(&m, 88);
    assert_eq!((l[40], l[42]), (rom_colour(1), rom_colour(2)));
    // Hires bitmap at $2000 (CB bit 3): colours from the screen nybbles.
    let b = &mut m.board;
    b.write(cpu(0xd011), 0x3b);
    b.write(cpu(0xd018), 0x18);
    b.chip[0x2000] = 0x80;
    b.chip[0x0400] = 0x27; // fg 2, bg 7
    let l = render_line(&m, 88);
    assert_eq!((l[40], l[42]), (rom_colour(2), rom_colour(7)));
}

#[test]
fn sixteen_bit_full_colour_characters_and_gotox() {
    let mut m = video();
    let b = &mut m.board;
    b.write(cpu(0xd054), 0x45); // VFAST, CHR16, FCLRHI
                                // Character $100: 64 bytes at $4000, pixel 0 colour 3, pixel 1 transparent.
    b.chip[0x4000] = 3;
    b.chip[0x4002] = 0xff; // $FF: the colour RAM colour
    b.chip[0x0400] = 0x00;
    b.chip[0x0401] = 0x01;
    b.colour[1] = 5;
    let l = render_line(&m, 88);
    assert_eq!([l[40], l[42], l[44]], [3, 6, 5].map(rom_colour));
    // GOTOX in the first cell: the next one starts at X 16 (32 pixels on).
    let b = &mut m.board;
    b.chip[0x0400..0x0404].copy_from_slice(&[16, 0, 0x00, 0x01]);
    b.colour[0..4].copy_from_slice(&[0x10, 0, 0, 5]);
    let l = render_line(&m, 88);
    assert_eq!((l[40], l[72]), (rom_colour(6), rom_colour(3)));
}

#[test]
fn sprites_position_expand_priority_and_collide() {
    let mut m = video();
    let b = &mut m.board;
    // Sprite 0 at X 24, Y 50: the display's top left; its image at $0C00
    // (pointer $30 at $07F8); row 0 = $80 00 00.
    b.chip[0x07f8] = 0x30;
    b.chip[0x0c00] = 0x80;
    b.vic.regs[0x00] = 24;
    b.vic.regs[0x01] = 50;
    b.vic.regs[0x27] = 2;
    b.vic.regs[0x15] = 1;
    let l = render_line(&m, 88);
    assert_eq!(
        (l[40], l[41], l[42]),
        (rom_colour(2), rom_colour(2), rom_colour(6))
    );
    assert_eq!(
        render_line(&m, 86)[40],
        rom_colour(14),
        "the line before: border"
    );
    // X expanded: 4 pixels.
    m.board.vic.regs[0x1d] = 1;
    assert_eq!(render_line(&m, 88)[43], rom_colour(2));
    // Behind the foreground: the text dot wins, the collision is seen.
    let b = &mut m.board;
    b.chip[0x0400] = 1;
    b.colour[0] = 1;
    b.vic.regs[0x1b] = 1;
    let mut out = vec![Rgb8::default(); vic::WIDTH];
    let (ss, sd) = render::line(&m.board, 88, true, &mut out);
    assert_eq!(out[40], rom_colour(1));
    assert_eq!(out[42], rom_colour(2));
    assert_eq!((ss, sd), (0, 1));
    m.board.vic.collide(ss, sd);
    assert_eq!(m.board.vic.irq_flags & 2, 2);
    assert_eq!(m.board.read(cpu(0xd01f)), 1);
    assert_eq!(m.board.read(cpu(0xd01f)), 0, "cleared by the read");
}

#[test]
fn palette_ram_banks_and_the_rom_colours() {
    let mut m = video();
    let b = &mut m.board;
    // Bank 1 mapped for the CPU, red of colour 5 = $F0 swapped: 15 -> 255?
    b.write(cpu(0xd070), 0x40);
    b.write(cpu(0xd105), 0x0f);
    b.write(cpu(0xd205), 0x80);
    b.write(cpu(0xd305), 0x00);
    assert_eq!(b.vic.palette[1][0][5], 0x0f);
    assert_eq!(b.vic.palette[0][0][5], 0);
    // Shown from bank 1 with PAL set: the nybbles swapped.
    b.write(cpu(0xd070), 0x10);
    b.write(cpu(0xd030), 0x04);
    let c = b.vic.colours(1)[5];
    assert_eq!((c.r, c.g, c.b), (0xf0, 0x08, 0x00));
    // Without PAL colours 0-15 come from the ROM.
    b.write(cpu(0xd030), 0x00);
    assert_eq!(b.vic.colours(1)[5], rom_colour(5));
}

#[test]
fn vic_iii_bitplanes_combine_planes_in_cells() {
    let mut m = video();
    let b = &mut m.board;
    // BPM, 320 x 200; planes 0 and 1 on; plane 0 at $2000 (block 1), plane 1
    // at $10000 + $4000 (block 2); palette from the ROM.
    b.write(cpu(0xd031), 0x10);
    b.write(cpu(0xd032), 0x03);
    b.write(cpu(0xd033), 0x02);
    b.write(cpu(0xd034), 0x04);
    b.chip[0x2000] = 0xc0; // plane 0: pixels 0, 1
    b.chip[0x1_4000] = 0x60; // plane 1: pixels 1, 2
                             // Row 1 of the first cell: the next byte.
    b.chip[0x2001] = 0x80;
    let l = render_line(&m, 88);
    assert_eq!([l[40], l[42], l[44], l[46]], [1, 3, 2, 0].map(rom_colour));
    assert_eq!(render_line(&m, 90)[40], rom_colour(1));
    // The complement register turns plane 3 on everywhere (+8).
    m.board.write(cpu(0xd03b), 0x08);
    assert_eq!(render_line(&m, 88)[46], rom_colour(8));
}

fn key(m: &mut Mega65, k: u16, down: bool) {
    m.apply_input(TimedInput {
        at: m.now(),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: emu_core::input::MachineKey(k),
            down,
        },
    });
}

#[test]
fn the_matrix_reads_through_cia1_and_the_column_8_line() {
    use keyboard::keys;
    let mut m = video();
    key(&mut m, keys::RETURN, true);
    let b = &mut m.board;
    // Column 0 driven low: row 1 (RETURN) reads low.
    b.write(cpu(0xdc02), 0xff);
    b.write(cpu(0xdc00), 0xfe);
    assert_eq!(b.read(cpu(0xdc01)), 0xfd);
    b.write(cpu(0xdc00), 0xfd);
    assert_eq!(b.read(cpu(0xdc01)), 0xff);
    // ALT is column 8, row 2: seen only with $D607 bit 1 driven low.
    key(&mut m, keys::RETURN, false);
    key(&mut m, keys::ALT, true);
    let b = &mut m.board;
    b.write(cpu(0xdc00), 0xff);
    assert_eq!(b.read(cpu(0xdc01)), 0xff);
    b.write(cpu(0xd608), 0x02);
    b.write(cpu(0xd607), 0x00);
    assert_eq!(b.read(cpu(0xdc01)), 0xfb);
    // The matrix registers: $D614 column, $D613 rows.
    b.write(cpu(0xd614), 8);
    assert_eq!(b.read(cpu(0xd613)), 0xfb);
    assert_eq!(b.read(cpu(0xd611)) & 0x10, 0x10, "ALT in the modifiers");
}

#[test]
fn the_typing_queue_gives_ascii_petscii_and_dequeues() {
    use keyboard::keys;
    let mut m = video();
    let a = keys::key(1, 2);
    key(&mut m, a, true);
    key(&mut m, a, false);
    key(&mut m, keys::LSHIFT, true);
    key(&mut m, a, true);
    key(&mut m, a, false);
    key(&mut m, keys::LSHIFT, false);
    let b = &mut m.board;
    assert_eq!(b.read(cpu(0xd610)), b'a');
    assert_eq!(b.read(cpu(0xd619)), 0x41, "PETSCII unshifted letter");
    assert_eq!(b.read(cpu(0xd60a)), 0x80, "queue not empty, no modifiers");
    b.write(cpu(0xd610), 0);
    assert_eq!(b.read(cpu(0xd610)), b'A');
    assert_eq!(b.read(cpu(0xd619)), 0xc1);
    assert_eq!(b.read(cpu(0xd60a)), 0x81, "left SHIFT held");
    b.write(cpu(0xd619), 0);
    assert_eq!(b.read(cpu(0xd610)), 0, "empty");
    // CAPS LOCK capitalises and shows on the C65's port bit 6.
    key(&mut m, keyboard::CAPS_LOCK, true);
    key(&mut m, keyboard::CAPS_LOCK, false);
    key(&mut m, a, true);
    assert_eq!(m.board.read(cpu(0xd610)), b'A');
    assert_eq!(m.board.read(cpu(0x0001)) & 0x40, 0);
}

#[test]
fn separate_cursor_keys_virtual_keys_and_restore() {
    use keyboard::keys;
    let mut m = video();
    key(&mut m, keyboard::CRSR_LEFT, true);
    let b = &mut m.board;
    assert_eq!(b.read(cpu(0xd60f)) & 3, 1);
    // For C64 software: right SHIFT and CRSR right in the matrix.
    b.write(cpu(0xd614), 0);
    assert_eq!(b.read(cpu(0xd613)), !0x04);
    b.write(cpu(0xd614), 6);
    assert_eq!(b.read(cpu(0xd613)), !0x10);
    key(&mut m, keyboard::CRSR_LEFT, false);
    // A virtual key: RUN/STOP (column 7, row 7) held until $7F.
    let b = &mut m.board;
    b.write(cpu(0xd615), keys::RUN_STOP as u8);
    b.write(cpu(0xd614), 7);
    assert_eq!(b.read(cpu(0xd613)), 0x7f);
    b.write(cpu(0xd615), 0x7f);
    assert_eq!(b.read(cpu(0xd613)), 0xff);
    // RESTORE pulls the NMI line.
    key(&mut m, keyboard::RESTORE, true);
    m.board.chip[0x2000..0x2003].copy_from_slice(&[0x4c, 0x00, 0x20]);
    m.cpu.pc = 0x2000;
    frames(&mut m, 1);
    assert_eq!(m.cpu.pc, stub::DONE, "the stub's NMI vector");
}

/// A joystick held in one position (for the port tests).
struct Stick(u32);

impl PortDevice for Stick {
    fn kind(&self) -> emu_core::port::PortKind {
        emu_core::port::PortKind::CbmControl
    }
    fn read_lines(&self, _now: Cycle) -> emu_core::port::PortLines {
        emu_core::port::PortLines {
            pulled_low: self.0,
            analog: [0, u16::MAX],
        }
    }
    fn write_lines(&mut self, _lines: emu_core::port::PortLines, _now: Cycle) {}
    fn input(&mut self, _ev: &InputEvent) {}
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    fn load_state(&mut self, _s: &[u8]) -> Result<(), StateError> {
        Ok(())
    }
}

#[test]
fn joysticks_on_cia1_and_paddles_on_the_sids() {
    let mut m = video();
    // Port 2 holds the default joystick; replaced by one held up with
    // fire (bits 0 and 5 of the lines), seen on CIA 1 port A.
    assert!(m.ports()[1].connected);
    assert!(m.disconnect(PortId(1)).is_some());
    m.connect(PortId(1), Box::new(Stick(0x21))).unwrap();
    let b = &mut m.board;
    b.write(cpu(0xdc02), 0x00);
    assert_eq!(b.read(cpu(0xdc00)) & 0x1f, 0x1e & !0x10);
    // Port 1 on port B.
    m.connect(PortId(0), Box::new(Stick(0x08))).unwrap();
    let b = &mut m.board;
    b.write(cpu(0xdc03), 0x00);
    assert_eq!(b.read(cpu(0xdc01)) & 0x0f, 0x07);
    assert!(
        m.connect(PortId(0), Box::new(Stick(0))).is_err(),
        "occupied"
    );
    // POTX of port 1 when PA6 selects it: the SIDs measure it.
    let b = &mut m.board;
    b.write(cpu(0xdc02), 0xff);
    b.write(cpu(0xdc00), 0x40);
    for _ in 0..1024 {
        b.clock_sids();
    }
    assert_eq!(b.read(cpu(0xd419)), 0);
    assert_eq!(b.read(cpu(0xd41a)), 0xff);
}

#[test]
fn four_sids_mix_to_left_and_right() {
    let mut m = video();
    let b = &mut m.board;
    // A sawtooth on the left SID #1 ($D440) only.
    b.write(cpu(0xd440 + 1), 0x40);
    b.write(cpu(0xd440 + 5), 0x00);
    b.write(cpu(0xd440 + 6), 0xf0);
    b.write(cpu(0xd440 + 24), 0x0f);
    b.write(cpu(0xd440 + 4), 0x21);
    b.samples.clear();
    for _ in 0..board::SID_HZ / 10 {
        b.clock_sids();
    }
    let (mut l, mut r) = (0i64, 0i64);
    for pair in b.samples.chunks(2) {
        l += i64::from(pair[0]).abs();
        r += i64::from(pair[1]).abs();
    }
    assert!(l > 100 * r.max(1), "left {l}, right {r}");
    // $D440 mirrors at $D4C0; $D63C selects the 8580 per SID.
    assert_eq!(b.peek(cpu(0xd4c0 + 1)), b.peek(cpu(0xd440 + 1)));
    b.write(cpu(0xd63c), 0x04);
    assert_eq!(b.sids[2].model(), chips_cbm::sid::SidModel::Mos8580);
    assert_eq!(b.sids[0].model(), chips_cbm::sid::SidModel::Mos6581);
    assert_eq!(m.audio(&mut Vec::new()).channels, 2);
}

/// Run `code` at $2000 (MEGA65 I/O open) up to its final `JMP *`.
fn run_program(code: &[u8]) -> Mega65 {
    let mut m = video();
    let mut prog = code.to_vec();
    let end = 0x2000 + prog.len() as u16;
    prog.extend_from_slice(&[0x4c, end as u8, (end >> 8) as u8]);
    m.board.chip[0x2000..0x2000 + prog.len()].copy_from_slice(&prog);
    m.cpu.pc = 0x2000;
    frames(&mut m, 2);
    assert_eq!(m.cpu.pc, end, "the program ran to its end");
    m
}

#[test]
fn hypervisor_traps_getversion_errors_and_the_rom_protection() {
    use cpu_45gs02::flag::C;
    // LDA #$00, STA $D640, CLV: getversion.
    let m = run_program(&[0xa9, 0x00, 0x8d, 0x40, 0xd6, 0xb8]);
    assert_eq!((m.cpu.a, m.cpu.x, m.cpu.y, m.cpu.z), (1, 2, 1, 2));
    assert_ne!(m.cpu.p & C, 0);
    assert_eq!(m.board.traps, 1);
    // An unknown service: C clear, A $FF; geterrorcode returns it.
    let m = run_program(&[
        0xa9, 0x9a, 0x8d, 0x40, 0xd6, 0xb8, 0x85, 0x10, 0xa9, 0x38, 0x8d, 0x40, 0xd6, 0xb8,
    ]);
    assert_eq!(m.board.chip[0x10], 0xff);
    assert_eq!(m.cpu.a, 0xff);
    // rom_writeenable ($D641 A=2), then a flat write to $2E000 sticks.
    let mut m = run_program(&[0xa9, 0x02, 0x8d, 0x41, 0xd6, 0xb8]);
    assert!(!m.board.rom_protect);
    m.board.write(0x2e000, 0x5a);
    assert_eq!(m.board.chip[0x2e000], 0x5a);
    // toggle_rom_writeprotect: protected again, A bit 2.
    let m = run_program(&[
        0xa9, 0x02, 0x8d, 0x41, 0xd6, 0xb8, 0xa9, 0x70, 0x8d, 0x40, 0xd6, 0xb8,
    ]);
    assert!(m.board.rom_protect);
    assert_eq!(m.cpu.a & 0x04, 0x04);
    // In the C64 personality $D640 is no trap.
    let mut m = video();
    m.board.write(cpu(0xd02f), 0x00);
    m.board.write(cpu(0xd640), 0x00);
    assert!(m.board.trap.is_none());
}

#[test]
fn hypervisor_get_and_set_mapping() {
    // MAP with A=$80 X=$8D (lower: offset $D80, block 3), Y=$00 Z=$14
    // (upper: offset $400, block 4), EOM; LDY #$30, LDA #$74, STA $D640.
    let m = run_program(&[
        0xa9, 0x80, 0xa2, 0x8d, 0xa0, 0x00, 0xa3, 0x14, 0x5c, 0xea, 0xa0, 0x30, 0xa9, 0x74, 0x8d,
        0x40, 0xd6, 0xb8,
    ]);
    assert_eq!(
        &m.board.chip[0x3000..0x3006],
        &[0x8d, 0x80, 0x14, 0x00, 0x00, 0x00]
    );
    // set_mapping from $3100: lower half mapped $1000 bytes up for block 1.
    let mut m = video();
    m.board.chip[0x3100..0x3106].copy_from_slice(&[0x20, 0x10, 0x00, 0x00, 0x00, 0x00]);
    let prog = [
        0xa0, 0x31, 0xa9, 0x76, 0x8d, 0x40, 0xd6, 0xb8, 0x4c, 0x08, 0x20,
    ];
    m.board.chip[0x2000..0x2000 + prog.len()].copy_from_slice(&prog);
    m.cpu.pc = 0x2000;
    frames(&mut m, 1);
    assert_eq!(m.cpu.map.lo_mask, 0x2);
    assert_eq!(m.cpu.map.translate(0x2000), 0x3000);
}

#[test]
fn the_math_unit_multiplies_and_divides() {
    let mut m = video();
    let b = &mut m.board;
    for (i, v) in 100_000u32.to_le_bytes().into_iter().enumerate() {
        b.write(cpu(0xd770 + i as u16), v);
    }
    for (i, v) in 7u32.to_le_bytes().into_iter().enumerate() {
        b.write(cpu(0xd774 + i as u16), v);
    }
    let read64 = |b: &mut board::Board, at: u16| {
        u64::from_le_bytes(std::array::from_fn(|i| b.read(cpu(at + i as u16))))
    };
    assert_eq!(read64(b, 0xd778), 700_000);
    let q = read64(b, 0xd768);
    assert_eq!(q >> 32, 14_285, "the whole part");
    // 32.32 fixed point: q × 7 falls just short of 100000.
    assert_eq!(((u128::from(q) * 7) >> 32) as u64, 99_999);
    assert_eq!(b.read(cpu(0xd70f)) & 0xc0, 0, "never busy");
}

/// An image in memory (the frontends' backends live outside the machine).
struct Mem(Vec<u8>);

impl emu_core::media::MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), emu_core::media::MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), emu_core::media::MediaError> {
        let o = offset as usize;
        self.0[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), emu_core::media::MediaError> {
        Ok(())
    }
}

fn insert(m: &mut Mega65, slot: SlotId, name: &str, data: Vec<u8>) {
    use emu_core::media::{Media, MediaKind};
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            name: name.into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(data)),
        })
        .unwrap();
}

/// A blank D81 named MIMIC.
fn d81() -> Vec<u8> {
    media::d64::D64::blank_d81(b"MIMIC", *b"65").to_bytes()
}

#[test]
fn the_f011_reads_and_writes_d81_sectors() {
    let mut m = video();
    let image = d81();
    insert(&mut m, SLOT_FLOPPY0, "test.d81", image.clone());
    let b = &mut m.board;
    // Motor on, drive 0; the 1581 header (logical 40/0) is physical track
    // 39, side 0, sector 1. Read with the buffer pointers cleared.
    b.write(cpu(0xd080), 0x60);
    assert_eq!(b.read(cpu(0xd083)) & 0x08, 0x08, "disk in");
    b.write(cpu(0xd084), 39);
    b.write(cpu(0xd085), 1);
    b.write(cpu(0xd086), 0);
    b.write(cpu(0xd081), 0x41);
    assert_eq!(b.read(cpu(0xd082)) & 0xf0, 0x60, "not busy, DRQ, EQ, found");
    assert_eq!(b.read(cpu(0xd083)) & 0x80, 0x80, "RDREQ");
    let at = 39 * 2 * 10 * 512;
    let sector: Vec<u8> = (0..512).map(|_| b.read(cpu(0xd087))).collect();
    assert_eq!(&sector[..], &image[at..at + 512]);
    assert_eq!(&sector[4..9], b"MIMIC");
    // The same buffer at $FFD6C00 (BUFSEL 0).
    assert_eq!(b.read(0x0ffd_6c04), b'M');
    // Write: fill the buffer through $D087, write side 1 sector 10 of
    // track 0.
    b.write(cpu(0xd081), 0x01);
    for i in 0..512u16 {
        b.write(cpu(0xd087), i as u8);
    }
    b.write(cpu(0xd084), 0);
    b.write(cpu(0xd085), 10);
    b.write(cpu(0xd086), 1);
    b.write(cpu(0xd081), 0x80);
    assert_eq!(b.read(cpu(0xd083)) & 0x40, 0x40, "WTREQ");
    let slot = m.media_slot(SLOT_FLOPPY0).unwrap();
    assert!(slot.dirty());
    slot.flush().unwrap();
    let mut back = m.media_slot(SLOT_FLOPPY0).unwrap().eject().unwrap();
    let mut data = vec![0; storage::D81_SIZE];
    back.backend.read(0, &mut data).unwrap();
    let at = (10 + 9) * 512;
    assert_eq!(data[at + 3], 3);
    assert_eq!(data[at + 300], 44);
}

#[test]
fn f011_drive_swap_missing_sectors_and_steps() {
    let mut m = video();
    insert(&mut m, SLOT_FLOPPY1, "one.d81", d81());
    let b = &mut m.board;
    // Drive 0 is empty: not found, no disk.
    b.write(cpu(0xd080), 0x20);
    b.write(cpu(0xd085), 1);
    b.write(cpu(0xd081), 0x40);
    assert_eq!(b.read(cpu(0xd082)) & 0x10, 0x10);
    assert_eq!(b.read(cpu(0xd083)) & 0x08, 0);
    // $D689 bit 5 swaps the drives: drive 0 is now the one with a disk.
    b.write(cpu(0xd689), 0x20);
    b.write(cpu(0xd081), 0x40);
    assert_eq!(b.read(cpu(0xd082)) & 0x10, 0);
    // Sector 11 does not exist.
    b.write(cpu(0xd085), 11);
    b.write(cpu(0xd081), 0x40);
    assert_eq!(b.read(cpu(0xd082)) & 0x10, 0x10);
    // Steps: track 0 flag.
    assert_eq!(b.read(cpu(0xd082)) & 1, 1);
    b.write(cpu(0xd081), 0x18);
    assert_eq!(b.read(cpu(0xd082)) & 1, 0);
    b.write(cpu(0xd081), 0x10);
    assert_eq!(b.read(cpu(0xd082)) & 1, 1);
    // A wrong image size is refused.
    use emu_core::media::{Media, MediaKind};
    assert!(m
        .media_slot(SLOT_FLOPPY0)
        .unwrap()
        .insert(Media {
            name: "x.d81".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(vec![0; 1000])),
        })
        .is_err());
}

#[test]
fn the_sd_card_reads_and_writes_sectors_through_its_buffer() {
    let mut m = video();
    let mut card = vec![0u8; 4 * 512];
    card[2 * 512..3 * 512].fill(0xab);
    insert(&mut m, SLOT_SDCARD, "card.img", card);
    let b = &mut m.board;
    // Sector 2, read; BUFSEL 1 shows the SD buffer, mapped at $DE00.
    b.write(cpu(0xd681), 2);
    b.write(cpu(0xd680), 0x02);
    b.write(cpu(0xd689), 0x80);
    b.write(cpu(0xd680), 0x81);
    assert_eq!(b.read(cpu(0xd680)) & 0x08, 0x08, "mapped");
    assert_eq!(b.read(cpu(0xde00)), 0xab);
    assert_eq!(b.read(0x0ffd_6e10), 0xab);
    // A write without the gate fails; with $57 it goes in.
    b.write(cpu(0xde00), 0x11);
    b.write(cpu(0xd681), 1);
    b.write(cpu(0xd680), 0x03);
    assert_eq!(b.read(cpu(0xd680)) & 0x40, 0x40, "error");
    b.write(cpu(0xd680), 0x57);
    b.write(cpu(0xd680), 0x03);
    assert_eq!(b.read(cpu(0xd680)) & 0x40, 0);
    // Sector 0 needs its own gate ($4D); fill mode writes $D686.
    b.write(cpu(0xd681), 0);
    b.write(cpu(0xd686), 0x5a);
    b.write(cpu(0xd680), 0x83);
    b.write(cpu(0xd680), 0x57);
    b.write(cpu(0xd680), 0x03);
    assert_eq!(b.read(cpu(0xd680)) & 0x40, 0x40, "the MBR gate is another");
    b.write(cpu(0xd680), 0x4d);
    b.write(cpu(0xd680), 0x03);
    b.write(cpu(0xd680), 0x84);
    let mut back = m.media_slot(SLOT_SDCARD).unwrap().eject().unwrap();
    let mut data = vec![0; 4 * 512];
    back.backend.read(0, &mut data).unwrap();
    assert_eq!(data[512], 0x11);
    assert_eq!(data[0], 0x5a);
    assert_eq!(data[511], 0x5a);
}

#[test]
fn savestates_keep_the_floppy_and_leave_the_media_in_place() {
    let mut m = video();
    insert(&mut m, SLOT_FLOPPY0, "test.d81", d81());
    m.board.write(cpu(0xd080), 0x20);
    m.board.write(cpu(0xd084), 39);
    m.board.write(cpu(0xd085), 1);
    m.board.write(cpu(0xd081), 0x41);
    let state = m.save_state();
    m.board.write(cpu(0xd081), 0x01);
    m.load_state(&state).unwrap();
    assert_eq!(m.board.read(cpu(0xd087)), 0x28, "the header's first byte");
    assert!(
        m.media_slot(SLOT_FLOPPY0).unwrap().eject().is_some(),
        "the medium stayed"
    );
}

#[test]
fn gfx_views_show_the_screen_charset_sprites_and_palette() {
    let mut m = video();
    m.board.chip[0x0400] = 1;
    m.board.colour[0] = 1;
    m.board.chip[0x07f8] = 0x30;
    m.board.chip[0x0c00] = 0xff;
    m.board.vic.regs[0x15] = 1;
    m.board.vic.regs[0x27] = 2;
    let views = m.gfx_views();
    assert_eq!(views.len(), 4);
    let opts = emu_core::GfxOptions::new();
    let before = m.save_state();
    for v in &views {
        let g = m.gfx_view(v.id, &opts).unwrap();
        assert!(g.width > 0 && g.height > 0, "{}", v.id);
    }
    assert_eq!(m.save_state(), before, "views change nothing");
    let screen = m.gfx_view("screen", &opts).unwrap();
    assert_eq!((screen.width, screen.height), (640, 400));
    assert_eq!(
        screen.frame().rgb_at(0, 0),
        Some(rom_colour(1)),
        "the dot of character 1"
    );
    let charset = m.gfx_view("charset", &opts).unwrap();
    assert_eq!(
        charset.raw[8], 0x80,
        "character 1, row 0 from the character ROM"
    );
    let s = m
        .gfx_view("sprites", &emu_core::GfxOptions::new().with("index", 0))
        .unwrap();
    assert_eq!((s.width, s.height), (24, 21));
    assert_eq!(s.frame().rgb_at(0, 0), Some(rom_colour(2)));
    let p = m.gfx_view("palette", &opts).unwrap();
    assert_eq!(p.param("count"), Some(256));
    let names: Vec<&str> = m.chips().iter().map(|c| c.name).collect();
    for n in [
        "vic",
        "cia1",
        "sid_l1",
        "f011",
        "sdcard",
        "dmagic",
        "math",
        "keyboard",
        "hypervisor",
    ] {
        assert!(names.contains(&n), "{n}");
    }
}
