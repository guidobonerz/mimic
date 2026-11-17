//! Emulated time.

/// Absolute emulated time in master-clock cycles since power-on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cycle(pub u64);
