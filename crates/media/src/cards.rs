//! Punched cards: 80-column Hollerith cards, the IBM 029 card punch's
//! character code, and three deck file formats.
//!
//! A column is 12 rows: 12, 11, 0, 1 … 9 from the top edge. Here a column
//! is a 12-bit value, row 12 in bit 11, row 11 in bit 10, row 0 in bit 9,
//! rows 1–9 in bits 8–0 (row `r` in bit `9 - r`).
//!
//! Deck files (all **[S]**, this project's formats):
//!
//! - **`.deck`, binary**: 160 bytes a card, its 80 columns as big-endian
//!   16-bit words, left-justified: row 12 in bit 15 … row 9 in bit 4, the
//!   low four bits 0 (the "column binary" layout card readers deliver).
//!   The file is just the cards one after the other; an empty file is an
//!   empty deck.
//! - **`.crd`, text**: one line a card, the characters the 029 punches
//!   for them, UTF-8. Lower case is taken as upper case; `[` and `^` are
//!   accepted for `¢` and `¬`. An empty line is a blank card; more than 80
//!   characters is an error. Written without trailing blanks, lines end in
//!   LF (CR LF reads too).
//! - **`.chx`, hex columns**: one line a card, up to 80 columns as up to
//!   three hex digits each (the 12-bit value above), separated by blanks;
//!   missing columns are blank. Lines starting with `#` are comments, empty
//!   lines are skipped. Every hole pattern fits, also those no key punches.
//!
//! Source: IBM 29 Card Punch Reference Manual (A24-3332), the keyboard
//! and code chart; IBM System/360 Principles of Operation (EBCDIC card
//! code). The `[`/`^` aliases and [`to_ascii`] are **[S]**.

use std::fmt;

/// Columns on a card.
pub const COLUMNS: usize = 80;
/// Bytes a card takes in a binary deck.
pub const CARD_BYTES: usize = 2 * COLUMNS;

pub const ROW_12: u16 = 1 << 11;
pub const ROW_11: u16 = 1 << 10;
pub const ROW_0: u16 = 1 << 9;

/// The bit of digit row `r` (1–9).
pub const fn row(r: u8) -> u16 {
    1 << (9 - r)
}

/// One card: 80 columns of 12 rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub columns: [u16; COLUMNS],
}

impl Default for Card {
    fn default() -> Self {
        Self {
            columns: [0; COLUMNS],
        }
    }
}

impl Card {
    /// A card punched with `text` from column 1 (characters without a
    /// code are an error).
    pub fn from_text(text: &str) -> Result<Self, CardError> {
        let mut card = Card::default();
        for (i, ch) in text.chars().enumerate() {
            if i >= COLUMNS {
                return Err(CardError::TooLong);
            }
            card.columns[i] = hollerith(ch).ok_or(CardError::NoCode(ch))?;
        }
        Ok(card)
    }

    /// The characters the 029 would print over the columns; `None` if a
    /// column holds a pattern no key punches. Trailing blanks are cut.
    pub fn text(&self) -> Option<String> {
        let mut s: String = self
            .columns
            .iter()
            .map(|&c| character(c))
            .collect::<Option<_>>()?;
        s.truncate(s.trim_end_matches(' ').len());
        Some(s)
    }

    pub fn is_blank(&self) -> bool {
        self.columns.iter().all(|&c| c == 0)
    }
}

