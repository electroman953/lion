//! The interpreted mode: a register-based bytecode virtual machine (spec §22.1).
//!
//! [`compile`] turns the typed IR into a [`Chunk`] of typed instructions; [`run`]
//! executes it. Types are known statically, so each instruction works on values of
//! a known type (`AddInt`, `AddFloat`, ...) without dynamic dispatch. The primitive
//! operations come from `lion_runtime`, shared with the future native compiler.

mod bytecode;
mod compile;
mod constants;
mod machine;
mod natives;
mod set;
mod value;

pub use bytecode::{Chunk, Cmp, Instr, Program, Reg, Target, disassemble};
pub use compile::compile;
pub use constants::evaluate_compile;
pub use machine::{Alert, AlertKind, Failure, Session, Trap, run, run_from, run_test};
pub use value::Value;

#[cfg(test)]
mod tests;
