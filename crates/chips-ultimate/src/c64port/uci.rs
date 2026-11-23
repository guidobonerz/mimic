//! The firmware's side of the Ultimate Command Interface, `$044000`-
//! `$044FFF`: sixteen registers (decoded on the low four address bits
//! below `$044800`) and 2 KB of memory shared with the C64 side. Here the
//! C64 side is missing, so the block idles: no command ever arrives.
//!
//! | Addr | Name | Bits |
//! |---|---|---|
//! | `$044000` | SLOT_BASE | bits 6:1 = bits 8:3 of the C64 I/O offset: `$47` puts the four registers at `$DF1C`, `$7F` at `$DFFC`, `$07` at `$DE1C` |
//! | `$044001` | SLOT_ENABLE | W with bit 7 clear: bit 0 enables; with bit 7 set: bits 4:0 are the bus number the C64 reads. R bit 0 enabled |
//! | `$044002` | HANDSHAKE_OUT | W bit 0 new command taken, bit 1 "more data" taken, bit 2 abort taken, bit 4 answer valid (bit 5: more follows), bit 7 back to idle. R bit 7 C64 frozen, bit 6 trigger, bits 5:4 state. **Reads 0** |
//! | `$044003` | STATUSBYTE | R bit 0 new command, bit 1 data accepted, bit 2 abort, bit 3 error, bits 5:4 state, bit 6 status there, bit 7 answer there. **Reads 0** |
//! | `$044004` | COMMAND_START / IRQMASK_SET | R `$00`; W sets mask bits 2:0 |
//! | `$044005` | COMMAND_END / IRQMASK_CLEAR | R `$6F`; W clears mask bits 2:0 |
//! | `$044006`, `$044007` | RESPONSE_START, _END | R `$70`, `$DF` |
//! | `$044008`, `$044009` | STATUS_START, _END | R `$E0`, `$FF` |
//! | `$04400A` | STATUS_LENGTH | W length; R the C64's read position in the status, low byte: `$00` at rest |
//! | `$04400B` | IRQMASK | bits 2:0, power-on 7 |
//! | `$04400C`, `$04400D` | RESPONSE_LEN | W length (11 bits); R the C64's read position in the answer: `$80`, `$03` at rest |
//! | `$04400E`, `$04400F` | COMMAND_LEN | R bytes the C64 wrote: 0 |
//! | `$044800`-`$044FFF` | memory | command at `$044800` (896 bytes), answer at `$044B80` (896), status at `$044F00` (256) |
//!
//! The start registers are in units of 8 bytes; the firmware computes its
//! three buffer addresses from them when it starts, so they must be right
//! even while nothing else works.
//!
//! Interrupt: the ITU's low source 4 is a level, the request bits of the
//! status byte that the mask lets through. There are no requests here, so
//! the port never raises it.
//!
//! To make it work later (the plan is mimic's existing command interface
//! of the C64): the C64 side needs its four registers in I/O 2 (control /
//! status, command write, answer read, status read), the shared memory
//! with the three pointers, the handshake state machine behind
//! HANDSHAKE_OUT and STATUSBYTE, the freeze of the C64 for the DMA
//! commands, and from the port the level on source 4 and the events "C64
//! was reset" (source 7) and "unlock sequence seen" (high source 6).
//!
//! Source: UE2-C64U-Emulator `docs/hw/11-drives-iec-periph.md` (address
//! map "UltiCommand interface", hazards H6, H7, H14, the T0 row).

use mimic_core::{StateError, StateReader, StateWriter};

const RAM_BYTES: usize = 0x800;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uci {
    pub slot_base: u8,
    pub enabled: bool,
    /// The bus number the C64 reads (5 bits).
    pub bus_id: u8,
    /// 3 bits.
    pub irq_mask: u8,
    pub status_length: u8,
    /// 11 bits.
    pub response_length: u16,
    ram: Vec<u8>,
}

impl Default for Uci {
    fn default() -> Self {
        Self::new()
    }
}

impl Uci {
    pub fn new() -> Self {
        Self {
            slot_base: 0,
            enabled: false,
            bus_id: 0,
            irq_mask: 7,
            status_length: 0,
            response_length: 0,
            ram: vec![0; RAM_BYTES],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `off` counts from `$044000`.
    pub fn read(&self, off: u32) -> u8 {
        let off = off & 0xfff;
        if off >= 0x800 {
            return self.ram[(off & 0x7ff) as usize];
        }
        match off & 0xf {
            0x0 => self.slot_base,
            0x1 => u8::from(self.enabled),
            0x5 => 0x6f,
            0x6 => 0x70,
            0x7 => 0xdf,
            0x8 => 0xe0,
            0x9 => 0xff,
            0xb => self.irq_mask,
            // The answer's read position at rest: `$380`.
            0xc => 0x80,
            0xd => 0x03,
            // Handshake, status byte, command start, the status's read
            // position (`$700`, low byte), command length.
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        let off = off & 0xfff;
        if off >= 0x800 {
            self.ram[(off & 0x7ff) as usize] = v;
            return;
        }
        match off & 0xf {
            0x0 => self.slot_base = v,
            0x1 => {
                if v & 0x80 == 0 {
                    self.enabled = v & 1 != 0;
                } else {
                    self.bus_id = v & 0x1f;
                }
            }
            0x4 => self.irq_mask |= v & 7,
            0x5 => self.irq_mask &= !(v & 7),
            0xa => self.status_length = v,
            0xc => self.response_length = (self.response_length & 0x700) | u16::from(v),
            0xd => self.response_length = (self.response_length & 0xff) | (u16::from(v & 7) << 8),
            // The handshake strobes: there is nothing to acknowledge.
            _ => {}
        }
    }

    /// The shared memory.
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Where in the C64's I/O area the four registers lie (`$DE1C`,
    /// `$DF1C`, `$DFFC` ...) as SLOT_BASE selects it.
    pub fn c64_address(&self) -> u16 {
        0xde00 | (u16::from(self.slot_base & 0x7e) << 2) | 4
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.slot_base);
        w.bool(self.enabled);
        w.u8(self.bus_id);
        w.u8(self.irq_mask);
        w.u8(self.status_length);
        w.u16(self.response_length);
        w.raw(&self.ram);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.slot_base = r.u8()?;
        self.enabled = r.bool()?;
        self.bus_id = r.u8()? & 0x1f;
        self.irq_mask = r.u8()? & 7;
        self.status_length = r.u8()?;
        self.response_length = r.u16()? & 0x7ff;
        r.raw_into(&mut self.ram)
    }
}
