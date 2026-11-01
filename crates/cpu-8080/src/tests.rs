use super::*;
use emu_core::Flow;

/// 64 KB of RAM counting T-states and recording the machine cycles.
struct Ram {
    m: Vec<u8>,
    t: u32,
    cycles: Vec<(u8, u16)>,
    ports: [u8; 256],
}

impl Ram {
    fn with(code: &[u8]) -> Self {
        let mut m = vec![0; 0x10000];
        m[..code.len()].copy_from_slice(code);
        Self {
            m,
            t: 0,
            cycles: Vec::new(),
            ports: [0; 256],
        }
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u32, _s: Size) -> u32 {
        if addr >= IO_BASE {
            u32::from(self.ports[addr as usize & 0xff])
        } else {
            u32::from(self.m[addr as usize & 0xffff])
        }
    }
    fn peek(&self, addr: u32, _s: Size) -> u32 {
        u32::from(self.m[addr as usize & 0xffff])
    }
    fn write(&mut self, addr: u32, _s: Size, v: u32) {
        if addr >= IO_BASE {
            self.ports[addr as usize & 0xff] = v as u8;
        } else {
            self.m[addr as usize & 0xffff] = v as u8;
        }
    }
    fn addr_bits(&self) -> u8 {
        17
    }
}

impl StatusBus for Ram {
    fn cycle(&mut self, status: u8, addr: u16, _data: u8, t: u32) {
        self.t += t;
        self.cycles.push((status, addr));
    }
}

/// T-states of every opcode, the shorter one for conditional RET and CALL
/// (8080 User's Manual, instruction summary).
#[rustfmt::skip]
const CYCLES: [u8; 256] = [
//  0   1   2   3   4   5   6   7   8   9   A   B   C   D   E   F
    4, 10,  7,  5,  5,  5,  7,  4,  4, 10,  7,  5,  5,  5,  7,  4, // 0
    4, 10,  7,  5,  5,  5,  7,  4,  4, 10,  7,  5,  5,  5,  7,  4, // 1
    4, 10, 16,  5,  5,  5,  7,  4,  4, 10, 16,  5,  5,  5,  7,  4, // 2
    4, 10, 13,  5, 10, 10, 10,  4,  4, 10, 13,  5,  5,  5,  7,  4, // 3
    5,  5,  5,  5,  5,  5,  7,  5,  5,  5,  5,  5,  5,  5,  7,  5, // 4
    5,  5,  5,  5,  5,  5,  7,  5,  5,  5,  5,  5,  5,  5,  7,  5, // 5
    5,  5,  5,  5,  5,  5,  7,  5,  5,  5,  5,  5,  5,  5,  7,  5, // 6
    7,  7,  7,  7,  7,  7,  7,  7,  5,  5,  5,  5,  5,  5,  7,  5, // 7
    4,  4,  4,  4,  4,  4,  7,  4,  4,  4,  4,  4,  4,  4,  7,  4, // 8
    4,  4,  4,  4,  4,  4,  7,  4,  4,  4,  4,  4,  4,  4,  7,  4, // 9
    4,  4,  4,  4,  4,  4,  7,  4,  4,  4,  4,  4,  4,  4,  7,  4, // A
    4,  4,  4,  4,  4,  4,  7,  4,  4,  4,  4,  4,  4,  4,  7,  4, // B
    5, 10, 10, 10, 11, 11,  7, 11,  5, 10, 10, 10, 11, 17,  7, 11, // C
    5, 10, 10, 10, 11, 11,  7, 11,  5, 10, 10, 10, 11, 17,  7, 11, // D
    5, 10, 10, 18, 11, 11,  7, 11,  5,  5, 10,  4, 11, 17,  7, 11, // E
    5, 10, 10,  4, 11, 11,  7, 11,  5,  5, 10,  4, 11, 17,  7, 11, // F
];

#[test]
fn every_opcode_takes_its_t_states() {
    for op in 0..=255u8 {
        let mut cpu = I8080::new();
        cpu.sp = 0x8000;
        // Conditional RET and CALL: make the condition false.
        if op & 0xc7 == 0xc0 || op & 0xc7 == 0xc4 {
            let wants_set = op >> 3 & 1 == 1;
            match op >> 4 & 3 {
                0 => cpu.z = !wants_set,
                1 => cpu.cy = !wants_set,
                2 => cpu.p = !wants_set,
                _ => cpu.s = !wants_set,
            }
        }
        let mut bus = Ram::with(&[op, 0x00, 0x90]);
        let t = cpu.step(&mut bus);
        // The table's $76 is MOV M,M's slot: HLT.
        assert_eq!(t, u32::from(CYCLES[op as usize]), "opcode {op:02X}");
        assert_eq!(bus.t, t, "opcode {op:02X}: reported cycles");
    }
}

