use super::*;
use crate::flag::{C, H, N, Z};
use mimic_core::Flow;

/// 64 KB of RAM that records the machine cycles: 'r', 'w' or 'i' and the
/// address.
struct Ram {
    m: Vec<u8>,
    cycles: Vec<(char, u16)>,
    /// IF and IE.
    requested: u8,
    enabled: u8,
    speed_switch: bool,
}

impl Ram {
    fn with(code: &[u8]) -> Self {
        let mut m = vec![0; 0x10000];
        m[..code.len()].copy_from_slice(code);
        Self {
            m,
            cycles: Vec::new(),
            requested: 0,
            enabled: 0,
            speed_switch: false,
        }
    }
}

impl MBus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.cycles.push(('r', addr));
        self.m[usize::from(addr)]
    }
    fn write(&mut self, addr: u16, v: u8) {
        self.cycles.push(('w', addr));
        self.m[usize::from(addr)] = v;
    }
    fn idle(&mut self) {
        self.cycles.push(('i', 0));
    }
    fn pending(&self) -> u8 {
        self.requested & self.enabled
    }
    fn acknowledge(&mut self, bit: u8) {
        self.requested &= !bit;
    }
    fn stop(&mut self) -> bool {
        self.speed_switch
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u32, _s: Size) -> u32 {
        u32::from(self.m[addr as usize & 0xffff])
    }
    fn peek(&self, addr: u32, _s: Size) -> u32 {
        u32::from(self.m[addr as usize & 0xffff])
    }
    fn write(&mut self, addr: u32, _s: Size, v: u32) {
        self.m[addr as usize & 0xffff] = v as u8;
    }
    fn addr_bits(&self) -> u8 {
        16
    }
}

fn run(code: &[u8], steps: usize) -> (Sm83, Ram) {
    let mut ram = Ram::with(code);
    let mut cpu = Sm83::new();
    cpu.sp = 0xfffe;
    for _ in 0..steps {
        cpu.step(&mut ram);
    }
    (cpu, ram)
}

/// Machine cycles per opcode, conditions not met; 0: no instruction.
#[rustfmt::skip]
const CYCLES: [u8; 256] = [
    1,3,2,2,1,1,2,1, 5,2,2,2,1,1,2,1,
    1,3,2,2,1,1,2,1, 3,2,2,2,1,1,2,1,
    2,3,2,2,1,1,2,1, 2,2,2,2,1,1,2,1,
    2,3,2,2,3,3,3,1, 2,2,2,2,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    2,2,2,2,2,2,1,2, 1,1,1,1,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    1,1,1,1,1,1,2,1, 1,1,1,1,1,1,2,1,
    2,3,3,4,3,4,2,4, 2,4,3,2,3,6,2,4,
    2,3,3,0,3,4,2,4, 2,4,3,0,3,0,2,4,
    3,3,2,0,0,4,2,4, 4,1,4,0,0,0,2,4,
    3,3,2,1,0,4,2,4, 3,2,4,1,0,0,2,4,
];

#[test]
fn every_opcode_takes_its_machine_cycles() {
    for op in 0..=255u8 {
        // Flags so that no condition is met: Z and C clear for NZ/NC
        // would be met, so test both ways below.
        let mut ram = Ram::with(&[op, 0x00, 0x80]);
        let mut cpu = Sm83::new();
        cpu.sp = 0xc000;
        cpu.set_hl(0xd000);
        // Conditions on Z: odd rows of the condition field want Z set.
        let cond = op >> 3 & 3;
        cpu.f = match cond {
            0 => Z,
            2 => C,
            _ => 0,
        };
        let m = cpu.step(&mut ram);
        let want = u32::from(CYCLES[usize::from(op)]);
        if want == 0 {
            assert!(cpu.locked, "${op:02X} locks the processor up");
            assert_eq!(cpu.step(&mut ram), 1);
            continue;
        }
        assert_eq!(m, want, "opcode ${op:02X}");
        assert_eq!(
            ram.cycles.len() as u32,
            want,
            "opcode ${op:02X}: on the bus"
        );
        assert!(!cpu.locked, "${op:02X}");
    }
}

#[test]
fn conditions_met_take_longer() {
    for (op, flags, want) in [
        (0x20u8, 0u8, 3u32), // JR NZ
        (0x28, Z, 3),
        (0xc2, 0, 4), // JP NZ
        (0xda, C, 4),
        (0xc4, 0, 6), // CALL NZ
        (0xdc, C, 6),
        (0xc0, 0, 5), // RET NZ
        (0xd8, C, 5),
    ] {
        let mut ram = Ram::with(&[op, 0x02, 0x80]);
        let mut cpu = Sm83::new();
        cpu.sp = 0xc000;
        cpu.f = flags;
        assert_eq!(cpu.step(&mut ram), want, "opcode ${op:02X}");
    }
}

