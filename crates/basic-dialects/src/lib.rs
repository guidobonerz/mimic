//! Decoders of BASIC interpreters' memory for the BASIC debugger
//! ([`mimic_core::basic`]): one module per dialect family. A decoder knows
//! where its interpreter keeps the program, the variables and the stack
//! and where it begins a statement; it reads the machine only through the
//! main CPU's [`DebugTarget`] with side-effect-free reads (golden rule 2).
//!
//! A machine keeps the decoder as a field, refreshes it in
//! [`mimic_core::Machine::basic`] (the interpreter may have been banked out
//! or replaced) and implements [`mimic_core::basic::BasicDebug`] with
//! [`impl_basic_debug!`].

#![forbid(unsafe_code)]

pub mod atari;
pub mod bbc;
pub mod bbcarm;
pub mod cbm;
pub mod hcbasic;
pub mod level1;
pub mod locomotive;
pub mod msz80;
pub mod sinclair;
pub mod tibasic;

use mimic_core::basic::{
    BasicError, BasicFrame, BasicInfo, BasicLine, BasicState, BasicStopPoint, BasicValue, BasicVar,
};
use mimic_core::{DebugTarget, MemSpaceId};

/// A decoder for one interpreter; `host` is the machine's main CPU.
pub trait Dialect {
    /// The memory space the addresses of lines and variables are in and
    /// `plan_set`'s bytes are written to (the main CPU's view, or the RAM
    /// behind its ROMs).
    fn space(&self) -> MemSpaceId;
    fn info(&self) -> BasicInfo;
    fn state(&self, host: &dyn DebugTarget) -> BasicState;
    fn program(&self, host: &dyn DebugTarget) -> Vec<BasicLine>;
    fn variables(&self, host: &dyn DebugTarget) -> Vec<BasicVar>;
    fn array(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        first: u32,
        count: u32,
    ) -> Result<Vec<BasicValue>, BasicError>;
    fn stack(&self, host: &dyn DebugTarget) -> Vec<BasicFrame>;
    /// The bytes to write for `set_variable`, as (address, byte).
    fn plan_set(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError>;
    fn stop_points(&self, host: &dyn DebugTarget) -> Vec<BasicStopPoint>;
}

/// Implement [`mimic_core::basic::BasicDebug`] for a machine that is its own
/// main CPU [`DebugTarget`] and keeps a [`Dialect`] in field `$field`.
/// A machine whose `DebugTarget` may stand for another processor names a
/// method as third argument that returns the main CPU's (reads only; the
/// writes of `set_variable` go through the machine's own `poke`).
#[macro_export]
macro_rules! impl_basic_debug {
    (@host $me:ident) => {
        $me
    };
    (@host $me:ident, $host:ident) => {
        &$me.$host()
    };
    ($machine:ty, $field:ident $(, $host:ident)?) => {
        impl ::mimic_core::basic::BasicDebug for $machine {
            fn info(&self) -> ::mimic_core::basic::BasicInfo {
                $crate::Dialect::info(&self.$field)
            }

            fn state(&self) -> ::mimic_core::basic::BasicState {
                $crate::Dialect::state(
                    &self.$field,
                    $crate::impl_basic_debug!(@host self $(, $host)?),
                )
            }

            fn program(&self) -> Vec<::mimic_core::basic::BasicLine> {
                $crate::Dialect::program(
                    &self.$field,
                    $crate::impl_basic_debug!(@host self $(, $host)?),
                )
            }

            fn variables(&self) -> Vec<::mimic_core::basic::BasicVar> {
                $crate::Dialect::variables(
                    &self.$field,
                    $crate::impl_basic_debug!(@host self $(, $host)?),
                )
            }

            fn array(
                &self,
                name: &str,
                first: u32,
                count: u32,
            ) -> Result<Vec<::mimic_core::basic::BasicValue>, ::mimic_core::basic::BasicError> {
                $crate::Dialect::array(
                    &self.$field,
                    $crate::impl_basic_debug!(@host self $(, $host)?),
                    name,
                    first,
                    count,
                )
            }

            fn stack(&self) -> Vec<::mimic_core::basic::BasicFrame> {
                $crate::Dialect::stack(
                    &self.$field,
                    $crate::impl_basic_debug!(@host self $(, $host)?),
                )
            }

            fn set_variable(
                &mut self,
                name: &str,
                index: Option<u32>,
                value: &::mimic_core::basic::BasicValue,
            ) -> Result<(), ::mimic_core::basic::BasicError> {
                let space = $crate::Dialect::space(&self.$field);
                let writes = {
                    let me = &*self;
                    $crate::Dialect::plan_set(
                        &me.$field,
                        $crate::impl_basic_debug!(@host me $(, $host)?),
                        name,
                        index,
                        value,
                    )?
                };
                for (addr, byte) in writes {
                    let done = ::mimic_core::DebugTarget::poke(
                        self,
                        space,
                        addr,
                        ::mimic_core::Size::Byte,
                        u32::from(byte),
                    );
                    if !done {
                        return Err(::mimic_core::basic::BasicError::Unsupported);
                    }
                }
                Ok(())
            }

            fn stop_points(&self) -> Vec<::mimic_core::basic::BasicStopPoint> {
                $crate::Dialect::stop_points(
                    &self.$field,
                    $crate::impl_basic_debug!(@host self $(, $host)?),
                )
            }
        }
    };
}
