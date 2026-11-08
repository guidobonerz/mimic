//! IBM 029 card punch: a machine of its own, for punching card decks that
//! other machines read (the IMSAI's card reader).
//!
//! **The machine.** Blank cards come from the hopper (or the cards of a
//! deck put into the hopper slot, to punch more holes into them); FEED
//! (or Enter without a card) feeds one and registers it at the punch
//! station, column 1 under the dies. Typing punches the key's Hollerith
//! code into the column ([`media::cards`]) and, with PRINT on, prints the
//! character along the card's top edge; the card then moves on a column.
//! Backspace moves it back a column (the holes stay, as on the 029), the
//! space bar or → moves on without punching. REL (Enter) releases the card
//! into the stacker; with AUTO FEED on the next card is fed at once, and a
//! card is released by itself after column 80. Tab is SKIP: on to the next
//! field of the program.
//!
//! **Programs** (the PROG switch): the field starts of a preprinted card
//! format, where Tab stops and the picture draws field lines (on the 029
//! a program card on the drum does this): FREE (none: Tab skips to column
//! 80), FORTRAN (statement number 1–5, continuation 6, statement 7–72,
//! identification 73–80), COBOL (sequence 1–6, indicator 7, area A 8–11,
//! area B 12–72, identification 73–80) and ASSEMBLER (System/360: name
//! 1–8, operation 10–14, operands 16–71, continuation 72, identification
//! 73–80). With SEQ on, a released card that is not blank gets a sequence
//! number punched into an empty sequence field (73–80, COBOL 1–6), from 10
//! in steps of 10. Programs and SEQ are **[S]** (helpers, not 029 parts).
//!
//! **Decks.** The stacker slot takes a deck file (`.deck`, `.crd`, `.chx`,
//! see [`media::cards`]); every card released is written into it, in the
//! file's format, after the cards it already had. Cards stacked before a
//! file is inserted go into it when it is. A card a text deck cannot hold
//! (a hole pattern without a character) stays in the stacker and the
//! status line says so. Ejecting the deck empties the stacker.
//!
//! **Time.** Nothing here has a clock of its own: a frame is 1000 ticks
//! at 60 frames a second; input applies at its tick. Punching is instant.
//!
//! Sources: IBM "Reference Manual, IBM 29 Card Punch" (A24-3332): keyboard,
//! FEED/REG/REL, AUTO FEED, PRINT, SKIP, backspace, column indicator; the
//! card formats from IBM FORTRAN IV (C28-6515), COBOL (C28-6516) and
//! System/360 Assembler (C28-6514) language references.

#![forbid(unsafe_code)]

mod picture;

use emu_core::frame::{FrameRef, PixelData};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, ChipInfo, ControlInfo, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, Register, RegisterSet, ResetKind, Savestate, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use media::cards::{self, Card, Deck, DeckFormat, COLUMNS};

pub use picture::{HEIGHT, WIDTH};

/// The punch as the debug target. Named "cpu" as every machine's first
/// device, the debugger's default target (the 029 has no processor).
pub const PUNCH: DeviceId = DeviceId("cpu");
/// The card at the punch station: 160 bytes, the binary deck layout.
pub const CARD_SPACE: MemSpaceId = MemSpaceId(0);
pub const SLOT_HOPPER: SlotId = SlotId(0);
pub const SLOT_STACKER: SlotId = SlotId(1);
pub const FPS: u32 = 60;
/// Ticks a frame (the time base of input).
pub const FRAME: u64 = 1000;

/// Control ids.
pub mod ctl {
    pub const POWER: u16 = 0;
    pub const FEED: u16 = 1;
    pub const REL: u16 = 2;
    pub const BACKSPACE: u16 = 3;
    pub const SKIP: u16 = 4;
    pub const CLEAR: u16 = 5;
    pub const AUTO_FEED: u16 = 6;
    pub const PRINT: u16 = 7;
    pub const PROGRAM: u16 = 8;
    pub const SEQ: u16 = 9;
}

