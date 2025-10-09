mod bump;
#[cfg(all(feature = "alloc", feature="nightly_chain"))]
mod chain;

pub use bump::{Bump, FromMemError, MemBump};
#[cfg(all(feature="alloc", feature="nightly_chain"))]
pub use chain::{Chain};
