//! The REU and the UCI's machine-side commands in a running C64 (a JMP
//! loop in a test KERNAL; commands go through the debugger's bus pokes
//! and the interface's queues).

use super::*;
use crate::memory::KERNAL_SIZE;
use devices_net::testing::FakeNet;
use emu_core::host::MemFileHost;

/// A KERNAL that loops at $E000.
fn config(options: &[(&str, &str)]) -> MachineConfig {
    let mut kernal = vec![0xea; KERNAL_SIZE];
    kernal[..3].copy_from_slice(&[0x4c, 0x00, 0xe0]);
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe0, 0x00, 0xe0, 0x00, 0xe0]);
    let mut c = MachineConfig::default();
    c.roms.insert("kernal".into(), kernal);
    c.roms.insert("basic".into(), vec![0xb0; BASIC_SIZE]);
    c.roms.insert("chargen".into(), vec![0xc0; CHARGEN_SIZE]);
    for (k, v) in options {
        c.options.insert((*k).into(), (*v).into());
    }
    c
}

fn poke(m: &mut C64, a: u16, v: u8) {
    assert!(m.poke(CPU_SPACE, u32::from(a), Size::Byte, u32::from(v)));
}

fn step(m: &mut C64, n: u32) {
    for _ in 0..n {
        m.step_cycle();
    }
}

/// Send a UCI command; all packets and the last status.
fn uci(m: &mut C64, bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    for &b in bytes {
        poke(m, 0xdf1d, b);
    }
    poke(m, 0xdf1c, 1);
    let mut data = Vec::new();
    let mut status = Vec::new();
    loop {
        for _ in 0..1_000_000 {
            if m.uci
                .as_ref()
                .is_none_or(|u| u.state() != devices_net::uci::State::Busy)
            {
                break;
            }
            m.step_cycle();
        }
        let Some(u) = m.uci.as_mut() else {
            return (data, status);
        };
        let more = u.state() == devices_net::uci::State::DataMore;
        let mut stat = Vec::new();
        while u.status() & 0x80 != 0 {
            data.push(u.read(0xdf1e));
        }
        while u.status() & 0x40 != 0 {
            stat.push(u.read(0xdf1f));
        }
        if !stat.is_empty() {
            status = stat;
        }
        poke(m, 0xdf1c, 2);
        if !more {
            return (data, status);
        }
    }
}

fn with_name(head: &[u8], name: &str) -> Vec<u8> {
    let mut v = head.to_vec();
    v.extend(name.bytes());
    v
}

#[test]
fn reu_moves_blocks_while_the_cpu_waits() {
    let mut m = C64::new(&config(&[("reu", "512k")])).unwrap();
    step(&mut m, 1000);
    for (i, b) in m.mem.ram[0x0400..0x0500].iter_mut().enumerate() {
        *b = i as u8;
    }
    assert_eq!(m.peek(CPU_SPACE, 0xdf00, Size::Byte), Some(0x10));
    // STASH $0400 → REU $070000, 256 bytes, interrupt at the end.
    for (r, v) in [
        (2, 0x00),
        (3, 0x04),
        (4, 0),
        (5, 0),
        (6, 7),
        (7, 0),
        (8, 1),
        (9, 0xc0),
    ] {
        poke(&mut m, 0xdf00 + r, v);
    }
    poke(&mut m, 0xdf01, 0x90);
    let pc = m.cpu.pc();
    let mut cycles = 0;
    while m.reu.as_ref().unwrap().dma_active() {
        m.step_cycle();
        cycles += 1;
        assert_eq!(m.cpu.pc(), pc, "the CPU is stopped");
        assert!(cycles < 2000);
    }
    // One byte a cycle, plus the cycles the VIC-II held BA.
    assert!(cycles >= 256, "{cycles}");
    assert_eq!(
        m.reu.as_ref().unwrap().ram()[0x70000..0x70100],
        m.mem.ram[0x0400..0x0500]
    );
    step(&mut m, 2);
    assert_ne!(m.ints.irq & irq::REU, 0);
    // The CPU's status read releases /IRQ.
    let mut bus = C64Bus {
        mem: &mut m.mem,
        io: &mut m.io,
        cart: None,
        uci: None,
        reu: m.reu.as_mut(),
    };
    assert_eq!(bus.read(0xdf00, Size::Byte) & 0xc0, 0xc0);
    step(&mut m, 2);
    assert_eq!(m.ints.irq & irq::REU, 0);
    // FETCH back to $C000, started by a write to $FF00.
    // Without autoload the registers point past the block: set them anew.
    assert_eq!(m.peek(CPU_SPACE, 0xdf07, Size::Byte), Some(1));
    for (r, v) in [(2, 0x00), (3, 0xc0), (4, 0), (5, 0), (6, 7), (7, 0), (8, 1)] {
        poke(&mut m, 0xdf00 + r, v);
    }
    poke(&mut m, 0xdf01, 0x81);
    step(&mut m, 10);
    assert!(!m.reu.as_ref().unwrap().dma_active());
    // A CPU write (the KERNAL ROM is banked in: it lands in RAM).
    C64Bus {
        mem: &mut m.mem,
        io: &mut m.io,
        cart: None,
        uci: None,
        reu: m.reu.as_mut(),
    }
    .write(0xff00, Size::Byte, 0);
    assert!(m.reu.as_ref().unwrap().dma_active());
    step(&mut m, 2000);
    assert_eq!(m.mem.ram[0xc000..0xc100], m.mem.ram[0x0400..0x0500]);
    // The REU's memory in the debugger and the chip list.
    assert_eq!(m.peek(REU_SPACE, 0x70005, Size::Byte), Some(5));
    assert!(m.poke(REU_SPACE, 0x70005, Size::Byte, 0x55));
    assert!(m.memory_spaces().iter().any(|s| s.name == "reu"));
    assert!(m.chips().iter().any(|c| c.name == "reu"));
    // Savestate with the REU; another size refuses it.
    let state = m.save_state();
    m.reu.as_mut().unwrap().ram_mut()[0] = 0x99;
    m.load_state(&state).unwrap();
    assert_eq!(m.reu.as_ref().unwrap().ram()[0x70005], 0x55);
    assert_ne!(m.reu.as_ref().unwrap().ram()[0], 0x99);
    let mut other = C64::new(&config(&[("reu", "1764")])).unwrap();
    assert!(other.load_state(&state).is_err());
    let mut none = C64::new(&config(&[])).unwrap();
    assert!(none.load_state(&state).is_err());
    assert!(C64::new(&config(&[("reu", "3m")])).is_err());
}

