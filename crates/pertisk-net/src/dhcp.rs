//! In-process DHCPv4 client (no BusyBox / shell / udhcpc).
//!
//! Sole lease path for production images. After the initial DISCOVER/REQUEST,
//! a per-interface maintainer thread renews at T1 (unicast) and rebinds at T2
//! (broadcast) so long-lived nodes keep their address past the first lease.
//!
//! Across reboot, the last ACK is persisted under STATE (`machine/dhcp/`) and
//! the boot path tries RFC 2131 INIT-REBOOT (REQUEST previous IP) before a
//! full DISCOVER — so Proxmox stop/start keeps the same address when the DHCP
//! server still honors that binding.

#[cfg(target_os = "linux")]
use crate::apply::NetError;

#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::net::Ipv4Addr;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(target_os = "linux")]
use std::thread::JoinHandle;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Directory under STATE where DHCP leases are persisted (`machine/dhcp`).
#[cfg(target_os = "linux")]
fn lease_dir_slot() -> &'static Mutex<Option<PathBuf>> {
    static D: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(None))
}

/// Point the DHCP client at STATE so leases survive reboot.
///
/// Call after STATE is mounted (e.g. `{state_root}/machine/dhcp`).
#[cfg(target_os = "linux")]
pub fn set_lease_dir(dir: Option<&Path>) {
    let mut g = match lease_dir_slot().lock() {
        Ok(m) => m,
        Err(p) => p.into_inner(),
    };
    *g = dir.map(|p| p.to_path_buf());
    if let Some(p) = dir {
        let _ = std::fs::create_dir_all(p);
        tracing::info!(path = %p.display(), "DHCP lease dir set");
    }
}

/// Active DHCPv4 lease (in-memory; also persisted under STATE when configured).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct Lease {
    pub iface: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
    pub server: Ipv4Addr,
    pub routers: Vec<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    /// Lease lifetime in seconds (`0xFFFF_FFFF` = infinite).
    pub lease_secs: u32,
    /// Seconds from acquire until unicast renew (T1).
    pub t1_secs: u32,
    /// Seconds from acquire until broadcast rebind (T2).
    pub t2_secs: u32,
    pub acquired: Instant,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedLease {
    iface: String,
    ip: String,
    prefix: u8,
    server: String,
    routers: Vec<String>,
    dns: Vec<String>,
    lease_secs: u32,
    t1_secs: u32,
    t2_secs: u32,
    acquired_unix: u64,
}

#[cfg(target_os = "linux")]
impl Lease {
    fn is_infinite(&self) -> bool {
        self.lease_secs == u32::MAX
    }

    fn renew_at(&self) -> Instant {
        self.acquired + Duration::from_secs(u64::from(self.t1_secs))
    }

    fn rebind_at(&self) -> Instant {
        self.acquired + Duration::from_secs(u64::from(self.t2_secs))
    }

    fn expire_at(&self) -> Instant {
        if self.is_infinite() {
            Instant::now() + Duration::from_secs(u64::MAX / 4)
        } else {
            self.acquired + Duration::from_secs(u64::from(self.lease_secs))
        }
    }

    fn to_persisted(&self) -> PersistedLease {
        let acquired_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        PersistedLease {
            iface: self.iface.clone(),
            ip: self.ip.to_string(),
            prefix: self.prefix,
            server: self.server.to_string(),
            routers: self.routers.iter().map(ToString::to_string).collect(),
            dns: self.dns.iter().map(ToString::to_string).collect(),
            lease_secs: self.lease_secs,
            t1_secs: self.t1_secs,
            t2_secs: self.t2_secs,
            acquired_unix,
        }
    }
}

#[cfg(target_os = "linux")]
fn lease_path(iface: &str) -> Option<PathBuf> {
    let dir = match lease_dir_slot().lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }?;
    let safe: String = iface
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    Some(dir.join(format!("{safe}.lease")))
}

#[cfg(target_os = "linux")]
fn persist_lease(lease: &Lease) {
    let Some(path) = lease_path(&lease.iface) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&lease.to_persisted()) {
        Ok(body) => {
            if let Err(err) = std::fs::write(&path, body) {
                tracing::warn!(
                    interface = %lease.iface,
                    path = %path.display(),
                    error = %err,
                    "DHCP lease persist failed"
                );
            } else {
                tracing::debug!(
                    interface = %lease.iface,
                    path = %path.display(),
                    ip = %lease.ip,
                    "DHCP lease persisted"
                );
            }
        }
        Err(err) => tracing::warn!(error = %err, "DHCP lease serialize failed"),
    }
}

