//! The interpreted mode: a register-based bytecode virtual machine (spec §22.1).
//!
//! [`compile`] turns the typed IR into a [`Chunk`] of typed instructions; [`run`]
//! executes it. Types are known statically, so each instruction works on values of
//! a known type (`AddInt`, `AddFloat`, ...) without dynamic dispatch. The primitive
//! operations come from `lion_runtime`, shared with the future native compiler.

mod bytecode;
mod compile;
mod constants;
mod ffi;
mod machine;
mod map;
pub mod natives;
pub mod parallel;
mod set;
pub mod shared;
mod value;

pub use bytecode::{Chunk, Cmp, EnumLayout, Instr, Layout, Program, Reg, Target, disassemble};
pub use compile::compile;
pub use constants::evaluate_compile;
pub use machine::{Alert, AlertKind, Failure, Session, Trap, run, run_from, run_test};
pub use map::MapValue;
pub use set::SetValue;
pub use value::{Closure, Record, Value, kinds, lock};

#[cfg(test)]
mod tests;
