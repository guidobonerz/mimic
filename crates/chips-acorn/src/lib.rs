//! The chips around the ARM in an Acorn Archimedes.

#![forbid(unsafe_code)]

pub mod ioc;
pub mod memc;
pub mod pcf8583;
pub mod vidc;
pub mod wd1772;

pub use ioc::Ioc;
pub use memc::{Memc, Target};
pub use pcf8583::Pcf8583;
pub use vidc::Vidc;