#[test]
fn uci_commands_reach_memory_drive_reu_and_palette() {
    let mut m = C64::new(&config(&[
        ("uci", "on"),
        ("reu", "128k"),
        ("drive8", "virtual"),
    ]))
    .unwrap();
    let mut fs = MemFileHost::new();
    fs.insert("/hello.prg", vec![0x00, 0xc0, 1, 2, 3]).unwrap();
    fs.insert("/data.reu", vec![0xab; 300]).unwrap();
    let disk = media::d64::D64::blank(b"TEST", *b"01").to_bytes();
    fs.insert("/disk-1.d64", disk.clone()).unwrap();
    fs.insert("/disk-2.d64", disk).unwrap();
    m.attach_host(HostServices::new(
        Some(Box::new(FakeNet::new())),
        Some(Box::new(fs)),
    ));
    step(&mut m, 1000);
    // Software IEC: LOAD"HELLO",8,1 by DMA.
    let (data, _) = uci(&mut m, &with_name(&[5, 0x10, 1, 0, 0, 0], "HELLO"));
    assert_eq!(data, [0x00, 0xc0]);
    let (data, status) = uci(&mut m, &[5, 0x11, 1, 0]);
    assert_eq!((data, status), (vec![0, 0x03, 0xc0], vec![0]));
    assert_eq!(m.mem.ram[0xc000..0xc003], [1, 2, 3]);
    // SAVE back out of memory.
    let (_, status) = uci(
        &mut m,
        &with_name(&[5, 0x12, 0, 1, 0x00, 0xc0, 0x02, 0xc0], "COPY"),
    );
    assert_eq!(status, [0]);
    // A REU image through the control target.
    let (data, status) = uci(&mut m, &with_name(&[4, 0x08], "data.reu"));
    assert_eq!(
        (data, status),
        (300i32.to_le_bytes().to_vec(), b"00,OK".to_vec())
    );
    assert_eq!(m.reu.as_ref().unwrap().ram()[299], 0xab);
    // Drive 8: a virtual drive reports as Software IEC; power off and on.
    assert_eq!(uci(&mut m, &[4, 0x29, 0]).0, [1, 0x0f, 8, 1]);
    uci(&mut m, &[4, 0x31]);
    assert!(!m.drive_power);
    assert_eq!(uci(&mut m, &[4, 0x34]).0, b"off");
    uci(&mut m, &[4, 0x30]);
    assert!(m.drive_power);
    // Mount a disk from the folder, swap to the set's next one.
    let (_, status) = uci(&mut m, &with_name(&[1, 0x23, 8], "disk-1.d64"));
    assert_eq!(status, b"00,OK");
    assert!(m
        .media_slots()
        .iter()
        .any(|s| s.id == SLOT_DRIVE8 && s.inserted));
    assert_eq!(uci(&mut m, &[1, 0x25, 8]).1, b"00,OK");
    assert_eq!(
        uci(&mut m, &with_name(&[1, 0x23, 8], "hello.prg")).1,
        b"89,NOT A DISK IMAGE"
    );
    assert_eq!(uci(&mut m, &[1, 0x24, 8]).1, b"00,OK");
    assert!(m
        .media_slots()
        .iter()
        .any(|s| s.id == SLOT_DRIVE8 && !s.inserted));
    // The palette: set colour 1, the frame uses it, the savestate keeps it.
    uci(&mut m, &[4, 0x53, 1, 0x12, 0x34, 0x56]);
    let PixelData::Indexed { palette, .. } = m.framebuffer().pixels else {
        panic!("indexed frame");
    };
    assert_eq!(
        palette[1],
        Rgb8 {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
    let state = m.save_state();
    uci(&mut m, &[4, 0x54]);
    assert_ne!(m.palette[1].r, 0x12);
    uci(&mut m, &[4, 0x31]);
    m.load_state(&state).unwrap();
    assert_eq!(m.palette[1].r, 0x12);
    assert!(m.drive_power);
    // SAVEMEM writes the RAM; REBOOT resets the C64 and the interface.
    assert_eq!(uci(&mut m, &with_name(&[4, 0x0f], "mem.bin")).1, b"00,OK");
    m.mem.ram[0x1234] = 0x77;
    m.cpu.set_reg("pc", 0x1000);
    for &b in &[4, 0x06] {
        poke(&mut m, 0xdf1d, b);
    }
    poke(&mut m, 0xdf1c, 1);
    step(&mut m, 200);
    assert_eq!(
        m.uci.as_ref().unwrap().state(),
        devices_net::uci::State::Idle
    );
    assert_eq!(m.mem.ram[0x1234], 0x77, "RAM survives the reboot");
    assert!((0xe000..0xe003).contains(&m.cpu.pc()), "{:04x}", m.cpu.pc());
}
