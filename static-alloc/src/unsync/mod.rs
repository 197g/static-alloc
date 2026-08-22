mod bump;
#[cfg(all(feature = "alloc", feature="nightly_chain"))]
mod chain;

pub use bump::{Bump, FromMemError, BumpSlice};
#[cfg(all(feature="alloc", feature="nightly_chain"))]
pub use chain::{Chain};
