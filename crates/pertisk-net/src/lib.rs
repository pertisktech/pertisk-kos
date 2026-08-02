//! Host networking for Pertisk KOS (Phase 1 / M2).
//!
//! Brings links up, applies static addressing via netlink, or requests DHCP
//! through `udhcpc` / `dhclient` when present.
//!
//! Production images have no shell, so DHCP leases are applied by the
//! `pertisk-udhcpc-hook` binary (`udhcpc -s /usr/lib/pertisk/udhcpc-hook`).

mod apply;
mod dns;
mod link;
pub mod udhcpc_hook;

pub use apply::{apply_network, NetError};
