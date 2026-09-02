use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiResult, AppError};

#[derive(Debug, Clone)]
pub struct ProxmoxClient {
    pub url: String,
    pub token_id: String,
    pub token_secret: String,
    pub insecure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxmoxNode {
    pub node: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxmoxStorage {
    pub storage: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub content: Option<String>,
    pub active: Option<i64>,
    pub enabled: Option<i64>,
    pub avail: Option<i64>,
    pub total: Option<i64>,
}

/// Hypervisor host capacity (CPU cores, memory/disk bytes).
#[derive(Debug, Clone, Default)]
pub struct HypervisorCapacity {
    pub cpu_used: Option<f64>,
    pub cpu_total: Option<f64>,
    pub mem_used_bytes: Option<f64>,
    pub mem_total_bytes: Option<f64>,
    pub disk_used_bytes: Option<f64>,
    pub disk_avail_bytes: Option<f64>,
    pub disk_total_bytes: Option<f64>,
    pub node: String,
    pub storage: String,
}

fn json_f64(v: Option<&Value>) -> Option<f64> {
    json_f64_val(v?)
}

pub(crate) fn json_f64_val(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_u64().map(|n| n as f64))
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

#[derive(Debug, Serialize)]
pub struct StorageValidation {
    pub ok: bool,
    pub storage: String,
    pub node: String,
    pub type_: Option<String>,
    pub content: Option<String>,
    pub active: bool,
    pub enabled: bool,
    pub message: String,
    /// Other storage ids on the node (for UI dropdowns).
    pub available: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct VmIdConflict {
    pub vmid: i64,
    pub name: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VmIdCheck {
    pub ok: bool,
    pub node: String,
    pub range_start: i64,
    pub range_end: i64,
    pub conflicts: Vec<VmIdConflict>,
    pub free: Vec<i64>,
    pub message: String,
}

impl ProxmoxClient {
    pub fn auth_header(&self) -> String {
        format!("PVEAPIToken={}={}", self.token_id, self.token_secret)
    }

    fn client(&self) -> ApiResult<reqwest::Client> {
        // Proxmox labs almost always use self-signed certs on an IP URL.
        // native-tls + invalid_hostnames is required (rustls alone is often not enough).
        let mut b = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(0);
        if self.insecure {
            b = b
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
        b.build().map_err(|e| AppError::Anyhow(e.into()))
    }

    fn map_req_err(&self, e: reqwest::Error) -> AppError {
        let mut msg = format!("proxmox request failed: {e}");
        if let Some(src) = std::error::Error::source(&e) {
            msg.push_str(&format!(" ({src})"));
        }
        if !self.insecure {
            msg.push_str(
                " — tip: enable Insecure TLS for lab self-signed certificates (edit provider)",
            );
        } else {
            msg.push_str(
                " — check URL reachability from this host, token, and that the Proxmox API is up",
            );
        }
        AppError::bad(msg)
    }

    async fn get_json(&self, path: &str) -> ApiResult<Value> {
        let base = self.url.trim_end_matches('/');
        let url = format!("{base}/api2/json{path}");
        let resp = self
            .client()?
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::bad(format!("proxmox {status}: {body}")));
        }
        resp.json()
            .await
            .map_err(|e| AppError::bad(format!("proxmox json: {e}")))
    }

    pub async fn test_connection(&self) -> ApiResult<TestResult> {
        let version = self.get_json("/version").await?;
        let nodes_v = self.get_json("/nodes").await?;
        let nodes: Vec<ProxmoxNode> = nodes_v
            .get("data")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let version_str = version
            .pointer("/data/version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        Ok(TestResult {
            ok: true,
            version: version_str,
            nodes,
            insecure: self.insecure,
            url: self.url.clone(),
        })
    }

    /// Fast API reachability (version only, 2s cap).
    pub async fn ping(&self) -> bool {
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), self.get_json("/version"),)
                .await,
            Ok(Ok(_))
        )
    }

    pub async fn list_storage(&self, node: &str) -> ApiResult<Vec<ProxmoxStorage>> {
        let v = self.get_json(&format!("/nodes/{node}/storage")).await?;
        let list: Vec<ProxmoxStorage> = v
            .get("data")
            .cloned()
            .and_then(|x| serde_json::from_value(x).ok())
            .unwrap_or_default();
        Ok(list)
    }

    /// Confirm `storage` exists on `node` and is usable for VM disks.
    pub async fn validate_storage(
        &self,
        node: &str,
        storage: &str,
    ) -> ApiResult<StorageValidation> {
        let list = self.list_storage(node).await?;
        let available: Vec<String> = list.iter().map(|s| s.storage.clone()).collect();
        let Some(found) = list.iter().find(|s| s.storage == storage) else {
            return Ok(StorageValidation {
                ok: false,
                storage: storage.to_string(),
                node: node.to_string(),
                type_: None,
                content: None,
                active: false,
                enabled: false,
                message: format!(
                    "storage `{storage}` not found on node `{node}` — available: {}",
                    if available.is_empty() {
                        "(none)".into()
                    } else {
                        available.join(", ")
                    }
                ),
                available,
            });
        };
        let enabled = found.enabled.unwrap_or(1) != 0;
        let active = found.active.unwrap_or(1) != 0;
        let content = found.content.clone().unwrap_or_default();
        let can_images = content
            .split(',')
            .any(|c| matches!(c.trim(), "images" | "rootdir" | "import"));
        let mut ok = enabled && active;
        let mut message = format!(
            "storage `{storage}` ok on `{node}` (type={}, content={})",
            found.type_.as_deref().unwrap_or("?"),
            if content.is_empty() { "?" } else { &content }
        );
        if !enabled {
            ok = false;
            message = format!("storage `{storage}` is disabled on node `{node}`");
        } else if !active {
            ok = false;
            message = format!("storage `{storage}` is not active on node `{node}`");
        } else if !content.is_empty() && !can_images {
            ok = false;
            message = format!(
                "storage `{storage}` content `{content}` cannot hold VM disks (need images/rootdir)"
            );
        }
        Ok(StorageValidation {
            ok,
            storage: storage.to_string(),
            node: node.to_string(),
            type_: found.type_.clone(),
            content: found.content.clone(),
            active,
            enabled,
            message,
            available,
        })
    }

    /// List QEMU VMs on a node (vmid / name / status).
    pub async fn list_qemu(
        &self,
        node: &str,
    ) -> ApiResult<Vec<(i64, Option<String>, Option<String>)>> {
        let v = self.get_json(&format!("/nodes/{node}/qemu")).await?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for item in arr {
                let vmid = item
                    .get("vmid")
                    .and_then(|x| x.as_i64())
                    .or_else(|| item.get("vmid").and_then(|x| x.as_u64()).map(|u| u as i64));
                let Some(vmid) = vmid else { continue };
                let name = item
                    .get("name")
                    .and_then(|x| x.as_str())
                    .map(str::to_string);
                let status = item
                    .get("status")
                    .and_then(|x| x.as_str())
                    .map(str::to_string);
                out.push((vmid, name, status));
            }
        }
        Ok(out)
    }

    /// Check whether VMIDs in `[start, start+count)` are free on the node.
    pub async fn check_vmids(&self, node: &str, start: i64, count: i64) -> ApiResult<VmIdCheck> {
        if start < 100 {
            return Ok(VmIdCheck {
                ok: false,
                node: node.to_string(),
                range_start: start,
                range_end: start,
                conflicts: vec![],
                free: vec![],
                message: "base VMID must be >= 100".into(),
            });
        }
        if count < 1 {
            return Ok(VmIdCheck {
                ok: false,
                node: node.to_string(),
                range_start: start,
                range_end: start,
                conflicts: vec![],
                free: vec![],
                message: "VM count must be >= 1".into(),
            });
        }
        let end = start + count - 1;
        let existing = self.list_qemu(node).await?;
        let mut conflicts = Vec::new();
        let mut free = Vec::new();
        for vmid in start..=end {
            if let Some((_, name, status)) = existing.iter().find(|(id, _, _)| *id == vmid) {
                conflicts.push(VmIdConflict {
                    vmid,
                    name: name.clone(),
                    status: status.clone(),
                });
            } else {
                free.push(vmid);
            }
        }
        let ok = conflicts.is_empty();
        let message = if ok {
            format!("VMIDs {start}–{end} are free on `{node}`")
        } else {
            let detail = conflicts
                .iter()
                .map(|c| format!("{} ({})", c.vmid, c.name.as_deref().unwrap_or("unnamed")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("VMIDs already in use on `{node}`: {detail}")
        };
        Ok(VmIdCheck {
            ok,
            node: node.to_string(),
            range_start: start,
            range_end: end,
            conflicts,
            free,
            message,
        })
    }

    /// Connection + optional node/storage checks used by provider probe/test.
    pub async fn probe(&self, node: Option<&str>, storage: Option<&str>) -> ApiResult<ProbeResult> {
        let conn = self.test_connection().await?;
        let mut node_ok = true;
        let mut node_message = String::new();
        let mut arch: Option<String> = None;
        if let Some(n) = node {
            if n.is_empty() {
                node_ok = false;
                node_message = "node is required".into();
            } else if !conn.nodes.iter().any(|x| x.node == n) {
                node_ok = false;
                let names: Vec<_> = conn.nodes.iter().map(|x| x.node.as_str()).collect();
                node_message = format!(
                    "node `{n}` not found — available: {}",
                    if names.is_empty() {
                        "(none)".into()
                    } else {
                        names.join(", ")
                    }
                );
            } else {
                node_message = format!("node `{n}` ok");
                match self.detect_node_arch(n).await {
                    Ok(a) => {
                        arch = Some(a.clone());
                        node_message = format!("node `{n}` ok (host arch={a})");
                    }
                    Err(e) => {
                        tracing::debug!(node = %n, error = %e, "could not detect node arch");
                    }
                }
            }
        }
        let storage_check = match (node, storage) {
            (Some(n), Some(s)) if node_ok && !s.is_empty() => {
                Some(self.validate_storage(n, s).await?)
            }
            _ => None,
        };
        let storage_ok = storage_check.as_ref().map(|s| s.ok).unwrap_or(true);
        let ok = conn.ok && node_ok && storage_ok;
        Ok(ProbeResult {
            ok,
            version: conn.version,
            nodes: conn.nodes,
            insecure: conn.insecure,
            url: conn.url,
            node_ok,
            node_message,
            storage: storage_check,
            arch,
        })
    }

    /// Map Proxmox node CPU/kernel machine to guest image arch (amd64|arm64).
    pub async fn detect_node_arch(&self, node: &str) -> ApiResult<String> {
        let v = self.get_json(&format!("/nodes/{node}/status")).await?;
        let machine = v
            .pointer("/data/current-kernel/machine")
            .and_then(|x| x.as_str())
            .or_else(|| {
                // Older PVE: "Linux 6.x.x #1 SMP … x86_64" / "aarch64"
                v.pointer("/data/kversion")
                    .and_then(|x| x.as_str())
                    .and_then(|kv| {
                        kv.split_whitespace().rev().find(|t| {
                            matches!(*t, "x86_64" | "amd64" | "aarch64" | "arm64" | "armv8l")
                        })
                    })
            })
            .unwrap_or("");
        Ok(normalize_host_arch(machine))
    }

    /// Node CPU / memory plus selected storage used / available / total.
    pub async fn host_capacity(&self, node: &str, storage: &str) -> ApiResult<HypervisorCapacity> {
        let status = self.get_json(&format!("/nodes/{node}/status")).await?;
        let data = status.get("data").cloned().unwrap_or(Value::Null);
        let cpu_frac = json_f64(data.get("cpu"));
        let cpu_total = data
            .pointer("/cpuinfo/cpus")
            .and_then(json_f64_val)
            .or_else(|| json_f64(data.get("maxcpu")));
        let cpu_used = match (cpu_frac, cpu_total) {
            (Some(f), Some(t)) => Some((f * t).clamp(0.0, t)),
            _ => None,
        };
        let mem = data.get("memory").cloned().unwrap_or(Value::Null);
        let mem_total = json_f64(mem.get("total"));
        let mem_used = json_f64(mem.get("used"));
        let mut cap = HypervisorCapacity {
            cpu_used,
            cpu_total,
            mem_used_bytes: mem_used,
            mem_total_bytes: mem_total,
            node: node.to_string(),
            storage: storage.to_string(),
            ..HypervisorCapacity::default()
        };
        if !storage.trim().is_empty() {
            if let Ok(list) = self.list_storage(node).await {
                if let Some(st) = list.iter().find(|s| s.storage == storage) {
                    cap.disk_total_bytes = st.total.map(|v| v as f64);
                    cap.disk_avail_bytes = st.avail.map(|v| v as f64);
                    cap.disk_used_bytes = match (st.total, st.avail) {
                        (Some(t), Some(a)) => Some((t - a).max(0) as f64),
                        _ => None,
                    };
                    cap.storage = st.storage.clone();
                }
            }
        }
        Ok(cap)
    }

    /// PUT/POST form helpers — Proxmox often returns HTTP 200 with `errors` in JSON.
    async fn put_form(&self, path: &str, form: &[(&str, String)]) -> ApiResult<String> {
        let base = self.url.trim_end_matches('/');
        let url = format!("{base}/api2/json{path}");
        let resp = self
            .client()?
            .put(&url)
            .header("Authorization", self.auth_header())
            .form(form)
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::bad(format!("proxmox {status}: {body}")));
        }
        if proxmox_body_has_errors(&body) {
            return Err(AppError::bad(format!("proxmox error: {body}")));
        }
        Ok(body)
    }

    async fn post_form(&self, path: &str, form: &[(&str, String)]) -> ApiResult<String> {
        let base = self.url.trim_end_matches('/');
        let url = format!("{base}/api2/json{path}");
        let resp = self
            .client()?
            .post(&url)
            .header("Authorization", self.auth_header())
            .form(form)
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::bad(format!("proxmox {status}: {body}")));
        }
        if proxmox_body_has_errors(&body) {
            return Err(AppError::bad(format!("proxmox error: {body}")));
        }
        Ok(body)
    }

    pub async fn vm_qmp_status(&self, node: &str, vmid: i64) -> ApiResult<String> {
        let v = self
            .get_json(&format!("/nodes/{node}/qemu/{vmid}/status/current"))
            .await?;
        Ok(v.pointer("/data/status")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string())
    }

    /// Guest IPv4 via qemu-ga (`network-get-interfaces`). None if the agent is down.
    pub async fn vm_guest_ipv4(&self, node: &str, vmid: i64) -> ApiResult<Option<String>> {
        let v = match self
            .get_json(&format!(
                "/nodes/{node}/qemu/{vmid}/agent/network-get-interfaces"
            ))
            .await
        {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        Ok(first_qemu_ga_ipv4(&v))
    }

    /// Cloud-init / ipconfig IPv4s from QEMU config (present while the VM is off).
    pub async fn all_guest_ipv4s(&self, node: &str) -> Vec<String> {
        let mut nodes = Vec::new();
        if !node.trim().is_empty() {
            nodes.push(node.to_string());
        } else if let Ok(v) = self.get_json("/nodes").await {
            if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
                for item in arr {
                    if let Some(n) = item.get("node").and_then(|x| x.as_str()) {
                        nodes.push(n.to_string());
                    }
                }
            }
        }
        let mut ips = Vec::new();
        for n in nodes {
            let Ok(vms) = self.list_qemu(&n).await else {
                continue;
            };
            let futs = vms.into_iter().map(|(vmid, _, _)| {
                let this = self.clone();
                let node = n.clone();
                async move {
                    this.get_json(&format!("/nodes/{node}/qemu/{vmid}/config"))
                        .await
                        .ok()
                        .map(|cfg| ipv4s_from_qemu_config(&cfg))
                        .unwrap_or_default()
                }
            });
            for extra in futures::future::join_all(futs).await {
                ips.extend(extra);
            }
        }
        ips.sort();
        ips.dedup();
        ips
    }

    /// Set CPU/memory on a QEMU VM (Proxmox `config` PUT). Values in MB / cores.
    ///
    /// Also enables `hotplug`/`numa` so increases can apply live when the guest
    /// supports it. Pending changes still need a QEMU stop+start (see
    /// [`Self::restart_vm`]) — a guest OS reboot alone does not apply them.
    pub async fn set_vm_hardware(
        &self,
        node: &str,
        vmid: i64,
        cores: Option<i64>,
        memory_mb: Option<i64>,
    ) -> ApiResult<()> {
        if cores.is_none() && memory_mb.is_none() {
            return Ok(());
        }
        let mut form: Vec<(&str, String)> = vec![("hotplug", "cpu,memory,disk,network,usb".into())];
        if memory_mb.is_some() {
            form.push(("numa", "1".into()));
        }
        if let Some(c) = cores {
            form.push(("cores", c.to_string()));
            // vcpus = currently plugged count (hotplug path); cores = max.
            form.push(("vcpus", c.to_string()));
        }
        if let Some(m) = memory_mb {
            form.push(("memory", m.to_string()));
        }
        self.put_form(&format!("/nodes/{node}/qemu/{vmid}/config"), &form)
            .await
            .map_err(|e| AppError::bad(format!("set vm hardware {vmid} failed: {e}")))?;
        Ok(())
    }

    /// Hard restart (stop → start) so pending CPU/memory config actually applies.
    pub async fn restart_vm(&self, node: &str, vmid: i64) -> ApiResult<()> {
        let status = self.vm_qmp_status(node, vmid).await.unwrap_or_default();
        if status == "stopped" {
            self.post_form(&format!("/nodes/{node}/qemu/{vmid}/status/start"), &[])
                .await
                .map_err(|e| AppError::bad(format!("start vm {vmid} failed: {e}")))?;
            return Ok(());
        }

        // Force stop — ACPI shutdown can hang if the guest is unhealthy.
        let _ = self
            .post_form(
                &format!("/nodes/{node}/qemu/{vmid}/status/stop"),
                &[("timeout", "30".into())],
            )
            .await;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let st = self.vm_qmp_status(node, vmid).await.unwrap_or_default();
            if st == "stopped" {
                break;
            }
        }
        self.post_form(&format!("/nodes/{node}/qemu/{vmid}/status/start"), &[])
            .await
            .map_err(|e| AppError::bad(format!("start vm {vmid} after resize failed: {e}")))?;
        Ok(())
    }

    /// Read current `scsi0` size in whole GiB (floor). Returns None if missing/unparsed.
    pub async fn vm_disk_gb(&self, node: &str, vmid: i64) -> ApiResult<Option<i64>> {
        let v = self
            .get_json(&format!("/nodes/{node}/qemu/{vmid}/config"))
            .await?;
        let scsi0 = v
            .pointer("/data/scsi0")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        Ok(parse_scsi0_size_gb(scsi0))
    }

    /// Grow the primary disk (`scsi0`) to at least `disk_gb` GiB (never shrinks).
    pub async fn grow_vm_disk(&self, node: &str, vmid: i64, disk_gb: i64) -> ApiResult<()> {
        if disk_gb < 1 {
            return Err(AppError::bad("disk_gb must be >= 1"));
        }
        if let Some(cur) = self.vm_disk_gb(node, vmid).await? {
            if cur >= disk_gb {
                return Ok(());
            }
        }
        let size = format!("{disk_gb}G");
        let form = vec![("disk", "scsi0".to_string()), ("size", size)];
        match self
            .put_form(&format!("/nodes/{node}/qemu/{vmid}/resize"), &form)
            .await
        {
            Ok(body) => {
                // Async task UPID — wait until finished.
                if let Ok(v) = serde_json::from_str::<Value>(&body) {
                    if let Some(upid) = v.get("data").and_then(|d| d.as_str()) {
                        if upid.starts_with("UPID:") {
                            self.wait_task(node, upid).await?;
                        }
                    }
                }
                // Verify grow landed (ZFS/local can report success while size lags).
                for _ in 0..10 {
                    if let Some(cur) = self.vm_disk_gb(node, vmid).await? {
                        if cur >= disk_gb {
                            return Ok(());
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                let got = self.vm_disk_gb(node, vmid).await?;
                Err(AppError::bad(format!(
                    "resize disk {vmid} to {disk_gb}G did not take effect (scsi0={got:?}G)"
                )))
            }
            Err(e) => {
                let msg = e.to_string();
                // Already at/above size is often reported as an error — treat as soft ok.
                if msg.contains("smaller") || msg.contains("already") || msg.contains("equal") {
                    return Ok(());
                }
                Err(AppError::bad(format!("resize disk {vmid} failed: {msg}")))
            }
        }
    }

    async fn wait_task(&self, node: &str, upid: &str) -> ApiResult<()> {
        let encoded = urlencoding_upid(upid);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            let v = self
                .get_json(&format!("/nodes/{node}/tasks/{encoded}/status"))
                .await?;
            let status = v
                .pointer("/data/status")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if status == "stopped" {
                let exit = v
                    .pointer("/data/exitstatus")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if exit == "OK" || exit.is_empty() {
                    return Ok(());
                }
                return Err(AppError::bad(format!("proxmox task {upid} failed: {exit}")));
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Err(AppError::bad(format!("proxmox task {upid} timed out")))
    }

    pub async fn delete_vm(&self, node: &str, vmid: i64) -> ApiResult<()> {
        // Failed creates often leave DB/node rows with VMIDs that never existed.
        // Probe first so we don't queue a Proxmox task that fails in the UI.
        if !self.vm_exists(node, vmid).await? {
            return Ok(());
        }

        let base = self.url.trim_end_matches('/');
        let stop_url = format!("{base}/api2/json/nodes/{node}/qemu/{vmid}/status/stop");
        let _ = self
            .client()?
            .post(&stop_url)
            .header("Authorization", self.auth_header())
            .send()
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Re-check after stop — VM may have vanished, or never had a config.
        if !self.vm_exists(node, vmid).await? {
            return Ok(());
        }

        let del_url = format!("{base}/api2/json/nodes/{node}/qemu/{vmid}");
        let resp = self
            .client()?
            .delete(&del_url)
            .header("Authorization", self.auth_header())
            .query(&[("purge", "1"), ("destroy-unreferenced-disks", "1")])
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            if proxmox_vm_missing(&body) {
                return Ok(());
            }
            return Err(AppError::bad(format!("delete vm failed: {body}")));
        }
        // Async delete returns UPID — wait and treat "already gone" as success.
        if let Ok(v) = serde_json::from_str::<Value>(&body) {
            if let Some(upid) = v.get("data").and_then(|d| d.as_str()) {
                if upid.starts_with("UPID:") {
                    match self.wait_task(node, upid).await {
                        Ok(()) => {}
                        Err(e) if proxmox_vm_missing(&e.to_string()) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    /// True if the QEMU VM config exists on the node.
    pub async fn vm_exists(&self, node: &str, vmid: i64) -> ApiResult<bool> {
        match self
            .get_json(&format!("/nodes/{node}/qemu/{vmid}/status/current"))
            .await
        {
            Ok(v) => Ok(v.get("data").is_some_and(|d| !d.is_null())),
            Err(e) => {
                if proxmox_vm_missing(&e.to_string()) {
                    Ok(false)
                } else {
                    // Ambiguous API error — assume present so delete still tries.
                    Ok(true)
                }
            }
        }
    }
}

/// Proxmox wording for a VM that was never created / already removed.
fn proxmox_vm_missing(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    b.contains("does not exist")
        || b.contains("not found")
        || b.contains("unable to find configuration")
        || b.contains("no such guest")
        || b.contains("no such vm")
        || b.contains("configuration file for vm")
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub version: String,
    pub nodes: Vec<ProxmoxNode>,
    pub insecure: bool,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct ProbeResult {
    pub ok: bool,
    pub version: String,
    pub nodes: Vec<ProxmoxNode>,
    pub insecure: bool,
    pub url: String,
    pub node_ok: bool,
    pub node_message: String,
    pub storage: Option<StorageValidation>,
    /// Detected host CPU arch mapped to guest image arch (amd64|arm64), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// Map uname/Proxmox machine string → guest image arch (amd64|arm64).
pub fn normalize_host_arch(machine: &str) -> String {
    let m = machine.trim().to_ascii_lowercase();
    if m.contains("aarch64") || m.contains("arm64") || m == "armv8l" || m.starts_with("arm") {
        "arm64".into()
    } else {
        // Default x86_64 / amd64 / unknown → amd64 (most lab hosts).
        "amd64".into()
    }
}

fn proxmox_body_has_errors(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        if !msg.is_empty() {
            return true;
        }
    }
    match v.get("errors") {
        Some(Value::Object(map)) => !map.is_empty(),
        Some(Value::Array(arr)) => !arr.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        _ => false,
    }
}

/// Parse `size=30G` / `size=32` from a Proxmox scsi0 config string → GiB.
fn parse_scsi0_size_gb(scsi0: &str) -> Option<i64> {
    for part in scsi0.split(',') {
        let part = part.trim();
        let Some(rest) = part.strip_prefix("size=") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(n) = rest.strip_suffix('G').or_else(|| rest.strip_suffix('g')) {
            return n.parse::<i64>().ok().filter(|v| *v > 0);
        }
        if let Some(n) = rest.strip_suffix('T').or_else(|| rest.strip_suffix('t')) {
            return n
                .parse::<i64>()
                .ok()
                .filter(|v| *v > 0)
                .map(|t| t.saturating_mul(1024));
        }
        if let Some(n) = rest.strip_suffix('M').or_else(|| rest.strip_suffix('m')) {
            return n.parse::<i64>().ok().map(|m| (m / 1024).max(0));
        }
        // Bare number is GiB in recent PVE.
        if let Ok(n) = rest.parse::<i64>() {
            if n > 0 {
                return Some(n);
            }
        }
    }
    None
}

fn urlencoding_upid(upid: &str) -> String {
    // Match jq @uri used by upload-vm (encode path segment).
    let mut out = String::with_capacity(upid.len() * 3);
    for b in upid.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn first_qemu_ga_ipv4(v: &Value) -> Option<String> {
    let ifaces = v
        .pointer("/data/result")
        .or_else(|| v.get("result"))
        .and_then(|x| x.as_array())?;
    for iface in ifaces {
        let name = iface.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name == "lo" || name.starts_with("lo:") {
            continue;
        }
        let addrs = iface
            .get("ip-addresses")
            .or_else(|| iface.get("ip_addresses"))
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        for addr in addrs {
            let ip = addr
                .get("ip-address")
                .or_else(|| addr.get("ip_address"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let ty = addr
                .get("ip-address-type")
                .or_else(|| addr.get("ip_address_type"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if ty == "ipv6" || ip.contains(':') || !ip.contains('.') {
                continue;
            }
            let Ok(v4) = ip.parse::<std::net::Ipv4Addr>() else {
                continue;
            };
            if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
                continue;
            }
            return Some(ip.to_string());
        }
    }
    None
}

pub fn parse_ipconfig_ipv4(s: &str) -> Option<String> {
    for part in s.split(',') {
        let part = part.trim();
        let Some(rest) = part.strip_prefix("ip=") else {
            continue;
        };
        let ip = rest.split('/').next().unwrap_or(rest).trim();
        if ip.is_empty() || ip.eq_ignore_ascii_case("dhcp") {
            return None;
        }
        if ip.parse::<std::net::Ipv4Addr>().is_ok() {
            return Some(ip.to_string());
        }
    }
    None
}

fn ipv4s_from_qemu_config(v: &Value) -> Vec<String> {
    let obj = v.get("data").unwrap_or(v);
    let Some(map) = obj.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, val) in map {
        if !k.starts_with("ipconfig") {
            continue;
        }
        if let Some(s) = val.as_str() {
            if let Some(ip) = parse_ipconfig_ipv4(s) {
                out.push(ip);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_qemu_ga_ipv4() {
        let v = json!({
            "data": {
                "result": [
                    {"name": "lo", "ip-addresses": [{"ip-address": "127.0.0.1", "ip-address-type": "ipv4"}]},
                    {"name": "eth0", "ip-addresses": [
                        {"ip-address": "fe80::1", "ip-address-type": "ipv6"},
                        {"ip-address": "10.1.1.40", "ip-address-type": "ipv4"}
                    ]}
                ]
            }
        });
        assert_eq!(first_qemu_ga_ipv4(&v).as_deref(), Some("10.1.1.40"));
    }

    #[test]
    fn parses_ipconfig_static() {
        assert_eq!(
            parse_ipconfig_ipv4("ip=10.1.1.248/24,gw=10.1.1.10").as_deref(),
            Some("10.1.1.248")
        );
        assert_eq!(parse_ipconfig_ipv4("ip=dhcp"), None);
        let cfg = json!({
            "data": { "ipconfig0": "ip=10.1.1.247/24,gw=10.1.1.10" }
        });
        assert_eq!(ipv4s_from_qemu_config(&cfg), vec!["10.1.1.247".to_string()]);
    }
}
