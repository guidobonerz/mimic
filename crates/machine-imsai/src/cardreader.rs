//! A card reader on the IMSAI (slot "cards"): reads decks the card punch
//! machine (`keypunch`) makes, in one of two ways (option `cardreader`).
//!
//! **`text`** (default): the reader sits on the reader line, as a
//! Teletype-style reader would: each card arrives as its characters
//! (`media::cards::to_ascii`; a column without a character as `?`), the
//! trailing blanks left off, then CR LF; after the last card a ^Z, the
//! CP/M end of file. So `PIP FILE.TXT=RDR:` reads a deck into a file. A
//! paper tape in the tape slot is read first.
//!
//! **`raw`**: a parallel card reader interface that hands the program
//! every column's 12 rows (**[S]**: this project's interface, not a model
//! of a real card; programs written for it read any hole pattern):
//!
//! | Port | Direction | Meaning |
//! | --- | --- | --- |
//! | $20 | OUT | bit 0: PICK (feed the next card past the read head) |
//! | $20 | IN  | bit 0 column ready, bit 1 busy (card moving), bit 6 hopper empty, bit 7 end of card (all 80 columns taken) |
//! | $21 | IN  | rows 2–9 of the column (bit 7 = row 2 … bit 0 = row 9) |
//! | $22 | IN  | rows 12, 11, 0, 1 in bits 3–0; reading it moves on to the next column |
//!
//! A pick takes [`PICK_CLOCKS`] (a card's travel to the first column at
//! 1000 cards a minute); the columns then wait for the program.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::cards::{self, Card, Deck, DeckFormat, COLUMNS};

/// From PICK to the first column: 60 ms at 2 MHz.
pub const PICK_CLOCKS: u64 = 120_000;
/// The reader's ports in `raw` mode.
pub const PORT_STATUS: u8 = 0x20;
pub const PORT_LOW: u8 = 0x21;
pub const PORT_HIGH: u8 = 0x22;

pub mod status {
    pub const READY: u8 = 0x01;
    pub const BUSY: u8 = 0x02;
    pub const EMPTY: u8 = 0x40;
    pub const END: u8 = 0x80;
}

/// End of a text file to CP/M.
const CPM_EOF: u8 = 0x1a;

#[derive(Default)]
pub struct CardReader {
    pub media: Option<Media>,
    cards: Vec<Card>,
    /// `raw`: cards go to the parallel ports, not the reader line.
    pub raw: bool,
    /// The deck as the reader line's bytes, and how far it is read.
    text: Vec<u8>,
    pub text_pos: usize,
    /// Raw: the next card to pick, the card at the read head, the column.
    next_card: usize,
    head: Option<usize>,
    column: usize,
    ready_at: u64,
}

impl CardReader {
    pub fn new(raw: bool) -> Self {
        Self {
            raw,
            ..Self::default()
        }
    }

    fn make_text(&mut self) {
        self.text.clear();
        if self.cards.is_empty() {
            return;
        }
        for card in &self.cards {
            let mut line: Vec<u8> = card
                .columns
                .iter()
                .map(|&c| cards::to_ascii(c).unwrap_or(b'?'))
                .collect();
            while line.last() == Some(&b' ') {
                line.pop();
            }
            self.text.extend_from_slice(&line);
            self.text.extend_from_slice(b"\r\n");
        }
        self.text.push(CPM_EOF);
    }

    /// The next byte for the reader line (text mode).
    pub fn text_byte(&mut self) -> Option<u8> {
        if self.raw {
            return None;
        }
        let b = self.text.get(self.text_pos).copied()?;
        self.text_pos += 1;
        Some(b)
    }

    /// Bytes left on the reader line.
    pub fn text_left(&self) -> usize {
        if self.raw {
            0
        } else {
            self.text.len() - self.text_pos.min(self.text.len())
        }
    }

    fn column_code(&self) -> u16 {
        self.head
            .and_then(|n| self.cards.get(n))
            .and_then(|c| c.columns.get(self.column).copied())
            .unwrap_or(0)
    }

    /// Whether the reader answers on `port` (raw mode only).
    pub fn decodes(&self, port: u8) -> bool {
        self.raw && (PORT_STATUS..=PORT_HIGH).contains(&port)
    }

    pub fn peek(&self, port: u8, t: u64) -> u8 {
        match port {
            PORT_STATUS => {
                let mut s = 0;
                if self.head.is_some() {
                    if t < self.ready_at {
                        s |= status::BUSY;
                    } else if self.column < COLUMNS {
                        s |= status::READY;
                    } else {
                        s |= status::END;
                    }
                }
                if self.next_card >= self.cards.len() {
                    s |= status::EMPTY;
                }
                s
            }
            PORT_LOW if self.ready(t) => self.column_code() as u8,
            PORT_HIGH if self.ready(t) => (self.column_code() >> 8) as u8 & 0x0f,
            _ => 0,
        }
    }

    fn ready(&self, t: u64) -> bool {
        self.head.is_some() && t >= self.ready_at && self.column < COLUMNS
    }

