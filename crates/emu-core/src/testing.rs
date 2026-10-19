//! Helpers for tests of CPU cores and other bus masters.

use crate::bus::{Bus, Size};

/// Flat RAM covering the whole address space, without side effects or
/// mirroring. Multi-byte accesses are little-endian and wrap at the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlatBus {
    pub mem: Vec<u8>,
    addr_bits: u8,
}

impl FlatBus {
    /// `addr_bits` up to 24 (16 MiB).
    pub fn new(addr_bits: u8) -> Self {
        let addr_bits = addr_bits.min(24);
        Self {
            mem: vec![0; 1 << addr_bits],
            addr_bits,
        }
    }

    /// Copy `data` to `addr`, wrapping at the top of the address space.
    pub fn load(&mut self, addr: u32, data: &[u8]) {
        let mask = self.mask();
        for (i, &b) in data.iter().enumerate() {
            self.mem[(addr as usize).wrapping_add(i) & mask] = b;
        }
    }

    fn mask(&self) -> usize {
        self.mem.len() - 1
    }
}

impl Bus for FlatBus {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        let mask = self.mask();
        (0..size.bytes()).fold(0, |v, i| {
            let b = self.mem[(addr as usize).wrapping_add(i as usize) & mask];
            v | (u32::from(b) << (i * 8))
        })
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        let mask = self.mask();
        for i in 0..size.bytes() {
            self.mem[(addr as usize).wrapping_add(i as usize) & mask] = (v >> (i * 8)) as u8;
        }
    }

    fn addr_bits(&self) -> u8 {
        self.addr_bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_and_wrapping() {
        let mut bus = FlatBus::new(16);
        bus.write(0xffff, Size::Word, 0x1234);
        assert_eq!(bus.mem[0xffff], 0x34);
        assert_eq!(bus.mem[0x0000], 0x12);
        assert_eq!(bus.read(0xffff, Size::Word), 0x1234);
        bus.load(0x0400, &[1, 2, 3, 4]);
        assert_eq!(bus.peek(0x0400, Size::Long), 0x0403_0201);
        assert_eq!(bus.addr_bits(), 16);
    }
}
