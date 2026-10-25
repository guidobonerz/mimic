use super::*;

fn stub(model: &str) -> Cpc {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Cpc::new(&config).unwrap()
}

fn frames(m: &mut Cpc, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        while m.run_frame(&mut dbg) != StopReason::FrameDone {}
    }
}

fn pixel(m: &Cpc, x: usize, y: usize) -> u8 {
    m.io.ga.pixels()[y * gate_array::WIDTH as usize + x]
}

#[test]
fn stub_program_draws_and_counts_interrupts() {
    let mut m = stub("6128");
    frames(&mut m, 20);
    // Screen: 36 lines and 4 µs (64 pixels) of border, then pen 1.
    assert_eq!(pixel(&m, 10, 10), 20, "border black");
    assert_eq!(pixel(&m, 64, 36), 11, "screen white");
    assert_eq!(pixel(&m, 63, 36), 20, "left of the screen");
    assert_eq!(pixel(&m, 64 + 40 * 16, 36), 20, "right of the screen");
    // Six interrupts per field (300 Hz).
    let ints = peek_cpu(&m, 0x8000);
    let before = ints;
    frames(&mut m, 10);
    let after = peek_cpu(&m, 0x8000);
    assert_eq!(after.wrapping_sub(before), 60, "{before} -> {after}");
}

#[test]
fn field_is_20_ms() {
    let mut m = stub("464");
    frames(&mut m, 3);
    let t0 = m.now().0;
    frames(&mut m, 1);
    // 312 lines × 64 µs × 4 T-states, give or take the last instruction.
    let t = m.now().0 - t0;
    assert!((79_860..=79_884).contains(&t), "{t}");
}

#[test]
fn keyboard_reaches_the_ay_port() {
    let mut m = stub("6128");
    m.key(keyboard::keys::SPACE.0, true);
    // Select AY register 14, then read with keyboard line 5.
    let mut mem = m.mem.clone();
    m.io.write(&mut mem, 0xf782, 0x82); // A out
    m.io.write(&mut mem, 0xf40e, 14);
    m.io.write(&mut mem, 0xf6c0, 0xc0); // latch address
    m.io.write(&mut mem, 0xf600, 0x00);
    m.io.write(&mut mem, 0xf792, 0x92); // A in
    m.io.write(&mut mem, 0xf645, 0x45); // read, line 5
    assert_eq!(m.io.read(0xf400), 0x7f, "SPACE is bit 7 of line 5");
    assert_eq!(m.io.read(0xf500) & 0x0e, 0x0e, "Amstrad links");
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = stub("6128");
    frames(&mut a, 5);
    let state = a.save_state();
    frames(&mut a, 3);
    let mut b = stub("6128");
    b.load_state(&state).unwrap();
    frames(&mut b, 3);
    assert_eq!(a.io.ga.pixels(), b.io.ga.pixels());
    assert_eq!(a.now(), b.now());
    assert_eq!(a.cpu, b.cpu);
    assert!(b.load_state(&state[..state.len() / 2]).is_err());
    let mut c = stub("464");
    assert!(c.load_state(&state).is_err(), "other model");
}

#[test]
fn joystick_reads_as_keyboard_line_9() {
    use emu_core::input::{Dirs, InputEvent, InputTarget, TimedInput};
    let mut m = stub("6128");
    m.input(TimedInput {
        at: m.now(),
        target: InputTarget::Port(JOY0),
        event: InputEvent::Digital {
            dirs: Dirs::UP | Dirs::RIGHT,
            buttons: 1,
        },
    });
    frames(&mut m, 1);
    let mut mem = m.mem.clone();
    m.io.write(&mut mem, 0xf792, 0x92); // A in
    m.io.ay.addr = 14;
    m.io.write(&mut mem, 0xf649, 0x49); // read, line 9
                                        // Up (0), right (3), fire 1 (4) closed.
    assert_eq!(m.io.read(0xf400), !0b1_1001);
    m.io.write(&mut mem, 0xf646, 0x46); // line 6: joystick 1 absent
    assert_eq!(m.io.read(0xf400), 0xff);
    assert_eq!(m.ports().len(), 3, "two joysticks and the expansion port");
    assert!(m.disconnect(JOY0).is_some());
    assert!(m
        .connect(
            JOY0,
            Box::new(devices_input::Joystick::new(PortKind::AtariJoy))
        )
        .is_err());
}

