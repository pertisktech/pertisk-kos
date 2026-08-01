//! Management gRPC API for Pertisk KOS (M4).

mod server;
mod state;

pub use server::{serve, TlsPaths, DEFAULT_LISTEN};
pub use state::{shared, NodeState, PowerAction, SharedState};
