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
