//! Blocking, owned Rust bindings for the process-wide embedded tailnet node.
//!
//! Call blocking operations through `spawn_blocking` in asynchronous code.
//! [`node::Tailnet::net_dialer`] delegates that work to `FdNetDialer`.

pub mod abi;
pub mod types;
pub use abi::Code;
pub use types::*;

#[cfg(unix)]
pub mod fdsource;
#[cfg(unix)]
pub mod load;
#[cfg(unix)]
pub mod node;
#[cfg(all(unix, feature = "testkit"))]
pub mod testkit;

#[cfg(unix)]
pub use fdsource::TailnetFdSource;
#[cfg(unix)]
pub use load::{load, load_default, LoadError};
#[cfg(unix)]
pub use node::Tailnet;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct TailnetError {
    pub code: Code,
    pub message: String,
}
