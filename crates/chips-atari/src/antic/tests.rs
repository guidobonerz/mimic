use super::*;

struct Mem(Vec<u8>);

impl AnticMemory for Mem {
    fn dma_read(&self, addr: u16) -> u8 {
        self.0[usize::from(addr)]
    }
}

const DL: u16 = 0x1000;
const SCREEN: u16 = 0x2000;
const CHARSET: u16 = 0xe000;

/// 24 blank lines, a mode 2 line (with `dli`), mode F, JVB.
fn setup(mode2_ir: u8) -> (Antic, Mem) {
    let mut mem = Mem(vec![0; 0x10000]);
    let dl = [
        0x70,
        0x70,
        0x70,
        mode2_ir,
        SCREEN as u8,
        (SCREEN >> 8) as u8,
        0x0f,
        0x41,
        DL as u8,
        (DL >> 8) as u8,
    ];
    mem.0[usize::from(DL)..usize::from(DL) + dl.len()].copy_from_slice(&dl);
    // Character 1 is a solid block.
    for r in 0..8 {
        mem.0[usize::from(CHARSET) + 8 + r] = 0xff;
    }
    mem.0[usize::from(SCREEN)] = 0x01;
    let mut a = Antic::new(VideoStandard::Pal);
    a.write(0x02, DL as u8);
    a.write(0x03, (DL >> 8) as u8);
    a.write(0x09, (CHARSET >> 8) as u8);
    a.write(0x00, 0x22);
    (a, mem)
}

/// Run until the beam is at (line, cycle) of the next field; returns the
/// outputs of all cycles run.
fn run_to(a: &mut Antic, mem: &Mem, line: u16, cycle: u16) -> Vec<CycleOut> {
    let mut outs = Vec::new();
    // Finish the current field first.
    loop {
        outs.push(a.tick(mem));
        if a.end_cycle() {
            break;
        }
    }
    while (a.line(), a.cycle()) != (line, cycle) {
        outs.push(a.tick(mem));
        a.end_cycle();
    }
    outs
}

/// Halted cycles of one whole line starting at the beam position.
fn halts_of_line(a: &mut Antic, mem: &Mem) -> usize {
    (0..CYCLES_PER_LINE)
        .filter(|_| {
            let h = a.tick(mem).halt;
            a.end_cycle();
            h
        })
        .count()
}

#[test]
fn vbi_nmi_on_line_248() {
    let (mut a, mem) = setup(0x42);
    a.write(0x0e, 0x40);
    // NMIST at cycle 7, the NMI itself at cycle 8.
    run_to(&mut a, &mem, VBLANK_LINE, NMI_CYCLE);
    a.tick(&mem);
    a.end_cycle();
    let o = a.tick(&mem);
    assert!(o.nmi);
    assert_eq!(a.peek(0x0f) & 0xc0, 0x40, "NMIST VBI");
    a.write(0x0f, 0);
    assert_eq!(a.peek(0x0f) & 0xc0, 0x00, "NMIRES");
    assert_eq!(a.peek(0x0b), (VBLANK_LINE / 2) as u8, "VCOUNT");
}

#[test]
fn mode_2_line_dma_and_pixels() {
    let (mut a, mem) = setup(0x42);
    run_to(&mut a, &mem, 32, 0);
    // First line of the mode line: instruction, LMS (2), 40 names,
    // 40 character bytes; the playfield blocks all refresh requests but
    // one, which waits for a free cycle.
    assert_eq!(halts_of_line(&mut a, &mem), 1 + 2 + 40 + 40 + 1);
    // Second line: character data again, no names.
    assert_eq!(halts_of_line(&mut a, &mem), 40 + 9);
    // The solid block in the first cell, background after it.
    assert_eq!(a.playfield(48), Playfield::Hires(3));
    assert_eq!(a.playfield(51), Playfield::Hires(3));
    assert_eq!(a.playfield(52), Playfield::Hires(0));
    assert_eq!(
        a.playfield(47),
        Playfield::Background,
        "left of the playfield"
    );
}

