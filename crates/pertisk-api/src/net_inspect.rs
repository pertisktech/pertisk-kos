//! Host network inspect (lab ops).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetIfaceRow {
    pub name: String,
    pub operstate: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetInspectSnapshot {
    pub available: bool,
    pub message: String,
    pub interfaces: Vec<NetIfaceRow>,
}

/// List non-loopback interfaces with operstate + addresses.
pub fn inspect_net() -> NetInspectSnapshot {
    #[cfg(not(target_os = "linux"))]
    {
        NetInspectSnapshot {
            available: false,
            message: "net inspect is Linux-only".into(),
            interfaces: Vec::new(),
        }
    }
    #[cfg(target_os = "linux")]
    {
        use pertisk_net::{list_addresses, list_interfaces};

        let names = match list_interfaces() {
            Ok(n) => n,
            Err(err) => {
                return NetInspectSnapshot {
                    available: false,
                    message: format!("list interfaces failed: {err}"),
                    interfaces: Vec::new(),
                };
            }
        };
        let mut interfaces = Vec::with_capacity(names.len());
        for name in names {
            let operstate = std::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "?".into());
            let addresses = list_addresses(&name).unwrap_or_default();
            interfaces.push(NetIfaceRow {
                name,
                operstate,
                addresses,
            });
        }
        interfaces.sort_by(|a, b| a.name.cmp(&b.name));
        NetInspectSnapshot {
            available: true,
            message: format!("listed {} interface(s)", interfaces.len()),
            interfaces,
        }
    }
}