/// The 029's special characters (the EBCDIC card code); letters and
/// digits follow from their rows in [`hollerith`].
const CODES: &[(char, u16)] = &[
    (' ', 0),
    ('&', ROW_12),
    ('-', ROW_11),
    ('0', ROW_0),
    ('/', ROW_0 | row(1)),
    // 12-2-8 … 12-7-8
    ('¢', ROW_12 | row(2) | row(8)),
    ('.', ROW_12 | row(3) | row(8)),
    ('<', ROW_12 | row(4) | row(8)),
    ('(', ROW_12 | row(5) | row(8)),
    ('+', ROW_12 | row(6) | row(8)),
    ('|', ROW_12 | row(7) | row(8)),
    // 11-2-8 … 11-7-8
    ('!', ROW_11 | row(2) | row(8)),
    ('$', ROW_11 | row(3) | row(8)),
    ('*', ROW_11 | row(4) | row(8)),
    (')', ROW_11 | row(5) | row(8)),
    (';', ROW_11 | row(6) | row(8)),
    ('¬', ROW_11 | row(7) | row(8)),
    // 0-3-8 … 0-7-8
    (',', ROW_0 | row(3) | row(8)),
    ('%', ROW_0 | row(4) | row(8)),
    ('_', ROW_0 | row(5) | row(8)),
    ('>', ROW_0 | row(6) | row(8)),
    ('?', ROW_0 | row(7) | row(8)),
    // 2-8 … 7-8
    (':', row(2) | row(8)),
    ('#', row(3) | row(8)),
    ('@', row(4) | row(8)),
    ('\'', row(5) | row(8)),
    ('=', row(6) | row(8)),
    ('"', row(7) | row(8)),
];

/// The holes the 029 punches for `ch` (`None`: no key for it).
pub fn hollerith(ch: char) -> Option<u16> {
    let ch = match ch {
        '[' => '¢',
        '^' => '¬',
        c => c.to_ascii_uppercase(),
    };
    match ch {
        '1'..='9' => Some(row(ch as u8 - b'0')),
        'A'..='I' => Some(ROW_12 | row(ch as u8 - b'A' + 1)),
        'J'..='R' => Some(ROW_11 | row(ch as u8 - b'J' + 1)),
        'S'..='Z' => Some(ROW_0 | row(ch as u8 - b'S' + 2)),
        _ => CODES.iter().find(|(c, _)| *c == ch).map(|&(_, h)| h),
    }
}

/// The character the 029 prints for a column (`None`: not one of its
/// 63 codes).
pub fn character(code: u16) -> Option<char> {
    let zone = code & (ROW_12 | ROW_11 | ROW_0);
    let digits = code & 0x1ff;
    if let Some(d) = (1..=9u8).find(|&r| digits == row(r)) {
        let c = match zone {
            0 => Some(b'0' + d),
            ROW_12 => Some(b'A' + d - 1),
            ROW_11 => Some(b'J' + d - 1),
            ROW_0 if d >= 2 => Some(b'S' + d - 2),
            _ => None,
        };
        if let Some(c) = c {
            return Some(c as char);
        }
    }
    CODES.iter().find(|(_, h)| *h == code).map(|&(c, _)| c)
}

/// A column as an ASCII byte (for a reader that delivers text): `¢` is
/// `[` and `¬` is `^`; a pattern without a character is `None`.
pub fn to_ascii(code: u16) -> Option<u8> {
    match character(code)? {
        '¢' => Some(b'['),
        '¬' => Some(b'^'),
        c => Some(c as u8),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardError {
    /// More than 80 columns on a line.
    TooLong,
    /// A character the 029 has no code for.
    NoCode(char),
    /// A binary deck whose length is not a multiple of 160.
    PartialCard,
    /// A hex column that is not up to three hex digits.
    BadHex(String),
    /// A column `(card, column)` (both from 1) has no character to write
    /// into a text deck.
    NoCharacter(usize, usize),
}

impl fmt::Display for CardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CardError::TooLong => write!(f, "more than 80 columns on a card"),
            CardError::NoCode(c) => write!(f, "no card code for {c:?}"),
            CardError::PartialCard => write!(f, "binary deck ends inside a card"),
            CardError::BadHex(s) => write!(f, "bad hex column {s:?}"),
            CardError::NoCharacter(card, col) => {
                write!(f, "card {card} column {col} has no character")
            }
        }
    }
}

/// A deck file's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckFormat {
    Binary,
    Text,
    Hex,
}