#[cfg(target_os = "linux")]
fn peek_persisted_ip(iface: &str) -> Option<Ipv4Addr> {
    let path = lease_path(iface)?;
    let body = std::fs::read_to_string(&path).ok()?;
    let p: PersistedLease = serde_json::from_str(&body).ok()?;
    let ip: Ipv4Addr = p.ip.parse().ok()?;
    if ip.is_unspecified() || ip.is_broadcast() {
        return None;
    }
    Some(ip)
}

#[cfg(target_os = "linux")]
fn load_persisted_ip(iface: &str) -> Option<Ipv4Addr> {
    let ip = peek_persisted_ip(iface)?;
    if let Some(path) = lease_path(iface) {
        tracing::info!(
            interface = iface,
            ip = %ip,
            path = %path.display(),
            "DHCP loaded previous lease from STATE"
        );
    }
    Some(ip)
}

/// True when STATE has a preferred IP that differs from the address currently on the iface.
#[cfg(target_os = "linux")]
pub fn should_reclaim(iface: &str, current_v4: Option<Ipv4Addr>) -> bool {
    match (peek_persisted_ip(iface), current_v4) {
        (Some(want), Some(have)) => want != have,
        (Some(_), None) => true,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
struct Maintainer {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

#[cfg(target_os = "linux")]
fn maintainers() -> &'static Mutex<HashMap<String, Maintainer>> {
    static M: OnceLock<Mutex<HashMap<String, Maintainer>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Freshly acquired leases handed to a just-started maintainer (avoids
/// reconstructing short synthetic timers after a successful ACK).
#[cfg(target_os = "linux")]
fn seeded_leases() -> &'static Mutex<HashMap<String, Lease>> {
    static S: OnceLock<Mutex<HashMap<String, Lease>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_os = "linux")]
fn seed_lease(lease: Lease) {
    let iface = lease.iface.clone();
    let mut map = match seeded_leases().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.insert(iface, lease);
}

#[cfg(target_os = "linux")]
fn take_seeded_lease(iface: &str) -> Option<Lease> {
    let mut map = match seeded_leases().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.remove(iface)
}

/// Ensure a background renew/rebind loop is running for `iface`.
///
/// Idempotent: replaces any prior maintainer for the same interface.
#[cfg(target_os = "linux")]
pub fn ensure_maintainer(iface: &str) {
    let mut map = match maintainers().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(prev) = map.remove(iface) {
        prev.stop.store(true, Ordering::SeqCst);
        let _ = prev.handle.join();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = Arc::clone(&stop);
    let iface_owned = iface.to_string();
    let handle = std::thread::Builder::new()
        .name(format!("dhcp-{iface}"))
        .spawn(move || maintain_loop(&iface_owned, &stop_c))
        .unwrap_or_else(|e| {
            tracing::error!(interface = iface, error = %e, "failed to spawn DHCP maintainer");
            std::thread::spawn(|| {})
        });
    map.insert(
        iface.to_string(),
        Maintainer { stop, handle },
    );
    tracing::info!(interface = iface, "DHCP lease maintainer started");
}

/// One-shot DISCOVER → REQUEST → ACK and apply the lease (boot path).
///
/// Prefers INIT-REBOOT from a STATE-persisted lease when available.
#[cfg(target_os = "linux")]
pub fn run_dhcp(iface: &str) -> Result<(), NetError> {
    let lease = acquire(iface)?;
    apply_lease(&lease)?;
    persist_lease(&lease);
    seed_lease(lease);
    ensure_maintainer(iface);
    Ok(())
}

#[cfg(target_os = "linux")]
fn maintain_loop(iface: &str, stop: &AtomicBool) {
    let mut lease = match take_seeded_lease(iface) {
        Some(l) => {
            tracing::info!(
                interface = iface,
                ip = %l.ip,
                lease_secs = l.lease_secs,
                t1 = l.t1_secs,
                t2 = l.t2_secs,
                "DHCP maintainer using acquired lease"
            );
            Some(l)
        }
        None => match current_v4_lease(iface) {
            // Apply skipped DHCP because IPv4 was already present — rebind soon.
            Some(l) => {
                tracing::info!(
                    interface = iface,
                    ip = %l.ip,
                    lease_secs = l.lease_secs,
                    "DHCP maintainer adopting existing IPv4"
                );
                Some(l)
            }
            None => None,
        },
    };

    while !stop.load(Ordering::SeqCst) {
        if lease.is_none() {
            match acquire(iface) {
                Ok(l) => {
                    if let Err(err) = apply_lease(&l) {
                        tracing::warn!(interface = iface, error = %err, "DHCP apply after acquire failed");
                        sleep_interruptible(Duration::from_secs(15), stop);
                        continue;
                    }
                    persist_lease(&l);
                    lease = Some(l);
                }
                Err(err) => {
                    tracing::warn!(interface = iface, error = %err, "DHCP acquire failed; retry");
                    sleep_interruptible(Duration::from_secs(15), stop);
                    continue;
                }
            }
        }

        let Some(cur) = lease.as_ref() else {
            continue;
        };
        if cur.is_infinite() {
            // Still wake occasionally in case the address disappears.
            sleep_interruptible(Duration::from_secs(3600), stop);
            if iface_has_v4(iface) {
                continue;
            }
            tracing::warn!(interface = iface, "IPv4 lost on infinite lease; rediscovering");
            lease = None;
            continue;
        }

        let now = Instant::now();
        if now < cur.renew_at() {
            let wait = cur.renew_at().saturating_duration_since(now);
            sleep_interruptible(wait.min(Duration::from_secs(60)), stop);
            continue;
        }

        if now < cur.rebind_at() {
            // Adopted leases (unknown server) skip unicast renew.
            if cur.server.is_unspecified() || cur.server.is_broadcast() {
                let wait = cur
                    .rebind_at()
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(30));
                sleep_interruptible(wait, stop);
                continue;
            }
            match renew(cur) {
                Ok(l) => {
                    if let Err(err) = apply_lease(&l) {
                        tracing::warn!(interface = iface, error = %err, "DHCP apply after renew failed");
                    } else {
                        tracing::info!(
                            interface = iface,
                            ip = %l.ip,
                            lease_secs = l.lease_secs,
                            "DHCP lease renewed"
                        );
                        persist_lease(&l);
                        lease = Some(l);
                    }
                    continue;
                }
                Err(err) => {
                    tracing::warn!(interface = iface, error = %err, "DHCP renew failed; will rebind at T2");
                    let wait = cur
                        .rebind_at()
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(30));
                    sleep_interruptible(wait, stop);
                    continue;
                }
            }
        }

        if now < cur.expire_at() {
            match rebind(cur) {
                Ok(l) => {
                    if let Err(err) = apply_lease(&l) {
                        tracing::warn!(interface = iface, error = %err, "DHCP apply after rebind failed");
                    } else {
                        tracing::info!(
                            interface = iface,
                            ip = %l.ip,
                            lease_secs = l.lease_secs,
                            "DHCP lease rebound"
                        );
                        persist_lease(&l);
                        lease = Some(l);
                    }
                    continue;
                }
                Err(err) => {
                    tracing::warn!(interface = iface, error = %err, "DHCP rebind failed; retry until expiry");
                    let wait = cur
                        .expire_at()
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(15));
                    if wait.is_zero() {
                        tracing::warn!(interface = iface, "DHCP lease expired; rediscovering");
                        lease = None;
                    } else {
                        sleep_interruptible(wait, stop);
                    }
                    continue;
                }
            }
        }

        tracing::warn!(interface = iface, "DHCP lease expired; rediscovering");
        lease = None;
    }
    tracing::info!(interface = iface, "DHCP lease maintainer stopped");
}

