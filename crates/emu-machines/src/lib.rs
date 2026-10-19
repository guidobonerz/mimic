//! Machine registry population: the only crate that links every machine-* crate.

#![forbid(unsafe_code)]

use emu_core::registry::Registry;

/// Every machine this build supports. Machines are added here as they
/// become runnable.
pub fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(machine_sim6502::entry());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;
    use emu_core::{DebugHooks, StopReason};

    #[test]
    fn registered_machines_run_a_frame() {
        let r = registry();
        assert!(!r.entries().is_empty());
        for entry in r.entries() {
            let mut m = r.create(entry.id, &MachineConfig::default()).unwrap();
            assert_eq!(m.run_frame(&mut DebugHooks::new()), StopReason::FrameDone);
            assert_eq!(m.devices().first().map(|d| d.0), Some("cpu"));
        }
    }
}
