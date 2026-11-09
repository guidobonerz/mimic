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

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};

use crate::dsp;
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

#[derive(Default)]
pub struct BatterySlot {
    media: Option<Media>,
    /// The SRAM size of the cartridge in the port, if it has a battery.
    expected: Option<usize>,
    /// What the battery holds; `None` until a file is loaded or the game
    /// has run.
    pub contents: Option<Vec<u8>>,
    /// A file went in: its contents replace the SRAM's.
    pub loaded: bool,
}

impl BatterySlot {
    /// Another cartridge (with `len` bytes of battery RAM, or none): the
    /// old file is written and taken out, the contents forgotten.
    fn cart_changed(&mut self, len: Option<usize>) {
        let _ = self.write_back();
        self.media = None;
        self.contents = None;
        self.loaded = false;
        self.expected = len;
    }

    pub fn is_inserted(&self) -> bool {
        self.media.is_some()
    }

    /// The game's SRAM as it is now: kept, and written to the file when it
    /// changed.
    pub fn update(&mut self, sram: &[u8]) {
        if self.contents.as_deref() == Some(sram) {
            return;
        }
        self.contents = Some(sram.to_vec());
        let _ = self.write_back();
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        let (Some(media), Some(data)) = (&mut self.media, &self.contents) else {
            return Ok(());
        };
        if media.backend.len() != data.len() as u64 {
            media.backend.set_len(data.len() as u64)?;
        }
        media.backend.write(0, data)
    }
}

impl MediaSlot for BatterySlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::BatteryRam {
            return Err(MediaError::Unsupported(media.kind));
        }
        let Some(len) = self.expected else {
            return Err(MediaError::Unsupported(media.kind));
        };
        let n = media.backend.len() as usize;
        if n != 0 && n != len {
            return Err(MediaError::Format(format!(
                "battery RAM file of {n} bytes; the cartridge has {len}"
            )));
        }
        let _ = self.write_back();
        if n == len {
            let mut data = vec![0; len];
            media.backend.read(0, &mut data)?;
            self.contents = Some(data);
            self.loaded = true;
        }
        self.media = Some(media);
        // A new (empty) file gets what the battery holds now.
        let _ = self.write_back();
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }
}