#[cfg(target_os = "linux")]
fn acquire(iface: &str) -> Result<Lease, NetError> {
    if let Some(prev_ip) = load_persisted_ip(iface) {
        match init_reboot(iface, prev_ip) {
            Ok(lease) => {
                tracing::info!(
                    interface = iface,
                    ip = %lease.ip,
                    prefix = lease.prefix,
                    lease_secs = lease.lease_secs,
                    "DHCP bound (INIT-REBOOT)"
                );
                return Ok(lease);
            }
            Err(err) => {
                tracing::warn!(
                    interface = iface,
                    previous_ip = %prev_ip,
                    error = %err,
                    "DHCP INIT-REBOOT failed; falling back to DISCOVER"
                );
            }
        }
    }
    acquire_discover(iface)
}

/// RFC 2131 INIT-REBOOT: broadcast REQUEST for a previously bound address.
#[cfg(target_os = "linux")]
fn init_reboot(iface: &str, requested: Ipv4Addr) -> Result<Lease, NetError> {
    use dhcproto::v4::{
        DhcpOption, Encodable, Encoder, Flags, Message, MessageType, Opcode, OptionCode,
    };
    use rand::RngCore;
    use std::net::SocketAddrV4;

    wait_iface(iface, Duration::from_secs(15))?;
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;
        let _ = rt.block_on(crate::link::set_link_up(iface));
    }
    let mac = read_mac(iface)?;
    let mut xid_bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut xid_bytes);
    let xid = u32::from_be_bytes(xid_bytes);
    let client_id = client_id_from_mac(&mac);
    let sock = open_dhcp_socket(iface, Ipv4Addr::UNSPECIFIED)?;
    let dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, 67);
    let deadline = Instant::now() + Duration::from_secs(12);

    let mut attempts = 0u32;
    while Instant::now() < deadline {
        attempts += 1;
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_xid(xid);
        msg.set_flags(Flags::default().set_broadcast());
        msg.set_chaddr(&mac);
        // INIT-REBOOT: ciaddr must be 0; Requested-IP set; Server-ID must NOT be set.
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Request));
        msg.opts_mut()
            .insert(DhcpOption::ClientIdentifier(client_id.clone()));
        msg.opts_mut()
            .insert(DhcpOption::RequestedIpAddress(requested));
        msg.opts_mut()
            .insert(DhcpOption::ParameterRequestList(param_request_list()));
        let mut buf = Vec::new();
        msg.encode(&mut Encoder::new(&mut buf))
            .map_err(|e| NetError::Msg(format!("dhcp encode init-reboot: {e}")))?;
        tracing::debug!(
            interface = iface,
            %requested,
            attempts,
            "DHCP INIT-REBOOT request"
        );
        if let Err(err) = sock.send_to(&buf, dest) {
            tracing::warn!(interface = iface, error = %err, "DHCP INIT-REBOOT send failed");
        }
        if let Some(resp) = recv_matching(&sock, xid, deadline) {
            match resp.opts().get(OptionCode::MessageType) {
                Some(DhcpOption::MessageType(MessageType::Ack)) => {
                    let server_ip = match resp.opts().get(OptionCode::ServerIdentifier) {
                        Some(DhcpOption::ServerIdentifier(ip)) => *ip,
                        _ => resp.siaddr(),
                    };
                    return lease_from_ack(iface, &resp, server_ip);
                }
                Some(DhcpOption::MessageType(MessageType::Nak)) => {
                    return Err(NetError::Msg(format!(
                        "DHCP NAK on INIT-REBOOT for {requested}"
                    )));
                }
                _ => {}
            }
        }
    }
    Err(NetError::Msg(format!(
        "DHCP no ACK on INIT-REBOOT for {requested} after {attempts} attempts"
    )))
}

