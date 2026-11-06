//! The Phonograph on the machine (SPEC.md 9.1): an Ada program copies an
//! MP3 file into a ring in chip RAM and starts the chip; its sound comes
//! out of Calliope's mix, its bytes cost fixed-DMA slots, the half-empty
//! interrupt reaches the CPU, and a savestate taken while it plays
//! resumes.

use super::*;
use chips_logophor::phonograph::{
    PHCHAN, PHFRMH, PHFRML, PHKBPS, PHLEVL, PHLEVR, PHRATE, PHRD, PHSTAT, PHWR,
};

/// 44.1 kHz stereo, 128 kbit/s, 2 s: 440 Hz left, 660 Hz right.
const MP3: &[u8] = include_bytes!("../../../tests/data/logophor16/tone.mp3");
/// Where the test puts the file (fast RAM) and the vector table.
const FILE_SEG: u32 = 0x0050;
const VBR_SEG: u32 = 0x0060;

fn machine() -> Logophor {
    Logophor::new(&MachineConfig::default()).unwrap()
}

fn frames(m: &mut Logophor, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
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

fn poke(m: &mut Logophor, addr: u32, size: Size, v: u32) {
    assert!(DebugTarget::poke(m, CPU_SPACE, addr, size, v));
}

/// The program: copy the file (fast RAM 0050:0000) into a 32 KiB ring at
/// 0001:0000, start the Phonograph with the half-empty interrupt, and
/// idle. Its interrupt handler (at offset 2, vector 14 of a table at
/// 0060:0040) reads PHSTAT into r8, counts in r9 and clears INTREQ.
fn program(dmacon: u16) -> String {
    let words = MP3.len() / 2;
    let len = MP3.len();
    format!(
        "
        bra main
    handler:
        lwx r8, (r6)          ; PHSTAT (clears bits 2 and 3)
        addi r9, #1
        li r2, #$4280
        li r1, #$1000
        swx r1, (r2)          ; INTREQ: clear the Phonograph
        rti
    main:
        li r1, #{VBR_SEG}
        mtsc #6, r1           ; VBR
        li r1, #{FILE_SEG}
        mts s1, r1
        movi r1, #1
        mts s3, r1
        movi r2, #0
        movi r3, #0
        li r4, #{words}
    copy:
        lw r1, 0(r2)
        swx r1, (r3)+
        addi r2, #2
        addi r4, #-1
        bne copy
        li r1, #$ffff
        mts s3, r1
        li r6, #$320e         ; PHSTAT, for the handler
        li r2, #$4282
        li r1, #$9000
        swx r1, (r2)          ; INTENA: Phonograph
        li r2, #$0002
        li r1, #{dmacon}
        swx r1, (r2)          ; DMACON
        li r2, #$3100
        movi r1, #127
        swx r1, (r2)          ; AUDMASTER
        li r2, #$3204
        movi r1, #1
        swx r1, (r2)          ; PHSEG
        li r2, #$3208
        li r1, #$8000
        swx r1, (r2)          ; PHSIZE: 32 KiB
        li r2, #$320a
        li r1, #{len}
        swx r1, (r2)          ; PHWR: the whole file
        li r2, #$3216
        movi r1, #127
        swx r1, (r2)          ; PHVOL
        li r2, #$3202
        movi r1, #5
        swx r1, (r2)          ; PHCON: on, MP3, interrupt
        ei
    idle:
        bra idle
        "
    )
}

/// A machine running the program with the given DMACON value.
fn playing(dmacon: u16) -> Logophor {
    let mut m = machine();
    for (i, b) in MP3.iter().enumerate() {
        poke(
            &mut m,
            (FILE_SEG << 16) + i as u32,
            Size::Byte,
            u32::from(*b),
        );
    }
    // Vector 14 (level 6): the program's segment, offset 2.
    let v = (VBR_SEG << 16) + 0x40 + 4 * 14;
    poke(&mut m, v, Size::Word, PROGRAM_LOAD >> 16);
    poke(&mut m, v + 2, Size::Word, 2);
    insert_program(&mut m, assemble(&program(dmacon)));
    m
}

fn reg(m: &Logophor, off: u16) -> u16 {
    m.board.phonograph.peek(off)
}

/// Frequency of one side from its rising zero crossings.
fn freq(out: &[i16], side: usize) -> f64 {
    let x: Vec<i16> = out.iter().skip(side).step_by(2).copied().collect();
    let ups: Vec<usize> = (1..x.len())
        .filter(|&i| x[i - 1] < 0 && x[i] >= 0)
        .collect();
    let (first, last) = (ups[0], *ups.last().unwrap());
    (ups.len() - 1) as f64 * 48_000.0 / (last - first) as f64
}

/// `n` fields step by step: the fixed-DMA slots of every line (read just
/// after slot 640 has set them up).
fn field_slots(m: &mut Logophor, n: u64) -> u32 {
    let mut slots = 0;
    let end = m.fields + n;
    while m.fields < end {
        m.step_cycle();
        if m.cycle == FIXED_START * 3 / 8 + 1 {
            slots += u32::from(m.fixed);
        }
    }
    slots
}

#[test]
fn the_phonograph_plays_an_mp3_from_a_ring_in_chip_ram() {
    let mut m = playing(0xc080);
    frames(&mut m, 40);
    let mut out = Vec::new();
    m.audio(&mut out);
    // Both sides sound with the file's tones (the last half second).
    let tail = &out[out.len() - 48_000..];
    assert!(
        (freq(tail, 0) - 440.0).abs() < 1.0,
        "left {}",
        freq(tail, 0)
    );
    assert!(
        (freq(tail, 1) - 660.0).abs() < 1.0,
        "right {}",
        freq(tail, 1)
    );
    let peak = tail.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak > 3000, "audible: {peak}");
    assert_eq!(reg(&m, PHRATE), 4410);
    assert_eq!(reg(&m, PHCHAN), 2);
    assert_eq!(reg(&m, PHKBPS), 128);
    assert_eq!(reg(&m, PHSTAT) & 3, 1, "decoding, not starved");
    assert!(reg(&m, PHLEVL) > 3000 && reg(&m, PHLEVR) > 3000);
    // 40 fields of 1/60.09 s, less the copy: about 25 frames of 26.1 ms
    // decoded, one or two ahead.
    let frames_done = u32::from(reg(&m, PHFRMH)) << 16 | u32::from(reg(&m, PHFRML));
    assert!((20..=28).contains(&frames_done), "{frames_done} frames");
    // The bytes taken cost fixed-DMA slots: compare a field with a
    // machine whose Phonograph DMA is off (it takes nothing).
    let mut off = playing(0xc000);
    frames(&mut off, 40);
    assert_eq!(reg(&off, PHRD), 0);
    assert_eq!(reg(&off, PHSTAT), 0, "not running");
    let mut silent = Vec::new();
    off.audio(&mut silent);
    assert!(silent.iter().all(|s| *s == 0));
    let rd = reg(&m, PHRD);
    let (with, without) = (field_slots(&mut m, 6), field_slots(&mut off, 6));
    let taken = u32::from(reg(&m, PHRD).wrapping_sub(rd));
    let extra = with - without;
    // 128 kbit/s: about 266 bytes a field; a slot for 4 bytes of a frame,
    // charged on the lines after it was taken (so up to a frame's slots
    // early or late).
    assert!((1000..=2200).contains(&taken), "{taken} bytes");
    assert!(
        extra * 4 + 4 * 110 >= taken && extra * 4 <= taken + 4 * 110,
        "{extra} slots for {taken} bytes"
    );
    // The ring empties to half: the interrupt (level 6, vector 14), once.
    frames(&mut m, 44);
    assert_eq!(m.cpu.r[9], 1, "one interrupt");
    assert_eq!(m.cpu.cause, 14);
    assert_eq!(m.cpu.r[8] & 8, 8, "PHSTAT bit 3 seen by the handler");
    assert_eq!(reg(&m, PHSTAT) & 8, 0, "cleared by the read");
    assert_eq!(
        m.board.ints.peek(telegraph::INTREQ) & telegraph::INT_PHONO,
        0
    );
    // The file ends: the ring runs dry, starved, silence.
    frames(&mut m, 40);
    assert_eq!(reg(&m, PHRD), reg(&m, PHWR));
    assert_eq!(reg(&m, PHSTAT) & 3, 3, "running, starved");
    let mut out = Vec::new();
    m.audio(&mut out);
    assert!(out[out.len() - 2000..].iter().all(|s| *s == 0));
    // The debugger lists the chip's registers.
    let chips = m.chips();
    let ph = chips.iter().find(|c| c.name == "phonograph").unwrap();
    let wr = ph.regs.iter().find(|r| r.name == "PHWR").unwrap();
    assert_eq!(wr.value, MP3.len() as u32);
}

