//! The cartridge port's two media slots: the cartridge itself and its
//! battery RAM.
//!
//! **Cartridge:** a SNES cartridge slot that refuses a NEC DSP game whose
//! program ROM the user has not supplied ([`crate::dsp`]).
//!
//! **Battery RAM** (`SLOT_BATTERY`, media kind `BatteryRam`, a `.srm`
//! file of exactly the cartridge's SRAM size; frontends attach it with
//! `media_backends::image::attach_battery_ram`). Taken only while a
//! cartridge with a battery ($xFD6 = $02, $05, $06) is in. The machine
//! keeps the battery's contents ([`BatterySlot::contents`]): they survive
//! a power cycle, are loaded from the file when it goes in, and go back to
//! it at the end of every frame the game changed them, on flush and on
//! eject. A new cartridge starts from zeros (and ejects the old file).

use emu_core::media::{Media, MediaError, MediaSlot};

use crate::dsp;
pub use devices_cartridge::slot::BatterySlot;
use devices_cartridge::snes::SnesCart;
use devices_cartridge::CartSlot;
use std::collections::BTreeMap;

pub struct SnesSlot {
    pub inner: CartSlot<SnesCart>,
    pub roms: BTreeMap<String, Vec<u8>>,
    pub battery: BatterySlot,
}

impl MediaSlot for SnesSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.inner.insert(media)?;
        self.battery.cart_changed(None);
        if let Some(cart) = self.inner.cart() {
            if cart.dsp.is_some() && dsp::program(cart, &self.roms).is_none() {
                let wanted = dsp::roles(cart).join(" or ");
                let _ = self.inner.eject();
                return Err(MediaError::Format(format!(
                    "this cartridge has a NEC DSP: its program ROM ({wanted}, 8 KB, \
                     user-supplied) is missing from the ROM folder"
                )));
            }
            let len = (cart.battery && !cart.sram.is_empty()).then_some(cart.sram.len());
            self.battery.cart_changed(len);
        }
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.battery.cart_changed(None);
        self.inner.eject()
    }

    fn write_protect(&mut self, on: bool) {
        self.inner.write_protect(on);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.inner.flush()
    }
}
