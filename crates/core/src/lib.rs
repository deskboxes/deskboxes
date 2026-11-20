//! Deskboxes core: the data model, pure layout operations, persistence formats.
//! No Win32 here; everything is unit-testable (docs/ARCHITECTURE.md §7, §8).

pub mod geom;
pub mod model;
pub mod layout;
pub mod selection;
pub mod snapshot;
pub mod persist;
pub mod rules;

pub use geom::*;
pub use model::*;