#[cfg(target_os = "linux")]
fn acquire_discover(iface: &str) -> Result<Lease, NetError> {
    use dhcproto::v4::{
        DhcpOption, Encodable, Encoder, Flags, Message, MessageType, Opcode, OptionCode,
    };
    use rand::RngCore;
    use std::net::SocketAddrV4;

    wait_iface(iface, Duration::from_secs(15))?;
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;
        let _ = rt.block_on(crate::link::set_link_up(iface));
    }
    let mac = read_mac(iface)?;
    let mut xid_bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut xid_bytes);
    let xid = u32::from_be_bytes(xid_bytes);
    let client_id = client_id_from_mac(&mac);

    let sock = open_dhcp_socket(iface, Ipv4Addr::UNSPECIFIED)?;
    let dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, 67);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut offer: Option<Message> = None;
    let mut discovers = 0u32;

    while Instant::now() < deadline && offer.is_none() {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_xid(xid);
        msg.set_flags(Flags::default().set_broadcast());
        msg.set_chaddr(&mac);
        msg.set_secs(discovers.saturating_mul(2).min(u16::MAX as u32) as u16);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Discover));
        msg.opts_mut()
            .insert(DhcpOption::ClientIdentifier(client_id.clone()));
        msg.opts_mut()
            .insert(DhcpOption::ParameterRequestList(param_request_list()));
        let mut buf = Vec::new();
        msg.encode(&mut Encoder::new(&mut buf))
            .map_err(|e| NetError::Msg(format!("dhcp encode discover: {e}")))?;
        discovers += 1;
        tracing::debug!(interface = iface, discovers, mac = ?mac, "DHCP discover");
        match sock.send_to(&buf, dest) {
            Ok(n) => tracing::debug!(bytes = n, "DHCP discover sent"),
            Err(err) => {
                tracing::warn!(interface = iface, error = %err, "DHCP discover send failed");
            }
        }
        if let Some(resp) = recv_matching(&sock, xid, deadline) {
            if matches!(
                resp.opts().get(OptionCode::MessageType),
                Some(DhcpOption::MessageType(MessageType::Offer))
            ) {
                offer = Some(resp);
            }
        }
    }

    let offer = offer.ok_or_else(|| {
        NetError::Msg(format!(
            "DHCP no offer on {iface} after {discovers} discovers (is there a DHCP server on this bridge?)"
        ))
    })?;
    let yiaddr = offer.yiaddr();
    if yiaddr.is_unspecified() {
        return Err(NetError::Msg("DHCP offer missing yiaddr".into()));
    }
    let server_ip = match offer.opts().get(OptionCode::ServerIdentifier) {
        Some(DhcpOption::ServerIdentifier(ip)) => *ip,
        _ => offer.siaddr(),
    };
    tracing::info!(interface = iface, %yiaddr, %server_ip, "DHCP offer");

    let mut ack: Option<Message> = None;
    while Instant::now() < deadline && ack.is_none() {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_xid(xid);
        msg.set_flags(Flags::default().set_broadcast());
        msg.set_chaddr(&mac);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Request));
        msg.opts_mut()
            .insert(DhcpOption::ClientIdentifier(client_id.clone()));
        msg.opts_mut()
            .insert(DhcpOption::RequestedIpAddress(yiaddr));
        msg.opts_mut()
            .insert(DhcpOption::ServerIdentifier(server_ip));
        msg.opts_mut()
            .insert(DhcpOption::ParameterRequestList(param_request_list()));
        let mut buf = Vec::new();
        msg.encode(&mut Encoder::new(&mut buf))
            .map_err(|e| NetError::Msg(format!("dhcp encode request: {e}")))?;
        tracing::debug!(interface = iface, %yiaddr, "DHCP request");
        if let Err(err) = sock.send_to(&buf, dest) {
            tracing::warn!(interface = iface, error = %err, "DHCP request send failed");
        }
        if let Some(resp) = recv_matching(&sock, xid, deadline) {
            match resp.opts().get(OptionCode::MessageType) {
                Some(DhcpOption::MessageType(MessageType::Ack)) => ack = Some(resp),
                Some(DhcpOption::MessageType(MessageType::Nak)) => {
                    return Err(NetError::Msg(format!("DHCP NAK on {iface}")));
                }
                _ => {}
            }
        }
    }

    let ack = ack.ok_or_else(|| NetError::Msg(format!("DHCP no ACK on {iface}")))?;
    let lease = lease_from_ack(iface, &ack, server_ip)?;
    tracing::info!(
        interface = iface,
        ip = %lease.ip,
        prefix = lease.prefix,
        lease_secs = lease.lease_secs,
        t1 = lease.t1_secs,
        t2 = lease.t2_secs,
        "DHCP bound (builtin)"
    );
    Ok(lease)
}

