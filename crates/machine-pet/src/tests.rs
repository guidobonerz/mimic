use super::*;
use emu_core::input::{InputEvent, MachineKey};
use emu_core::media::{Media, MediaBackend, MediaError};

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::WriteProtected)
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn stub_with(model: &str, options: &[(&str, &str)]) -> Pet {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    config.model = Some(model.into());
    Pet::new(&config).unwrap()
}

fn stub() -> Pet {
    stub_with("4032", &[])
}

fn run_cycles(m: &mut Pet, n: u32) {
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
fn stub_boots_and_takes_retrace_interrupts() {
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
    // One retrace interrupt per frame.
    let start = m.now().0;
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(irq));
    let first = m.now().0;
    loop {
        if m.run_frame(&mut dbg) == StopReason::Breakpoint(irq) {
            break;
        }
    }
    // 50 Hz: 400 lines × 50 µs, give or take the instruction in progress.
    let period = m.now().0 - first;
    assert!((19_993..=20_007).contains(&period), "{period}");
    assert!(first > start);
}

#[test]
fn frame_rates_per_model() {
    for (model, hz, cycles) in [
        ("2001", "50", 64 * 260),
        ("4032", "50", 50 * 400),
        ("4032", "60", 50 * 333),
        ("8032", "50", 51 * 392),
    ] {
        let mut m = stub_with(model, &[("hz", hz)]);
        let mut dbg = DebugHooks::new();
        m.run_frame(&mut dbg);
        let t = m.now().0;
        m.run_frame(&mut dbg);
        assert_eq!(m.now().0 - t, cycles, "{model} {hz} Hz");
    }
}

#[test]
fn memory_map() {
    let mut m = stub_with("3032", &[("ram", "16k")]);
    assert!(m.poke(CPU_SPACE, 0x3fff, Size::Byte, 0x5a));
    assert_eq!(m.peek(CPU_SPACE, 0x3fff, Size::Byte), Some(0x5a));
    assert!(!m.poke(CPU_SPACE, 0x4000, Size::Byte, 0x5a), "no RAM there");
    assert_eq!(
        m.peek(CPU_SPACE, 0x4000, Size::Byte),
        Some(0x40),
        "open bus"
    );
    assert_eq!(m.peek(CPU_SPACE, 0x9123, Size::Byte), Some(0x91));
    // 40-column video RAM is 1 KB, mirrored up to $8FFF.
    assert!(m.poke(CPU_SPACE, 0x8001, Size::Byte, 0x41));
    assert_eq!(m.peek(CPU_SPACE, 0x8401, Size::Byte), Some(0x41));
    assert!(!m.poke(CPU_SPACE, 0xf000, Size::Byte, 0));

    let mut m = stub_with("8032", &[]);
    assert!(m.poke(CPU_SPACE, 0x8401, Size::Byte, 0x42));
    assert_eq!(m.peek(CPU_SPACE, 0x8001, Size::Byte), Some(0x00), "2 KB");
    assert_eq!(m.peek(CPU_SPACE, 0x8c01, Size::Byte), Some(0x42));
}

#[test]
fn io_chips_are_selected_by_address_bits() {
    let mut m = stub();
    assert!(m.poke(CPU_SPACE, 0xe843, Size::Byte, 0x12)); // VIA DDRA
    assert_eq!(m.io.via.ddra, 0x12);
    assert!(m.poke(CPU_SPACE, 0xe810, Size::Byte, 0x0f)); // PIA1 DDRA
    assert_eq!(m.io.pia1.a.ddr, 0x0f);
    assert!(m.poke(CPU_SPACE, 0xe880, Size::Byte, 1)); // CRTC R1
    assert!(m.poke(CPU_SPACE, 0xe881, Size::Byte, 39));
    assert_eq!(m.video.crtc.regs[1], 39);
    assert_eq!(
        m.peek(CPU_SPACE, 0xe800, Size::Byte),
        Some(0xe8),
        "nothing selected"
    );

    let mut m = stub_with("3032", &[]);
    assert!(m.poke(CPU_SPACE, 0xe880, Size::Byte, 1));
    assert!(m.poke(CPU_SPACE, 0xe881, Size::Byte, 39));
    assert_eq!(m.video.crtc.regs[1], 40, "no CRTC on the 3032");
}

#[test]
fn keyboard_rows_through_pia1() {
    let mut m = stub();
    // DDRA: PA0–3 out; then select port A and row 6.
    assert!(m.poke(CPU_SPACE, 0xe811, Size::Byte, 0x00));
    assert!(m.poke(CPU_SPACE, 0xe810, Size::Byte, 0x0f));
    assert!(m.poke(CPU_SPACE, 0xe811, Size::Byte, 0x04));
    assert!(m.poke(CPU_SPACE, 0xe810, Size::Byte, 0x06));
    assert!(m.poke(CPU_SPACE, 0xe813, Size::Byte, 0x04));
    let ret = Layout::Graphics.key("RETURN").unwrap();
    let now = m.now().0;
    m.input(key_event(now, ret, true));
    run_cycles(&mut m, 1);
    assert_eq!(m.peek(CPU_SPACE, 0xe812, Size::Byte), Some(0xdf));
    assert!(m.poke(CPU_SPACE, 0xe810, Size::Byte, 0x05));
    assert_eq!(m.peek(CPU_SPACE, 0xe812, Size::Byte), Some(0xff));
}