/// Machine keys: what a key sends unshifted (the Apple II host keymap's
/// codes, as on the IMSAI), and the modifiers.
pub mod keys {
    pub const SHIFT: u16 = 0x100;
    pub const CTRL: u16 = 0x101;
    pub const BACKSPACE: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const ENTER: u16 = 0x0d;
    pub const RIGHT: u16 = 0x15;
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "keypunch",
        name: "IBM 029 Card Punch",
        models: &["029"],
        video_formats: &[],
        create: |config| Ok(Box::new(Keypunch::new(config)?)),
    }
}

/// A card format: field starts (1-based) and the sequence field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    Free,
    Fortran,
    Cobol,
    Assembler,
}

impl Program {
    const ALL: [Program; 4] = [
        Program::Free,
        Program::Fortran,
        Program::Cobol,
        Program::Assembler,
    ];
    const NAMES: &'static [&'static str] = &["FREE", "FORTRAN", "COBOL", "ASSEMBLER"];

    fn index(self) -> u8 {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0) as u8
    }

    fn from_index(i: u8) -> Option<Self> {
        Self::ALL.get(usize::from(i)).copied()
    }

    fn from_name(s: &str) -> Option<Self> {
        let s = s.to_ascii_uppercase();
        let s = if s == "ASM" { "ASSEMBLER".into() } else { s };
        Self::NAMES
            .iter()
            .position(|&n| n == s)
            .and_then(|i| Self::from_index(i as u8))
    }

    /// Where fields start (1-based columns), column 1 left out.
    pub fn fields(self) -> &'static [usize] {
        match self {
            Program::Free => &[],
            Program::Fortran => &[6, 7, 73],
            Program::Cobol => &[7, 8, 12, 73],
            Program::Assembler => &[10, 16, 72, 73],
        }
    }

    /// The sequence field: first column (1-based) and width.
    pub fn sequence(self) -> (usize, usize) {
        match self {
            Program::Cobol => (1, 6),
            _ => (73, 8),
        }
    }
}

/// A card in the machine: its holes and what was printed over them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Station {
    columns: [u16; COLUMNS],
    printed: [u8; COLUMNS],
}

impl Station {
    /// A card from a deck; the 029 cannot tell what was printed on it, so
    /// it shows the characters its holes stand for.
    fn from_card(card: &Card) -> Self {
        let mut printed = [0; COLUMNS];
        for (p, &c) in printed.iter_mut().zip(&card.columns) {
            *p = if c == 0 {
                0
            } else {
                cards::to_ascii(c).unwrap_or(0)
            };
        }
        Self {
            columns: card.columns,
            printed,
        }
    }

    fn card(&self) -> Card {
        Card {
            columns: self.columns,
        }
    }

    fn save(&self, w: &mut StateWriter) {
        for &c in &self.columns {
            w.u16(c);
        }
        w.bytes(&self.printed);
    }

    fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut columns = [0; COLUMNS];
        for c in &mut columns {
            *c = r.u16()? & 0xfff;
        }
        let printed: [u8; COLUMNS] = r
            .bytes()?
            .try_into()
            .map_err(|_| StateError::Invalid("printed columns"))?;
        Ok(Self { columns, printed })
    }
}

fn save_cards(w: &mut StateWriter, cards: &[Card]) {
    w.u32(cards.len() as u32);
    for c in cards {
        for &col in &c.columns {
            w.u16(col);
        }
    }
}

fn load_cards(r: &mut StateReader<'_>) -> Result<Vec<Card>, StateError> {
    let n = r.u32()? as usize;
    // A card takes 160 bytes; more cards than bytes left is corrupt.
    if n > r.remaining() / (2 * COLUMNS) {
        return Err(StateError::Invalid("card count"));
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let mut card = Card::default();
        for c in &mut card.columns {
            *c = r.u16()? & 0xfff;
        }
        v.push(card);
    }
    Ok(v)
}

