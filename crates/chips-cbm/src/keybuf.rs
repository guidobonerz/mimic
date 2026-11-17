//! The KERNAL's keyboard buffer, filled directly for pasted text.
//!
//! Every Commodore 8-bit KERNAL keeps the keys its interrupt scan found in
//! a short queue in RAM: a byte that counts the waiting characters (NDX)
//! and the characters themselves behind one another (KEYD), PETSCII codes.
//! The screen editor and GETIN take them from the front. Text put there
//! reaches BASIC as fast as the editor reads, several hundred characters a
//! second, where the keyboard matrix allows about twelve.
//!
//! The buffer is filled only while it is empty: the KERNAL takes a
//! character out by moving the others down and decrementing the count, and
//! a count changed under that would lose characters.
//!
//! | Machine            | NDX     | KEYD    | Size |
//! | ------------------ | ------- | ------- | ---- |
//! | PET BASIC 1.0      | `$020D` | `$020F` | 9    |
//! | PET BASIC 2.0, 4.0 | `$009E` | `$026F` | 9    |
//! | VIC-20, C64        | `$00C6` | `$0277` | 10   |
//! | C16, Plus/4        | `$00EF` | `$0527` | 10   |
//! | C128               | `$00D0` | `$034A` | 10   |
//!
//! Sources: Sheldon Leemon, *Mapping the Commodore 64* (locations 198 and
//! 631–640); *VIC-20 Programmer's Reference Guide*, memory map; Raeto
//! West, *Programming the PET/CBM*, memory maps of BASIC 1, 2 and 4;
//! *Commodore Plus/4 Programmer's Reference*, memory map; *Commodore 128
//! Programmer's Reference Guide*, chapter 15 (memory map); checked against
//! the running KERNALs (`tests/tests/paste_fast.rs`).

/// Where a KERNAL keeps its keyboard buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBuffer {
    /// NDX: the number of characters waiting.
    pub count: usize,
    /// KEYD: the first of them.
    pub chars: usize,
    /// Characters the buffer holds.
    pub size: usize,
}

pub const PET_BASIC1: KeyBuffer = KeyBuffer {
    count: 0x020d,
    chars: 0x020f,
    size: 9,
};
pub const PET: KeyBuffer = KeyBuffer {
    count: 0x009e,
    chars: 0x026f,
    size: 9,
};
pub const C64: KeyBuffer = KeyBuffer {
    count: 0x00c6,
    chars: 0x0277,
    size: 10,
};
pub const VIC20: KeyBuffer = C64;
pub const TED: KeyBuffer = KeyBuffer {
    count: 0x00ef,
    chars: 0x0527,
    size: 10,
};
pub const C128: KeyBuffer = KeyBuffer {
    count: 0x00d0,
    chars: 0x034a,
    size: 10,
};

/// The PETSCII code of a character of pasted text: letters in either case
/// are the unshifted letters, as the typed keys give them.
pub fn petscii(c: char) -> Option<u8> {
    match c {
        '\n' | '\r' => Some(0x0d),
        '\u{a3}' => Some(0x5c),
        'a'..='z' => Some(c.to_ascii_uppercase() as u8),
        ' '..=']' => Some(c as u8),
        _ => None,
    }
}

impl KeyBuffer {
    /// Put the first characters of `text` into the buffer in `ram` (the
    /// RAM the KERNAL's low memory lies in) if it is empty. `has_key`
    /// says whether the machine's keyboard has the character at all;
    /// others are dropped, as its typed keys drop them. Returns the
    /// characters of `text` used up, for `Machine::paste_chars`; `None`
    /// where the count is more than the buffer holds: no KERNAL keeps its
    /// keys there (another system, memory not set up yet).
    pub fn paste(
        self,
        ram: &mut [u8],
        text: &str,
        has_key: impl Fn(char) -> bool,
    ) -> Option<usize> {
        if ram.len() < self.chars + self.size || usize::from(ram[self.count]) > self.size {
            return None;
        }
        if ram[self.count] != 0 {
            return Some(0);
        }
        let code = |c: char| petscii(c).filter(|_| has_key(c));
        let mut stored = 0;
        let used = mimic_core::input::fill_key_buffer(text, self.size, code, |i, b| {
            ram[self.chars + i] = b;
            stored = i + 1;
        });
        ram[self.count] = stored as u8;
        Some(used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_buffer_takes_as_much_as_it_holds() {
        let mut ram = vec![0u8; 0x400];
        let used = C64.paste(&mut ram, "10 print\"a\"\n20 goto 10\n", |_| true);
        assert_eq!(used, Some(10));
        assert_eq!(ram[0xc6], 10);
        assert_eq!(&ram[0x277..0x281], b"10 PRINT\"A");
        // Still full: nothing more goes in.
        assert_eq!(C64.paste(&mut ram, "\"\n", |_| true), Some(0));
        assert_eq!(ram[0xc6], 10);
        // Read out by the KERNAL: the rest follows, RETURN as $0D.
        ram[0xc6] = 0;
        assert_eq!(C64.paste(&mut ram, "\"\n", |_| true), Some(2));
        assert_eq!((ram[0xc6], ram[0x277], ram[0x278]), (2, b'"', 0x0d));
    }

    #[test]
    fn characters_without_a_key_are_dropped() {
        let mut ram = vec![0u8; 0x400];
        // No code at all ("{"), and a code the keyboard has no key for.
        let used = C64.paste(&mut ram, "a{b_c", |c| c != 'b');
        assert_eq!(used, Some(5));
        assert_eq!(ram[0xc6], 2);
        assert_eq!(&ram[0x277..0x279], b"AC");
        assert_eq!(petscii('\u{a3}'), Some(0x5c));
        assert_eq!(petscii('~'), None);
    }

    #[test]
    fn no_buffer_where_the_count_cannot_be_one() {
        let mut ram = vec![0u8; 0x200];
        assert_eq!(C64.paste(&mut ram, "A", |_| true), None, "too little RAM");
        let mut ram = vec![0u8; 0x400];
        ram[0xc6] = 11;
        assert_eq!(C64.paste(&mut ram, "A", |_| true), None);
        assert_eq!(ram[0xc6], 11);
    }
}
