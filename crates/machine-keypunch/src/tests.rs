use super::*;
use emu_core::controls::ControlId;
use emu_core::input::MachineKey;
use emu_core::media::MediaBackend;

/// Bytes in memory that can grow, as a deck file behind the overlay.
#[derive(Default)]
struct Mem(Vec<u8>);

impl MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(self.0.get(o..o + buf.len()).ok_or(MediaError::OutOfRange)?);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        self.0
            .get_mut(o..o + data.len())
            .ok_or(MediaError::OutOfRange)?
            .copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        self.0.resize(len as usize, 0);
        Ok(())
    }
}

fn deck(name: &str, data: &[u8]) -> Media {
    Media {
        name: name.into(),
        kind: MediaKind::Cards,
        backend: Box::new(Mem(data.to_vec())),
    }
}

/// Eject a slot's deck and give its file's bytes.
fn file(m: &mut Keypunch, slot: SlotId) -> Vec<u8> {
    let mut media = m.media_slot(slot).unwrap().eject().unwrap();
    let mut buf = vec![0; media.backend.len() as usize];
    media.backend.read(0, &mut buf).unwrap();
    buf
}

fn punch() -> Keypunch {
    Keypunch::new(&MachineConfig::default()).unwrap()
}

fn key(m: &mut Keypunch, k: u16, down: bool) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(k),
            down,
        },
    });
    m.step_cycle();
}

fn press(m: &mut Keypunch, k: u16) {
    key(m, k, true);
    key(m, k, false);
}

/// Type text as on a US keyboard (Shift for upper-row symbols).
fn typ(m: &mut Keypunch, s: &str) {
    let unshifted = b"1234567890-=[]\\;',./`";
    let shifted = b"!@#$%^&*()_+{}|:\"<>?~";
    for ch in s.bytes() {
        if let Some(i) = shifted.iter().position(|&c| c == ch) {
            key(m, keys::SHIFT, true);
            press(m, u16::from(unshifted[i]));
            key(m, keys::SHIFT, false);
        } else {
            press(m, u16::from(ch.to_ascii_lowercase()));
        }
    }
}

fn control(m: &mut Keypunch, id: u16, value: u8) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Panel,
        event: InputEvent::Control {
            id: ControlId(id),
            value,
        },
    });
    m.step_cycle();
}

#[test]
fn typing_punches_prints_and_releases_into_the_stacker() {
    let mut m = punch();
    m.media_slot(SLOT_STACKER)
        .unwrap()
        .insert(deck("out.crd", b""))
        .unwrap();
    // The first key feeds a card.
    typ(&mut m, "      PRINT *, (1+2)");
    assert_eq!(m.column(), 21);
    press(&mut m, keys::BACKSPACE);
    assert_eq!(m.column(), 20, "backspace moves the card back");
    press(&mut m, keys::ENTER);
    assert_eq!(m.stacked().len(), 1);
    // AUTO FEED: the next card is there.
    assert_eq!(m.column(), 1);
    assert!(m.card().is_some());
    typ(&mut m, "      END");
    press(&mut m, keys::ENTER);
    let text = String::from_utf8(file(&mut m, SLOT_STACKER)).unwrap();
    assert_eq!(text, "      PRINT *, (1+2)\n      END\n");
}

#[test]
fn a_card_releases_itself_after_column_80() {
    let mut m = punch();
    typ(&mut m, &"X".repeat(81));
    assert_eq!(m.stacked().len(), 1);
    assert_eq!(m.stacked()[0].text().unwrap(), "X".repeat(80));
    assert_eq!(m.column(), 2, "the 81st X went onto the next card");
}

#[test]
fn without_auto_feed_feed_is_needed() {
    let mut m = punch();
    control(&mut m, ctl::AUTO_FEED, 0);
    typ(&mut m, "A");
    press(&mut m, keys::ENTER);
    assert!(m.card().is_none());
    control(&mut m, ctl::FEED, 1);
    control(&mut m, ctl::FEED, 0);
    assert!(m.card().is_some());
}