fn read_deck(media: &mut Media) -> Result<Deck, MediaError> {
    let mut bytes = vec![0; media.backend.len() as usize];
    media.backend.read(0, &mut bytes)?;
    let format = DeckFormat::for_name(&media.name).unwrap_or(DeckFormat::Binary);
    Deck::parse(&bytes, format).map_err(|e| MediaError::Format(e.to_string()))
}

/// The hopper: blank cards, or a deck's cards first.
#[derive(Default)]
struct Hopper {
    media: Option<Media>,
    cards: Vec<Card>,
    next: usize,
}

impl MediaSlot for Hopper {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Cards {
            return Err(MediaError::Unsupported(media.kind));
        }
        self.cards = read_deck(&mut media)?.cards;
        self.next = 0;
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.cards.clear();
        self.next = 0;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The stacker: the cards released, and the deck file they go into.
#[derive(Default)]
struct Stacker {
    media: Option<Media>,
    cards: Vec<Card>,
    /// Why the file does not hold every card.
    error: Option<String>,
}

impl Stacker {
    fn stack(&mut self, card: Card) {
        self.cards.push(card);
        self.write_out();
    }

    /// The whole deck into the file, in its format.
    fn write_out(&mut self) {
        let Some(m) = &mut self.media else {
            return;
        };
        let format = DeckFormat::for_name(&m.name).unwrap_or(DeckFormat::Binary);
        let deck = Deck {
            cards: self.cards.clone(),
        };
        let result = deck
            .to_bytes(format)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                m.backend
                    .set_len(bytes.len() as u64)
                    .and_then(|()| m.backend.write(0, &bytes))
                    .map_err(|e| e.to_string())
            });
        self.error = result.err();
    }
}

impl MediaSlot for Stacker {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Cards {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut deck = read_deck(&mut media)?;
        deck.cards.append(&mut self.cards);
        self.cards = deck.cards;
        self.media = Some(media);
        self.write_out();
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.cards.clear();
        self.error = None;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }
}

pub struct Keypunch {
    t: u64,
    frame_end: u64,
    input: InputQueue,
    power: bool,
    station: Option<Station>,
    /// Column under the dies, 0-based (80: past the last).
    column: usize,
    auto_feed: bool,
    print: bool,
    program: Program,
    seq: bool,
    seq_next: u32,
    hopper: Hopper,
    stacker: Stacker,
    shift: bool,
    ctrl: bool,
    /// Buttons held (bit = control id).
    held: u16,
    /// What the last key could not do.
    note: String,
    pixels: Vec<u8>,
}