#[test]
fn ieee_bus_reads_back_the_pets_own_lines() {
    let mut m = stub();
    // PIA2 port B (data out) = $5A; port A (data in) reads it.
    assert!(m.poke(CPU_SPACE, 0xe823, Size::Byte, 0x00));
    assert!(m.poke(CPU_SPACE, 0xe822, Size::Byte, 0xff));
    assert!(m.poke(CPU_SPACE, 0xe823, Size::Byte, 0x04));
    assert!(m.poke(CPU_SPACE, 0xe822, Size::Byte, 0x5a));
    assert!(m.poke(CPU_SPACE, 0xe821, Size::Byte, 0x04));
    assert_eq!(m.peek(CPU_SPACE, 0xe820, Size::Byte), Some(0x5a));
    // NDAC out low (PIA2 CA2 manual low) shows on VIA PB0.
    assert!(m.poke(CPU_SPACE, 0xe821, Size::Byte, 0x34));
    assert_eq!(m.peek(CPU_SPACE, 0xe840, Size::Byte).unwrap() & 1, 0);
}

#[test]
fn video_shows_characters_and_reverse() {
    let mut m = stub();
    m.mem.roms.chargen[1 << 3] = 0xf0; // character 1, line 0
    assert!(m.poke(CPU_SPACE, 0xe84c, Size::Byte, 0x0c)); // graphics set
    assert!(m.poke(CPU_SPACE, 0x8000, Size::Byte, 0x01));
    assert!(m.poke(CPU_SPACE, 0x8001, Size::Byte, 0x81));
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    m.run_frame(&mut dbg);
    let w = m.video.width() as usize;
    let row: Vec<u8> = m.video.pixels()[20 * w + 32..20 * w + 48].to_vec();
    assert_eq!(row, [1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1]);
    assert_eq!(m.video.pixels()[19 * w + 32], 0, "margin");
}

#[test]
fn shift_register_sound() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    let mut audio = Vec::new();
    m.audio(&mut audio);
    assert!(audio.iter().all(|&s| s == 0), "silent");
    assert!(m.poke(CPU_SPACE, 0xe848, Size::Byte, 100));
    assert!(m.poke(CPU_SPACE, 0xe84b, Size::Byte, 0x10));
    assert!(m.poke(CPU_SPACE, 0xe84a, Size::Byte, 0x0f));
    audio.clear();
    m.run_frame(&mut dbg);
    m.audio(&mut audio);
    assert!(audio.iter().any(|&s| s > 3000) && audio.iter().any(|&s| s < -3000));
}

#[test]
fn savestate_resumes_identically() {
    let mut a = stub_with("8032", &[]);
    run_cycles(&mut a, 34_567);
    a.mem.vram[0x7ff] = 9;
    let state = a.save_state();
    let mut b = stub_with("8032", &[]);
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    let mut dbg = DebugHooks::new();
    a.run_frame(&mut dbg);
    b.run_frame(&mut dbg);
    assert_eq!(a.save_state(), b.save_state());

    let before = b.save_state();
    assert!(b.load_state(&state[..state.len() - 1]).is_err());
    assert_eq!(
        b.save_state(),
        before,
        "failed load leaves the machine unchanged"
    );
    assert!(stub().load_state(&state).is_err(), "other video RAM size");
}

#[test]
fn program_slot_puts_basic_at_0401() {
    let mut m = stub();
    let basic = [0x01, 0x04, 0x07, 0x04, 0x0a, 0x00, 0x80, 0x00, 0x00, 0x00];
    let media = Media {
        name: "t.prg".into(),
        kind: MediaKind::Program,
        backend: Box::new(Bytes(basic.to_vec())),
    };
    m.media_slot(SLOT_PROGRAM).unwrap().insert(media).unwrap();
    let mut dbg = DebugHooks::new();
    while m.now().0 < BOOT_CYCLES + 40_000 {
        m.run_frame(&mut dbg);
    }
    assert_eq!(&m.mem.ram[0x0401..0x0405], &[0x07, 0x04, 0x0a, 0x00]);
    assert_eq!(&m.mem.ram[0x2a..0x2c], &[0x09, 0x04], "VARTAB");
    assert!(!m.input.is_empty(), "RUN is being typed");
}

#[test]
fn configuration_errors() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.model = Some("9000".into());
    assert!(Pet::new(&config).is_err());
    config.model = Some("2001".into());
    config.options.insert("hz".into(), "55".into());
    assert!(Pet::new(&config).is_err());
    // Missing ROMs name the role.
    let mut config = MachineConfig {
        model: Some("4032".into()),
        ..MachineConfig::default()
    };
    config.roms.insert("basic4".into(), vec![0; 0x3000]);
    let err = Pet::new(&config).err().unwrap();
    assert!(format!("{err:?}").contains("edit4-40n-50"), "{err:?}");
}

#[test]
fn debugger_view() {
    let m = stub_with("8032", &[]);
    let names: Vec<_> = m.chips().iter().map(|c| c.name).collect();
    assert_eq!(names, ["pia1", "pia2", "via", "crtc"]);
    let m = stub_with("2001", &[]);
    assert_eq!(m.chips().len(), 3);
    assert_eq!(m.disassemble(0xf000).mnemonic, "SEI");
    assert_eq!(m.layout(), Layout::Graphics);
}