/// Unicast REQUEST to the leasing server (RENEWING).
#[cfg(target_os = "linux")]
fn renew(lease: &Lease) -> Result<Lease, NetError> {
    request_keep(lease, /*broadcast=*/ false)
}

/// Broadcast REQUEST (REBINDING).
#[cfg(target_os = "linux")]
fn rebind(lease: &Lease) -> Result<Lease, NetError> {
    request_keep(lease, /*broadcast=*/ true)
}

#[cfg(target_os = "linux")]
fn request_keep(lease: &Lease, broadcast: bool) -> Result<Lease, NetError> {
    use dhcproto::v4::{
        DhcpOption, Encodable, Encoder, Flags, Message, MessageType, Opcode, OptionCode,
    };
    use rand::RngCore;
    use std::net::SocketAddrV4;

    let mac = read_mac(&lease.iface)?;
    let client_id = client_id_from_mac(&mac);
    let mut xid_bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut xid_bytes);
    let xid = u32::from_be_bytes(xid_bytes);

    // Bind to the leased address so ciaddr traffic is sourced correctly.
    let sock = open_dhcp_socket(&lease.iface, lease.ip)?;
    let dest = if broadcast {
        SocketAddrV4::new(Ipv4Addr::BROADCAST, 67)
    } else {
        SocketAddrV4::new(lease.server, 67)
    };
    let deadline = Instant::now() + Duration::from_secs(10);

    let mut msg = Message::default();
    msg.set_opcode(Opcode::BootRequest);
    msg.set_xid(xid);
    if broadcast {
        msg.set_flags(Flags::default().set_broadcast());
    } else {
        msg.set_flags(Flags::default());
    }
    msg.set_ciaddr(lease.ip);
    msg.set_chaddr(&mac);
    msg.opts_mut()
        .insert(DhcpOption::MessageType(MessageType::Request));
    msg.opts_mut()
        .insert(DhcpOption::ClientIdentifier(client_id));
    msg.opts_mut()
        .insert(DhcpOption::ParameterRequestList(param_request_list()));
    // RFC 2131: RENEWING/REBINDING must not include Requested IP / Server ID.

    let mut buf = Vec::new();
    msg.encode(&mut Encoder::new(&mut buf))
        .map_err(|e| NetError::Msg(format!("dhcp encode renew/rebind: {e}")))?;
    let kind = if broadcast { "rebind" } else { "renew" };
    tracing::debug!(interface = %lease.iface, %dest, kind, "DHCP keep-alive request");
    sock.send_to(&buf, dest)
        .map_err(|e| NetError::Msg(format!("dhcp {kind} send: {e}")))?;

    while Instant::now() < deadline {
        if let Some(resp) = recv_matching(&sock, xid, deadline) {
            match resp.opts().get(OptionCode::MessageType) {
                Some(DhcpOption::MessageType(MessageType::Ack)) => {
                    let server = match resp.opts().get(OptionCode::ServerIdentifier) {
                        Some(DhcpOption::ServerIdentifier(ip)) => *ip,
                        _ => lease.server,
                    };
                    return lease_from_ack(&lease.iface, &resp, server);
                }
                Some(DhcpOption::MessageType(MessageType::Nak)) => {
                    return Err(NetError::Msg(format!(
                        "DHCP NAK during {kind} on {}",
                        lease.iface
                    )));
                }
                _ => {}
            }
        }
    }
    Err(NetError::Msg(format!(
        "DHCP no ACK during {kind} on {}",
        lease.iface
    )))
}