impl Keypunch {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(m) = &config.model {
            if m != "029" {
                return Err(ConfigError::UnknownModel(m.clone()));
            }
        }
        let flag = |name: &str, default: bool| -> Result<bool, ConfigError> {
            match config.options.get(name).map(String::as_str) {
                None => Ok(default),
                Some("on") => Ok(true),
                Some("off") => Ok(false),
                Some(v) => Err(ConfigError::Invalid(format!("{name}={v}: on or off"))),
            }
        };
        let program = match config.options.get("program") {
            None => Program::Free,
            Some(p) => Program::from_name(p).ok_or_else(|| {
                ConfigError::Invalid(format!("program={p}: free, fortran, cobol or assembler"))
            })?,
        };
        let mut k = Self {
            t: 0,
            frame_end: FRAME,
            input: InputQueue::new(),
            power: true,
            station: None,
            column: 0,
            auto_feed: flag("autofeed", true)?,
            print: flag("print", true)?,
            program,
            seq: flag("seq", false)?,
            seq_next: 10,
            hopper: Hopper::default(),
            stacker: Stacker::default(),
            shift: false,
            ctrl: false,
            held: 0,
            note: String::new(),
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
        };
        k.render();
        Ok(k)
    }

    /// The cards in the stacker.
    pub fn stacked(&self) -> &[Card] {
        &self.stacker.cards
    }

    /// The card at the punch station.
    pub fn card(&self) -> Option<Card> {
        self.station.as_ref().map(Station::card)
    }

    /// Column under the dies, 1-based (81: past the last).
    pub fn column(&self) -> usize {
        self.column + 1
    }

    fn feed(&mut self) {
        if !self.power || self.station.is_some() {
            return;
        }
        let card = match self.hopper.cards.get(self.hopper.next) {
            Some(c) => {
                self.hopper.next += 1;
                Station::from_card(c)
            }
            None => Station::from_card(&Card::default()),
        };
        self.station = Some(card);
        self.column = 0;
    }

    fn release(&mut self) {
        if !self.power {
            return;
        }
        let Some(mut s) = self.station.take() else {
            return;
        };
        if self.seq {
            self.number(&mut s);
        }
        self.stacker.stack(s.card());
        self.column = 0;
        if self.auto_feed {
            self.feed();
        }
    }

    /// The sequence number into an empty sequence field of a card with
    /// holes elsewhere.
    fn number(&mut self, s: &mut Station) {
        let (first, width) = self.program.sequence();
        let field = first - 1..first - 1 + width;
        let blank_field = s.columns[field.clone()].iter().all(|&c| c == 0);
        let holes_elsewhere = s
            .columns
            .iter()
            .enumerate()
            .any(|(i, &c)| c != 0 && !field.contains(&i));
        if !blank_field || !holes_elsewhere {
            return;
        }
        let digits = format!("{:0width$}", self.seq_next % 10u32.pow(width as u32));
        for (i, d) in field.zip(digits.bytes()) {
            s.columns[i] = cards::hollerith(char::from(d)).unwrap_or(0);
            if self.print {
                s.printed[i] = d;
            }
        }
        self.seq_next += 10;
    }

    fn punch(&mut self, ch: char) {
        let Some(code) = cards::hollerith(ch) else {
            self.note = format!("NO CARD CODE FOR '{ch}'");
            return;
        };
        if self.station.is_none() && self.power {
            self.feed();
        }
        let print = self.print;
        let Some(s) = &mut self.station else {
            return;
        };
        if self.column >= COLUMNS {
            return;
        }
        s.columns[self.column] |= code;
        if print && code != 0 {
            s.printed[self.column] = cards::to_ascii(code).unwrap_or(0);
        }
        self.advance(1);
    }

    /// The card on by `n` columns; past column 80 it releases itself when
    /// AUTO FEED is on.
    fn advance(&mut self, n: usize) {
        if self.station.is_none() {
            return;
        }
        self.column = (self.column + n).min(COLUMNS);
        if self.column >= COLUMNS && self.auto_feed {
            self.release();
        }
    }

    fn backspace(&mut self) {
        if self.station.is_some() {
            self.column = self.column.saturating_sub(1);
        }
    }

    /// SKIP: on to the next field start, or column 80.
    fn skip(&mut self) {
        if self.station.is_none() {
            return;
        }
        let here = self.column + 1;
        let to = self
            .program
            .fields()
            .iter()
            .copied()
            .find(|&f| f > here)
            .unwrap_or(COLUMNS);
        self.column = to - 1;
    }

    /// CLEAR: the card at the station into the stacker, no new card.
    fn clear(&mut self) {
        if let Some(s) = self.station.take() {
            self.stacker.stack(s.card());
        }
        self.column = 0;
    }

    fn key(&mut self, key: u16, down: bool) {
        match key {
            keys::SHIFT => self.shift = down,
            keys::CTRL => self.ctrl = down,
            _ if !down || !self.power => {}
            keys::ENTER => {
                if self.station.is_some() {
                    self.release();
                } else {
                    self.feed();
                }
            }
            keys::BACKSPACE => self.backspace(),
            keys::TAB => self.skip(),
            keys::RIGHT | 0x20 => {
                // Like a punching key, the first one feeds a card.
                if self.station.is_none() {
                    self.feed();
                }
                self.advance(1);
            }
            k @ 0x21..=0x7e if !self.ctrl => {
                let ch = shifted(k as u8, self.shift);
                self.punch(char::from(ch));
            }
            _ => {}
        }
    }

    fn control(&mut self, id: u16, value: u8) {
        let bit = 1u16 << id;
        let pressed = value != 0 && self.held & bit == 0;
        if id <= ctl::CLEAR {
            if value != 0 {
                self.held |= bit;
            } else {
                self.held &= !bit;
            }
        }
        match id {
            ctl::POWER if pressed => self.reset(ResetKind::Hard),
            ctl::FEED if pressed => self.feed(),
            ctl::REL if pressed => self.release(),
            ctl::BACKSPACE if pressed => self.backspace(),
            ctl::SKIP if pressed => self.skip(),
            ctl::CLEAR if pressed => self.clear(),
            ctl::AUTO_FEED => self.auto_feed = value != 0,
            ctl::PRINT => self.print = value != 0,
            ctl::PROGRAM => {
                if let Some(p) = Program::from_index(value) {
                    self.program = p;
                }
            }
            ctl::SEQ => self.seq = value != 0,
            _ => {}
        }
    }

    fn apply(&mut self, ev: TimedInput) {
        self.note.clear();
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.key(machine_key.0, down);
            }
            (InputTarget::Panel, InputEvent::Control { id, value }) => {
                self.control(id.0, value);
            }
            _ => {}
        }
    }

    fn service(&mut self, until: u64) {
        while let Some(ev) = self.input.pop_due(Cycle(until)) {
            self.t = self.t.max(ev.at.0);
            self.apply(ev);
        }
    }

    fn render(&mut self) {
        let on_off = |b: bool| if b { "ON" } else { "OFF" };
        let col = if self.station.is_some() {
            format!("COLUMN {:02}", (self.column + 1).min(COLUMNS))
        } else {
            "NO CARD".into()
        };
        let hopper = match &self.hopper.media {
            Some(m) => format!(
                "HOPPER {} ({} OF {} CARDS LEFT, THEN BLANK)",
                m.name,
                self.hopper.cards.len() - self.hopper.next.min(self.hopper.cards.len()),
                self.hopper.cards.len()
            ),
            None => "HOPPER BLANK CARDS".into(),
        };
        let stacker = match &self.stacker.media {
            Some(m) => format!("STACKER {} CARDS -> {}", self.stacker.cards.len(), m.name),
            None => format!("STACKER {} CARDS (NO DECK FILE)", self.stacker.cards.len()),
        };
        let mut third = self
            .stacker
            .error
            .as_ref()
            .map(|e| format!("DECK FILE: {}", e.to_uppercase()))
            .unwrap_or_default();
        if !self.note.is_empty() {
            third = self.note.clone();
        }
        let status = if self.power {
            [
                format!(
                    "{col}   PROG {}   AUTO FEED {}   PRINT {}   SEQ {}",
                    Program::NAMES[usize::from(self.program.index())],
                    on_off(self.auto_feed),
                    on_off(self.print),
                    on_off(self.seq)
                ),
                format!("{hopper}   {stacker}"),
                third,
            ]
        } else {
            ["POWER OFF".into(), String::new(), String::new()]
        };
        let view = picture::View {
            card: self.station.as_ref().map(|s| (&s.columns, &s.printed)),
            column: if self.station.is_some() {
                self.column
            } else {
                COLUMNS
            },
            fields: self.program.fields(),
            status,
        };
        picture::render(&mut self.pixels, &view);
    }
}