#[test]
fn taken_conditions_take_six_more() {
    let mut cpu = I8080::new();
    cpu.sp = 0x8000;
    cpu.z = true;
    assert_eq!(cpu.step(&mut Ram::with(&[0xc8])), 11, "RZ");
    cpu.pc = 0;
    assert_eq!(cpu.step(&mut Ram::with(&[0xcc, 0, 0x90])), 17, "CZ");
}

#[test]
fn status_words_follow_the_bus() {
    let mut cpu = I8080::new();
    cpu.sp = 0x8000;
    // PUSH B, then STA $9000, IN 5, OUT 6.
    let mut bus = Ram::with(&[0xc5, 0x32, 0, 0x90, 0xdb, 5, 0xd3, 6]);
    for _ in 0..4 {
        cpu.step(&mut bus);
    }
    let writes: Vec<_> = bus
        .cycles
        .iter()
        .copied()
        .filter(|c| c.0 & status::WO == 0)
        .collect();
    assert_eq!(
        writes,
        [
            (status::STACK_WRITE, 0x7fff),
            (status::STACK_WRITE, 0x7ffe),
            (status::WRITE, 0x9000),
            (status::OUTPUT, 6),
        ]
    );
    assert!(bus.cycles.iter().any(|c| c.0 == status::INPUT));
    assert_eq!(bus.cycles[0], (status::FETCH, 0));
}

#[test]
fn flags_of_subtraction_and_logic() {
    let mut cpu = I8080::new();
    cpu.a = 0x3e;
    cpu.step(&mut Ram::with(&[0xd6, 0x3e])); // SUI $3E
    assert_eq!(cpu.a, 0);
    assert!(
        cpu.z && cpu.p && !cpu.cy && cpu.ac,
        "the 8080's AC after SUB"
    );
    cpu.pc = 0;
    cpu.a = 0x08;
    cpu.step(&mut Ram::with(&[0xe6, 0x00])); // ANI 0: AC = OR of bit 3
    assert!(cpu.ac && !cpu.cy);
    assert_eq!(cpu.psw() & 0x2a, 0x02, "fixed bits");
}

#[test]
fn daa_from_the_manual() {
    let mut cpu = I8080::new();
    cpu.a = 0x9b;
    cpu.step(&mut Ram::with(&[0x27]));
    assert_eq!(cpu.a, 0x01);
    assert!(cpu.cy && cpu.ac);
}

#[test]
fn ei_waits_one_instruction_and_int_jams_rst7() {
    let mut cpu = I8080::new();
    cpu.sp = 0x8000;
    cpu.set_int(true);
    let mut bus = Ram::with(&[0xfb, 0x00, 0x00]);
    cpu.step(&mut bus); // EI
    cpu.step(&mut bus); // NOP still runs
    assert_eq!(cpu.pc, 2);
    assert_eq!(cpu.step(&mut bus), 11);
    assert_eq!(cpu.pc, 0x38);
    assert!(!cpu.inte);
    assert_eq!(bus.m[0x7ffe], 2, "return address");
}

#[test]
fn hlt_idles_until_an_interrupt() {
    let mut cpu = I8080::new();
    cpu.sp = 0x8000;
    cpu.inte = true;
    let mut bus = Ram::with(&[0x76]);
    assert_eq!(cpu.step(&mut bus), 7);
    assert_eq!(cpu.step(&mut bus), 4);
    assert!(cpu.halted);
    cpu.set_int(true);
    cpu.step(&mut bus);
    assert!(!cpu.halted);
    assert_eq!(cpu.pc, 0x38);
}

#[test]
fn disassembly() {
    let bus = Ram::with(&[
        0x3e, 0x12, 0xc3, 0x34, 0x12, 0x36, 1, 0x77, 0xc8, 0xff, 0x34,
    ]);
    let cpu = I8080::new();
    let show = |a| cpu.disassemble(&bus, a).to_string();
    assert_eq!(show(0), "MVI A,$12");
    assert_eq!(show(2), "JMP $1234");
    assert_eq!(cpu.disassemble(&bus, 2).target, Some(0x1234));
    assert_eq!(show(5), "MVI M,$01");
    assert_eq!(show(7), "MOV M,A");
    assert_eq!(show(8), "RZ");
    assert_eq!(cpu.disassemble(&bus, 8).flow, Flow::Return);
    assert_eq!(show(9), "RST 7");
    assert_eq!(show(10), "INR M");
}

#[test]
fn savestate_round_trip() {
    let mut cpu = I8080::new();
    cpu.a = 0x55;
    cpu.sp = 0x1234;
    cpu.inte = true;
    cpu.set_psw(0xd7);
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut other = I8080::new();
    other.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(other, cpu);
}
