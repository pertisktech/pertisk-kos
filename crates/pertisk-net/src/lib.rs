//! Host networking for Pertisk KOS (Phase 1 / M2).
//!
//! Brings links up, applies static addressing via netlink, or requests DHCP
//! through `udhcpc` / `dhclient` when present.

mod apply;
mod dns;
mod link;

pub use apply::{apply_network, NetError};
