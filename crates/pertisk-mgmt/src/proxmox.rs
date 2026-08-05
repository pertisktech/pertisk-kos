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

#[derive(Debug, Serialize, Deserialize)]
pub struct ProxmoxNode {
    pub node: String,
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
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
        format!(
            "PVEAPIToken={}={}",
            self.token_id, self.token_secret
        )
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
            return Err(AppError::bad(format!(
                "proxmox {status}: {body}"
            )));
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
        let can_images = content.split(',').any(|c| {
            matches!(c.trim(), "images" | "rootdir" | "import")
        });
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
    pub async fn list_qemu(&self, node: &str) -> ApiResult<Vec<(i64, Option<String>, Option<String>)>> {
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
    pub async fn check_vmids(
        &self,
        node: &str,
        start: i64,
        count: i64,
    ) -> ApiResult<VmIdCheck> {
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
                .map(|c| {
                    format!(
                        "{} ({})",
                        c.vmid,
                        c.name.as_deref().unwrap_or("unnamed")
                    )
                })
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
    pub async fn probe(
        &self,
        node: Option<&str>,
        storage: Option<&str>,
    ) -> ApiResult<ProbeResult> {
        let conn = self.test_connection().await?;
        let mut node_ok = true;
        let mut node_message = String::new();
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
        })
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
        let mut form: Vec<(&str, String)> = vec![(
            "hotplug",
            "cpu,memory,disk,network,usb".into(),
        )];
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
            self.post_form(
                &format!("/nodes/{node}/qemu/{vmid}/status/start"),
                &[],
            )
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
        self.post_form(
            &format!("/nodes/{node}/qemu/{vmid}/status/start"),
            &[],
        )
        .await
        .map_err(|e| AppError::bad(format!("start vm {vmid} after resize failed: {e}")))?;
        Ok(())
    }

    /// Grow the primary disk (`scsi0`) to at least `disk_gb` GiB (never shrinks).
    pub async fn grow_vm_disk(&self, node: &str, vmid: i64, disk_gb: i64) -> ApiResult<()> {
        if disk_gb < 1 {
            return Err(AppError::bad("disk_gb must be >= 1"));
        }
        let size = format!("{disk_gb}G");
        let form = vec![
            ("disk", "scsi0".to_string()),
            ("size", size),
        ];
        match self
            .put_form(&format!("/nodes/{node}/qemu/{vmid}/resize"), &form)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                // Already at/above size is often reported as an error — treat as soft ok.
                if msg.contains("smaller")
                    || msg.contains("already")
                    || msg.contains("equal")
                {
                    return Ok(());
                }
                Err(AppError::bad(format!("resize disk {vmid} failed: {msg}")))
            }
        }
    }

    pub async fn delete_vm(&self, node: &str, vmid: i64) -> ApiResult<()> {
        let base = self.url.trim_end_matches('/');
        let stop_url = format!("{base}/api2/json/nodes/{node}/qemu/{vmid}/status/stop");
        let _ = self
            .client()?
            .post(&stop_url)
            .header("Authorization", self.auth_header())
            .send()
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let del_url = format!("{base}/api2/json/nodes/{node}/qemu/{vmid}");
        let resp = self
            .client()?
            .delete(&del_url)
            .header("Authorization", self.auth_header())
            .query(&[("purge", "1"), ("destroy-unreferenced-disks", "1")])
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            if !body.contains("does not exist") && !body.contains("not found") {
                return Err(AppError::bad(format!("delete vm failed: {body}")));
            }
        }
        Ok(())
    }
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
