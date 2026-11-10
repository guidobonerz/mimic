//! The chips around the ARM in an Acorn Archimedes, and the BBC Micro's
//! [`video_ula`] and [`saa5050`] (teletext, MODE 7).

#![forbid(unsafe_code)]

pub mod ioc;
pub mod memc;
pub mod pcf8583;
pub mod saa5050;
pub mod vidc;
pub mod video_ula;
pub mod wd1772;

pub use ioc::Ioc;
pub use memc::{Memc, Target};
pub use pcf8583::Pcf8583;
pub use saa5050::Saa5050;
pub use vidc::Vidc;
pub use video_ula::VideoUla;