#[test]
fn the_cb_page_takes_two_to_four_cycles() {
    for (second, want) in [
        (0x00u8, 2u32),
        (0x06, 4),
        (0x46, 3),
        (0x7e, 3),
        (0x86, 4),
        (0xc7, 2),
    ] {
        let mut ram = Ram::with(&[0xcb, second]);
        let mut cpu = Sm83::new();
        cpu.set_hl(0xd000);
        assert_eq!(cpu.step(&mut ram), want, "CB ${second:02X}");
    }
    // SWAP, SRA keeping the sign, RL through carry, BIT keeping carry.
    let (cpu, _) = run(&[0x3e, 0xa5, 0xcb, 0x37], 2);
    assert_eq!((cpu.a, cpu.f), (0x5a, 0));
    let (cpu, _) = run(&[0x3e, 0x81, 0xcb, 0x2f], 2);
    assert_eq!((cpu.a, cpu.f), (0xc0, C));
    let (cpu, _) = run(&[0x37, 0x3e, 0x80, 0xcb, 0x17], 3);
    assert_eq!((cpu.a, cpu.f), (0x01, C));
    let (cpu, _) = run(&[0x37, 0x3e, 0x80, 0xcb, 0x77], 3);
    assert_eq!(cpu.f, Z | H | C);
    let (cpu, ram) = run(&[0x21, 0x00, 0xd0, 0xcb, 0xfe, 0xcb, 0x86], 3);
    assert_eq!((cpu.hl(), ram.m[0xd000]), (0xd000, 0x80 & !1));
}

#[test]
fn the_bus_sees_every_cycle_in_order() {
    // CALL $1234: fetch, low, high, internal, push high, push low.
    let (cpu, ram) = run(&[0xcd, 0x34, 0x12], 1);
    assert_eq!(cpu.pc, 0x1234);
    assert_eq!(
        ram.cycles,
        [
            ('r', 0),
            ('r', 1),
            ('r', 2),
            ('i', 0),
            ('w', 0xfffd),
            ('w', 0xfffc)
        ]
    );
    assert_eq!((ram.m[0xfffd], ram.m[0xfffc]), (0x00, 0x03));
    // PUSH BC: the internal cycle comes first; RET NZ: before the test.
    let (_, ram) = run(&[0xc5], 1);
    assert_eq!(ram.cycles[1], ('i', 0));
    // LD ($C000),SP writes low then high.
    let (_, ram) = run(&[0x08, 0x00, 0xc0], 1);
    assert_eq!(ram.cycles[3..], [('w', 0xc000), ('w', 0xc001)]);
    assert_eq!((ram.m[0xc000], ram.m[0xc001]), (0xfe, 0xff));
    // LDH ($80),A and LDH A,(C).
    let (cpu, ram) = run(&[0x3e, 0x42, 0xe0, 0x80, 0x0e, 0x80, 0xaf, 0xf2], 5);
    assert_eq!((ram.m[0xff80], cpu.a), (0x42, 0x42));
}

