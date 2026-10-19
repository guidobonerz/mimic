//! Memory bus abstraction shared by every CPU and machine.

/// Width of a bus access. 8-bit machines only ever use [`Size::Byte`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Size {
    Byte,
    Word,
    Long,
}

impl Size {
    /// Number of bytes covered by an access of this size.
    pub const fn bytes(self) -> u32 {
        match self {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        }
    }

    /// Mask selecting the valid bits of a value of this size.
    pub const fn mask(self) -> u32 {
        match self {
            Size::Byte => 0xff,
            Size::Word => 0xffff,
            Size::Long => 0xffff_ffff,
        }
    }
}

/// The address/data bus as seen by a bus master (CPU, DMA, debugger).
pub trait Bus {
    /// Guest read. May have side effects (e.g. reading the CIA ICR clears it).
    fn read(&mut self, addr: u32, size: Size) -> u32;

    /// Inspection read. MUST be side-effect free; debugger, preview and any
    /// inspection code use this exclusively (golden rule 2).
    fn peek(&self, addr: u32, size: Size) -> u32;

    /// Guest write.
    fn write(&mut self, addr: u32, size: Size, v: u32);

    /// Width of the address bus: 16 for 8-bit machines, 24 for the Amiga.
    fn addr_bits(&self) -> u8;
}
