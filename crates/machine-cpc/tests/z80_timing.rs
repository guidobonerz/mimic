//! Z80 instruction lengths on the CPC: the Z80's M-cycle placement
//! (`Z80::step_timed`) with the Gate Array's wait states
//! (`gate_array::cpu_wait_states`) must give the instruction lengths in
//! microseconds ("NOPs") of the CPC timing table (cpcwiki, "Z80").
//!
//! Each instruction runs repeatedly from the same address; its length is
//! the steady-state average, so instructions that end off the 1 µs grid
//! count with the stretch they cause in the next one.

use chips_amstrad::gate_array::cpu_wait_states;
use cpu_z80::{Access, TimedBus, Z80};
use emu_core::{Bus, Size};

const AT: u16 = 0x1000;

struct CpcBus {
    mem: Vec<u8>,
    /// Master T-state the current instruction started on.
    start: u64,
}

impl Bus for CpcBus {
    fn read(&mut self, addr: u32, _: Size) -> u32 {
        if addr > 0xffff {
            return 0xff;
        }
        u32::from(self.mem[addr as usize])
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        if addr > 0xffff {
            return 0xff;
        }
        let _ = size;
        u32::from(self.mem[addr as usize])
    }

    fn write(&mut self, addr: u32, _: Size, v: u32) {
        if addr <= 0xffff {
            self.mem[addr as usize] = v as u8;
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl TimedBus for CpcBus {
    fn wait_states(&mut self, _: Access, t: u32) -> u32 {
        cpu_wait_states(self.start + u64::from(t))
    }
}

/// Register setup before each run.
#[derive(Clone, Copy)]
enum Setup {
    /// Flags and counters that make conditions true / repeats repeat.
    Taken,
    /// The opposite.
    NotTaken,
    /// BC = 1: the last LDIR step.
    LastByte,
}

fn prepare(cpu: &mut Z80, setup: Setup) {
    cpu.pc = AT;
    cpu.sp = 0x8000;
    cpu.set_hl(0x4000);
    cpu.set_de(0x5000);
    cpu.ix = 0x4000;
    cpu.iy = 0x4000;
    cpu.halted = false;
    cpu.iff1 = false;
    cpu.a = 0x55;
    match setup {
        Setup::Taken => {
            // NZ true; B and BC > 1 so loops repeat.
            cpu.f = 0;
            cpu.set_bc(0x0505);
        }
        Setup::NotTaken => {
            cpu.f = 0x40;
            cpu.set_bc(0x0101);
        }
        Setup::LastByte => {
            cpu.f = 0;
            cpu.set_bc(0x0001);
        }
    }
}

/// Average length in µs of `bytes` executed over and over.
fn nops(bytes: &[u8], setup: Setup) -> f64 {
    let mut bus = CpcBus {
        mem: vec![0; 0x10000],
        start: 0,
    };
    // Return addresses on the stack point back at the instruction; the
    // memory operands differ from A so CPIR repeats.
    for a in (0x7f00..0x8100).step_by(2) {
        bus.mem[a] = AT as u8;
        bus.mem[a + 1] = (AT >> 8) as u8;
    }
    bus.mem[0x4000..0x4100].fill(0xaa);
    let mut cpu = Z80::new();
    let mut lengths = Vec::new();
    for _ in 0..12 {
        bus.mem[usize::from(AT)..usize::from(AT) + bytes.len()].copy_from_slice(bytes);
        prepare(&mut cpu, setup);
        let t = cpu.step_timed(&mut bus);
        bus.start += u64::from(t);
        lengths.push(t);
    }
    let tail = &lengths[4..];
    f64::from(tail.iter().sum::<u32>()) / tail.len() as f64 / 4.0
}

#[test]
fn instruction_lengths_match_the_cpc_table() {
    use Setup::{LastByte as L, NotTaken as N, Taken as T};
    // (bytes, setup, µs, name) — the cpcwiki "Z80" CPC timing table.
    let table: &[(&[u8], Setup, f64, &str)] = &[
        (&[0x00], T, 1.0, "nop"),
        (&[0x41], T, 1.0, "ld b,c"),
        (&[0x70], T, 2.0, "ld (hl),b"),
        (&[0x46], T, 2.0, "ld b,(hl)"),
        (&[0xdd, 0x44], T, 2.0, "ld b,ixh"),
        (&[0xdd, 0x70, 0x01], T, 5.0, "ld (ix+d),b"),
        (&[0xdd, 0x46, 0x01], T, 5.0, "ld b,(ix+d)"),
        (&[0x06, 0x12], T, 2.0, "ld b,n"),
        (&[0x36, 0x12], T, 3.0, "ld (hl),n"),
        (&[0xdd, 0x36, 0x01, 0x12], T, 6.0, "ld (ix+d),n"),
        (&[0x02], T, 2.0, "ld (bc),a"),
        (&[0x0a], T, 2.0, "ld a,(bc)"),
        (&[0x32, 0x00, 0x40], T, 4.0, "ld (nn),a"),
        (&[0x3a, 0x00, 0x40], T, 4.0, "ld a,(nn)"),
        (&[0x01, 0x34, 0x12], T, 3.0, "ld bc,nn"),
        (&[0xdd, 0x21, 0x34, 0x12], T, 4.0, "ld ix,nn"),
        (&[0x22, 0x00, 0x40], T, 5.0, "ld (nn),hl"),
        (&[0xdd, 0x22, 0x00, 0x40], T, 6.0, "ld (nn),ix"),
        (&[0xed, 0x43, 0x00, 0x40], T, 6.0, "ld (nn),bc"),
        (&[0x2a, 0x00, 0x40], T, 5.0, "ld hl,(nn)"),
        (&[0xdd, 0x2a, 0x00, 0x40], T, 6.0, "ld ix,(nn)"),
        (&[0xed, 0x4b, 0x00, 0x40], T, 6.0, "ld bc,(nn)"),
        (&[0xf9], T, 2.0, "ld sp,hl"),
        (&[0xdd, 0xf9], T, 3.0, "ld sp,ix"),
        (&[0xc1], T, 3.0, "pop bc"),
        (&[0xdd, 0xe1], T, 4.0, "pop ix"),
        (&[0xc5], T, 4.0, "push bc"),
        (&[0xdd, 0xe5], T, 5.0, "push ix"),
        (&[0x03], T, 2.0, "inc bc"),
        (&[0xdd, 0x23], T, 3.0, "inc ix"),
        (&[0x09], T, 3.0, "add hl,bc"),
        (&[0xdd, 0x09], T, 4.0, "add ix,bc"),
        (&[0xed, 0x4a], T, 4.0, "adc hl,bc"),
        (&[0x04], T, 1.0, "inc b"),
        (&[0xc6, 0x01], T, 2.0, "add a,n"),
        (&[0x86], T, 2.0, "add a,(hl)"),
        (&[0x34], T, 3.0, "inc (hl)"),
        (&[0xdd, 0x86, 0x01], T, 5.0, "add a,(ix+d)"),
        (&[0xdd, 0x34, 0x01], T, 6.0, "inc (ix+d)"),
        (&[0xed, 0x6f], T, 5.0, "rld"),
        (&[0xcb, 0x40], T, 2.0, "bit 0,b"),
        (&[0xcb, 0x46], T, 3.0, "bit 0,(hl)"),
        (&[0xdd, 0xcb, 0x01, 0x46], T, 6.0, "bit 0,(ix+d)"),
        (&[0xcb, 0xc6], T, 4.0, "set 0,(hl)"),
        (&[0xdd, 0xcb, 0x01, 0xc6], T, 7.0, "set 0,(ix+d)"),
        (&[0xcb, 0x06], T, 4.0, "rlc (hl)"),
        (&[0xed, 0x44], T, 2.0, "neg"),
        (&[0xed, 0xa0], T, 5.0, "ldi"),
        (&[0xed, 0xb0], T, 6.0, "ldir repeating"),
        (&[0xed, 0xb0], L, 5.0, "ldir last"),
        (&[0xed, 0xa1], T, 4.0, "cpi"),
        (&[0xed, 0xb1], T, 6.0, "cpir repeating"),
        (&[0xed, 0xa2], T, 5.0, "ini"),
        (&[0xed, 0xb2], T, 6.0, "inir repeating"),
        (&[0xed, 0xb2], N, 5.0, "inir last"),
        (&[0xed, 0xa3], T, 5.0, "outi"),
        (&[0xed, 0xb3], T, 6.0, "otir repeating"),
        (&[0xed, 0xb3], N, 5.0, "otir last"),
        (&[0xdb, 0x12], T, 3.0, "in a,(n)"),
        (&[0xd3, 0x12], T, 3.0, "out (n),a"),
        (&[0xed, 0x78], T, 4.0, "in a,(c)"),
        (&[0xed, 0x79], T, 4.0, "out (c),a"),
        (&[0xed, 0x71], T, 4.0, "out (c),0"),
        (&[0xc7], T, 4.0, "rst 0"),
        (&[0xc9], T, 3.0, "ret"),
        (&[0xc0], T, 4.0, "ret nz taken"),
        (&[0xc0], N, 2.0, "ret nz not taken"),
        (&[0xed, 0x4d], T, 4.0, "reti"),
        (&[0xcd, 0x00, 0x10], T, 5.0, "call nn"),
        (&[0xc4, 0x00, 0x10], T, 5.0, "call nz taken"),
        (&[0xc4, 0x00, 0x10], N, 3.0, "call nz not taken"),
        (&[0xe9], T, 1.0, "jp (hl)"),
        (&[0xdd, 0xe9], T, 2.0, "jp (ix)"),
        (&[0x18, 0xfe], T, 3.0, "jr e"),
        (&[0x20, 0xfe], T, 3.0, "jr nz taken"),
        (&[0x20, 0xfe], N, 2.0, "jr nz not taken"),
        (&[0x10, 0xfe], T, 4.0, "djnz taken"),
        (&[0x10, 0xfe], N, 3.0, "djnz not taken"),
        (&[0xc3, 0x00, 0x10], T, 3.0, "jp nn"),
        (&[0xed, 0x47], T, 3.0, "ld i,a"),
        (&[0xed, 0x57], T, 3.0, "ld a,i"),
        (&[0xed, 0x46], T, 2.0, "im 0"),
        (&[0xd9], T, 1.0, "exx"),
        (&[0x76], T, 1.0, "halt"),
        (&[0xe3], T, 6.0, "ex (sp),hl"),
        (&[0xdd, 0xe3], T, 7.0, "ex (sp),ix"),
    ];
    let mut wrong = Vec::new();
    for &(bytes, setup, want, name) in table {
        let got = nops(bytes, setup);
        if got != want {
            wrong.push(format!("{name}: {got} µs, expected {want}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