#[test]
fn a_savestate_taken_while_the_phonograph_plays_resumes() {
    let mut a = playing(0xc080);
    frames(&mut a, 30);
    for _ in 0..5000 {
        a.step_cycle();
    }
    a.audio(&mut Vec::new());
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    frames(&mut a, 30);
    frames(&mut b, 30);
    assert_eq!(a.now(), b.now());
    assert_eq!(a.framebuffer().pixels, b.framebuffer().pixels);
    assert_eq!(reg(&a, PHRD), reg(&b, PHRD));
    assert_eq!(a.cpu.r[9], b.cpu.r[9]);
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    a.audio(&mut sa);
    b.audio(&mut sb);
    assert_eq!(sa.len(), sb.len());
    // The loaded machine plays the samples that were waiting as silence
    // and decodes afresh from the next frame (an MP3 frame may lean on the
    // one before): the same sound from then on.
    assert_ne!(sa, sb);
    assert_eq!(sa[16_000..], sb[16_000..]);
    // Everything else is identical (the peak registers follow the sound).
    for m in [&mut a, &mut b] {
        m.board.phonograph.read(PHLEVL);
        m.board.phonograph.read(PHLEVR);
    }
    assert_eq!(a.save_state(), b.save_state());
    // A state without the Phonograph's chunk (older) loads with it off;
    // a damaged chunk is refused and changes nothing.
    let before = b.save_state();
    let cut = state.len() - 10;
    assert!(b.load_state(&state[..cut]).is_err());
    assert_eq!(b.save_state(), before);
    let tag = state.windows(4).rposition(|w| w == b"PHON").unwrap();
    b.load_state(&state[..tag]).unwrap();
    assert_eq!(reg(&b, PHSTAT), 0, "chip off");
    assert_eq!(reg(&b, PHWR), 0);
}