#[cfg(target_os = "linux")]
fn apply_lease(lease: &Lease) -> Result<(), NetError> {
    crate::link::apply_dhcp_v4_lease(&lease.iface, lease.ip, lease.prefix, &lease.routers)?;
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;
        let addrs = rt
            .block_on(crate::link::list_addresses(&lease.iface))
            .unwrap_or_default();
        if !addrs.iter().any(|a| a.contains('.')) {
            return Err(NetError::Msg(format!(
                "DHCP ACK {}/{} but no IPv4 on {} afterwards (addrs={addrs:?})",
                lease.ip, lease.prefix, lease.iface
            )));
        }
    }
    if !lease.dns.is_empty() {
        let servers: Vec<String> = lease.dns.iter().map(ToString::to_string).collect();
        let _ = crate::dns::write_resolv_conf(&servers);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn lease_from_ack(
    iface: &str,
    ack: &dhcproto::v4::Message,
    fallback_server: Ipv4Addr,
) -> Result<Lease, NetError> {
    use dhcproto::v4::{DhcpOption, OptionCode};

    let ip = {
        let y = ack.yiaddr();
        if y.is_unspecified() {
            // Renew/rebind ACKs may leave yiaddr unset; keep ciaddr.
            let c = ack.ciaddr();
            if c.is_unspecified() {
                return Err(NetError::Msg("DHCP ACK missing yiaddr/ciaddr".into()));
            }
            c
        } else {
            y
        }
    };
    let prefix = match ack.opts().get(OptionCode::SubnetMask) {
        Some(DhcpOption::SubnetMask(mask)) => ipv4_mask_to_prefix(*mask)?,
        _ => 24,
    };
    let server = match ack.opts().get(OptionCode::ServerIdentifier) {
        Some(DhcpOption::ServerIdentifier(ip)) => *ip,
        _ => fallback_server,
    };
    let mut routers = Vec::new();
    if let Some(DhcpOption::Router(r)) = ack.opts().get(OptionCode::Router) {
        routers.extend(r.iter().copied());
    }
    let mut dns = Vec::new();
    if let Some(DhcpOption::DomainNameServer(d)) = ack.opts().get(OptionCode::DomainNameServer) {
        dns.extend(d.iter().copied());
    }
    let lease_secs = match ack.opts().get(OptionCode::AddressLeaseTime) {
        Some(DhcpOption::AddressLeaseTime(t)) => *t,
        _ => 3600,
    };
    let (t1_secs, t2_secs) = lease_timers(lease_secs, ack);
    Ok(Lease {
        iface: iface.to_string(),
        ip,
        prefix,
        server,
        routers,
        dns,
        lease_secs,
        t1_secs,
        t2_secs,
        acquired: Instant::now(),
    })
}

/// Compute T1/T2 from ACK options or RFC defaults (50% / 87.5%).
#[cfg(target_os = "linux")]
fn lease_timers(lease_secs: u32, ack: &dhcproto::v4::Message) -> (u32, u32) {
    use dhcproto::v4::{DhcpOption, OptionCode};

    if lease_secs == u32::MAX {
        return (u32::MAX / 2, u32::MAX / 2 + u32::MAX / 4);
    }
    let t1 = match ack.opts().get(OptionCode::Renewal) {
        Some(DhcpOption::Renewal(t)) if *t > 0 && *t < lease_secs => *t,
        _ => (lease_secs / 2).max(1),
    };
    let t2 = match ack.opts().get(OptionCode::Rebinding) {
        Some(DhcpOption::Rebinding(t)) if *t > t1 && *t < lease_secs => *t,
        _ => ((lease_secs as u64 * 7) / 8).min(u64::from(lease_secs.saturating_sub(1))) as u32,
    };
    let t2 = t2.max(t1.saturating_add(1).min(lease_secs.saturating_sub(1).max(1)));
    (t1.min(t2.saturating_sub(1)).max(1), t2.max(1))
}

#[cfg(target_os = "linux")]
fn current_v4_lease(iface: &str) -> Option<Lease> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let addrs = rt.block_on(crate::link::list_addresses(iface)).ok()?;
    let (ip, prefix) = addrs.iter().find_map(|a| {
        let (ip_s, pref_s) = a.split_once('/')?;
        if !ip_s.contains('.') || ip_s.starts_with("127.") {
            return None;
        }
        let ip: Ipv4Addr = ip_s.parse().ok()?;
        let prefix: u8 = pref_s.parse().unwrap_or(24);
        Some((ip, prefix))
    })?;
    // Unknown server → rebind path (broadcast) after a short T1.
    Some(Lease {
        iface: iface.to_string(),
        ip,
        prefix,
        server: Ipv4Addr::BROADCAST,
        routers: Vec::new(),
        dns: Vec::new(),
        lease_secs: 600,
        t1_secs: 60,
        t2_secs: 300,
        acquired: Instant::now(),
    })
}