impl DeckFormat {
    /// The format of a file name's extension (`.deck`, `.crd`, `.chx`).
    pub fn for_name(name: &str) -> Option<Self> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "deck" => Some(Self::Binary),
            "crd" => Some(Self::Text),
            "chx" => Some(Self::Hex),
            _ => None,
        }
    }
}

/// Cards in order, the first card first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Deck {
    pub cards: Vec<Card>,
}

impl Deck {
    pub fn parse(data: &[u8], format: DeckFormat) -> Result<Self, CardError> {
        match format {
            DeckFormat::Binary => parse_binary(data),
            DeckFormat::Text => parse_text(data),
            DeckFormat::Hex => parse_hex(data),
        }
    }

    /// The deck as a file; a text deck fails on a column without a
    /// character.
    pub fn to_bytes(&self, format: DeckFormat) -> Result<Vec<u8>, CardError> {
        match format {
            DeckFormat::Binary => Ok(self.binary()),
            DeckFormat::Text => self.text(),
            DeckFormat::Hex => Ok(self.hex()),
        }
    }

    fn binary(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.cards.len() * CARD_BYTES);
        for card in &self.cards {
            for &c in &card.columns {
                out.extend_from_slice(&((c & 0xfff) << 4).to_be_bytes());
            }
        }
        out
    }

    fn text(&self) -> Result<Vec<u8>, CardError> {
        let mut out = String::new();
        for (n, card) in self.cards.iter().enumerate() {
            let Some(t) = card.text() else {
                let col = card
                    .columns
                    .iter()
                    .position(|&c| character(c).is_none())
                    .unwrap_or(0);
                return Err(CardError::NoCharacter(n + 1, col + 1));
            };
            out.push_str(&t);
            out.push('\n');
        }
        Ok(out.into_bytes())
    }

    fn hex(&self) -> Vec<u8> {
        let mut out = String::new();
        for card in &self.cards {
            let cols: Vec<String> = card
                .columns
                .iter()
                .map(|c| format!("{:03X}", c & 0xfff))
                .collect();
            out.push_str(&cols.join(" "));
            out.push('\n');
        }
        out.into_bytes()
    }
}

fn parse_binary(data: &[u8]) -> Result<Deck, CardError> {
    let (chunks, rest) = data.as_chunks::<CARD_BYTES>();
    if !rest.is_empty() {
        return Err(CardError::PartialCard);
    }
    let cards = chunks
        .iter()
        .map(|chunk| {
            let mut card = Card::default();
            for (c, w) in card.columns.iter_mut().zip(chunk.as_chunks::<2>().0) {
                *c = u16::from_be_bytes(*w) >> 4;
            }
            card
        })
        .collect();
    Ok(Deck { cards })
}

/// A text file's lines; the break after the last line does not start
/// another.
fn lines(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut v: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if v.last().is_some_and(String::is_empty) {
        v.pop();
    }
    v
}

fn parse_text(data: &[u8]) -> Result<Deck, CardError> {
    let cards = lines(data)
        .iter()
        .map(|l| Card::from_text(l))
        .collect::<Result<_, _>>()?;
    Ok(Deck { cards })
}

