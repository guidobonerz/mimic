use super::*;
use devices_input::Joystick;
use emu_core::input::{Dirs, InputEvent, MachineKey};

fn stub_with(variant: Variant16, model: &str, options: &[(&str, &str)]) -> TedMachine {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    config.model = Some(model.into());
    TedMachine::new(variant, &config).unwrap()
}

fn stub() -> TedMachine {
    stub_with(Variant16::Plus4, "pal", &[])
}

fn run_cycles(m: &mut TedMachine, n: u32) {
    for _ in 0..n {
        assert_eq!(m.step_cycle(), StopReason::StepDone);
    }
}

fn key_event(at: u64, k: MachineKey, down: bool) -> TimedInput {
    TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: k,
            down,
        },
    }
}

#[test]
fn stub_boots_and_takes_raster_interrupts() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    let idle = dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: u32::from(stub::IDLE),
    });
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(idle));
    dbg.remove_breakpoint(idle);
    let irq = dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: u32::from(stub::IRQ),
    });
    // The flag latched at line 0 before the compare value was set fires
    // first; then line 204.
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(irq));
    while m.run_frame(&mut dbg) != StopReason::Breakpoint(irq) {}
    assert_eq!(m.beam_pos().line, 204);
    let first = m.now().0;
    while m.run_frame(&mut dbg) != StopReason::Breakpoint(irq) {}
    // Once per field, give or take the instruction in progress.
    let period = m.now().0 - first;
    assert!(period.abs_diff(312 * 114) <= 8, "{period}");
}

#[test]
fn rom_banking_and_ram_underneath() {
    let mut m = stub();
    // ROM on: $C000 is the stub KERNAL, writes go to RAM.
    assert_eq!(m.peek(CPU_SPACE, 0xc000, Size::Byte), Some(0x78));
    assert!(m.poke(CPU_SPACE, 0xc000, Size::Byte, 0x42));
    assert_eq!(m.peek(CPU_SPACE, 0xc000, Size::Byte), Some(0x78));
    assert_eq!(m.peek(CPU_SPACE, 0xff13, Size::Byte).unwrap() & 1, 1);
    assert!(m.poke(CPU_SPACE, 0xff3f, Size::Byte, 0)); // RAM on
    assert_eq!(m.peek(CPU_SPACE, 0xc000, Size::Byte), Some(0x42));
    assert_eq!(m.peek(CPU_SPACE, 0xff13, Size::Byte).unwrap() & 1, 0);
    assert!(m.poke(CPU_SPACE, 0xff3e, Size::Byte, 0)); // ROM on
                                                       // Bank select by address; the stub has no function ROM: empty socket.
    assert!(m.poke(CPU_SPACE, 0xfdd5, Size::Byte, 0));
    assert_eq!(m.mem.banks, 5);
    assert_eq!(m.peek(CPU_SPACE, 0x8000, Size::Byte), Some(0xff));
    assert_eq!(
        m.peek(CPU_SPACE, 0xfc00, Size::Byte),
        Some(0xff),
        "KERNAL page"
    );
    // I/O stays visible with RAM on.
    assert!(m.poke(CPU_SPACE, 0xff3f, Size::Byte, 0));
    assert!(m.poke(CPU_SPACE, 0xff19, Size::Byte, 0x33));
    assert_eq!(m.peek(CPU_SPACE, 0xff19, Size::Byte), Some(0x33));
}

#[test]
fn c16_ram_is_mirrored() {
    let mut m = stub_with(Variant16::C16, "pal", &[]);
    assert!(m.poke(CPU_SPACE, 0x1234, Size::Byte, 0x5a));
    assert_eq!(m.peek(CPU_SPACE, 0x5234, Size::Byte), Some(0x5a));
    assert_eq!(
        m.peek(CPU_SPACE, 0xfd10, Size::Byte),
        Some(0xff),
        "no user port"
    );
    let mut m = stub_with(Variant16::C16, "pal", &[("ram", "64k")]);
    assert!(m.poke(CPU_SPACE, 0x1234, Size::Byte, 0x5a));
    assert_eq!(m.peek(CPU_SPACE, 0x5234, Size::Byte), Some(0x00));
}

#[test]
fn keyboard_and_joystick_through_ff08() {
    let mut m = stub();
    let now = m.now().0;
    m.input(key_event(now, keyboard::keys::RETURN, true));
    m.input(TimedInput {
        at: Cycle(now),
        target: InputTarget::Port(PortId(0)),
        event: InputEvent::Digital {
            dirs: Dirs::LEFT,
            buttons: 1,
        },
    });
    run_cycles(&mut m, 1);
    assert!(m.poke(CPU_SPACE, 0xfd30, Size::Byte, 0xfe)); // column 0
    assert!(m.poke(CPU_SPACE, 0xff08, Size::Byte, 0xff)); // no joystick
    assert_eq!(m.peek(CPU_SPACE, 0xff08, Size::Byte), Some(0xfd));
    assert!(m.poke(CPU_SPACE, 0xfd30, Size::Byte, 0xff));
    assert!(m.poke(CPU_SPACE, 0xff08, Size::Byte, 0xfb)); // joystick 1
    assert_eq!(m.peek(CPU_SPACE, 0xff08, Size::Byte), Some(0xbb));
    assert!(m.poke(CPU_SPACE, 0xff08, Size::Byte, 0xfd)); // joystick 2: empty
    assert_eq!(m.peek(CPU_SPACE, 0xff08, Size::Byte), Some(0xff));
}