#[cfg(target_os = "linux")]
fn iface_has_v4(iface: &str) -> bool {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return false;
    };
    let addrs = rt
        .block_on(crate::link::list_addresses(iface))
        .unwrap_or_default();
    addrs
        .iter()
        .any(|a| a.contains('.') && !a.starts_with("127."))
}

#[cfg(target_os = "linux")]
fn open_dhcp_socket(iface: &str, bind_ip: Ipv4Addr) -> Result<std::net::UdpSocket, NetError> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::SocketAddrV4;

    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| NetError::Msg(format!("dhcp socket: {e}")))?;
    sock.set_reuse_address(true)
        .map_err(|e| NetError::Msg(format!("dhcp reuseaddr: {e}")))?;
    sock.set_broadcast(true)
        .map_err(|e| NetError::Msg(format!("dhcp broadcast: {e}")))?;
    if let Err(err) = sock.bind_device(Some(iface.as_bytes())) {
        tracing::warn!(interface = iface, error = %err, "DHCP bind_device failed; continuing");
    }
    sock.bind(&SocketAddrV4::new(bind_ip, 68).into())
        .map_err(|e| NetError::Msg(format!("dhcp bind {bind_ip}:68: {e}")))?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| NetError::Msg(format!("dhcp timeout: {e}")))?;
    Ok(sock.into())
}