#[test]
fn arithmetic_and_its_flags() {
    // ADD: half carry and carry; ADC with carry in.
    let (cpu, _) = run(&[0x3e, 0x8f, 0xc6, 0x81], 2);
    assert_eq!((cpu.a, cpu.f), (0x10, H | C));
    let (cpu, _) = run(&[0x37, 0x3e, 0xff, 0xce, 0x00], 3);
    assert_eq!((cpu.a, cpu.f), (0x00, Z | H | C));
    // SUB and CP: borrow; SBC.
    let (cpu, _) = run(&[0x3e, 0x10, 0xd6, 0x01], 2);
    assert_eq!((cpu.a, cpu.f), (0x0f, N | H));
    let (cpu, _) = run(&[0x3e, 0x10, 0xfe, 0x20], 2);
    assert_eq!((cpu.a, cpu.f), (0x10, N | C));
    let (cpu, _) = run(&[0x37, 0x3e, 0x00, 0xde, 0x00], 3);
    assert_eq!((cpu.a, cpu.f), (0xff, N | H | C));
    // AND sets H; XOR A is zero.
    let (cpu, _) = run(&[0x3e, 0xf0, 0xe6, 0x0f], 2);
    assert_eq!((cpu.a, cpu.f), (0, Z | H));
    // INC and DEC keep the carry.
    let (cpu, _) = run(&[0x37, 0x06, 0xff, 0x04, 0x05], 4);
    assert_eq!((cpu.b, cpu.f), (0xff, N | H | C));
    // ADD HL keeps Z; the half carry is from bit 11.
    let (cpu, _) = run(&[0xaf, 0x21, 0xff, 0x0f, 0x01, 0x01, 0x00, 0x09], 4);
    assert_eq!((cpu.hl(), cpu.f), (0x1000, Z | H));
    // ADD SP,-1 and LD HL,SP+1: flags of the low bytes.
    let (cpu, _) = run(&[0x31, 0x00, 0xd0, 0xe8, 0xff], 2);
    assert_eq!((cpu.sp, cpu.f), (0xcfff, 0));
    let (cpu, _) = run(&[0x31, 0xff, 0xd0, 0xf8, 0x01], 2);
    assert_eq!((cpu.hl(), cpu.sp, cpu.f), (0xd100, 0xd0ff, H | C));
    // RLCA clears Z even for 0; CCF and SCF.
    let (cpu, _) = run(&[0xaf, 0x07, 0x37, 0x3f], 4);
    assert_eq!((cpu.a, cpu.f), (0, 0));
    // POP AF: the low four bits of F do not exist.
    let (cpu, _) = run(&[0x01, 0xff, 0x12, 0xc5, 0xf1], 3);
    assert_eq!(cpu.af(), 0x12f0);
}

#[test]
fn daa_adjusts_after_additions_and_subtractions() {
    // 0x15 + 0x27 = 0x3c -> 42.
    let (cpu, _) = run(&[0x3e, 0x15, 0xc6, 0x27, 0x27], 3);
    assert_eq!((cpu.a, cpu.f), (0x42, 0));
    // 0x99 + 0x01 = 0x9a -> 00 with carry.
    let (cpu, _) = run(&[0x3e, 0x99, 0xc6, 0x01, 0x27], 3);
    assert_eq!((cpu.a, cpu.f), (0x00, Z | C));
    // 0x42 - 0x15 = 0x2d -> 27.
    let (cpu, _) = run(&[0x3e, 0x42, 0xd6, 0x15, 0x27], 3);
    assert_eq!((cpu.a, cpu.f), (0x27, N));
    // 0x00 - 0x01 = 0xff -> 99 with carry.
    let (cpu, _) = run(&[0x3e, 0x00, 0xd6, 0x01, 0x27], 3);
    assert_eq!((cpu.a, cpu.f), (0x99, N | C));
}

#[test]
fn interrupts_are_taken_with_ime_and_ei_is_late() {
    // EI, NOP, NOP at 0; a timer interrupt (bit 2) is pending all along.
    let mut ram = Ram::with(&[0xfb, 0x00, 0x00]);
    ram.requested = 0x04;
    ram.enabled = 0x04;
    let mut cpu = Sm83::new();
    cpu.sp = 0xd000;
    cpu.step(&mut ram);
    assert!(!cpu.ime, "not yet");
    // The instruction behind EI still runs.
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 2);
    ram.cycles.clear();
    assert_eq!(cpu.step(&mut ram), 5);
    assert_eq!(cpu.pc, 0x50);
    assert!(!cpu.ime);
    assert_eq!(ram.requested, 0, "acknowledged");
    assert_eq!(
        ram.cycles,
        [('i', 0), ('i', 0), ('w', 0xcfff), ('w', 0xcffe), ('i', 0)]
    );
    assert_eq!((ram.m[0xcfff], ram.m[0xcffe]), (0x00, 0x02));
    // The lowest bit wins; RETI sets IME at once.
    let mut ram = Ram::with(&[0x00]);
    ram.m[0x40] = 0xd9;
    ram.requested = 0x13;
    ram.enabled = 0x1f;
    let mut cpu = Sm83::new();
    cpu.sp = 0xd000;
    cpu.ime = true;
    cpu.step(&mut ram);
    assert_eq!((cpu.pc, ram.requested), (0x40, 0x12));
    cpu.step(&mut ram);
    assert!(cpu.ime);
    assert_eq!(cpu.pc, 0);
    cpu.step(&mut ram);
    assert_eq!((cpu.pc, ram.requested), (0x48, 0x10));
    // DI behind EI: no interrupt.
    let mut ram = Ram::with(&[0xfb, 0xf3, 0x00]);
    ram.requested = 1;
    ram.enabled = 1;
    let mut cpu = Sm83::new();
    for _ in 0..3 {
        cpu.step(&mut ram);
    }
    assert_eq!(cpu.pc, 3);
}

