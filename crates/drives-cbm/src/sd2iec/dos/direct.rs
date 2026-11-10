//! Direct access: buffers opened with `#` (256 bytes, pointer at 1) or
//! `##n` (n × 256 bytes, pointer at 0), block read and write (`U1`/`UA`,
//! `U2`/`UB`, `B-R`, `B-W`), the buffer pointer (`B-P`, with the high byte
//! as third argument) and the BAM (`B-A`, `B-F`). Blocks exist only in a
//! mounted disk image (sd2iec README, "U1/U2/B-R/B-W", "B-P", "Large
//! buffers"); like the sd2iec, sectors of tracks the format does not list
//! are reached if the image holds them.
//!
//! As on CBM drives, `B-W` stores the pointer of the last written byte in
//! byte 0 of the block and `B-R` stops reading after the byte that byte 0
//! names (*Inside Commodore DOS*, block commands).

use super::{
    numbers, Channel, DosResult, Sd2iecDos, ILLEGAL_TS, NOT_READY, NO_BLOCK, NO_CHANNEL, OK,
    PROTECTED,
};

/// `OPEN n,dev,sa,"#"` (any buffer number after `#` is taken as a wish
/// and ignored) or `"##d"`: a large buffer of `d` blocks.
pub(super) fn open_buffer(name: &[u8]) -> Channel {
    match name {
        [b'#', b'#', d @ b'1'..=b'9'] => {
            let len = usize::from(d - b'0') * 256;
            Channel::Buffer {
                data: vec![0; len],
                pos: 0,
                end: len,
            }
        }
        _ => Channel::Buffer {
            data: vec![0; 256],
            pos: 1,
            end: 256,
        },
    }
}

/// A block's error code from the image's error info (1 = none) as a DOS
/// status: 2–11 are the 1541's errors 20–29.
fn read_error(code: u8) -> Option<u8> {
    (2..=11).contains(&code).then(|| code + 18)
}

const ERROR_TEXTS: [&str; 10] = [
    "READ ERROR",
    "READ ERROR",
    "READ ERROR",
    "READ ERROR",
    "READ ERROR",
    "WRITE ERROR",
    "WRITE PROTECT ON",
    "READ ERROR",
    "WRITE ERROR",
    "DISK ID MISMATCH",
];

impl Sd2iecDos {
    /// The buffer channel `sa` names, or `70,NO CHANNEL`.
    fn buffer(&mut self, sa: u32) -> DosResult<(&mut Vec<u8>, &mut usize, &mut usize)> {
        let ch = usize::try_from(sa)
            .ok()
            .filter(|&c| c < 15)
            .ok_or(NO_CHANNEL)?;
        match &mut self.channels[ch] {
            Channel::Buffer { data, pos, end } => Ok((data, pos, end)),
            _ => Err(NO_CHANNEL),
        }
    }

    /// `U1:sa,drive,track,sector` / `B-R`: the block into the buffer.
    pub(super) fn block_read(&mut self, args: &[u8], b_r: bool) -> DosResult<()> {
        let [sa, _drive, track, sector, ..] = numbers(args)[..] else {
            return Err(super::SYNTAX);
        };
        self.buffer(sa)?;
        let (t, s) = (track as u8, sector as u8);
        let image = self.image().map_err(|_| NOT_READY)?;
        let Some(block) = image
            .sector(t, s)
            .filter(|_| track <= 255 && sector <= 255)
            .map(<[u8]>::to_vec)
        else {
            self.set_status(ILLEGAL_TS.0, ILLEGAL_TS.1, t, s);
            return Ok(());
        };
        let error = image.error(t, s);
        let (data, pos, end) = self.buffer(sa)?;
        data[..256].copy_from_slice(&block);
        if b_r {
            *pos = 1;
            *end = usize::from(block[0]) + 1;
        } else {
            *pos = 0;
            *end = data.len();
        }
        match read_error(error) {
            Some(code) => self.set_status(code, ERROR_TEXTS[usize::from(code - 20)], t, s),
            None => self.set(OK),
        }
        Ok(())
    }

    /// `U2:sa,drive,track,sector` / `B-W`: the buffer onto the block.
    pub(super) fn block_write(&mut self, args: &[u8], b_w: bool) -> DosResult<()> {
        let [sa, _drive, track, sector, ..] = numbers(args)[..] else {
            return Err(super::SYNTAX);
        };
        let block = {
            let (data, pos, _) = self.buffer(sa)?;
            if b_w {
                data[0] = pos.saturating_sub(1).min(255) as u8;
            }
            data[..256].to_vec()
        };
        if self.protected {
            return Err(PROTECTED);
        }
        let (t, s) = (track as u8, sector as u8);
        let image = self.image().map_err(|_| NOT_READY)?;
        let Some(target) = image
            .sector_mut(t, s)
            .filter(|_| track <= 255 && sector <= 255)
        else {
            self.set_status(ILLEGAL_TS.0, ILLEGAL_TS.1, t, s);
            return Ok(());
        };
        target.copy_from_slice(&block);
        self.write_back()?;
        self.set(OK);
        Ok(())
    }

    /// `B-P:sa,pos[,high]`.
    pub(super) fn buffer_pointer(&mut self, args: &[u8]) -> DosResult<()> {
        let n = numbers(args);
        let (&sa, &lo) = (n.first().ok_or(super::SYNTAX)?, n.get(1).unwrap_or(&0));
        let hi = n.get(2).copied().unwrap_or(0);
        let (data, pos, end) = self.buffer(sa)?;
        *pos = ((lo & 0xff) as usize + 256 * (hi & 0xff) as usize).min(data.len());
        *end = data.len();
        self.set(OK);
        Ok(())
    }

    /// `B-A:drive,track,sector` (`allocate`) or `B-F`: a block of the
    /// mounted image's BAM. Allocating a used block answers `65,NO BLOCK`
    /// with the next free one (00,00: none).
    pub(super) fn block_allocate(&mut self, args: &[u8], allocate: bool) -> DosResult<()> {
        let [_drive, track, sector, ..] = numbers(args)[..] else {
            return Err(super::SYNTAX);
        };
        if self.protected {
            return Err(PROTECTED);
        }
        let (t, s) = (track as u8, sector as u8);
        let image = self.image().map_err(|_| NOT_READY)?;
        if image.sector(t, s).is_none() || track > 255 || sector > 255 {
            self.set_status(ILLEGAL_TS.0, ILLEGAL_TS.1, t, s);
            return Ok(());
        }
        if allocate {
            if !image.block_free(t, s) {
                let (nt, ns) = image.next_free_block(t, s).unwrap_or((0, 0));
                self.set_status(NO_BLOCK.0, NO_BLOCK.1, nt, ns);
                return Ok(());
            }
            image.allocate_block(t, s);
        } else {
            image.free_block(t, s);
        }
        self.write_back()?;
        self.set(OK);
        Ok(())
    }
}
