//! The chips around the ARM in an Acorn Archimedes, and the BBC Micro's
//! [`video_ula`], [`saa5050`] (teletext, MODE 7), [`i8271`] (the Model
//! B's floppy disc controller) and [`mc146818`] (the Master's clock and
//! CMOS RAM).

#![forbid(unsafe_code)]

pub mod i8271;
pub mod ioc;
pub mod mc146818;
pub mod memc;
pub mod pcf8583;
pub mod saa5050;
pub mod vidc;
pub mod video_ula;
pub mod wd1772;

pub use i8271::I8271;
pub use ioc::Ioc;
pub use mc146818::Mc146818;
pub use memc::{Memc, Target};
pub use pcf8583::Pcf8583;
pub use saa5050::Saa5050;
pub use vidc::Vidc;
pub use video_ula::VideoUla;