fn parse_hex(data: &[u8]) -> Result<Deck, CardError> {
    let mut cards = Vec::new();
    for line in lines(data) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut card = Card::default();
        for (i, word) in line.split_whitespace().enumerate() {
            if i >= COLUMNS {
                return Err(CardError::TooLong);
            }
            card.columns[i] = (word.len() <= 3)
                .then(|| u16::from_str_radix(word, 16).ok())
                .flatten()
                .ok_or_else(|| CardError::BadHex(word.to_string()))?;
        }
        cards.push(card);
    }
    Ok(Deck { cards })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_029_code_chart() {
        assert_eq!(hollerith('A'), Some(ROW_12 | row(1)));
        assert_eq!(hollerith('j'), Some(ROW_11 | row(1)));
        assert_eq!(hollerith('S'), Some(ROW_0 | row(2)));
        assert_eq!(hollerith('Z'), Some(ROW_0 | row(9)));
        assert_eq!(hollerith('/'), Some(ROW_0 | row(1)));
        assert_eq!(hollerith('='), Some(row(6) | row(8)));
        assert_eq!(hollerith('['), hollerith('¢'));
        assert_eq!(hollerith('~'), None);
        // All 63 codes (with the blank) are distinct and map back.
        let all: Vec<char> = (' '..='Z')
            .chain(['¢', '|', '¬', '_'])
            .filter(|&c| hollerith(c).is_some())
            .collect();
        assert_eq!(all.len(), 63);
        let mut codes: Vec<u16> = all.iter().map(|&c| hollerith(c).unwrap()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 63);
        for c in all {
            assert_eq!(character(hollerith(c).unwrap()), Some(c), "{c}");
        }
        assert_eq!(character(ROW_12 | ROW_11), None);
        assert_eq!(to_ascii(hollerith('¬').unwrap()), Some(b'^'));
    }

    #[test]
    fn three_formats_round_trip() {
        let deck = Deck {
            cards: vec![
                Card::from_text("      PROGRAM HELLO").unwrap(),
                Card::default(),
                Card::from_text("C THIS IS A COMMENT (1+2)*3=9 $¢¬").unwrap(),
            ],
        };
        for f in [DeckFormat::Binary, DeckFormat::Text, DeckFormat::Hex] {
            let bytes = deck.to_bytes(f).unwrap();
            assert_eq!(Deck::parse(&bytes, f).unwrap(), deck, "{f:?}");
        }
        assert_eq!(deck.to_bytes(DeckFormat::Binary).unwrap().len(), 480);
        let text = String::from_utf8(deck.to_bytes(DeckFormat::Text).unwrap()).unwrap();
        assert!(text.starts_with("      PROGRAM HELLO\n\nC THIS"));
    }

    #[test]
    fn binary_layout_is_left_justified_big_endian() {
        let deck = Deck {
            cards: vec![Card::from_text("A9").unwrap()],
        };
        let b = deck.to_bytes(DeckFormat::Binary).unwrap();
        // A = 12-1: bits 15 and 12 → $9000; 9 = row 9: bit 4 → $0010.
        assert_eq!(b[..4], [0x90, 0x00, 0x00, 0x10]);
        assert_eq!(
            Deck::parse(&b[..100], DeckFormat::Binary),
            Err(CardError::PartialCard)
        );
    }

    #[test]
    fn hex_takes_comments_short_lines_and_any_pattern() {
        let src = b"# two cards\r\n\nFFF 801 \r\n000 000 A00\n";
        let deck = Deck::parse(src, DeckFormat::Hex).unwrap();
        assert_eq!(deck.cards.len(), 2);
        assert_eq!(deck.cards[0].columns[..3], [0xfff, 0x801, 0]);
        assert_eq!(deck.cards[1].columns[2], ROW_12 | ROW_0);
        // A lace card has no text form.
        assert_eq!(
            deck.to_bytes(DeckFormat::Text),
            Err(CardError::NoCharacter(1, 1))
        );
        assert!(Deck::parse(b"1000", DeckFormat::Hex).is_err());
        assert!(Deck::parse(b"xyz", DeckFormat::Hex).is_err());
    }

    #[test]
    fn text_limits() {
        assert_eq!(Card::from_text(&"X".repeat(81)), Err(CardError::TooLong));
        assert_eq!(Card::from_text("{"), Err(CardError::NoCode('{')));
        assert_eq!(DeckFormat::for_name("prog.CRD"), Some(DeckFormat::Text));
        assert_eq!(DeckFormat::for_name("a.deck"), Some(DeckFormat::Binary));
        assert_eq!(DeckFormat::for_name("noext"), None);
    }
}
