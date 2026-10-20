use super::*;
use media::crt::{hw, Chip, CrtMachine};

/// A CRT with `banks` 8 KB banks at `load`, each filled with its number.
fn crt(hardware: u16, banks: u16, load: u16, exrom: bool, game: bool) -> Crt {
    Crt {
        machine: CrtMachine::C64,
        version: 0x0100,
        hardware,
        exrom,
        game,
        subtype: 0,
        name: "T".into(),
        chips: (0..banks)
            .map(|b| Chip {
                kind: 0,
                bank: b,
                load,
                data: vec![b as u8; 0x2000],
            })
            .collect(),
    }
}

#[test]
fn normal_8k_16k_and_ultimax() {
    let c = Banked::from_crt(&crt(hw::NORMAL, 1, 0x8000, false, true), Scheme::Normal).unwrap();
    assert_eq!(
        c.lines(),
        CartLines {
            exrom: true,
            ..CartLines::default()
        }
    );
    assert_eq!(c.peek_roml(0x8123), Some(0));
    assert_eq!(c.peek_romh(0xa000), None);
    let mut big = crt(hw::NORMAL, 1, 0x8000, false, false);
    big.chips[0].data = (0..0x4000u32).map(|i| (i >> 13) as u8 + 5).collect();
    let c = Banked::from_crt(&big, Scheme::Normal).unwrap();
    assert_eq!(
        (c.peek_roml(0x8000), c.peek_romh(0xbfff)),
        (Some(5), Some(6))
    );
    let ultimax = crt(hw::NORMAL, 1, 0xe000, true, false);
    let c = Banked::from_crt(&ultimax, Scheme::Normal).unwrap();
    assert!(c.lines().game && !c.lines().exrom);
    assert_eq!(c.peek_romh(0xfffc), Some(0));
}

#[test]
fn bank_registers() {
    let mut c = Banked::from_crt(&crt(hw::OCEAN, 16, 0x8000, false, true), Scheme::Ocean).unwrap();
    c.write_io(0xde00, 0x45);
    assert_eq!(c.peek_roml(0x8000), Some(5));
    let mut c =
        Banked::from_crt(&crt(hw::SYSTEM_3, 64, 0x8000, false, true), Scheme::System3).unwrap();
    c.write_io(0xde21, 0);
    assert_eq!(c.peek_roml(0x8000), Some(0x21));
    let mut c =
        Banked::from_crt(&crt(hw::DINAMIC, 16, 0x8000, false, true), Scheme::Dinamic).unwrap();
    assert_eq!(c.read_io(0xde0a), None);
    assert_eq!(c.peek_roml(0x8000), Some(10));
    let mut c = Banked::from_crt(
        &crt(hw::MAGIC_DESK, 32, 0x8000, false, true),
        Scheme::MagicDesk,
    )
    .unwrap();
    c.write_io(0xde00, 17);
    assert_eq!(c.peek_roml(0x8000), Some(17));
    c.write_io(0xde00, 0x80);
    assert!(!c.lines().exrom, "switched off");
    let mut c =
        Banked::from_crt(&crt(hw::FUN_PLAY, 16, 0x8000, false, true), Scheme::FunPlay).unwrap();
    c.write_io(0xde00, 0x09); // bits 3–5 = 1, bit 0 = 1: bank 9
    assert_eq!(c.peek_roml(0x8000), Some(9));
    c.write_io(0xde00, 0x86);
    assert!(!c.lines().exrom);
    let mut sg = crt(hw::SUPER_GAMES, 4, 0x8000, false, false);
    for (i, ch) in sg.chips.iter_mut().enumerate() {
        ch.data = vec![i as u8; 0x4000];
    }
    let mut c = Banked::from_crt(&sg, Scheme::SuperGames).unwrap();
    c.write_io(0xdf00, 0x0a); // bank 2, locked
    assert_eq!(c.peek_romh(0xa000), Some(2));
    c.write_io(0xdf00, 0x01);
    assert_eq!(c.peek_roml(0x8000), Some(2), "locked");
    c.reset();
    c.write_io(0xdf00, 0x04);
    assert!(!c.lines().game && !c.lines().exrom, "off");
}

#[test]
fn savestate_round_trip() {
    let mut c = Banked::from_crt(&crt(hw::OCEAN, 8, 0x8000, false, true), Scheme::Ocean).unwrap();
    c.write_io(0xde00, 3);
    let mut w = StateWriter::new();
    c.save(&mut w);
    let data = w.into_bytes();
    let mut d = Banked::from_crt(&crt(hw::OCEAN, 8, 0x8000, false, true), Scheme::Ocean).unwrap();
    d.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(d, c);
    let mut small =
        Banked::from_crt(&crt(hw::OCEAN, 2, 0x8000, false, true), Scheme::Ocean).unwrap();
    assert!(
        small.load(&mut StateReader::new(&data)).is_err(),
        "bank 3 of 2"
    );
}