#[test]
fn mode_f_follows_and_jvb_restarts() {
    let (mut a, mut mem) = setup(0x42);
    // The mode F line reads the 40 bytes after the mode 2 line.
    mem.0[usize::from(SCREEN) + 40] = 0x80;
    run_to(&mut a, &mem, 40, 0);
    for _ in 0..CYCLES_PER_LINE {
        a.tick(&mem);
        a.end_cycle();
    }
    // Still in line 40's playfield (cycles advanced into line 41): check
    // during the line instead.
    run_to(&mut a, &mem, 40, 100);
    assert_eq!(a.playfield(48), Playfield::Hires(2));
    // JVB: blank afterwards, and the next field starts from DL again.
    run_to(&mut a, &mem, 60, 100);
    assert_eq!(a.playfield(48), Playfield::Background);
    run_to(&mut a, &mem, 32, 100);
    assert_eq!(a.playfield(48), Playfield::Hires(3));
}

#[test]
fn dli_on_the_last_line_of_its_mode_line() {
    let (mut a, mem) = setup(0xc2);
    a.write(0x0e, 0x80);
    let all = run_to(&mut a, &mem, 40, 0);
    // The lines of this field from line 30 on.
    let outs = &all[all.len() - 10 * usize::from(CYCLES_PER_LINE)..];
    let nmis: Vec<usize> = outs
        .iter()
        .enumerate()
        .filter(|(_, o)| o.nmi)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(nmis.len(), 1, "one DLI");
    assert_eq!(a.peek(0x0f) & 0x80, 0x80);
    // The DLI was at cycle 7 of line 39 (rows 0–7 of lines 32–39).
    let last = outs.len() - 1;
    // NMIST at cycle 7, the NMI to the CPU at cycle 8.
    assert_eq!(last - nmis[0], usize::from(CYCLES_PER_LINE - 2 - NMI_CYCLE));
}

#[test]
fn wsync_releases_at_cycle_105() {
    let (mut a, mem) = setup(0x42);
    a.write(0x00, 0); // no DMA
    run_to(&mut a, &mem, 100, 20);
    a.write(0x0a, 0);
    a.end_cycle();
    let mut halted = Vec::new();
    for _ in 0..90 {
        let c = a.cycle();
        if a.tick(&mem).wsync {
            halted.push(c);
        }
        a.end_cycle();
    }
    // Written in cycle 20: the CPU still runs in cycle 21 (it fetches the
    // next opcode), RDY is low from 22 to 104.
    assert_eq!(halted.first(), Some(&22));
    assert_eq!(halted.last(), Some(&(WSYNC_RELEASE - 1)));
    assert_eq!(halted.len(), usize::from(WSYNC_RELEASE - 22));
}

#[test]
fn player_dma_addresses() {
    let (mut a, mut mem) = setup(0x42);
    a.write(0x07, 0x30); // PMBASE $3000
    a.write(0x00, 0x22 | 0x0c | 0x10); // + players, missiles, single line
    mem.0[0x3000 + 0x400 + 50] = 0xaa; // player 0, line 50
    mem.0[0x3000 + 0x300 + 50] = 0x55; // missiles
    run_to(&mut a, &mem, 50, 0);
    assert_eq!(a.tick(&mem).pm, Some((4, 0x55)));
    a.end_cycle();
    a.tick(&mem);
    a.end_cycle();
    assert_eq!(a.tick(&mem).pm, Some((0, 0xaa)));
}

#[test]
fn savestate_round_trip() {
    let (mut a, mem) = setup(0x42);
    run_to(&mut a, &mem, 35, 50);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let bytes = w.into_bytes();
    let mut b = Antic::new(VideoStandard::Pal);
    b.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(a, b);
    for _ in 0..1000 {
        assert_eq!(a.tick(&mem), b.tick(&mem));
        a.end_cycle();
        b.end_cycle();
    }
    assert!(b.load(&mut StateReader::new(&bytes[..10])).is_err());
}
