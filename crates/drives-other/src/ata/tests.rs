use super::*;
use emu_core::media::Media;

/// A backend in memory (the tests' image).
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
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        self.0[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// An image whose block n starts with n (big-endian long) and "BLK".
fn image(blocks: usize) -> Vec<u8> {
    let mut d = vec![0u8; blocks * BLOCK];
    for n in 0..blocks {
        d[n * BLOCK..n * BLOCK + 4].copy_from_slice(&(n as u32).to_be_bytes());
        d[n * BLOCK + 4..n * BLOCK + 7].copy_from_slice(b"BLK");
    }
    d
}

fn channel(blocks: usize) -> Ata {
    let mut a = Ata::new();
    a.disks[0]
        .insert(Media {
            name: "hd.hdf".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(image(blocks))),
        })
        .unwrap();
    a
}

fn read_block(a: &mut Ata) -> Vec<u8> {
    (0..BLOCK / 2)
        .flat_map(|_| a.read_data().to_le_bytes())
        .collect()
}

#[test]
fn an_empty_channel_reads_ff_and_a_missing_slave_reads_0() {
    let mut a = Ata::new();
    assert_eq!(a.read(7), 0xff);
    assert_eq!(a.alt_status(), 0xff);
    assert_eq!(a.read_data(), 0xffff);
    let mut a = channel(100);
    assert_eq!(a.read(7), status::DRDY | status::DSC);
    a.write(6, 0x10);
    assert_eq!(a.read(7), 0, "device 1 is missing");
    a.write(7, 0xec);
    assert!(!a.irq(), "no command reaches a missing device");
}

#[test]
fn identify_gives_the_geometry_and_raises_intrq() {
    let mut a = channel(5000);
    a.write(6, 0xa0);
    a.write(7, 0xec);
    assert!(a.irq());
    assert_eq!(a.alt_status() & status::DRQ, status::DRQ);
    assert!(a.irq(), "the alternate status keeps INTRQ");
    assert_eq!(a.read(7) & status::DRQ, status::DRQ);
    assert!(!a.irq(), "the status read clears INTRQ");
    let words: Vec<u16> = (0..256).map(|_| a.read_data()).collect();
    assert_eq!(words[0], 0x0040);
    assert_eq!((words[1], words[3], words[6]), (5, 16, 63)); // 5000 / 1008 up
    assert_eq!(u32::from(words[61]) << 16 | u32::from(words[60]), 5000);
    let model: Vec<u8> = words[27..47].iter().flat_map(|w| w.to_be_bytes()).collect();
    assert!(model.starts_with(b"MIMIC HARD DISK"));
    assert_eq!(a.read(7) & status::DRQ, 0, "the block is read");
}

#[test]
fn read_sectors_by_chs_and_lba_continue_through_the_count() {
    let mut a = channel(3000);
    // CHS: cylinder 1, head 2, sector 5 = (1·16 + 2)·63 + 4 = 1138.
    a.write(2, 2);
    a.write(3, 5);
    a.write(4, 1);
    a.write(5, 0);
    a.write(6, 0xa2);
    a.write(7, 0x20);
    assert!(a.irq());
    a.read(7);
    let b = read_block(&mut a);
    assert_eq!(&b[..7], &[0, 0, 0x04, 0x72, b'B', b'L', b'K']);
    assert!(a.irq(), "the second block is ready");
    a.read(7);
    let b = read_block(&mut a);
    assert_eq!(u32::from_be_bytes([b[0], b[1], b[2], b[3]]), 1139);
    assert_eq!(a.read(7), status::DRDY | status::DSC);
    assert_eq!(a.sector, 6, "the task file points at the last block");
    // LBA 2999, the last block.
    a.write(2, 1);
    a.write(3, 2999u32 as u8);
    a.write(4, (2999 >> 8) as u8);
    a.write(5, 0);
    a.write(6, 0xe0);
    a.write(7, 0x20);
    let b = read_block(&mut a);
    assert_eq!(u32::from_be_bytes([b[0], b[1], b[2], b[3]]), 2999);
}

#[test]
fn bad_addresses_end_with_idnf() {
    let mut a = channel(100);
    a.write(2, 1);
    a.write(3, 0); // sector 0 does not exist
    a.write(6, 0xa0);
    a.write(7, 0x20);
    assert_eq!(a.read(7) & status::ERR, status::ERR);
    assert_eq!(a.read(1) & error::IDNF, error::IDNF);
    a.write(3, 0xff);
    a.write(4, 0xff);
    a.write(5, 0xff);
    a.write(6, 0xef); // LBA far past the end
    a.write(7, 0x20);
    assert_eq!(a.read(1), error::IDNF);
    a.write(7, 0xa1); // IDENTIFY PACKET DEVICE: not ATAPI
    assert_eq!(a.read(1), error::ABRT);
}

