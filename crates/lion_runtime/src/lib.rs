//! The semantics of Lion's primitive operations, shared by every backend.
//!
//! The interpreter calls these functions directly; the native compiler will link
//! against the same code. Keeping one implementation of each operation is how both
//! modes produce exactly the same results and the same bugs (spec §22.2).

pub mod bug;
pub mod format;
pub mod ops;

pub use bug::{BugKind, IntOp};
