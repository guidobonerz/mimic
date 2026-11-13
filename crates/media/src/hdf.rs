//! Amiga hard disk images (HDF): a whole disk with a Rigid Disk Block
//! (RDB) in one of its first 16 blocks, or a single partition holding a
//! file system from its first block on (WinUAE's "hardfile" without RDB,
//! or an ADF, which is such a partition of 1760 blocks).
//!
//! Kickstart's scsi.device boots only disks with an RDB, so a partition
//! image is shown behind a virtual RDB ([`with_rdb`]): its first cylinder
//! holds the RigidDiskBlock (block 0) and one PartitionBlock (block 1)
//! naming the partition DH0, bootable, with the image's DOS type (the
//! first four bytes, "DOS\0" to "DOS\7" are in Kickstart's ROM), from the
//! next cylinder on. Writes to the virtual blocks stay in memory; writes
//! to the partition reach the image.
//!
//! Block layouts after the AmigaOS include file `devices/hardblocks.h`
//! (RigidDiskBlock, PartitionBlock) and `dos/filehandler.h` (DosEnvec),
//! from memory of them: big-endian longs; `SummedLongs` longs (64) add up
//! to 0 with the checksum at long 2.

use emu_core::media::{MediaBackend, MediaError};

pub const BLOCK: usize = 512;
/// The blocks an RDB may start in.
pub const RDB_RANGE: usize = 16;

/// `image` (at least its first 16 blocks) has an RDB.
pub fn has_rdb(image: &[u8]) -> bool {
    image
        .chunks(BLOCK)
        .take(RDB_RANGE)
        .any(|b| b.starts_with(b"RDSK") && checksum_ok(b))
}

/// The partition image's DOS type (its first long), if it has a file
/// system: "DOS" and a digit 0–7, "PFS", "SFS".
pub fn partition_dos_type(image: &[u8]) -> Option<u32> {
    let t = image.get(..4)?;
    let known = (t[..3] == *b"DOS" && t[3] <= 7) || t[..3] == *b"PFS" || t[..3] == *b"SFS";
    known.then(|| u32::from_be_bytes([t[0], t[1], t[2], t[3]]))
}

fn checksum_ok(block: &[u8]) -> bool {
    let n = long(block, 1) as usize;
    if !(2..=BLOCK / 4).contains(&n) {
        return false;
    }
    (0..n).fold(0u32, |s, i| s.wrapping_add(long(block, i))) == 0
}

fn long(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
}

