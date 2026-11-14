//! Decoders of BASIC interpreters' memory for the BASIC debugger
//! ([`emu_core::basic`]): one module per dialect family. A decoder knows
//! where its interpreter keeps the program, the variables and the stack
//! and where it begins a statement; it reads the machine only through the
//! main CPU's [`DebugTarget`] with side-effect-free reads (golden rule 2).
//!
//! A machine keeps the decoder as a field, refreshes it in
//! [`emu_core::Machine::basic`] (the interpreter may have been banked out
//! or replaced) and implements [`emu_core::basic::BasicDebug`] with
//! [`impl_basic_debug!`].

#![forbid(unsafe_code)]

pub mod cbm;

use emu_core::basic::{
    BasicError, BasicFrame, BasicInfo, BasicLine, BasicState, BasicStopPoint, BasicValue, BasicVar,
};
use emu_core::{DebugTarget, MemSpaceId};

/// A decoder for one interpreter; `host` is the machine's main CPU.
pub trait Dialect {
    /// The memory space all addresses are in (the main CPU's view).
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

/// Implement [`emu_core::basic::BasicDebug`] for a machine that is its own
/// main CPU [`DebugTarget`] and keeps a [`Dialect`] in field `$field`.
#[macro_export]
macro_rules! impl_basic_debug {
    ($machine:ty, $field:ident) => {
        impl ::emu_core::basic::BasicDebug for $machine {
            fn info(&self) -> ::emu_core::basic::BasicInfo {
                $crate::Dialect::info(&self.$field)
            }

            fn state(&self) -> ::emu_core::basic::BasicState {
                $crate::Dialect::state(&self.$field, self)
            }

            fn program(&self) -> Vec<::emu_core::basic::BasicLine> {
                $crate::Dialect::program(&self.$field, self)
            }

            fn variables(&self) -> Vec<::emu_core::basic::BasicVar> {
                $crate::Dialect::variables(&self.$field, self)
            }

            fn array(
                &self,
                name: &str,
                first: u32,
                count: u32,
            ) -> Result<Vec<::emu_core::basic::BasicValue>, ::emu_core::basic::BasicError> {
                $crate::Dialect::array(&self.$field, self, name, first, count)
            }

            fn stack(&self) -> Vec<::emu_core::basic::BasicFrame> {
                $crate::Dialect::stack(&self.$field, self)
            }

            fn set_variable(
                &mut self,
                name: &str,
                index: Option<u32>,
                value: &::emu_core::basic::BasicValue,
            ) -> Result<(), ::emu_core::basic::BasicError> {
                let space = $crate::Dialect::space(&self.$field);
                let writes = $crate::Dialect::plan_set(&self.$field, &*self, name, index, value)?;
                for (addr, byte) in writes {
                    let done = ::emu_core::DebugTarget::poke(
                        self,
                        space,
                        addr,
                        ::emu_core::Size::Byte,
                        u32::from(byte),
                    );
                    if !done {
                        return Err(::emu_core::basic::BasicError::Unsupported);
                    }
                }
                Ok(())
            }

            fn stop_points(&self) -> Vec<::emu_core::basic::BasicStopPoint> {
                $crate::Dialect::stop_points(&self.$field, self)
            }
        }
    };
}