    pub fn read(&mut self, port: u8, t: u64) -> u8 {
        let v = self.peek(port, t);
        if port == PORT_HIGH && self.ready(t) {
            self.column += 1;
        }
        v
    }

    pub fn write(&mut self, port: u8, v: u8, t: u64) {
        if port != PORT_STATUS || v & 1 == 0 {
            return;
        }
        // A card still in motion is not picked over.
        if self.head.is_some() && t < self.ready_at {
            return;
        }
        if self.next_card < self.cards.len() {
            self.head = Some(self.next_card);
            self.next_card += 1;
            self.column = 0;
            self.ready_at = t + PICK_CLOCKS;
        } else {
            self.head = None;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u64(self.text_pos as u64);
        w.u32(self.next_card as u32);
        w.u32(self.head.map_or(u32::MAX, |n| n as u32));
        w.u8(self.column as u8);
        w.u64(self.ready_at);
    }

    /// The deck itself is media, not state: positions past its end are
    /// cut to it.
    pub fn set_position(&mut self, p: Position) {
        self.text_pos = p.text_pos.min(self.text.len());
        self.next_card = p.next_card.min(self.cards.len());
        self.head = p.head.filter(|&n| n < self.cards.len());
        self.column = p.column;
        self.ready_at = p.ready_at;
    }
}

/// Where the reader stands in its deck (the savestate's part).
#[derive(Clone, Copy, Debug)]
pub struct Position {
    text_pos: usize,
    next_card: usize,
    head: Option<usize>,
    column: usize,
    ready_at: u64,
}

impl Position {
    pub fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let text_pos = r.u64()? as usize;
        let next_card = r.u32()? as usize;
        let head = r.u32()?;
        let column = usize::from(r.u8()?);
        let ready_at = r.u64()?;
        if column > COLUMNS {
            return Err(StateError::Invalid("card column"));
        }
        Ok(Self {
            text_pos,
            next_card,
            head: (head != u32::MAX).then_some(head as usize),
            column,
            ready_at,
        })
    }
}

impl MediaSlot for CardReader {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Cards {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let format = DeckFormat::for_name(&media.name).unwrap_or(DeckFormat::Binary);
        let deck = Deck::parse(&bytes, format).map_err(|e| MediaError::Format(e.to_string()))?;
        self.cards = deck.cards;
        self.make_text();
        self.text_pos = 0;
        self.next_card = 0;
        self.head = None;
        self.column = 0;
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.cards.clear();
        self.text.clear();
        self.text_pos = 0;
        self.next_card = 0;
        self.head = None;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }

    /// "rewind": the deck back into the hopper.
    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        match command {
            "rewind" => {
                self.text_pos = 0;
                self.next_card = 0;
                self.head = None;
                self.column = 0;
                Ok(())
            }
            other => Err(MediaError::Control(format!("'{other}': only rewind"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);

    impl emu_core::media::MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
            Err(MediaError::WriteProtected)
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn reader(raw: bool, name: &str, data: &[u8]) -> CardReader {
        let mut r = CardReader::new(raw);
        r.insert(Media {
            name: name.into(),
            kind: MediaKind::Cards,
            backend: Box::new(Mem(data.to_vec())),
        })
        .unwrap();
        r
    }

    #[test]
    fn text_mode_gives_lines_and_an_end_of_file() {
        let mut r = reader(false, "a.crd", b"HELLO\n\n  X = 1   \n");
        let mut got = Vec::new();
        while let Some(b) = r.text_byte() {
            got.push(b);
        }
        assert_eq!(got, b"HELLO\r\n\r\n  X = 1\r\n\x1a");
        assert!(!r.decodes(PORT_STATUS));
    }

    #[test]
    fn raw_mode_hands_out_columns() {
        let hex = b"801 200 FFF\n";
        let mut r = reader(true, "a.chx", hex);
        assert_eq!(r.text_byte(), None);
        assert!(r.decodes(PORT_HIGH));
        assert_eq!(r.peek(PORT_STATUS, 0), 0, "a card waits");
        r.write(PORT_STATUS, 1, 100);
        assert_eq!(r.read(PORT_STATUS, 100), status::BUSY | status::EMPTY);
        let t = 100 + PICK_CLOCKS;
        let mut cols = Vec::new();
        while r.read(PORT_STATUS, t) & status::READY != 0 {
            let lo = r.read(PORT_LOW, t);
            let hi = r.read(PORT_HIGH, t);
            cols.push(u16::from(hi) << 8 | u16::from(lo));
        }
        assert_eq!(cols.len(), 80);
        assert_eq!(cols[..4], [0x801, 0x200, 0xfff, 0]);
        assert_eq!(r.peek(PORT_STATUS, t), status::END | status::EMPTY);
        // Savestate keeps the position.
        let mut w = StateWriter::new();
        r.save(&mut w);
        let data = w.into_bytes();
        let mut q = reader(true, "a.chx", hex);
        q.set_position(Position::load(&mut StateReader::new(&data)).unwrap());
        assert_eq!(q.peek(PORT_STATUS, t), status::END | status::EMPTY);
    }
}
