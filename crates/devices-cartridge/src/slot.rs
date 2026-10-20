//! A cartridge port as a media slot: insert a CRT (or raw ROM) image, eject
//! it; a change is a power cycle for the machine ([`CartSlot::take_changed`]).
//! What the machine wrote into flash goes back to the media on flush and
//! eject (same size: the chips of the image keep their layout).

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use media::crt::Crt;

use crate::expansion::{self, ExpansionCart};

pub struct CartSlot<T> {
    cart: Option<T>,
    media: Option<Media>,
    changed: bool,
    parse: fn(&[u8]) -> Result<T, MediaError>,
    image: fn(&T) -> Option<Vec<u8>>,
}

impl<T> CartSlot<T> {
    /// A slot that makes cartridges with `parse` and asks them for a
    /// changed image with `image`.
    pub fn new(
        parse: fn(&[u8]) -> Result<T, MediaError>,
        image: fn(&T) -> Option<Vec<u8>>,
    ) -> Self {
        Self {
            cart: None,
            media: None,
            changed: false,
            parse,
            image,
        }
    }

    pub fn cart(&self) -> Option<&T> {
        self.cart.as_ref()
    }

    pub fn cart_mut(&mut self) -> Option<&mut T> {
        self.cart.as_mut()
    }

    pub fn is_inserted(&self) -> bool {
        self.cart.is_some()
    }

    /// A cartridge went in or out since the last call: the machine powers
    /// up again (as one switches it off to change cartridges).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        let (Some(cart), Some(media)) = (&self.cart, &mut self.media) else {
            return Ok(());
        };
        let Some(bytes) = (self.image)(cart) else {
            return Ok(());
        };
        if bytes.len() as u64 != media.backend.len() {
            return Err(MediaError::Format(
                "the cartridge image changed its size".into(),
            ));
        }
        media.backend.write(0, &bytes)
    }
}

impl<T: Send> MediaSlot for CartSlot<T> {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Cartridge {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let cart = (self.parse)(&bytes)?;
        let _ = self.write_back();
        self.cart = Some(cart);
        self.media = Some(media);
        self.changed = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.cart = None;
        self.changed = true;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }
}

/// The expansion port slot of the C64/C128 (C64 CRT images).
pub type ExpansionSlot = CartSlot<Box<dyn ExpansionCart>>;

pub fn expansion_slot() -> ExpansionSlot {
    CartSlot::new(
        |bytes| expansion::from_crt(&Crt::parse(bytes)?),
        |cart| cart.written_image().map(|crt| crt.to_bytes()),
    )
}
