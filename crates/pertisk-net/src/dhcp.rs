//! In-process DHCPv4 client (no BusyBox / shell / udhcpc).
//!
//! Sole lease path for production images.

#[cfg(target_os = "linux")]
use crate::apply::NetError;

#[cfg(target_os = "linux")]
pub fn run_dhcp(iface: &str) -> Result<(), NetError> {
    use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
    use std::time::{Duration, Instant};

    use dhcproto::v4::{
        Decodable, Decoder, DhcpOption, Encodable, Encoder, Flags, Message, MessageType, Opcode,
        OptionCode,
    };
    use rand::RngCore;
    use socket2::{Domain, Protocol, Socket, Type};

    wait_iface(iface, Duration::from_secs(15))?;
    // Ensure UP even if caller forgot — DHCPv4 needs carrier; IPv6 LL can
    // appear while we still have no IPv4.
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

    // Client id = 0x01 + MAC (Ethernet hardware type).
    let mut client_id = vec![0x01];
    client_id.extend_from_slice(&mac);

    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| NetError::Msg(format!("dhcp socket: {e}")))?;
    sock.set_reuse_address(true)
        .map_err(|e| NetError::Msg(format!("dhcp reuseaddr: {e}")))?;
    sock.set_broadcast(true)
        .map_err(|e| NetError::Msg(format!("dhcp broadcast: {e}")))?;
    if let Err(err) = sock.bind_device(Some(iface.as_bytes())) {
        tracing::warn!(interface = iface, error = %err, "DHCP bind_device failed; continuing");
    }
    sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 68).into())
        .map_err(|e| NetError::Msg(format!("dhcp bind :68: {e}")))?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| NetError::Msg(format!("dhcp timeout: {e}")))?;
    let sock: UdpSocket = sock.into();

    let dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, 67);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut offer: Option<Message> = None;
    let mut discovers = 0u32;

    // DISCOVER → OFFER
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
        msg.opts_mut().insert(DhcpOption::ParameterRequestList(vec![
            OptionCode::SubnetMask,
            OptionCode::Router,
            OptionCode::DomainNameServer,
            OptionCode::DomainName,
        ]));
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

        let mut rbuf = [0u8; 1500];
        match sock.recv_from(&mut rbuf) {
            Ok((n, from)) => {
                tracing::debug!(bytes = n, %from, "DHCP packet received");
                if let Ok(resp) = Message::decode(&mut Decoder::new(&rbuf[..n])) {
                    if resp.xid() == xid {
                        if let Some(DhcpOption::MessageType(MessageType::Offer)) =
                            resp.opts().get(OptionCode::MessageType)
                        {
                            offer = Some(resp);
                        } else {
                            tracing::debug!("ignoring non-offer DHCP message");
                        }
                    } else {
                        tracing::debug!(got = resp.xid(), want = xid, "ignoring DHCP xid mismatch");
                    }
                } else {
                    tracing::debug!(bytes = n, "failed to decode DHCP packet");
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(err) => {
                return Err(NetError::Msg(format!("dhcp recv offer: {err}")));
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

    // REQUEST → ACK
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
        msg.opts_mut().insert(DhcpOption::ParameterRequestList(vec![
            OptionCode::SubnetMask,
            OptionCode::Router,
            OptionCode::DomainNameServer,
            OptionCode::DomainName,
        ]));
        let mut buf = Vec::new();
        msg.encode(&mut Encoder::new(&mut buf))
            .map_err(|e| NetError::Msg(format!("dhcp encode request: {e}")))?;
        tracing::debug!(interface = iface, %yiaddr, "DHCP request");
        if let Err(err) = sock.send_to(&buf, dest) {
            tracing::warn!(interface = iface, error = %err, "DHCP request send failed");
        }

        let mut rbuf = [0u8; 1500];
        match sock.recv_from(&mut rbuf) {
            Ok((n, _)) => {
                if let Ok(resp) = Message::decode(&mut Decoder::new(&rbuf[..n])) {
                    if resp.xid() != xid {
                        continue;
                    }
                    match resp.opts().get(OptionCode::MessageType) {
                        Some(DhcpOption::MessageType(MessageType::Ack)) => ack = Some(resp),
                        Some(DhcpOption::MessageType(MessageType::Nak)) => {
                            return Err(NetError::Msg(format!("DHCP NAK on {iface}")));
                        }
                        _ => {}
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(err) => {
                return Err(NetError::Msg(format!("dhcp recv ack: {err}")));
            }
        }
    }

    let ack = ack.ok_or_else(|| NetError::Msg(format!("DHCP no ACK on {iface}")))?;
    let ip = ack.yiaddr();
    let prefix = match ack.opts().get(OptionCode::SubnetMask) {
        Some(DhcpOption::SubnetMask(mask)) => ipv4_mask_to_prefix(*mask)?,
        _ => 24,
    };
    let cidr = format!("{ip}/{prefix}");

    let mut routers: Vec<Ipv4Addr> = Vec::new();
    if let Some(DhcpOption::Router(r)) = ack.opts().get(OptionCode::Router) {
        routers.extend(r.iter().copied());
    }

    // ioctl apply — avoids flaky netlink on virtio.
    crate::link::apply_dhcp_v4_lease(iface, ip, prefix, &routers)?;

    // Verify IPv4 actually landed — IPv6 LL alone must not count as success.
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;
        let addrs = rt
            .block_on(crate::link::list_addresses(iface))
            .unwrap_or_default();
        if !addrs.iter().any(|a| a.contains('.')) {
            return Err(NetError::Msg(format!(
                "DHCP ACK {cidr} but no IPv4 on {iface} afterwards (addrs={addrs:?})"
            )));
        }
    }

    if let Some(DhcpOption::DomainNameServer(dns)) = ack.opts().get(OptionCode::DomainNameServer) {
        let servers: Vec<String> = dns.iter().map(ToString::to_string).collect();
        if !servers.is_empty() {
            let _ = crate::dns::write_resolv_conf(&servers);
        }
    }

    tracing::info!(interface = iface, %cidr, "DHCP bound (builtin)");
    Ok(())
}

#[cfg(target_os = "linux")]
fn wait_iface(iface: &str, timeout: std::time::Duration) -> Result<(), NetError> {
    use std::thread;
    use std::time::{Duration, Instant};

    let path = format!("/sys/class/net/{iface}");
    let deadline = Instant::now() + timeout;
    while !std::path::Path::new(&path).exists() {
        if Instant::now() >= deadline {
            return Err(NetError::Msg(format!(
                "interface {iface} did not appear within {timeout:?}"
            )));
        }
        thread::sleep(Duration::from_millis(200));
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
fn ipv4_mask_to_prefix(mask: std::net::Ipv4Addr) -> Result<u8, NetError> {
    let bits = u32::from(mask);
    if bits == 0 {
        return Ok(0);
    }
    let prefix = bits.count_ones() as u8;
    // Contiguous ones from the MSB.
    if bits.leading_ones() != u32::from(prefix) || bits.trailing_zeros() != 32 - u32::from(prefix) {
        return Err(NetError::Msg(format!("non-contiguous DHCP mask {mask}")));
    }
    Ok(prefix)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::ipv4_mask_to_prefix;
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
}
