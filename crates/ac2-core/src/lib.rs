#![forbid(unsafe_code)]

//! Shared low-level vocabulary: ids, scalars, colours and the enums and flags used across
//! the client.
//!
//! This crate deliberately stays small. It owns shared ids, passive scalar wrappers,
//! common sentinels, deterministic low-level helpers, and portability-safe PODs. It does
//! not own legacy container/string/runtime scaffolding or client orchestration policy.

mod color;
mod constants;
mod enums;
mod ids;
pub mod pstring;
mod random;
mod timer;

pub use color::{ColorArgb, ColorArgbByteReader};
pub use enums::*;
pub use ids::{CellId, DataId, InstanceId, ObjectId, QualifiedDataId, VersionHandle};
pub use random::Random;
pub use timer::Timer;