#[test]
fn serial_bus_from_the_cpu_port() {
    let mut m = stub();
    assert!(m.poke(CPU_SPACE, 0x0000, Size::Byte, 0x0f));
    assert!(m.poke(CPU_SPACE, 0x0001, Size::Byte, 0x00));
    run_cycles(&mut m, 1);
    assert_eq!(m.io.iec, IecLines::default(), "released");
    assert!(m.poke(CPU_SPACE, 0x0001, Size::Byte, 0x07));
    run_cycles(&mut m, 1);
    assert!(m.io.iec.atn && m.io.iec.clk && m.io.iec.data);
    let port = m.peek(CPU_SPACE, 0x0001, Size::Byte).unwrap();
    assert_eq!(port & 0xc0, 0, "CLK and DATA in read the low lines");
}

#[test]
fn savestate_resumes_identically() {
    let mut a = stub_with(Variant16::Plus4, "ntsc", &[]);
    run_cycles(&mut a, 45_678);
    a.mem.ram[0x8000] = 7;
    let state = a.save_state();
    let mut b = stub_with(Variant16::Plus4, "ntsc", &[]);
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    let mut dbg = DebugHooks::new();
    a.run_frame(&mut dbg);
    b.run_frame(&mut dbg);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.framebuffer().pixels, b.framebuffer().pixels);

    let before = b.save_state();
    assert!(b.load_state(&state[..state.len() - 1]).is_err());
    assert_eq!(
        b.save_state(),
        before,
        "failed load leaves the machine unchanged"
    );
    assert!(stub().load_state(&state).is_err(), "PAL vs NTSC");
    assert!(
        stub_with(Variant16::C16, "ntsc", &[])
            .load_state(&state)
            .is_err(),
        "RAM size"
    );
}

#[test]
fn frames_and_geometry() {
    for (model, lines, height) in [("pal", 312, 284), ("ntsc", 262, 240)] {
        let mut m = stub_with(Variant16::Plus4, model, &[]);
        let mut dbg = DebugHooks::new();
        m.run_frame(&mut dbg);
        let t = m.now().0;
        m.run_frame(&mut dbg);
        assert_eq!(m.now().0 - t, lines * 114, "{model}");
        let f = m.framebuffer();
        assert_eq!((f.width, f.height), (384, height));
    }
}

#[test]
fn debugger_view_and_ports() {
    let mut m = stub();
    let names: Vec<_> = m.chips().iter().map(|c| c.name).collect();
    assert_eq!(names, ["ted", "memory"]);
    assert_eq!(m.chips()[0].regs.len(), 32);
    assert_eq!(m.disassemble(0xc000).mnemonic, "SEI");
    assert_eq!(m.ports().len(), 2);
    assert!(m.ports()[0].connected && !m.ports()[1].connected);
    assert!(m
        .connect(PortId(1), Box::new(Joystick::new(PortKind::CbmControl)))
        .is_ok());
}

#[test]
fn graphics_views_follow_the_ted() {
    let mut m = stub();
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["charset", "bitmap", "screen", "palette"]);
    // Matrix $0800 (codes $0C00), characters $1000 from RAM, bitmap $2000.
    for (reg, v) in [(0x12u32, 0x08u32), (0x13, 0x10), (0x14, 0x08), (0x15, 0x71)] {
        assert!(m.poke(CPU_SPACE, 0xff00 + reg, Size::Byte, v));
    }
    m.mem.ram[0x0800] = 0x46; // blue, luminance 4
    m.mem.ram[0x0c00] = 1;
    m.mem.ram[0x1008] = 0x80;
    m.mem.ram[0x2000] = 0x80;
    let sc = m.gfx_view("screen", &GfxOptions::new()).unwrap();
    assert_eq!((sc.width, sc.height, sc.addr), (320, 200, 0x0800));
    assert_eq!(sc.raw.len(), 2000);
    assert_eq!(sc.param("rom"), Some(0));
    assert_eq!(sc.frame().rgb_at(0, 0), Some(m.palette[0x46]));
    assert_eq!(sc.frame().rgb_at(1, 0), Some(m.palette[0x71]));
    let bm = m.gfx_view("bitmap", &GfxOptions::new()).unwrap();
    assert_eq!((bm.addr, bm.raw.len()), (0x2000, 8000));
    // Code $01: hue 1 for set bits with the attribute's luminance 6.
    assert_eq!(bm.frame().rgb_at(0, 0), Some(m.palette[0x61]));
    let cs = m.gfx_view("charset", &GfxOptions::new()).unwrap();
    assert_eq!(cs.raw[8], 0x80);
    let p = m.gfx_view("palette", &GfxOptions::new()).unwrap();
    assert_eq!(p.raw.len(), 128);
}

#[test]
fn overlays_record_dma_lines() {
    let mut m = stub();
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["badlines"]);
    assert_eq!(m.frame_row(274), Some(0));
    assert_eq!(m.frame_row(250), None);
    let plain = m.save_state();
    m.set_overlays(&["badlines"]);
    assert_eq!(m.save_state(), plain, "the record is not state");
    assert!(m.poke(CPU_SPACE, 0xff06, Size::Byte, 0x1b));
    let mut dbg = DebugHooks::new();
    for _ in 0..2 {
        while m.run_frame(&mut dbg) != StopReason::FrameDone {}
    }
    let o = m.overlay("badlines").unwrap();
    assert_eq!(o.shapes.len(), 25);
    m.set_overlays(&[]);
    assert!(m.overlay("badlines").is_none());
}