#[cfg(target_os = "linux")]
fn recv_matching(
    sock: &std::net::UdpSocket,
    xid: u32,
    deadline: Instant,
) -> Option<dhcproto::v4::Message> {
    use dhcproto::v4::{Decodable, Decoder, Message};

    while Instant::now() < deadline {
        let mut rbuf = [0u8; 1500];
        match sock.recv_from(&mut rbuf) {
            Ok((n, from)) => {
                tracing::debug!(bytes = n, %from, "DHCP packet received");
                if let Ok(resp) = Message::decode(&mut Decoder::new(&rbuf[..n])) {
                    if resp.xid() == xid {
                        return Some(resp);
                    }
                    tracing::debug!(got = resp.xid(), want = xid, "ignoring DHCP xid mismatch");
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return None,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => return None,
            Err(err) => {
                tracing::warn!(error = %err, "DHCP recv error");
                return None;
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn param_request_list() -> Vec<dhcproto::v4::OptionCode> {
    use dhcproto::v4::OptionCode;
    vec![
        OptionCode::SubnetMask,
        OptionCode::Router,
        OptionCode::DomainNameServer,
        OptionCode::DomainName,
        OptionCode::AddressLeaseTime,
        OptionCode::Renewal,
        OptionCode::Rebinding,
    ]
}

#[cfg(target_os = "linux")]
fn client_id_from_mac(mac: &[u8; 6]) -> Vec<u8> {
    let mut client_id = vec![0x01];
    client_id.extend_from_slice(mac);
    client_id
}

#[cfg(target_os = "linux")]
fn sleep_interruptible(total: Duration, stop: &AtomicBool) {
    let mut left = total;
    while left > Duration::ZERO && !stop.load(Ordering::SeqCst) {
        let slice = left.min(Duration::from_secs(1));
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}

#[cfg(target_os = "linux")]
fn wait_iface(iface: &str, timeout: Duration) -> Result<(), NetError> {
    let path = format!("/sys/class/net/{iface}");
    let deadline = Instant::now() + timeout;
    while !std::path::Path::new(&path).exists() {
        if Instant::now() >= deadline {
            return Err(NetError::Msg(format!(
                "interface {iface} did not appear within {timeout:?}"
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_mac(iface: &str) -> Result<[u8; 6], NetError> {
    let s = std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))
        .map_err(|e| NetError::Msg(format!("read MAC {iface}: {e}")))?;
    let mut mac = [0u8; 6];
    let parts: Vec<_> = s.trim().split(':').collect();
    if parts.len() != 6 {
        return Err(NetError::Msg(format!("bad MAC for {iface}: {s}")));
    }
    for (i, p) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(p, 16)
            .map_err(|_| NetError::Msg(format!("bad MAC for {iface}: {s}")))?;
    }
    Ok(mac)
}

#[cfg(target_os = "linux")]
fn ipv4_mask_to_prefix(mask: Ipv4Addr) -> Result<u8, NetError> {
    let bits = u32::from(mask);
    if bits == 0 {
        return Ok(0);
    }
    let prefix = bits.count_ones() as u8;
    if bits.leading_ones() != u32::from(prefix) || bits.trailing_zeros() != 32 - u32::from(prefix) {
        return Err(NetError::Msg(format!("non-contiguous DHCP mask {mask}")));
    }
    Ok(prefix)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{ipv4_mask_to_prefix, lease_timers};
    use dhcproto::v4::{DhcpOption, Message, OptionCode};
    use std::net::Ipv4Addr;

    #[test]
    fn mask_to_prefix() {
        assert_eq!(
            ipv4_mask_to_prefix(Ipv4Addr::new(255, 255, 255, 0)).unwrap(),
            24
        );
        assert_eq!(
            ipv4_mask_to_prefix(Ipv4Addr::new(255, 255, 0, 0)).unwrap(),
            16
        );
        assert_eq!(
            ipv4_mask_to_prefix(Ipv4Addr::new(255, 255, 255, 252)).unwrap(),
            30
        );
    }

    #[test]
    fn timers_default_half_and_seven_eighths() {
        let ack = Message::default();
        let (t1, t2) = lease_timers(800, &ack);
        assert_eq!(t1, 400);
        assert_eq!(t2, 700);
    }

    #[test]
    fn timers_honor_server_options() {
        let mut ack = Message::default();
        ack.opts_mut().insert(DhcpOption::Renewal(100));
        ack.opts_mut().insert(DhcpOption::Rebinding(200));
        let (t1, t2) = lease_timers(400, &ack);
        assert_eq!(t1, 100);
        assert_eq!(t2, 200);
        // Silence unused import warning if OptionCode is only used via DhcpOption.
        let _ = OptionCode::Renewal;
    }

    #[test]
    fn timers_infinite() {
        let ack = Message::default();
        let (t1, t2) = lease_timers(u32::MAX, &ack);
        assert!(t1 > 0 && t2 > t1);
    }

    #[test]
    fn persists_and_loads_preferred_ip() {
        use super::{peek_persisted_ip, persist_lease, set_lease_dir, should_reclaim, Lease};
        use std::time::Instant;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        set_lease_dir(Some(dir.path()));
        let lease = Lease {
            iface: "eth0".into(),
            ip: Ipv4Addr::new(10, 1, 1, 50),
            prefix: 24,
            server: Ipv4Addr::new(10, 1, 1, 1),
            routers: vec![Ipv4Addr::new(10, 1, 1, 1)],
            dns: vec![],
            lease_secs: 3600,
            t1_secs: 1800,
            t2_secs: 3150,
            acquired: Instant::now(),
        };
        persist_lease(&lease);
        assert_eq!(
            peek_persisted_ip("eth0"),
            Some(Ipv4Addr::new(10, 1, 1, 50))
        );
        assert!(!should_reclaim(
            "eth0",
            Some(Ipv4Addr::new(10, 1, 1, 50))
        ));
        assert!(should_reclaim(
            "eth0",
            Some(Ipv4Addr::new(10, 1, 1, 99))
        ));
        set_lease_dir(None);
    }
}
