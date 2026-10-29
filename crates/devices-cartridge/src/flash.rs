//! AMD Am29F040 512 KB flash memory (EasyFlash has two): reads return the
//! array; programming and erasing go through command cycles, each started
//! by the unlock writes `$AA` to `$555` and `$55` to `$2AA` (only address
//! bits A0–A10 count). Commands: `$A0` programs the next byte written (bits
//! can only go from 1 to 0); `$80`, a second unlock and `$10` erase the
//! chip; `$80`, a second unlock and `$30` at an address erase the 64 KB
//! sector there; `$90` enters autoselect (address 0 reads the manufacturer
//! `$01`, 1 the device `$A4`); `$F0` returns to reading. Operations complete
//! at once (status polling then sees the final data). Source: AMD Am29F040B
//! data sheet.

use emu_core::{StateError, StateReader, StateWriter};

pub const SIZE: usize = 0x8_0000;
const SECTOR: usize = 0x1_0000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Read,
    Unlock1,
    Unlock2,
    Program,
    Erase,
    EraseUnlock1,
    EraseUnlock2,
    Autoselect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flash {
    pub data: Vec<u8>,
    state: State,
    /// Something was programmed or erased since the image was loaded.
    pub modified: bool,
}

impl Flash {
    /// A chip holding `data` (padded with erased `$FF`).
    pub fn new(mut data: Vec<u8>) -> Self {
        data.resize(SIZE, 0xff);
        Self {
            data,
            state: State::Read,
            modified: false,
        }
    }

    pub fn peek(&self, offset: usize) -> u8 {
        let o = offset % SIZE;
        if self.state == State::Autoselect {
            return match o & 0xff {
                0 => 0x01,
                1 => 0xa4,
                _ => 0,
            };
        }
        self.data[o]
    }

    pub fn write(&mut self, offset: usize, v: u8) {
        let o = offset % SIZE;
        let a = o & 0x7ff;
        self.state = match (self.state, a, v) {
            (_, _, 0xf0) => State::Read,
            (State::Read | State::Autoselect, 0x555, 0xaa) => State::Unlock1,
            (State::Unlock1, 0x2aa, 0x55) => State::Unlock2,
            (State::Unlock2, 0x555, 0xa0) => State::Program,
            (State::Unlock2, 0x555, 0x80) => State::Erase,
            (State::Unlock2, 0x555, 0x90) => State::Autoselect,
            (State::Program, _, v) => {
                self.data[o] &= v;
                self.modified = true;
                State::Read
            }
            (State::Erase, 0x555, 0xaa) => State::EraseUnlock1,
            (State::EraseUnlock1, 0x2aa, 0x55) => State::EraseUnlock2,
            (State::EraseUnlock2, 0x555, 0x10) => {
                self.data.fill(0xff);
                self.modified = true;
                State::Read
            }
            (State::EraseUnlock2, _, 0x30) => {
                let start = o / SECTOR * SECTOR;
                self.data[start..start + SECTOR].fill(0xff);
                self.modified = true;
                State::Read
            }
            (State::Autoselect, _, _) => State::Autoselect,
            _ => State::Read,
        };
    }

    pub fn reset(&mut self) {
        self.state = State::Read;
    }

    /// Erase the 64 KB sector holding `offset` from outside the command
    /// cycles (a programmer, the Ultimate's EASYFLASH_ERASE).
    pub fn erase_sector(&mut self, offset: usize) {
        let start = offset % SIZE / SECTOR * SECTOR;
        self.data[start..start + SECTOR].fill(0xff);
        self.modified = true;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.state as u8);
    }

    pub fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        use State::*;
        self.state = *[
            Read,
            Unlock1,
            Unlock2,
            Program,
            Erase,
            EraseUnlock1,
            EraseUnlock2,
            Autoselect,
        ]
        .get(usize::from(c.u8()?))
        .ok_or(StateError::Invalid("flash state"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlock(f: &mut Flash) {
        f.write(0x555, 0xaa);
        f.write(0x2aa, 0x55);
    }

    #[test]
    fn program_erase_autoselect() {
        let mut f = Flash::new(vec![0xff; 16]);
        unlock(&mut f);
        f.write(0x555, 0xa0);
        f.write(0x12345, 0x5a);
        assert_eq!(f.peek(0x12345), 0x5a);
        assert!(f.modified);
        // Without the unlock a write does nothing.
        f.write(0x12346, 0x00);
        assert_eq!(f.peek(0x12346), 0xff);
        // Sector erase clears the 64 KB around it.
        unlock(&mut f);
        f.write(0x555, 0x80);
        unlock(&mut f);
        f.write(0x10000, 0x30);
        assert_eq!(f.peek(0x12345), 0xff);
        unlock(&mut f);
        f.write(0x555, 0x90);
        assert_eq!((f.peek(0), f.peek(1)), (0x01, 0xa4));
        f.write(0, 0xf0);
        assert_eq!(f.peek(0), 0xff);
    }
}
