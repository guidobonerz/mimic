//! Zilog Z80 family peripherals (and their East German clones, U857 CTC
//! and U855 PIO): the counter/timer circuit ([`ctc`]) and the parallel
//! I/O ([`pio`]), with the interrupt daisy chain they share ([`Chain`]).
//!
//! **Interrupts.** The Z80 family's devices sit in a priority chain
//! (IEI/IEO). A device with a request pulls /INT while no device above it
//! is being served; the CPU's acknowledge goes to the highest one with a
//! request, which answers with its vector and counts as "in service"
//! until the CPU runs RETI (the devices watch the bus for `ED 4D`).
//! While a device is in service, it and everything below it stay quiet.
//!
//! A machine asks its devices from the top of the chain down:
//! [`Chain::pending`] for /INT and the vector, [`Chain::acknowledge`] when
//! the CPU took the interrupt, [`Chain::reti`] at a RETI; a device passes
//! the chain on to the next one while [`Chain::ieo`] is true.
//!
//! Sources: Zilog, "Z80 CTC Counter/Timer Circuit" and "Z80 PIO Parallel
//! I/O" product specifications, "Z80 Family CPU Peripherals User Manual"
//! (UM0081; interrupt daisy chain). The implementation is this project's
//! own.

#![forbid(unsafe_code)]

pub mod ctc;
pub mod pio;

pub use ctc::Ctc;
pub use pio::Pio;

use emu_core::{StateError, StateReader, StateWriter};

/// The interrupt sources of one device, highest priority first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chain<const N: usize> {
    /// The source asks for an interrupt.
    pub request: [bool; N],
    /// The source's interrupt routine runs (until RETI).
    pub service: [bool; N],
}

impl<const N: usize> Default for Chain<N> {
    fn default() -> Self {
        Self {
            request: [false; N],
            service: [false; N],
        }
    }
}

impl<const N: usize> Chain<N> {
    /// The source that pulls /INT (given IEI high): the first with a
    /// request, unless it or one above it is in service.
    pub fn pending(&self) -> Option<usize> {
        for i in 0..N {
            if self.service[i] {
                return None;
            }
            if self.request[i] {
                return Some(i);
            }
        }
        None
    }

    /// IEO: devices further down may interrupt (nothing in service here).
    pub fn ieo(&self) -> bool {
        !self.service.contains(&true)
    }

    /// The CPU acknowledged this device's interrupt: the pending source
    /// goes into service. Returns it.
    pub fn acknowledge(&mut self) -> Option<usize> {
        let i = self.pending()?;
        self.request[i] = false;
        self.service[i] = true;
        Some(i)
    }

    /// The CPU ran RETI: the highest source in service is done. False if
    /// none was (the RETI belongs to a device further down).
    pub fn reti(&mut self) -> bool {
        match self.service.iter().position(|&s| s) {
            Some(i) => {
                self.service[i] = false;
                true
            }
            None => false,
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        for i in 0..N {
            w.bool(self.request[i]);
            w.bool(self.service[i]);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        for i in 0..N {
            self.request[i] = r.bool()?;
            self.service[i] = r.bool()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_in_service_holds_back_itself_and_those_below() {
        let mut c = Chain::<3>::default();
        c.request[1] = true;
        c.request[2] = true;
        assert_eq!(c.pending(), Some(1));
        assert_eq!(c.acknowledge(), Some(1));
        assert_eq!(c.pending(), None, "source 2 waits for the RETI");
        assert!(!c.ieo());
        c.request[0] = true;
        assert_eq!(c.pending(), Some(0), "a higher one nests");
        assert_eq!(c.acknowledge(), Some(0));
        assert!(c.reti());
        assert_eq!(c.pending(), None);
        assert!(c.reti());
        assert_eq!(c.pending(), Some(2));
        assert!(c.ieo());
        c.request[2] = false;
        assert!(!c.reti());
    }
}
