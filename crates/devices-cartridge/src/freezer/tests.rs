use media::crt::{Chip, Crt, CrtMachine};

use super::*;

fn cart(kind: Kind) -> Freezer {
    let chips = (0..4)
        .map(|bank| Chip {
            kind: 0,
            bank,
            load: 0x8000,
            data: vec![bank as u8 + 1; 0x2000],
        })
        .collect();
    let crt = Crt {
        machine: CrtMachine::C64,
        version: 0x100,
        hardware: 36,
        exrom: false,
        game: true,
        subtype: 0,
        name: String::new(),
        chips,
    };
    Freezer::from_crt(&crt, kind).unwrap()
}

#[test]
fn register_selects_mode_bank_and_ram() {
    let mut c = cart(Kind::ActionReplay);
    assert_eq!((c.lines().game, c.lines().exrom), (false, true));
    c.write_io(0xde00, 0x08);
    assert_eq!(c.peek_roml(0x8000), Some(2));
    c.write_io(0xde00, 0x23);
    assert_eq!((c.lines().game, c.lines().exrom), (true, false));
    c.write_roml(0x9f05, 0x55);
    assert_eq!(c.peek_io(0xdf05), Some(0x55));
    c.write_io(0xde00, 0x04);
    assert_eq!((c.lines().game, c.lines().exrom), (false, false));
    c.write_io(0xde00, 0x23);
    assert!(!c.lines().game, "locked until reset");
    c.reset();
    assert!(c.lines().exrom);
}

#[test]
fn extended_register_is_written_once() {
    let mut c = cart(Kind::RetroReplay);
    c.write_io(0xde01, REU_MAP);
    c.write_io(0xde01, ALLOW_BANK);
    assert_eq!(c.peek_io(0xde01).unwrap() & (REU_MAP | ALLOW_BANK), REU_MAP);
}

#[test]
fn nordic_mode_keeps_writes_at_a000_from_c64_ram() {
    let mut c = cart(Kind::NordicPower);
    c.write_io(0xde00, 0x22);
    assert!(c.blocks_ram_write(0xa000));
    assert!(!c.blocks_ram_write(0x8000));
    c.write_romh(0xa010, 7);
    assert_eq!(c.peek_romh(0xa010), Some(7));
    assert_eq!(c.peek_roml(0x8000), Some(1));
}

#[test]
fn button_freezes_until_bit_6() {
    let mut c = cart(Kind::RetroReplay);
    c.write_io(0xde00, 0x18);
    assert!(c.press_button());
    assert!(c.lines().nmi);
    assert_eq!((c.lines().game, c.lines().exrom), (true, false));
    assert_eq!(c.peek_romh(0xe000), Some(1));
    c.write_io(0xde00, 0x40);
    assert!(!c.lines().nmi);
}

#[test]
fn no_freeze_ignores_the_button() {
    let mut c = cart(Kind::RetroReplay);
    c.write_io(0xde01, NO_FREEZE);
    c.press_button();
    assert!(!c.lines().nmi);
}

#[test]
fn savestate_round_trip() {
    let mut c = cart(Kind::RetroReplay);
    c.write_io(0xde00, 0x2b);
    c.write_roml(0x8123, 9);
    let mut w = StateWriter::new();
    c.save(&mut w);
    let bytes = w.into_bytes();
    let mut other = cart(Kind::RetroReplay);
    other.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(other, c);
}