#[test]
fn write_sectors_reach_the_image() {
    let mut a = channel(100);
    a.write(2, 2);
    a.write(3, 10);
    a.write(4, 0);
    a.write(5, 0);
    a.write(6, 0xe0);
    a.write(7, 0x30);
    assert!(!a.irq(), "the first block is wanted without an interrupt");
    assert_eq!(a.read(7) & status::DRQ, status::DRQ);
    for blk in 0..2u16 {
        for i in 0..256u16 {
            a.write_data(0xa000 | blk << 8 | i);
        }
        assert!(a.irq(), "block {blk}");
        a.read(7);
    }
    assert_eq!(a.read(7), status::DRDY | status::DSC);
    // Read them back.
    a.write(2, 2);
    a.write(3, 10);
    a.write(7, 0x20);
    let b = read_block(&mut a);
    assert_eq!(&b[..4], &[0x00, 0xa0, 0x01, 0xa0], "the low byte first");
    let b = read_block(&mut a);
    assert_eq!(&b[..4], &[0x00, 0xa1, 0x01, 0xa1]);
}

#[test]
fn multiple_mode_moves_blocks_a_drq() {
    let mut a = channel(100);
    a.write(6, 0xe0);
    a.write(2, 4);
    a.write(7, 0xc4);
    assert_eq!(a.read(1), error::ABRT, "multiple mode is off");
    a.write(2, 3);
    a.write(7, 0xc6);
    assert_eq!(a.read(1), error::ABRT, "3 is no power of two");
    a.write(2, 4);
    a.write(7, 0xc6);
    assert_eq!(a.read(7), status::DRDY | status::DSC);
    a.write(2, 6);
    a.write(3, 20);
    a.write(4, 0);
    a.write(5, 0);
    a.write(7, 0xc4);
    a.read(7);
    for n in 20..24 {
        let b = read_block(&mut a);
        assert_eq!(b[3], n);
    }
    assert!(a.irq(), "the second chunk: two blocks");
    a.read(7);
    for n in 24..26 {
        let b = read_block(&mut a);
        assert_eq!(b[3], n);
    }
    assert_eq!(a.read(7) & status::DRQ, 0);
}

#[test]
fn initialize_device_parameters_changes_the_translation() {
    let mut a = channel(2000);
    a.write(2, 32); // 32 sectors
    a.write(6, 0xa0 | 7); // 8 heads
    a.write(7, 0x91);
    // Cylinder 2, head 3, sector 4 = (2·8 + 3)·32 + 3 = 611.
    a.write(2, 1);
    a.write(3, 4);
    a.write(4, 2);
    a.write(5, 0);
    a.write(6, 0xa3);
    a.write(7, 0x20);
    let b = read_block(&mut a);
    assert_eq!(u32::from_be_bytes([b[0], b[1], b[2], b[3]]), 611);
}

#[test]
fn nien_masks_intrq_and_srst_gives_the_signature() {
    let mut a = channel(100);
    a.write_control(2);
    a.write(7, 0xe7);
    assert!(!a.irq());
    a.write_control(0);
    assert!(a.irq());
    a.write(2, 0x55);
    a.write_control(4);
    assert_eq!(a.alt_status() & status::BSY, status::BSY);
    a.write_control(0);
    assert_eq!(a.read(7), status::DRDY | status::DSC);
    assert_eq!((a.read(1), a.read(2), a.read(3)), (1, 1, 1));
}

#[test]
fn savestates_keep_a_transfer_in_flight() {
    let mut a = channel(100);
    a.write(2, 2);
    a.write(3, 7);
    a.write(6, 0xe0);
    a.write(7, 0x20);
    for _ in 0..100 {
        a.read_data();
    }
    let mut w = StateWriter::new();
    a.save(&mut w);
    let bytes = w.into_bytes();
    let mut b = channel(100);
    b.load(&mut StateReader::new(&bytes)).unwrap();
    let rest_a: Vec<u16> = (0..412).map(|_| a.read_data()).collect();
    let rest_b: Vec<u16> = (0..412).map(|_| b.read_data()).collect();
    assert_eq!(rest_a, rest_b);
    assert!(Ata::new()
        .load(&mut StateReader::new(&bytes[..10]))
        .is_err());
}
