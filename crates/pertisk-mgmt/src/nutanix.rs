//! Nutanix Prism Element (AHV) client over REST API v2.0.
//!
//! Auth is HTTP Basic (username/password). Provider columns map:
//! token_id=user, token_secret=password, node=cluster/host name,
//! storage=storage container name, bridge=AHV network name.
//!
//! Default URL form: `https://<vip-or-cvm>:9440`.

use serde_json::Value;

use crate::error::{ApiResult, AppError};
use crate::proxmox::{
    json_f64_val, HypervisorCapacity, ProbeResult, ProxmoxNode, ProxmoxStorage, StorageValidation,
    TestResult, VmIdCheck, VmIdConflict,
};

#[derive(Debug, Clone)]
pub struct NutanixClient {
    pub url: String,
    pub username: String,
    pub password: String,
    pub insecure: bool,
}

#[derive(Debug, Clone)]
pub struct NutanixVm {
    pub uuid: String,
    pub name: String,
    pub power_state: Option<String>,
    #[allow(dead_code)]
    pub num_vcpus: Option<i64>,
    #[allow(dead_code)]
    pub memory_mb: Option<i64>,
    #[allow(dead_code)]
    pub mac: Option<String>,
}

#[derive(Debug, Clone)]
struct Inventory {
    #[allow(dead_code)]
    version: String,
    hosts: Vec<ProxmoxNode>,
    containers: Vec<ProxmoxStorage>,
    #[allow(dead_code)]
    networks: Vec<(String, String)>, // (name, uuid)
    vms: Vec<NutanixVm>,
    cluster_name: String,
}

impl NutanixClient {
    #[allow(dead_code)]
    pub fn new(url: String, username: String, password: String, insecure: bool) -> Self {
        Self {
            url,
            username,
            password,
            insecure,
        }
    }

    fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    fn api(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}/api/nutanix/v2.0/{}", self.base(), path)
    }

    fn api_v3(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}/api/nutanix/v3/{}", self.base(), path)
    }

    fn http(&self) -> ApiResult<reqwest::Client> {
        let mut b = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(15))
            .pool_max_idle_per_host(0);
        if self.insecure {
            b = b
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
        b.build().map_err(|e| AppError::Anyhow(e.into()))
    }

    fn map_req_err(&self, e: reqwest::Error) -> AppError {
        let mut msg = format!("nutanix request failed: {e}");
        if let Some(src) = std::error::Error::source(&e) {
            msg.push_str(&format!(" ({src})"));
        }
        if !self.insecure {
            msg.push_str(
                " — tip: enable Insecure TLS for lab self-signed certificates (edit provider)",
            );
        } else {
            msg.push_str(
                " — check URL reachability from this host (port 9440), credentials, and Prism Element",
            );
        }
        AppError::bad(msg)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> ApiResult<Value> {
        self.request_url(method, self.api(path), body).await
    }

    async fn request_url(
        &self,
        method: reqwest::Method,
        url: String,
        body: Option<&Value>,
    ) -> ApiResult<Value> {
        let client = self.http()?;
        let mut req = client
            .request(method, url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| self.map_req_err(e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.as_u16() == 404 {
            return Ok(Value::Null);
        }
        if !status.is_success() {
            let detail = if text.is_empty() {
                status.to_string()
            } else {
                text.chars().take(600).collect()
            };
            return Err(AppError::bad(format!("nutanix {status}: {detail}")));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| {
            AppError::bad(format!(
                "nutanix JSON parse failed: {e} ({})",
                text.chars().take(200).collect::<String>()
            ))
        })
    }

    async fn get(&self, path: &str) -> ApiResult<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }

    async fn put(&self, path: &str, body: &Value) -> ApiResult<Value> {
        self.request(reqwest::Method::PUT, path, Some(body)).await
    }

    async fn post(&self, path: &str, body: &Value) -> ApiResult<Value> {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    async fn delete(&self, path: &str) -> ApiResult<()> {
        let _ = self.request(reqwest::Method::DELETE, path, None).await?;
        Ok(())
    }

    async fn get_v3(&self, path: &str) -> ApiResult<Value> {
        self.request_url(reqwest::Method::GET, self.api_v3(path), None)
            .await
    }

    async fn put_v3(&self, path: &str, body: &Value) -> ApiResult<Value> {
        self.request_url(reqwest::Method::PUT, self.api_v3(path), Some(body))
            .await
    }

    fn entities(v: &Value) -> Vec<&Value> {
        if let Some(arr) = v.get("entities").and_then(|e| e.as_array()) {
            return arr.iter().collect();
        }
        if let Some(arr) = v.as_array() {
            return arr.iter().collect();
        }
        if v.is_object() && !v.is_null() {
            return vec![v];
        }
        Vec::new()
    }

    async fn inventory(&self) -> ApiResult<Inventory> {
        let cluster = self.get("cluster").await.unwrap_or(Value::Null);
        let version = cluster
            .get("version")
            .or_else(|| cluster.get("full_version"))
            .and_then(|v| v.as_str())
            .map(|s| format!("AOS {s}"))
            .unwrap_or_else(|| "Nutanix AHV".into());
        let cluster_name = cluster
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("cluster")
            .to_string();

        let hosts_json = self.get("hosts").await.unwrap_or(Value::Null);
        let mut hosts = Vec::new();
        for h in Self::entities(&hosts_json) {
            let name = h
                .get("name")
                .or_else(|| h.get("hypervisor_address"))
                .and_then(|v| v.as_str())
                .unwrap_or("host")
                .to_string();
            let status = h
                .get("state")
                .or_else(|| h.get("status"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            hosts.push(ProxmoxNode { node: name, status });
        }
        if hosts.is_empty() {
            hosts.push(ProxmoxNode {
                node: cluster_name.clone(),
                status: Some("online".into()),
            });
        }

        let containers_json = self.get("storage_containers").await?;
        let mut containers = Vec::new();
        for c in Self::entities(&containers_json) {
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let total = c
                .get("max_capacity")
                .or_else(|| c.get("storage_container_capacity_bytes"))
                .and_then(|v| v.as_i64());
            let used = c
                .get("usage_stats")
                .and_then(|u| u.get("storage.usage_bytes"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .or_else(|| c.get("usage_bytes").and_then(|v| v.as_i64()));
            let avail = match (total, used) {
                (Some(t), Some(u)) => Some((t - u).max(0)),
                (Some(t), None) => Some(t),
                _ => None,
            };
            containers.push(ProxmoxStorage {
                storage: name,
                type_: Some("container".into()),
                content: Some("images".into()),
                active: Some(1),
                enabled: Some(1),
                avail,
                total,
            });
        }

        let networks_json = self.get("networks").await.unwrap_or(Value::Null);
        let mut networks = Vec::new();
        for n in Self::entities(&networks_json) {
            let name = n
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let uuid = n
                .get("uuid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() && !uuid.is_empty() {
                networks.push((name, uuid));
            }
        }

        let vms_json = self.get("vms").await.unwrap_or(Value::Null);
        let mut vms = Vec::new();
        for vm in Self::entities(&vms_json) {
            if let Some(parsed) = parse_vm(vm) {
                vms.push(parsed);
            }
        }

        Ok(Inventory {
            version,
            hosts,
            containers,
            networks,
            vms,
            cluster_name,
        })
    }

    pub async fn test_connection(&self) -> ApiResult<TestResult> {
        let inv = self.inventory().await?;
        Ok(TestResult {
            ok: true,
            version: inv.version,
            nodes: inv.hosts,
            insecure: self.insecure,
            url: self.url.clone(),
        })
    }

    /// Fast API reachability (cluster GET, 2s cap).
    pub async fn ping(&self) -> bool {
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), self.get("cluster")).await,
            Ok(Ok(_))
        )
    }

    pub async fn list_storage(&self, _node: &str) -> ApiResult<Vec<ProxmoxStorage>> {
        Ok(self.inventory().await?.containers)
    }

    /// Cluster-wide CPU / memory (sum of hosts) plus selected storage container.
    pub async fn host_capacity(&self, node: &str, storage: &str) -> ApiResult<HypervisorCapacity> {
        let hosts_json = self.get("hosts").await.unwrap_or(Value::Null);
        let want = node.trim();
        let mut cpu_used = 0.0;
        let mut cpu_total = 0.0;
        let mut mem_used = 0.0;
        let mut mem_total = 0.0;
        let mut any = false;
        let mut node_name = want.to_string();
        for h in Self::entities(&hosts_json) {
            let name = h
                .get("name")
                .or_else(|| h.get("hypervisor_address"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !want.is_empty()
                && !name.eq_ignore_ascii_case(want)
                && !want.eq_ignore_ascii_case("cluster")
            {
                // Still include all hosts when node is the Prism cluster name.
                let cluster_like = want.is_empty() || name.is_empty();
                if !cluster_like && name != want {
                    continue;
                }
            }
            any = true;
            if node_name.is_empty() {
                node_name = name.to_string();
            }
            let cores = h
                .get("num_cpu_cores")
                .or_else(|| h.get("num_cpu_threads"))
                .and_then(json_f64_val)
                .or_else(|| {
                    let cap = h.get("cpu_capacity_in_hz").and_then(json_f64_val)?;
                    let hz = h.get("cpu_frequency_in_hz").and_then(json_f64_val)?;
                    if hz > 0.0 {
                        Some(cap / hz)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0);
            let stats = h.get("stats").or_else(|| h.get("usage_stats"));
            let ppm = stats
                .and_then(|s| s.get("hypervisor_cpu_usage_ppm"))
                .and_then(json_f64_val)
                .or_else(|| {
                    stats.and_then(|s| s.get("hypervisor.cpu_usage_ppm").and_then(json_f64_val))
                });
            let used_cores = ppm
                .map(|p| cores * (p / 1_000_000.0).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            cpu_total += cores;
            cpu_used += used_cores;
            let m_tot = h
                .get("memory_capacity_in_bytes")
                .and_then(json_f64_val)
                .unwrap_or(0.0);
            let m_use = stats
                .and_then(|s| {
                    s.get("memory_usage_bytes")
                        .or_else(|| s.get("hypervisor_memory_usage_bytes"))
                        .and_then(json_f64_val)
                })
                .or_else(|| {
                    let ppm = stats.and_then(|s| {
                        s.get("hypervisor_memory_usage_ppm")
                            .or_else(|| s.get("hypervisor.memory_usage_ppm"))
                            .and_then(json_f64_val)
                    })?;
                    Some(m_tot * (ppm / 1_000_000.0).clamp(0.0, 1.0))
                })
                .unwrap_or(0.0);
            mem_total += m_tot;
            mem_used += m_use;
        }
        if !any && !want.is_empty() {
            return Box::pin(self.host_capacity("", storage)).await;
        }
        let mut cap = HypervisorCapacity {
            cpu_used: (cpu_total > 0.0).then_some(cpu_used),
            cpu_total: (cpu_total > 0.0).then_some(cpu_total),
            mem_used_bytes: (mem_total > 0.0).then_some(mem_used),
            mem_total_bytes: (mem_total > 0.0).then_some(mem_total),
            node: if node_name.is_empty() {
                want.to_string()
            } else {
                node_name
            },
            storage: storage.to_string(),
            ..HypervisorCapacity::default()
        };
        if let Ok(list) = self.list_storage("").await {
            let st = list.iter().find(|s| s.storage == storage).or(list.first());
            if let Some(st) = st {
                cap.disk_total_bytes = st.total.map(|v| v as f64);
                cap.disk_avail_bytes = st.avail.map(|v| v as f64);
                cap.disk_used_bytes = match (st.total, st.avail) {
                    (Some(t), Some(a)) => Some((t - a).max(0) as f64),
                    _ => None,
                };
                cap.storage = st.storage.clone();
            }
        }
        Ok(cap)
    }

    #[allow(dead_code)]
    pub async fn list_networks(&self) -> ApiResult<Vec<String>> {
        Ok(self
            .inventory()
            .await?
            .networks
            .into_iter()
            .map(|(n, _)| n)
            .collect())
    }

    pub async fn list_vms(&self) -> ApiResult<Vec<NutanixVm>> {
        Ok(self.inventory().await?.vms)
    }

    pub async fn validate_storage(
        &self,
        node: &str,
        storage: &str,
    ) -> ApiResult<StorageValidation> {
        let inv = self.inventory().await?;
        let available: Vec<String> = inv.containers.iter().map(|s| s.storage.clone()).collect();
        let Some(found) = inv.containers.iter().find(|s| s.storage == storage) else {
            return Ok(StorageValidation {
                ok: false,
                storage: storage.to_string(),
                node: node.to_string(),
                type_: None,
                content: None,
                active: false,
                enabled: false,
                message: format!(
                    "storage container `{storage}` not found — available: {}",
                    if available.is_empty() {
                        "(none)".into()
                    } else {
                        available.join(", ")
                    }
                ),
                available,
            });
        };
        Ok(StorageValidation {
            ok: true,
            storage: storage.to_string(),
            node: node.to_string(),
            type_: found.type_.clone(),
            content: found.content.clone(),
            active: true,
            enabled: true,
            message: format!("storage container `{storage}` ok"),
            available,
        })
    }

    pub fn vm_name(prefix: Option<&str>, vmid: i64) -> String {
        match prefix.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => format!("{p}-{vmid}"),
            None => vmid.to_string(),
        }
    }

    async fn find_vm_for_vmid(
        &self,
        prefix: Option<&str>,
        vmid: i64,
    ) -> ApiResult<Option<NutanixVm>> {
        let want = Self::vm_name(prefix, vmid);
        let suffix = format!("-{vmid}");
        Ok(self
            .list_vms()
            .await?
            .into_iter()
            .find(|v| v.name == want || v.name == vmid.to_string() || v.name.ends_with(&suffix)))
    }

    pub async fn check_vmids(
        &self,
        _node: &str,
        start: i64,
        count: i64,
        prefix: Option<&str>,
    ) -> ApiResult<VmIdCheck> {
        if start < 1 {
            return Ok(VmIdCheck {
                ok: false,
                node: _node.to_string(),
                range_start: start,
                range_end: start,
                conflicts: vec![],
                free: vec![],
                message: "base VMID must be >= 1".into(),
            });
        }
        if count < 1 {
            return Ok(VmIdCheck {
                ok: false,
                node: _node.to_string(),
                range_start: start,
                range_end: start,
                conflicts: vec![],
                free: vec![],
                message: "VM count must be >= 1".into(),
            });
        }
        let end = start + count - 1;
        let existing = self.list_vms().await?;
        let mut conflicts = Vec::new();
        let mut free = Vec::new();
        for vmid in start..=end {
            let want = Self::vm_name(prefix, vmid);
            let suffix = format!("-{vmid}");
            if let Some(vm) = existing
                .iter()
                .find(|v| v.name == want || v.name.ends_with(&suffix) || v.name == vmid.to_string())
            {
                conflicts.push(VmIdConflict {
                    vmid,
                    name: Some(vm.name.clone()),
                    status: vm.power_state.clone(),
                });
            } else {
                free.push(vmid);
            }
        }
        let ok = conflicts.is_empty();
        let message = if ok {
            format!(
                "VM names {start}–{end} free on Nutanix (prefix={})",
                prefix.unwrap_or("")
            )
        } else {
            let detail = conflicts
                .iter()
                .map(|c| format!("{} ({})", c.vmid, c.name.as_deref().unwrap_or("unnamed")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("VM names already in use on Nutanix: {detail}")
        };
        Ok(VmIdCheck {
            ok,
            node: _node.to_string(),
            range_start: start,
            range_end: end,
            conflicts,
            free,
            message,
        })
    }

    pub async fn probe(
        &self,
        node: Option<&str>,
        storage: Option<&str>,
        network: Option<&str>,
    ) -> ApiResult<ProbeResult> {
        let inv = self.inventory().await?;
        let mut node_ok = true;
        let mut node_message = String::new();
        if let Some(n) = node {
            if n.is_empty() {
                node_ok = false;
                node_message = "cluster/host is required".into();
            } else if inv.hosts.iter().any(|x| x.node == n) || inv.cluster_name == n {
                node_message = format!("cluster/host `{n}` ok");
            } else if inv.hosts.len() == 1 {
                node_ok = true;
                node_message = format!(
                    "host ok (requested `{n}`, inventory `{}` / cluster `{}`)",
                    inv.hosts[0].node, inv.cluster_name
                );
            } else {
                node_ok = false;
                let mut names: Vec<_> = inv.hosts.iter().map(|x| x.node.as_str()).collect();
                names.push(inv.cluster_name.as_str());
                node_message = format!(
                    "host/cluster `{n}` not found — available: {}",
                    names.join(", ")
                );
            }
        }

        let storage_check = match (node, storage) {
            (Some(n), Some(s)) if node_ok && !s.is_empty() => {
                Some(self.validate_storage(n, s).await?)
            }
            _ => None,
        };
        let storage_ok = storage_check.as_ref().map(|s| s.ok).unwrap_or(true);

        if let Some(net) = network.filter(|s| !s.is_empty()) {
            if !inv.networks.iter().any(|(n, _)| n == net) {
                let avail = if inv.networks.is_empty() {
                    "(none)".to_string()
                } else {
                    inv.networks
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                node_ok = false;
                if node_message.is_empty() {
                    node_message = format!("network `{net}` not found — available: {avail}");
                } else {
                    node_message =
                        format!("{node_message}; network `{net}` not found — available: {avail}");
                }
            }
        }

        let ok = node_ok && storage_ok;
        Ok(ProbeResult {
            ok,
            version: inv.version,
            nodes: inv.hosts,
            insecure: self.insecure,
            url: self.url.clone(),
            node_ok,
            node_message,
            storage: storage_check,
            arch: Some("amd64".into()),
        })
    }

    async fn find_vm(&self, name: &str) -> ApiResult<Option<NutanixVm>> {
        Ok(self.list_vms().await?.into_iter().find(|v| v.name == name))
    }

    async fn set_power(&self, uuid: &str, transition: &str) -> ApiResult<()> {
        let body = serde_json::json!({ "transition": transition });
        let _ = self
            .post(&format!("vms/{uuid}/set_power_state"), &body)
            .await?;
        Ok(())
    }

    pub async fn power_off(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(());
        };
        if vm
            .power_state
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("powered_off"))
            .unwrap_or(false)
        {
            return Ok(());
        }
        self.set_power(&vm.uuid, "OFF").await
    }

    pub async fn power_on(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        if vm
            .power_state
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("on") || s.eq_ignore_ascii_case("powered_on"))
            .unwrap_or(false)
        {
            return Ok(());
        }
        self.set_power(&vm.uuid, "ON").await
    }

    /// Guest IPv4 from Prism NIC config (IPAM / learned address).
    pub async fn vm_ipv4(&self, name: &str) -> ApiResult<Option<String>> {
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(None);
        };
        let json = self
            .get(&format!("vms/{}?include_vm_nic_config=true", vm.uuid))
            .await
            .unwrap_or(Value::Null);
        Ok(first_nic_ipv4(&json))
    }

    /// Every guest IPv4 Prism knows about, including powered-off VMs
    /// (`requested_ip_address` / last learned address).
    pub async fn all_guest_ipv4s(&self) -> Vec<String> {
        let listed = match self.get("vms/?include_vm_nic_config=true").await {
            Ok(v) if !v.is_null() => v,
            _ => self
                .get("vms?include_vm_nic_config=true")
                .await
                .unwrap_or(Value::Null),
        };
        let mut ips = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for vm in Self::entities(&listed) {
            let got = all_nic_ipv4s(vm);
            if got.is_empty() {
                if let Some(uuid) = vm.get("uuid").and_then(|v| v.as_str()) {
                    missing.push(uuid.to_string());
                }
            } else {
                ips.extend(got);
            }
        }
        if ips.is_empty() && missing.is_empty() {
            if let Ok(vms) = self.list_vms().await {
                missing = vms.into_iter().map(|v| v.uuid).collect();
            }
        }
        if !missing.is_empty() {
            let futs = missing.into_iter().map(|uuid| {
                let this = self.clone();
                async move {
                    this.get(&format!("vms/{uuid}?include_vm_nic_config=true"))
                        .await
                        .ok()
                        .map(|json| all_nic_ipv4s(&json))
                        .unwrap_or_default()
                }
            });
            for extra in futures::future::join_all(futs).await {
                ips.extend(extra);
            }
        }
        unique_guest_ipv4s(ips)
    }

    pub async fn restart_vm_by_name(&self, name: &str) -> ApiResult<()> {
        let _ = self.power_off(name).await;
        self.wait_powered_off(name).await;
        self.power_on(name).await
    }

    async fn wait_powered_off(&self, name: &str) {
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if let Ok(Some(vm)) = self.find_vm(name).await {
                if vm
                    .power_state
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("powered_off"))
                    .unwrap_or(false)
                {
                    return;
                }
            }
        }
    }

    /// Pin UEFI to the OS image (bus:0). Extra IPAM netcfg virtio disks steal boot after
    /// a guest reboot and AHV firmware then shows "Unable to find valid boot device".
    pub async fn pin_uefi_os_disk(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        let bus = self.os_disk_bus(&vm.uuid).await;
        self.pin_uefi_os_disk_v2(&vm.uuid, &bus).await
    }

    /// Power off, pin UEFI via v2+v3, power on. Use when firmware is already hung.
    pub async fn pin_uefi_boot_and_power_cycle(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        let _ = self.power_off(name).await;
        self.wait_powered_off(name).await;
        let bus = self.os_disk_bus(&vm.uuid).await;
        let _ = self.pin_uefi_os_disk_v2(&vm.uuid, &bus).await;
        let _ = self.pin_uefi_os_disk_v3(&vm.uuid, &bus).await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        self.power_on(name).await
    }

    async fn os_disk_bus(&self, uuid: &str) -> String {
        let json = self
            .get(&format!("vms/{uuid}?include_vm_disk_config=true"))
            .await
            .unwrap_or(Value::Null);
        os_disk_bus_from_vm(&json)
    }

    async fn pin_uefi_os_disk_v2(&self, uuid: &str, bus: &str) -> ApiResult<()> {
        let body = serde_json::json!({
            "boot": {
                "uefi_boot": true,
                "secure_boot": false,
                "boot_device_type": "disk",
                "disk_address": { "device_bus": bus, "device_index": 0 }
            }
        });
        let _ = self.put(&format!("vms/{uuid}"), &body).await?;
        Ok(())
    }

    async fn pin_uefi_os_disk_v3(&self, uuid: &str, bus: &str) -> ApiResult<()> {
        let get3 = self.get_v3(&format!("vms/{uuid}")).await?;
        if get3.get("spec").and_then(|s| s.get("resources")).is_none() {
            return Ok(());
        }
        let adapter = get3
            .pointer("/spec/resources/disk_list/0/device_properties/disk_address/adapter_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| bus_to_v3_adapter(bus));
        let mut put3 = get3.clone();
        if let Some(obj) = put3.as_object_mut() {
            obj.remove("status");
        }
        if let Some(res) = put3.pointer_mut("/spec/resources") {
            if let Some(obj) = res.as_object_mut() {
                obj.insert("power_state".into(), Value::from("OFF"));
                let mut boot = obj
                    .get("boot_config")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                if let Some(b) = boot.as_object_mut() {
                    b.insert("boot_type".into(), Value::from("UEFI"));
                    b.insert(
                        "boot_device".into(),
                        serde_json::json!({
                            "disk_address": {
                                "adapter_type": adapter,
                                "device_index": 0
                            }
                        }),
                    );
                }
                obj.insert("boot_config".into(), boot);
            }
        }
        let _ = self.put_v3(&format!("vms/{uuid}"), &put3).await?;
        Ok(())
    }

    pub async fn delete_vm_by_name(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(());
        };
        let _ = self.power_off(name).await;
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if let Some(v) = self.find_vm(name).await? {
                if v.power_state
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("powered_off"))
                    .unwrap_or(false)
                {
                    break;
                }
            } else {
                return Ok(());
            }
        }
        self.delete(&format!("vms/{}?delete_snapshots=true", vm.uuid))
            .await
    }

    pub async fn delete_vm(&self, prefix: Option<&str>, vmid: i64) -> ApiResult<()> {
        let Some(vm) = self.find_vm_for_vmid(prefix, vmid).await? else {
            return Ok(());
        };
        self.delete_vm_by_name(&vm.name).await
    }

    pub async fn set_vm_hardware(
        &self,
        name: &str,
        cores: Option<i64>,
        memory_mb: Option<i64>,
    ) -> ApiResult<()> {
        if cores.is_none() && memory_mb.is_none() {
            return Ok(());
        }
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        // Fetch current config then PUT updates (PE requires full-ish body).
        let cur = self.get(&format!("vms/{}", vm.uuid)).await?;
        let mut body = cur.clone();
        if let Some(obj) = body.as_object_mut() {
            if let Some(c) = cores {
                obj.insert("num_vcpus".into(), Value::from(c));
                obj.insert("num_cores_per_vcpu".into(), Value::from(1));
            }
            if let Some(m) = memory_mb {
                obj.insert("memory_mb".into(), Value::from(m));
            }
            // Drop read-only noise that often breaks PUT.
            for k in [
                "vm_disk_info",
                "stats",
                "usage_stats",
                "host_uuid",
                "host_name",
            ] {
                obj.remove(k);
            }
        }
        let _ = self.put(&format!("vms/{}", vm.uuid), &body).await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn vm_mac(&self, name: &str) -> ApiResult<Option<String>> {
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(None);
        };
        if vm.mac.is_some() {
            return Ok(vm.mac);
        }
        let detail = self.get(&format!("vms/{}", vm.uuid)).await?;
        Ok(parse_vm(&detail).and_then(|v| v.mac))
    }

    pub async fn grow_vm_disk(&self, name: &str, disk_gb: i64) -> ApiResult<()> {
        if disk_gb < 1 {
            return Err(AppError::bad("disk_gb must be >= 1"));
        }
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        let cur = self.get(&format!("vms/{}", vm.uuid)).await?;
        let want_bytes = disk_gb.saturating_mul(1024 * 1024 * 1024);
        let disks = cur
            .get("vm_disk_info")
            .or_else(|| cur.get("vm_disks"))
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let Some(disk) = disks.iter().find(|d| {
            !d.get("is_cdrom").and_then(|v| v.as_bool()).unwrap_or(false)
                && !d.get("is_empty").and_then(|v| v.as_bool()).unwrap_or(false)
        }) else {
            return Err(AppError::bad(format!("VM `{name}` has no disk to grow")));
        };
        let disk_uuid = disk
            .get("disk_address")
            .and_then(|a| a.get("vmdisk_uuid"))
            .or_else(|| disk.get("vmdisk_uuid"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::bad(format!("VM `{name}` disk has no vmdisk_uuid")))?;
        let size = disk.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
        if size >= want_bytes {
            return Ok(());
        }
        // PE disk resize: update VM disk size via vms/{uuid}/disks/{disk_uuid} or PUT vm.
        let body = serde_json::json!({
            "size": want_bytes,
            "uuid": disk_uuid,
        });
        match self
            .put(&format!("vms/{}/disks/{}", vm.uuid, disk_uuid), &body)
            .await
        {
            Ok(_) => Ok(()),
            Err(_) => {
                // Fallback: rewrite vm_disks with larger size.
                let mut updated = cur.clone();
                let disk_key = if updated.get("vm_disk_info").is_some() {
                    "vm_disk_info"
                } else {
                    "vm_disks"
                };
                if let Some(arr) = updated.get_mut(disk_key).and_then(|d| d.as_array_mut()) {
                    for d in arr.iter_mut() {
                        let is_cd = d.get("is_cdrom").and_then(|v| v.as_bool()).unwrap_or(false);
                        if is_cd {
                            continue;
                        }
                        if let Some(obj) = d.as_object_mut() {
                            obj.insert("size".into(), Value::from(want_bytes));
                        }
                    }
                }
                if let Some(obj) = updated.as_object_mut() {
                    for k in ["stats", "usage_stats", "host_uuid", "host_name"] {
                        obj.remove(k);
                    }
                }
                let _ = self.put(&format!("vms/{}", vm.uuid), &updated).await?;
                Ok(())
            }
        }
    }
}

fn bus_to_v3_adapter(bus: &str) -> String {
    match bus.to_ascii_lowercase().as_str() {
        "scsi" => "SCSI".into(),
        "ide" => "IDE".into(),
        "sata" => "SATA".into(),
        _ => "PCI".into(),
    }
}

fn os_disk_bus_from_vm(json: &Value) -> String {
    let disks = json
        .get("vm_disks")
        .or_else(|| json.get("vm_disk_info"))
        .and_then(|d| d.as_array());
    let Some(disks) = disks else {
        return "pci".into();
    };
    for d in disks {
        if d.get("is_cdrom").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        if let Some(bus) = d
            .get("disk_address")
            .and_then(|a| a.get("device_bus"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_ascii_lowercase())
        {
            if !bus.is_empty() && bus != "null" {
                return bus;
            }
        }
    }
    "pci".into()
}

fn parse_vm(vm: &Value) -> Option<NutanixVm> {
    let uuid = vm.get("uuid")?.as_str()?.to_string();
    let name = vm
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&uuid)
        .to_string();
    let power_state = vm
        .get("power_state")
        .or_else(|| vm.get("state"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let num_vcpus = vm.get("num_vcpus").and_then(|v| v.as_i64());
    let memory_mb = vm.get("memory_mb").and_then(|v| v.as_i64());
    let mac = vm
        .get("vm_nics")
        .and_then(|n| n.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|nic| {
                nic.get("mac_address")
                    .or_else(|| nic.get("mac_addr"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_ascii_lowercase())
            })
        });
    Some(NutanixVm {
        uuid,
        name,
        power_state,
        num_vcpus,
        memory_mb,
        mac,
    })
}

/// First usable IPv4 on a Prism VM (v2 nic config).
pub fn first_nic_ipv4(vm: &Value) -> Option<String> {
    let root = nic_config_root(vm);
    let nics = root
        .get("vm_nics")
        .or_else(|| root.get("nic_list"))
        .and_then(|n| n.as_array())?;
    for nic in nics {
        for ip in nic_ipv4s(nic) {
            let ip = ip.split('/').next().unwrap_or(&ip).trim();
            if usable_guest_ipv4(ip) {
                return Some(ip.to_string());
            }
        }
    }
    None
}

fn nic_config_root(vm: &Value) -> &Value {
    if vm.get("vm_nics").is_some() || vm.get("nic_list").is_some() {
        vm
    } else if let Some(inner) = vm.get("vm").or_else(|| vm.get("status")) {
        inner
    } else if let Some(arr) = vm.get("entities").and_then(|e| e.as_array()) {
        arr.first().unwrap_or(vm)
    } else {
        vm
    }
}

/// Assigned + requested IPv4s (powered-off guests keep `requested_ip_address`).
fn all_nic_ipv4s(vm: &Value) -> Vec<String> {
    let root = nic_config_root(vm);
    let Some(nics) = root
        .get("vm_nics")
        .or_else(|| root.get("nic_list"))
        .and_then(|n| n.as_array())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for nic in nics {
        for ip in nic_ipv4s(nic) {
            let ip = ip.split('/').next().unwrap_or(&ip).trim();
            if usable_guest_ipv4(ip) {
                out.push(ip.to_string());
            }
        }
    }
    unique_guest_ipv4s(out)
}

fn unique_guest_ipv4s(mut ips: Vec<String>) -> Vec<String> {
    ips.sort();
    ips.dedup();
    ips
}

fn nic_ipv4s(nic: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_ip_value(nic.get("ip_addresses"), &mut out);
    collect_ip_value(nic.get("assigned_ips"), &mut out);
    collect_ip_value(nic.get("ip_address"), &mut out);
    collect_ip_value(nic.get("requested_ip_address"), &mut out);
    collect_ip_value(nic.get("endpoint_address"), &mut out);
    if let Some(list) = nic.get("ip_endpoint_list").and_then(|v| v.as_array()) {
        for e in list {
            collect_ip_value(e.get("ip"), &mut out);
            collect_ip_value(e.get("ip_address"), &mut out);
        }
    }
    out
}

fn collect_ip_value(v: Option<&Value>, out: &mut Vec<String>) {
    let Some(v) = v else {
        return;
    };
    if let Some(s) = v.as_str() {
        push_ip(s, out);
        return;
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(s) = item.as_str() {
                push_ip(s, out);
            } else if let Some(s) = item.get("ip").and_then(|x| x.as_str()) {
                push_ip(s, out);
            } else if let Some(s) = item.get("ip_address").and_then(|x| x.as_str()) {
                push_ip(s, out);
            }
        }
    }
}

fn push_ip(s: &str, out: &mut Vec<String>) {
    let t = s.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
}

fn usable_guest_ipv4(ip: &str) -> bool {
    let ip = ip.trim();
    if ip.contains(':') || !ip.contains('.') {
        return false;
    }
    let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    !(addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_broadcast()
        || addr.is_link_local()
        || addr.is_multicast())
        && addr.is_private()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_prism_nic_ipv4() {
        let vm = json!({
            "vm_nics": [{
                "mac_address": "52:54:00:00:00:d2",
                "ip_addresses": ["fe80::1", "10.1.1.40"],
                "requested_ip_address": "10.1.1.31"
            }]
        });
        assert_eq!(first_nic_ipv4(&vm).as_deref(), Some("10.1.1.40"));
        let all = all_nic_ipv4s(&vm);
        assert!(all.contains(&"10.1.1.40".into()));
        assert!(all.contains(&"10.1.1.31".into()));
    }

    #[test]
    fn powered_off_keeps_requested_ip() {
        let vm = json!({
            "power_state": "off",
            "vm_nics": [{
                "requested_ip_address": "10.1.1.248"
            }]
        });
        assert_eq!(first_nic_ipv4(&vm).as_deref(), Some("10.1.1.248"));
    }

    #[test]
    fn os_disk_bus_skips_cdrom() {
        let vm = json!({
            "vm_disks": [
                { "is_cdrom": true, "disk_address": { "device_bus": "ide", "device_index": 0 } },
                { "is_cdrom": false, "disk_address": { "device_bus": "PCI", "device_index": 0 } }
            ]
        });
        assert_eq!(os_disk_bus_from_vm(&vm), "pci");
    }
}