#[test]
fn fortran_program_tabs_and_sequence_numbers() {
    let mut m = Keypunch::new(&MachineConfig {
        options: [
            ("program".to_string(), "fortran".to_string()),
            ("seq".to_string(), "on".to_string()),
        ]
        .into_iter()
        .collect(),
        ..MachineConfig::default()
    })
    .unwrap();
    m.media_slot(SLOT_STACKER)
        .unwrap()
        .insert(deck("prog.chx", b""))
        .unwrap();
    typ(&mut m, "10");
    press(&mut m, keys::TAB);
    assert_eq!(m.column(), 6);
    press(&mut m, keys::TAB);
    typ(&mut m, "CONTINUE");
    press(&mut m, keys::TAB);
    assert_eq!(m.column(), 73);
    press(&mut m, keys::ENTER);
    // A blank card gets no number.
    press(&mut m, keys::ENTER);
    typ(&mut m, "      STOP");
    press(&mut m, keys::ENTER);
    let cards = Deck::parse(&file(&mut m, SLOT_STACKER), DeckFormat::Hex)
        .unwrap()
        .cards;
    assert_eq!(cards.len(), 3);
    assert_eq!(
        cards[0].text().unwrap(),
        format!("10    CONTINUE{}00000010", " ".repeat(58))
    );
    assert!(cards[1].is_blank());
    assert!(cards[2].text().unwrap().ends_with("00000020"));
}

#[test]
fn hopper_cards_get_more_holes() {
    let mut m = punch();
    let src = Deck {
        cards: vec![Card::from_text("AB").unwrap()],
    }
    .to_bytes(DeckFormat::Binary)
    .unwrap();
    m.media_slot(SLOT_HOPPER)
        .unwrap()
        .insert(deck("in.deck", &src))
        .unwrap();
    m.media_slot(SLOT_STACKER)
        .unwrap()
        .insert(deck("out.deck", b""))
        .unwrap();
    press(&mut m, keys::ENTER); // feeds the hopper's card
    press(&mut m, keys::RIGHT);
    press(&mut m, keys::RIGHT);
    typ(&mut m, "C");
    press(&mut m, keys::ENTER); // released; the next one is blank
    press(&mut m, keys::ENTER);
    let out = Deck::parse(&file(&mut m, SLOT_STACKER), DeckFormat::Binary).unwrap();
    assert_eq!(out.cards.len(), 2);
    assert_eq!(out.cards[0].text().unwrap(), "ABC");
    assert!(out.cards[1].is_blank());
}

#[test]
fn a_text_deck_refuses_a_pattern_without_character() {
    let mut m = punch();
    m.media_slot(SLOT_STACKER)
        .unwrap()
        .insert(deck("out.crd", b"OLD CARD\n"))
        .unwrap();
    typ(&mut m, "A");
    press(&mut m, keys::BACKSPACE);
    typ(&mut m, "J"); // 12-11-1 over the A: no character
    press(&mut m, keys::ENTER);
    assert_eq!(m.stacked().len(), 2, "the old card and the new one");
    assert!(m.stacker.error.is_some());
}

#[test]
fn cards_stacked_before_the_file_go_into_it() {
    let mut m = punch();
    typ(&mut m, "FIRST");
    press(&mut m, keys::ENTER);
    m.media_slot(SLOT_STACKER)
        .unwrap()
        .insert(deck("late.crd", b"ZERO\n"))
        .unwrap();
    assert_eq!(file(&mut m, SLOT_STACKER), b"ZERO\nFIRST\n");
    assert!(m.stacked().is_empty(), "ejecting empties the stacker");
}

#[test]
fn savestate_round_trip_and_corrupt_data() {
    let mut m = punch();
    typ(&mut m, "HELLO");
    press(&mut m, keys::ENTER);
    typ(&mut m, "WOR");
    let s = m.save_state();
    let mut n = punch();
    n.load_state(&s).unwrap();
    assert_eq!(n.card(), m.card());
    assert_eq!(n.column(), 4);
    assert_eq!(n.stacked(), m.stacked());
    for cut in [0, 5, s.len() / 2, s.len() - 1] {
        assert!(punch().load_state(&s[..cut]).is_err(), "cut {cut}");
    }
}

#[test]
fn the_card_space_shows_the_binary_layout() {
    let mut m = punch();
    typ(&mut m, "A9");
    let t = m.debug_target(PUNCH).unwrap();
    assert_eq!(t.peek(CARD_SPACE, 0, Size::Long), Some(0x9000_0010));
    assert!(t.poke(CARD_SPACE, 4, Size::Word, 0x2000)); // column 3: row 0
    assert_eq!(m.card().unwrap().columns[2], cards::ROW_0);
    assert_eq!(
        m.debug_target(PUNCH).unwrap().disassemble(4).operand,
        "200 '0'"
    );
}
