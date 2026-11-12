//! A cartridge slot with the battery RAM of its cartridge.
//!
//! The cartridge slot takes a ROM image (`devices_cartridge::msx`). Whether
//! the cartridge has battery RAM follows from its bank switching scheme,
//! which only the machine's `mapper1`/`mapper2` option can say ("ascii8sram",
//! "ascii16sram", "gamemaster2"); the slot knows the option and tells its
//! [`BatterySlot`] the RAM's size as soon as a cartridge goes in, so that
//! the frontend can put the `.srm` file in right after it.
//!
//! The machine shows the battery RAM of cartridge 1 as a media slot; that
//! of cartridge 2 is kept the same way (through power cycles and in
//! savestates) but has no file.

use devices_cartridge::msx::{Mapper, MsxCart};
use devices_cartridge::slot::BatterySlot;
use devices_cartridge::CartSlot;
use emu_core::media::{Media, MediaError, MediaSlot};

pub struct CartPort {
    pub inner: CartSlot<MsxCart>,
    /// The mapper option; `None` for "auto".
    mapper: Option<Mapper>,
    pub battery: BatterySlot,
}

impl CartPort {
    pub fn new(mapper: Option<Mapper>) -> Self {
        Self {
            inner: devices_cartridge::msx::slot(),
            mapper,
            battery: BatterySlot::default(),
        }
    }

    /// The cartridge as the machine runs it: a copy at power-on, with the
    /// mapper option's scheme.
    pub fn working_copy(&self) -> Option<MsxCart> {
        let mut cart = self.inner.cart().cloned()?;
        if let Some(m) = self.mapper {
            cart.set_mapper(m);
        }
        Some(cart)
    }

    fn tell_battery(&mut self) {
        let len = self
            .working_copy()
            .map(|c| c.sram().len())
            .filter(|&n| n > 0);
        self.battery.cart_changed(len);
    }
}

impl MediaSlot for CartPort {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.inner.insert(media)?;
        self.tell_battery();
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
