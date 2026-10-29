//! The C64 as the Ultimate command interface reaches it ([`UciMachine`]):
//! DMA through the CPU's memory map, the REU, drive 8 (power, mounting),
//! the EasyFlash in the expansion port and the VIC-II palette.

use devices_cartridge::reu::Reu;
use devices_cartridge::slot::ExpansionSlot;
use devices_net::uci::{drive_kind, UciDrive, UciMachine, UciSid};
use drives_cbm::DriveModel;
use emu_core::frame::Rgb8;
use emu_core::media::{Media, MediaError};
use emu_core::{Bus, Size};

use crate::drive::Drive8;
use crate::io::Io;
use crate::memory::Memory;
use crate::C64Bus;

/// What a UCI command may touch, borrowed from the machine for the
/// command's run.
pub(crate) struct UciContext<'a> {
    pub mem: &'a mut Memory,
    pub io: &'a mut Io,
    pub cart: &'a mut ExpansionSlot,
    pub reu: Option<&'a mut Reu>,
    pub drive: Option<&'a mut Drive8>,
    pub drive_power: &'a mut bool,
    pub palette: &'a mut [Rgb8; 16],
    /// REBOOT was asked for; the machine resets after the command.
    pub reboot: bool,
}

impl UciContext<'_> {
    fn bus(&mut self) -> C64Bus<'_> {
        C64Bus {
            mem: self.mem,
            io: self.io,
            cart: self.cart.cart_mut().map(|c| c.as_mut()),
            uci: None,
            reu: self.reu.as_deref_mut(),
        }
    }

    /// The drive with IEC number 8, if there is one.
    fn drive8(&mut self, address: u8) -> Option<&mut Drive8> {
        self.drive.as_deref_mut().filter(|_| address == 8)
    }
}

impl UciMachine for UciContext<'_> {
    fn dma_read(&mut self, addr: u16) -> u8 {
        self.bus().read(u32::from(addr), Size::Byte) as u8
    }

    fn dma_write(&mut self, addr: u16, v: u8) {
        self.bus().write(u32::from(addr), Size::Byte, u32::from(v));
    }

    fn ram(&self) -> Option<&[u8]> {
        Some(&self.mem.ram[..])
    }

    fn reu(&mut self) -> Option<&mut [u8]> {
        self.reu.as_deref_mut().map(Reu::ram_mut)
    }

    /// Drive A is drive 8; a virtual drive serves files at the IEC
    /// protocol level like the Ultimate's Software IEC drive.
    fn drives(&self) -> Vec<UciDrive> {
        self.drive
            .as_deref()
            .map(|d| UciDrive {
                kind: match d.model() {
                    Some(DriveModel::C1541) => drive_kind::C1541,
                    Some(DriveModel::C1571) => drive_kind::C1571,
                    Some(DriveModel::C1581) => drive_kind::C1581,
                    None => drive_kind::SOFTWARE_IEC,
                },
                address: 8,
                powered: *self.drive_power,
            })
            .into_iter()
            .collect()
    }

    /// Switching on powers the drive up from scratch.
    fn set_drive_power(&mut self, index: usize, on: bool) -> bool {
        let was = *self.drive_power;
        match self.drive.as_deref_mut() {
            Some(d) if index == 0 => {
                if on && !was {
                    d.reset(true);
                }
                *self.drive_power = on;
                true
            }
            _ => false,
        }
    }

    fn mount(&mut self, address: u8, media: Media) -> Result<(), MediaError> {
        let d = self.drive8(address).ok_or(MediaError::NoMedia)?;
        let slot = d.media_slot();
        // The disk in the drive is written back before it leaves.
        let _ = slot.flush();
        let old = slot.eject();
        match slot.insert(media) {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(m) = old {
                    let _ = slot.insert(m);
                }
                Err(e)
            }
        }
    }

    fn unmount(&mut self, address: u8) -> bool {
        match self.drive8(address) {
            Some(d) => {
                let slot = d.media_slot();
                let _ = slot.flush();
                slot.eject().is_some()
            }
            None => false,
        }
    }

    fn reboot(&mut self) {
        self.reboot = true;
    }

    fn erase_flash(&mut self, bank: u8, high: bool) -> bool {
        self.cart
            .cart_mut()
            .is_some_and(|c| c.erase_flash_sector(bank, high))
    }

    /// One SID at `$D400`; the type byte's meaning is undocumented (0).
    fn sids(&self) -> Vec<UciSid> {
        vec![UciSid {
            base: 0xd400,
            second: 0,
            kind: 0,
        }]
    }

    fn palette(&self) -> Option<[u8; 48]> {
        let mut rgb = [0; 48];
        for (i, c) in self.palette.iter().enumerate() {
            rgb[i * 3..i * 3 + 3].copy_from_slice(&[c.r, c.g, c.b]);
        }
        Some(rgb)
    }

    fn set_palette(&mut self, rgb: &[u8; 48]) -> bool {
        for (i, c) in self.palette.iter_mut().enumerate() {
            *c = Rgb8 {
                r: rgb[i * 3],
                g: rgb[i * 3 + 1],
                b: rgb[i * 3 + 2],
            };
        }
        true
    }

    fn reset_palette(&mut self) -> bool {
        *self.palette = chips_cbm::vic2::palette::palette();
        true
    }
}