#[test]
fn halt_waits_and_has_its_bug() {
    // HALT with IME: waits, then serves the interrupt.
    let mut ram = Ram::with(&[0x76, 0x00]);
    ram.enabled = 1;
    let mut cpu = Sm83::new();
    cpu.sp = 0xd000;
    cpu.ime = true;
    cpu.step(&mut ram);
    assert!(cpu.halted);
    assert_eq!(cpu.step(&mut ram), 1);
    assert_eq!(cpu.pc, 1);
    ram.requested = 1;
    assert_eq!(cpu.step(&mut ram), 5);
    assert_eq!((cpu.pc, cpu.halted), (0x40, false));
    // Without IME it wakes up and runs on.
    let mut ram = Ram::with(&[0x76, 0x3c]);
    ram.enabled = 1;
    let mut cpu = Sm83::new();
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    ram.requested = 1;
    cpu.step(&mut ram);
    assert_eq!((cpu.a, cpu.pc, ram.requested), (1, 2, 1));
    // The bug: an interrupt pending already, IME clear: the byte behind
    // HALT runs twice (INC A).
    let mut ram = Ram::with(&[0x76, 0x3c, 0x00]);
    ram.enabled = 1;
    ram.requested = 1;
    let mut cpu = Sm83::new();
    for _ in 0..3 {
        cpu.step(&mut ram);
    }
    assert_eq!((cpu.a, cpu.pc, cpu.halted), (2, 2, false));
}

#[test]
fn stop_waits_for_the_joypad_or_switches_the_speed() {
    let mut ram = Ram::with(&[0x10, 0x00, 0x3c]);
    ram.enabled = 0x1f;
    let mut cpu = Sm83::new();
    cpu.step(&mut ram);
    assert!(cpu.stopped);
    assert_eq!(cpu.pc, 2);
    ram.requested = 0x04;
    cpu.step(&mut ram);
    assert!(cpu.stopped, "only the joypad ends it");
    ram.requested = INT_JOYPAD;
    cpu.step(&mut ram);
    assert_eq!((cpu.stopped, cpu.a), (false, 1));
    let mut ram = Ram::with(&[0x10, 0x00]);
    ram.speed_switch = true;
    let mut cpu = Sm83::new();
    cpu.step(&mut ram);
    assert!(!cpu.stopped);
}

#[test]
fn tick_counts_four_clocks_a_machine_cycle() {
    let mut ram = Ram::with(&[0x3e, 0x07, 0x00]);
    let mut cpu = Sm83::new();
    let mut clocks = 0;
    loop {
        clocks += 1;
        if let CycleResult::InstrDone { next_pc } = cpu.tick(&mut ram) {
            assert_eq!(next_pc, 2);
            break;
        }
    }
    assert_eq!((clocks, cpu.a), (8, 7));
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, _) = run(&[0x3e, 0x99, 0xc6, 0x01, 0xfb, 0x76], 4);
    // The count of the instruction running is not state.
    cpu.m = 0;
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut other = Sm83::new();
    other.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(other, cpu);
    assert!(Sm83::new()
        .load(&mut StateReader::new(&bytes[..bytes.len() - 3]))
        .is_err());
}

#[test]
fn disassembly() {
    let code = [
        0x3e, 0x12, 0xcb, 0x7e, 0x20, 0xfe, 0xe0, 0x44, 0xcd, 0x34, 0x12, 0xf8, 0xfe, 0x2a, 0xd9,
        0xd3, 0x86, 0xc8,
    ];
    let ram = Ram::with(&code);
    let cpu = Sm83::new();
    let mut at = 0;
    let mut out = Vec::new();
    while at < code.len() as u32 {
        let i = cpu.disassemble(&ram, at);
        at += i.bytes.len() as u32;
        out.push((i.to_string(), i.flow, i.target));
    }
    assert_eq!(
        out,
        [
            ("LD A,$12".to_string(), Flow::Normal, None),
            ("BIT 7,(HL)".into(), Flow::Normal, None),
            ("JR NZ,$0004".into(), Flow::Branch, Some(4)),
            ("LDH ($FF44),A".into(), Flow::Normal, None),
            ("CALL $1234".into(), Flow::Call, Some(0x1234)),
            ("LD HL,SP-2".into(), Flow::Normal, None),
            ("LD A,(HL+)".into(), Flow::Normal, None),
            ("RETI".into(), Flow::Return, None),
            ("??? $D3".into(), Flow::Normal, None),
            ("ADD A,(HL)".into(), Flow::Normal, None),
            ("RET Z".into(), Flow::Return, None),
        ]
    );
}
