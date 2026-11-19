//! Which host gamepad is pad 0, 1 … of the input profiles.
//!
//! gilrs numbers every device it finds, and on Windows that includes HID
//! devices that are no game controller at all ("HID-compliant device"
//! ahead of an "Xbox 360 Controller for Windows", 2026-11-19): the profiles'
//! pad 0 then never moved. So a device gets a pad number only when it is
//! first worked (a button pressed, a stick pushed past half way), in that
//! order; until then its events are dropped. A device keeps its number
//! when it is unplugged and comes back.

/// A stick has to move this far (of 1.0) to claim a pad number.
pub const CLAIM_AXIS: f32 = 0.5;

#[derive(Debug, Default)]
pub struct PadSlots {
    /// The host device ids in the order they were first worked.
    ids: Vec<usize>,
}

impl PadSlots {
    /// The pad number of host device `id`, giving it the next one when
    /// `worked` (a press or a clear stick motion); `None`: the device has
    /// not been worked yet, its event is dropped.
    pub fn slot(&mut self, id: usize, worked: bool) -> Option<u8> {
        if let Some(n) = self.ids.iter().position(|&i| i == id) {
            return u8::try_from(n).ok();
        }
        if !worked {
            return None;
        }
        self.ids.push(id);
        u8::try_from(self.ids.len() - 1).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_device_worked_is_pad_0() {
        let mut s = PadSlots::default();
        // Device 0 (a HID device that is no controller) only drifts.
        assert_eq!(s.slot(0, false), None);
        // The Xbox pad (device 1) is pressed: pad 0.
        assert_eq!(s.slot(1, true), Some(0));
        assert_eq!(s.slot(1, false), Some(0), "its later events too");
        assert_eq!(s.slot(0, false), None);
        // A second controller worked later: pad 1.
        assert_eq!(s.slot(3, true), Some(1));
        assert_eq!(s.slot(1, true), Some(0));
    }
}