/// What a key sends with Shift (US layout, as the IMSAI's terminal).
fn shifted(k: u8, shift: bool) -> u8 {
    if !shift {
        return k;
    }
    match k {
        b'a'..=b'z' => k - 0x20,
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b'\\' => b'|',
        b';' => b':',
        b'\'' => b'"',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        b'`' => b'~',
        other => other,
    }
}

impl Machine for Keypunch {
    fn run_frame(&mut self, _dbg: &mut DebugHooks) -> StopReason {
        self.service(self.frame_end);
        self.t = self.frame_end;
        self.frame_end += FRAME;
        self.render();
        StopReason::FrameDone
    }

    fn step_cycle(&mut self) -> StopReason {
        self.t += 1;
        self.service(self.t);
        if self.t >= self.frame_end {
            self.frame_end += FRAME;
        }
        self.render();
        StopReason::StepDone
    }

    /// Hard: power on again, the card at the station gone, the stacker
    /// kept. Soft: CLEAR.
    fn reset(&mut self, kind: ResetKind) {
        match kind {
            ResetKind::Hard => {
                self.power = true;
                self.station = None;
                self.column = 0;
                self.held = 0;
            }
            ResetKind::Soft => self.clear(),
        }
        self.input.clear();
        self.render();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: WIDTH,
            height: HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &picture::PALETTE,
            },
            luma_chroma: None,
            signal: picture::signal(FPS),
        }
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> emu_core::AudioFormat {
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        None
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: CARD_SPACE,
            name: "card",
            device: PUNCH,
            addr_bits: 8,
            big_endian: true,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        vec![ChipInfo {
            name: "029",
            regs: vec![
                reg("COLUMN", (self.column + 1) as u32, 8),
                reg("CARD", u32::from(self.station.is_some()), 1),
                reg("STACKER", self.stacker.cards.len() as u32, 32),
                reg("HOPPER", self.hopper.next as u32, 32),
                reg("PROGRAM", u32::from(self.program.index()), 2),
                reg("SEQNEXT", self.seq_next, 32),
            ],
        }]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![PUNCH]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == PUNCH).then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        Cycle(self.t)
    }

    fn controls(&self) -> Vec<ControlInfo> {
        const OFF_ON: &[&str] = &["OFF", "ON"];
        let held = |id: u16| self.held >> id & 1 != 0;
        vec![
            ControlInfo::switch(ctl::POWER, "MAIN LINE", OFF_ON, u8::from(self.power)),
            ControlInfo::button(ctl::FEED, "FEED", held(ctl::FEED)),
            ControlInfo::button(ctl::REL, "REL", held(ctl::REL)),
            ControlInfo::button(ctl::BACKSPACE, "BACKSPACE", held(ctl::BACKSPACE)),
            ControlInfo::button(ctl::SKIP, "SKIP", held(ctl::SKIP)),
            ControlInfo::button(ctl::CLEAR, "CLEAR", held(ctl::CLEAR)),
            ControlInfo::switch(
                ctl::AUTO_FEED,
                "AUTO FEED",
                OFF_ON,
                u8::from(self.auto_feed),
            ),
            ControlInfo::switch(ctl::PRINT, "PRINT", OFF_ON, u8::from(self.print)),
            ControlInfo::switch(ctl::PROGRAM, "PROG", Program::NAMES, self.program.index()),
            ControlInfo::switch(ctl::SEQ, "SEQ", OFF_ON, u8::from(self.seq)),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        Vec::new()
    }

    fn connect(&mut self, port: PortId, _dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        Err(PortError::NoSuchPort(port))
    }

    fn disconnect(&mut self, _port: PortId) -> Option<Box<dyn PortDevice>> {
        None
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        vec![
            MediaSlotInfo {
                id: SLOT_HOPPER,
                name: "hopper",
                kind: MediaKind::Cards,
                inserted: self.hopper.media.is_some(),
            },
            MediaSlotInfo {
                id: SLOT_STACKER,
                name: "stacker",
                kind: MediaKind::Cards,
                inserted: self.stacker.media.is_some(),
            },
        ]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_HOPPER => Some(&mut self.hopper),
            SLOT_STACKER => Some(&mut self.stacker),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"K029", 1, |w| {
            w.u64(self.t);
            w.u64(self.frame_end);
            w.bool(self.power);
            w.bool(self.station.is_some());
            if let Some(s) = &self.station {
                s.save(w);
            }
            w.u8(self.column as u8);
            w.bool(self.auto_feed);
            w.bool(self.print);
            w.u8(self.program.index());
            w.bool(self.seq);
            w.u32(self.seq_next);
            save_cards(w, &self.hopper.cards);
            w.u32(self.hopper.next as u32);
            save_cards(w, &self.stacker.cards);
            w.bool(self.shift);
            w.bool(self.ctrl);
            w.u16(self.held);
        });
        self.input.save(&mut w);
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"K029", 1)?;
        let t = c.u64()?;
        let frame_end = c.u64()?;
        if frame_end < t || frame_end > t + FRAME {
            return Err(StateError::Invalid("frame position"));
        }
        let power = c.bool()?;
        let station = if c.bool()? {
            Some(Station::load(&mut c)?)
        } else {
            None
        };
        let column = usize::from(c.u8()?);
        if column > COLUMNS {
            return Err(StateError::Invalid("column"));
        }
        let auto_feed = c.bool()?;
        let print = c.bool()?;
        let program = Program::from_index(c.u8()?).ok_or(StateError::Invalid("program"))?;
        let seq = c.bool()?;
        let seq_next = c.u32()?;
        let hopper_cards = load_cards(&mut c)?;
        let hopper_next = (c.u32()? as usize).min(hopper_cards.len());
        let stacked = load_cards(&mut c)?;
        let shift = c.bool()?;
        let ctrl = c.bool()?;
        let held = c.u16()?;
        let mut input = self.input.clone();
        input.load(&mut r)?;
        self.t = t;
        self.frame_end = frame_end;
        self.power = power;
        self.station = station;
        self.column = column;
        self.auto_feed = auto_feed;
        self.print = print;
        self.program = program;
        self.seq = seq;
        self.seq_next = seq_next;
        self.hopper.cards = hopper_cards;
        self.hopper.next = hopper_next;
        self.stacker.cards = stacked;
        self.stacker.write_out();
        self.shift = shift;
        self.ctrl = ctrl;
        self.held = held;
        self.input = input;
        self.render();
        Ok(())
    }
}