#[test]
fn ay_tone_reaches_the_audio_output() {
    let mut m = stub("464");
    let mut mem = m.mem.clone();
    let mut ay = |reg: u8, v: u8| {
        m.io.write(&mut mem, 0xf782, 0x82);
        m.io.write(&mut mem, 0xf400, reg);
        m.io.write(&mut mem, 0xf6c0, 0xc0); // latch the register number
        m.io.write(&mut mem, 0xf600, 0x00); // inactive, as the firmware does
        m.io.write(&mut mem, 0xf400, v);
        m.io.write(&mut mem, 0xf680, 0x80);
        m.io.write(&mut mem, 0xf600, 0x00);
    };
    ay(0, 142); // ~440 Hz
    ay(7, 0x3e);
    ay(8, 15);
    assert_eq!(&m.io.ay.regs[..9], &[142, 0, 0, 0, 0, 0, 0, 0x3e, 15]);
    frames(&mut m, 5);
    let mut out = Vec::new();
    m.audio(&mut out);
    // About 960 samples per field; the first field after power-on is short.
    assert!(out.len() > 3 * 960, "{}", out.len());
    let (lo, hi) = (out.iter().min().unwrap(), out.iter().max().unwrap());
    assert!(hi - lo > 5000, "{lo}..{hi}");
}

#[test]
fn graphics_views_show_the_screen_without_side_effects() {
    let mut m = stub("6128");
    frames(&mut m, 5);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["screen", "palette"]);
    let before = m.save_state();
    let v = m.gfx_view("screen", &emu_core::GfxOptions::new()).unwrap();
    assert_eq!(v.space, RAM_SPACE);
    let r = &m.io.crtc.regs;
    assert_eq!(v.param("chars"), Some(u32::from(r[1])));
    assert_eq!(v.param("rows"), Some(u32::from(r[6])));
    assert_eq!(v.height, u32::from(r[6]) * (u32::from(r[9]) + 1));
    assert_eq!(v.raw.len() as u32, 2 * u32::from(r[1]) * v.height);
    // The first byte comes from the RAM the Gate Array reads.
    assert_eq!(
        v.raw[0],
        m.peek(RAM_SPACE, v.addr, Size::Byte).unwrap() as u8
    );
    // The stub fills the screen with pen 1: the picture shows its ink.
    let ink = m.io.ga.inks[1];
    let pal = chips_amstrad::palette::palette();
    assert_eq!(v.frame().rgb_at(0, 0), Some(pal[usize::from(ink)]));
    let p = m.gfx_view("palette", &emu_core::GfxOptions::new()).unwrap();
    assert_eq!(p.param("pen1"), Some(u32::from(ink)));
    assert_eq!(p.raw.len(), 17);
    assert!(m
        .gfx_view("sprites", &emu_core::GfxOptions::new())
        .is_none());
    assert_eq!(m.save_state(), before, "views change nothing");
    // The RAM space reaches the 6128's extra banks.
    assert!(m.poke(RAM_SPACE, 0x1_2345, Size::Byte, 0xa5));
    assert_eq!(m.peek(RAM_SPACE, 0x1_2345, Size::Byte), Some(0xa5));
}