fn set_long(b: &mut [u8], i: usize, v: u32) {
    b[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
}

/// Seal a block of 64 summed longs.
fn seal(b: &mut [u8]) {
    set_long(b, 2, 0);
    let sum = (0..64).fold(0u32, |s, i| s.wrapping_add(long(b, i)));
    set_long(b, 2, sum.wrapping_neg());
}

/// The geometry of a virtual RDB for `blocks` partition blocks: sectors a
/// track and the partition's cylinders (one surface), the first that
/// divides the partition evenly.
pub fn geometry(blocks: u64) -> (u32, u32) {
    let sectors = [32u32, 63, 16, 11, 8, 4, 2, 1]
        .into_iter()
        .find(|&s| blocks.is_multiple_of(u64::from(s)))
        .unwrap_or(1);
    (sectors, (blocks / u64::from(sectors)) as u32)
}

/// The virtual RDB's blocks for a partition of `blocks` blocks with
/// `dos_type`: the reserved cylinders in front of it.
pub fn rdb_blocks(blocks: u64, dos_type: u32) -> Vec<u8> {
    let (sectors, cylinders) = geometry(blocks);
    // Room for the two blocks.
    let reserved = 2u32.div_ceil(sectors);
    let total = cylinders + reserved;
    let mut head = vec![0u8; (reserved * sectors) as usize * BLOCK];
    {
        let rdsk = &mut head[..BLOCK];
        rdsk[..4].copy_from_slice(b"RDSK");
        let longs: [(usize, u32); 25] = [
            (1, 64),
            (3, 7),            // host id
            (4, BLOCK as u32), // block bytes
            (5, 0x07),         // last disk, LUN, target
            (6, u32::MAX),     // no bad block list
            (7, 1),            // the partition list
            (8, u32::MAX),     // no file systems
            (9, u32::MAX),     // no drive init code
            (16, total),       // cylinders
            (17, sectors),     // sectors a track
            (18, 1),           // heads
            (19, 1),           // interleave
            (20, total),       // park
            (24, total),       // write precompensation
            (25, total),       // reduced write
            (26, 3),           // step rate
            (32, 0),           // RDB blocks: low
            (33, reserved * sectors - 1),
            (34, reserved),  // first partition cylinder
            (35, total - 1), // last
            (36, sectors),   // blocks a cylinder
            (37, 0),
            (38, 1), // highest RDB block in use
            (10, u32::MAX),
            (11, u32::MAX),
        ];
        for (i, v) in longs {
            set_long(rdsk, i, v);
        }
        for i in 12..16 {
            set_long(rdsk, i, u32::MAX);
        }
        rdsk[160..168].copy_from_slice(b"MIMIC   ");
        rdsk[168..184].copy_from_slice(b"VIRTUAL RDB     ");
        rdsk[184..188].copy_from_slice(b"1.0 ");
        seal(rdsk);
    }
    {
        let part = &mut head[BLOCK..2 * BLOCK];
        part[..4].copy_from_slice(b"PART");
        set_long(part, 1, 64);
        set_long(part, 3, 7);
        set_long(part, 4, u32::MAX); // no next partition
        set_long(part, 5, 1); // bootable
                              // Drive name, a BCPL string at long 9.
        part[36] = 3;
        part[37..40].copy_from_slice(b"DH0");
        // DosEnvec at long 32.
        let env: [u32; 17] = [
            16,               // table size
            BLOCK as u32 / 4, // block size in longs
            0,                // sector origin
            1,                // surfaces
            1,                // sectors a block
            sectors,          // blocks a track
            2,                // reserved (boot blocks)
            0,                // pre-allocated
            0,                // interleave
            reserved,         // low cylinder
            total - 1,        // high cylinder
            30,               // buffers
            0,                // any memory
            0x0001_fe00,      // max transfer
            0x7fff_fffe,      // address mask
            0,                // boot priority
            dos_type,
        ];
        for (i, v) in env.iter().enumerate() {
            set_long(part, 32 + i, *v);
        }
        seal(part);
    }
    head
}

/// `inner` behind the virtual RDB `head`.
pub struct RdbBackend {
    head: Vec<u8>,
    inner: Box<dyn MediaBackend>,
}

impl MediaBackend for RdbBackend {
    fn len(&self) -> u64 {
        self.head.len() as u64 + self.inner.len()
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let h = self.head.len() as u64;
        let mut done = 0;
        if offset < h {
            let n = ((h - offset) as usize).min(buf.len());
            buf[..n].copy_from_slice(&self.head[offset as usize..offset as usize + n]);
            done = n;
        }
        if done < buf.len() {
            let at = offset + done as u64 - h;
            self.inner.read(at, &mut buf[done..])?;
        }
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let h = self.head.len() as u64;
        let mut done = 0;
        if offset < h {
            let n = ((h - offset) as usize).min(data.len());
            self.head[offset as usize..offset as usize + n].copy_from_slice(&data[..n]);
            done = n;
        }
        if done < data.len() {
            self.inner.write(offset + done as u64 - h, &data[done..])?;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        self.inner.commit()
    }

    fn dirty(&self) -> bool {
        self.inner.dirty()
    }
}

/// A hard disk image as Kickstart wants it: unchanged if it has an RDB or
/// no known file system, a partition image behind a virtual RDB.
pub fn with_rdb(mut inner: Box<dyn MediaBackend>) -> Result<Box<dyn MediaBackend>, MediaError> {
    let n = (inner.len() as usize).min(RDB_RANGE * BLOCK);
    let mut start = vec![0; n];
    inner.read(0, &mut start)?;
    if has_rdb(&start) {
        return Ok(inner);
    }
    let Some(dos_type) = partition_dos_type(&start) else {
        return Ok(inner);
    };
    let blocks = inner.len() / BLOCK as u64;
    Ok(Box::new(RdbBackend {
        head: rdb_blocks(blocks, dos_type),
        inner,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);

    impl MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, o: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            buf.copy_from_slice(&self.0[o as usize..o as usize + buf.len()]);
            Ok(())
        }
        fn write(&mut self, o: u64, d: &[u8]) -> Result<(), MediaError> {
            self.0[o as usize..o as usize + d.len()].copy_from_slice(d);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    #[test]
    fn a_partition_image_gets_a_valid_rdb_in_front() {
        // An ADF-sized partition with "DOS\1".
        let mut adf = vec![0u8; 1760 * BLOCK];
        adf[..4].copy_from_slice(b"DOS\x01");
        adf[880 * BLOCK] = 0x42;
        let mut b = with_rdb(Box::new(Mem(adf))).unwrap();
        // 1760 = 55 · 32: one reserved cylinder of 32 blocks.
        assert_eq!(b.len(), (1760 + 32) * BLOCK as u64);
        let mut head = vec![0; 2 * BLOCK];
        b.read(0, &mut head).unwrap();
        assert!(has_rdb(&head));
        let (rdsk, part) = head.split_at(BLOCK);
        assert!(part.starts_with(b"PART") && checksum_ok(part));
        assert_eq!(
            (long(rdsk, 16), long(rdsk, 17), long(rdsk, 18)),
            (56, 32, 1)
        );
        assert_eq!((long(part, 32 + 9), long(part, 32 + 10)), (1, 55));
        assert_eq!(long(part, 32 + 16), u32::from_be_bytes(*b"DOS\x01"));
        assert_eq!(&part[36..40], b"\x03DH0");
        // The partition's blocks follow, a read may span the border.
        let mut buf = vec![0; 2 * BLOCK];
        b.read(31 * BLOCK as u64, &mut buf).unwrap();
        assert_eq!(&buf[BLOCK..BLOCK + 4], b"DOS\x01");
        b.read((32 + 880) * BLOCK as u64, &mut buf[..1]).unwrap();
        assert_eq!(buf[0], 0x42);
        b.write(32 * BLOCK as u64 + 4, &[9]).unwrap();
        b.read(32 * BLOCK as u64, &mut buf[..5]).unwrap();
        assert_eq!(buf[4], 9);
    }

    #[test]
    fn disks_with_an_rdb_or_without_a_file_system_stay_as_they_are() {
        let mut disk = vec![0u8; 100 * BLOCK];
        let head = rdb_blocks(64, 0x444f_5303);
        disk[3 * BLOCK..4 * BLOCK].copy_from_slice(&head[..BLOCK]);
        let b = with_rdb(Box::new(Mem(disk))).unwrap();
        assert_eq!(b.len(), 100 * BLOCK as u64);
        let b = with_rdb(Box::new(Mem(vec![0u8; 100 * BLOCK]))).unwrap();
        assert_eq!(b.len(), 100 * BLOCK as u64);
        // A broken checksum is no RDB.
        let mut bad = head[..BLOCK].to_vec();
        bad[200] ^= 1;
        assert!(!has_rdb(&bad));
    }

    #[test]
    fn odd_sizes_get_a_geometry_that_divides_them() {
        assert_eq!(geometry(1760), (32, 55));
        assert_eq!(geometry(63 * 7), (63, 7));
        assert_eq!(geometry(1001), (11, 91));
        assert_eq!(geometry(997), (1, 997));
        // One sector a track: two reserved cylinders hold the two blocks.
        assert_eq!(rdb_blocks(997, 0x444f_5300).len(), 2 * BLOCK);
    }
}