impl DebugTarget for Keypunch {
    fn regs(&self) -> RegisterSet {
        RegisterSet(vec![
            Register {
                name: "COL",
                value: (self.column + 1) as u32,
                bits: 8,
            },
            Register {
                name: "CARDS",
                value: self.stacker.cards.len() as u32,
                bits: 32,
            },
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        if name.eq_ignore_ascii_case("COL") && self.station.is_some() {
            self.column = (v.max(1) as usize - 1).min(COLUMNS);
        }
    }

    /// The card at the station in the binary deck layout (a column a
    /// big-endian word, left-justified).
    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != CARD_SPACE {
            return None;
        }
        let s = self.station.as_ref()?;
        let byte = |a: u32| -> u8 {
            let col = (a / 2) as usize;
            let word = s.columns.get(col).map_or(0, |c| c << 4);
            if a.is_multiple_of(2) {
                (word >> 8) as u8
            } else {
                word as u8
            }
        };
        Some((0..size.bytes()).fold(0, |v, i| v << 8 | u32::from(byte(addr + i))))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CARD_SPACE {
            return false;
        }
        let Some(s) = &mut self.station else {
            return false;
        };
        let n = size.bytes();
        for i in 0..n {
            let a = addr + i;
            let byte = (v >> (8 * (n - 1 - i))) as u8;
            let Some(c) = s.columns.get_mut((a / 2) as usize) else {
                return false;
            };
            let word = *c << 4;
            let word = if a.is_multiple_of(2) {
                word & 0x00ff | u16::from(byte) << 8
            } else {
                word & 0xff00 | u16::from(byte)
            };
            *c = word >> 4;
        }
        true
    }

    /// A column: its holes and the character they stand for.
    fn disassemble(&self, addr: u32) -> Instr {
        let col = (addr / 2) as usize;
        let code = self
            .station
            .as_ref()
            .and_then(|s| s.columns.get(col).copied())
            .unwrap_or(0);
        let ch = cards::character(code).map_or_else(|| "?".to_string(), String::from);
        Instr {
            addr: addr & !1,
            bytes: (code << 4).to_be_bytes().to_vec(),
            mnemonic: format!("COL{:02}", col + 1),
            operand: format!("{code:03X} '{ch}'"),
            target: None,
            flow: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests;
