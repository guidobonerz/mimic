//! Typing text into a C64 as timed key events (for tests and replays).

pub use machine_c64::keyboard::typed_events as type_text;

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::InputEvent;
    use machine_c64::keyboard::keys;

    #[test]
    fn shifted_characters_hold_shift() {
        let (ev, end) = type_text("A\"", 100);
        assert_eq!(ev.len(), 2 + 4);
        assert_eq!(end, 100 + 2 * 80_000);
        assert!(matches!(
            ev[2].event,
            InputEvent::Key { machine_key, down: true } if machine_key == keys::LSHIFT
        ));
    }
}
