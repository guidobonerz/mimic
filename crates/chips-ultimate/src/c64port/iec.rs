//! The IEC processor, `$028000`-`$028FFF`: a small microcoded engine in
//! the FPGA that speaks the serial bus for the firmware's software drive
//! and printer. Here it is present and silent.
//!
//! | Addr | Name | Bits |
//! |---|---|---|
//! | `$028000` | VERSION | R `$25` |
//! | `$028001` | TX_FIFO_STATUS | R bit 0 empty, bit 1 full. **`$01` here** |
//! | `$028002` | RX_FIFO_STATUS | R bit 0 empty, bit 1 full, bit 7 the head is a control code. **`$01` here**: with 0 the firmware's server task would take 500 bytes from the empty queue on every round |
//! | `$028003` | RESET_ENABLE | W bit 0: 1 run, 0 hold; every write resets the engine and empties both queues |
//! | `$028006` | RX_DATA | R, takes the head off |
//! | `$028007` | RX_CTRL | R bit 0: the head is a control code |
//! | `$028008` | RX_DATA_32 / TX_DATA | R takes one byte per byte read; W queues a data byte |
//! | `$028009` | TX_CTRL | W queues a control byte |
//! | `$02800A` | TX_LAST | W queues a data byte with EOI |
//! | `$02800C` | IRQ | R bit 0 status; W clears it, bit 0 enables |
//! | `$02800D` | TX_FIFO_RELEASE | W |
//! | `$028800`-`$028FFF` | code memory | the microcode, 32-bit words; the firmware loads `$768` bytes |
//!
//! The firmware writes the bus addresses of its devices into the loaded
//! microcode. Its loop over four devices finds only three places and so
//! writes one byte in front of the code memory, `$0287FF`: that write is
//! accepted and dropped, as in the hardware.
//!
//! The engine has an interrupt output that the firmware does not use.
//!
//! To make it work later: keep the code memory, read the device numbers
//! from the patched bytes ([`IecProcessor::device_numbers`]), act as those
//! devices on the C64's serial bus byte by byte, and fill the receive
//! queue with data bytes and the control codes the firmware's server
//! expects (attention begin/end, ready to send, EOI, device addressed);
//! the transmit queue then carries data, "last byte" and control codes.
//!
//! Source: UE2-C64U-Emulator `docs/hw/11-drives-iec-periph.md` (address
//! map "IEC processor", hazards H10-H12, functional model, the T0 row).

use mimic_core::{StateError, StateReader, StateWriter};

/// VERSION.
pub const VERSION: u8 = 0x25;
/// A queue that is empty and not full.
const FIFO_EMPTY: u8 = 0x01;

const CODE_BYTES: usize = 0x800;

/// Offsets in the code memory of the (listener, talker) address bytes of
/// the three devices the microcode knows.
const SLOTS: [(usize, usize); 3] = [(0x44, 0x28), (0x7c, 0x60), (0xb4, 0x98)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IecProcessor {
    /// RESET_ENABLE bit 0: the engine runs.
    pub enabled: bool,
    /// IRQ enable (bit 0 of the IRQ register).
    pub irq_enable: bool,
    code: Vec<u8>,
}

impl Default for IecProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl IecProcessor {
    pub fn new() -> Self {
        Self {
            enabled: false,
            irq_enable: false,
            code: vec![0; CODE_BYTES],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `off` counts from `$028000`.
    pub fn read(&self, off: u32) -> u8 {
        if off & 0x800 != 0 {
            return self.code[(off & 0x7ff) as usize];
        }
        match off & 0x7ff {
            0 => VERSION,
            1 | 2 => FIFO_EMPTY,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        if off & 0x800 != 0 {
            self.code[(off & 0x7ff) as usize] = v;
            return;
        }
        match off & 0x7ff {
            3 => self.enabled = v & 1 != 0,
            0xc => self.irq_enable = v & 1 != 0,
            // The transmit queue, its release and the stray write at
            // `$7FF`: nothing listens.
            _ => {}
        }
    }

    /// The code memory as loaded.
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    /// The bus addresses the firmware gave its three devices, `None` for
    /// one that is switched off (address 31).
    pub fn device_numbers(&self) -> [Option<u8>; 3] {
        SLOTS.map(|(listen, _)| {
            let n = self.code[listen] & 0x1f;
            (self.code[listen] & 0xe0 == 0x20 && n != 0x1f).then_some(n)
        })
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bool(self.enabled);
        w.bool(self.irq_enable);
        w.raw(&self.code);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.enabled = r.bool()?;
        self.irq_enable = r.bool()?;
        r.raw_into(&mut self.code)
    }
}