#[test]
fn crtc_overlay_marks_writes_where_the_beam_was() {
    let mut m = stub("6128");
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["crtc", "vsync"]);
    assert_eq!(m.frame_row(0), Some(0));
    assert_eq!(m.frame_row(271), Some(271));
    assert_eq!(m.frame_row(272), None);
    frames(&mut m, 2);
    assert!(m.overlay("crtc").is_none(), "off");
    m.set_overlays(&["crtc", "vsync"]);
    while m.beam().unwrap().line != 100 {
        m.step_cycle();
    }
    let b = m.beam().unwrap();
    assert!(m.poke(CPU_SPACE, IO_BASE + 0xbc00, Size::Byte, 12));
    assert!(m.poke(CPU_SPACE, IO_BASE + 0xbd00, Size::Byte, 0x30));
    assert!(m.poke(CPU_SPACE, IO_BASE + 0x7f00, Size::Byte, 0x10));
    assert!(m.poke(CPU_SPACE, IO_BASE + 0x7f00, Size::Byte, 0x40 | 4));
    // Halted mid-frame: the field so far.
    let o = m.overlay("crtc").unwrap();
    let mark = |label: &str| emu_core::overlay::Shape::Mark {
        x: i32::from(b.hpos),
        y: 100,
        color: if label.starts_with('R') {
            emu_core::overlay::colors::YELLOW
        } else {
            emu_core::overlay::colors::CYAN
        },
        label: Some(label.into()),
    };
    assert!(o.shapes.contains(&mark("R12=30")), "{:?}", o.shapes);
    assert!(o.shapes.contains(&mark("BORDER=04")));
    frames(&mut m, 1);
    let o = m.overlay("crtc").unwrap();
    assert!(o.shapes.contains(&mark("R12=30")), "the last field");
    // VSYNC is below the picture while the monitor is locked.
    assert_eq!(m.overlay("vsync").unwrap().shapes, vec![]);
    m.set_overlays(&[]);
    assert!(m.overlay("crtc").is_none());
    assert!(m.io.rec.is_none());
}

#[test]
fn overlay_record_stays_out_of_savestates() {
    let mut m = stub("6128");
    let mut twin = stub("6128");
    m.set_overlays(&["crtc", "vsync"]);
    frames(&mut m, 3);
    frames(&mut twin, 3);
    assert_eq!(m.save_state(), twin.save_state());
    let s = twin.save_state();
    m.load_state(&s).unwrap();
    assert!(m.overlay("crtc").is_some(), "still recording after a load");
}

#[test]
fn vsync_bands_cover_the_rows_drawn_during_vsync() {
    let mut r = overlay::record(&["vsync"], None).unwrap();
    for line in 0..300u16 {
        r.tick((line, 0, 0), (100..104).contains(&line));
    }
    let o = r.overlay("vsync", 5).unwrap();
    assert_eq!(
        o.shapes,
        vec![emu_core::overlay::Shape::Band {
            y: 100,
            h: 4,
            color: emu_core::overlay::colors::MAGENTA,
            label: Some("VSYNC".into()),
        }]
    );
    assert!(r.overlay("crtc", 5).is_none(), "not switched on");
}

fn pen_event(at: u64, x: u16, y: u16, trigger: bool) -> TimedInput {
    TimedInput {
        at: Cycle(at),
        target: InputTarget::Port(EXPANSION),
        event: InputEvent::LightPen { x, y, trigger },
    }
}

fn lpen(m: &mut Cpc) -> (u8, u8) {
    let mut r = |n| {
        m.io.crtc.select(n);
        m.io.crtc.read()
    };
    (r(16), r(17))
}

#[test]
fn light_pen_on_the_expansion_port_latches_r16_r17() {
    let mut config = MachineConfig {
        model: Some("6128".into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("expansion".into(), "lightpen".into());
    let mut m = Cpc::new(&config).unwrap();
    assert_eq!(m.ports()[2].kind, PortKind::Cartridge);
    frames(&mut m, 20);
    // Character 5 of row 3 (white pen 1): MA = &3000 + 3 × 40 + 5.
    let (x, y) = (64 + 5 * 16 + 7, 36 + 3 * 8 + 2);
    let now = m.now().0;
    m.input(pen_event(now, x, y, false));
    frames(&mut m, 2);
    assert_eq!(lpen(&mut m), (0, 0), "no strobe without the switch");
    let now = m.now().0;
    m.input(pen_event(now, x, y, true));
    frames(&mut m, 2);
    assert_eq!(lpen(&mut m), (0x30, 0x7d));
    // Savestate: the pen's aim and the registers come back.
    let s = m.save_state();
    let now = m.now().0;
    m.input(pen_event(now, x + 16, y, true));
    frames(&mut m, 2);
    assert_eq!(lpen(&mut m), (0x30, 0x7e));
    m.load_state(&s).unwrap();
    assert_eq!(lpen(&mut m), (0x30, 0x7d));
    frames(&mut m, 2);
    assert_eq!(lpen(&mut m), (0x30, 0x7d));
    // The black border does not light the pen.
    let now = m.now().0;
    m.input(pen_event(now, 10, 10, true));
    frames(&mut m, 2);
    assert_eq!(lpen(&mut m), (0x30, 0x7d));
}
