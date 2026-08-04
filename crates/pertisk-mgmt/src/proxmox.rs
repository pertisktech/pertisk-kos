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

    /// Set CPU/memory on a QEMU VM (Proxmox `config` PUT). Values in MB / cores.
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
        let base = self.url.trim_end_matches('/');
        let url = format!("{base}/api2/json/nodes/{node}/qemu/{vmid}/config");
        let mut form = vec![];
        if let Some(c) = cores {
            form.push(("cores", c.to_string()));
        }
        if let Some(m) = memory_mb {
            form.push(("memory", m.to_string()));
        }
        let resp = self
            .client()?
            .put(&url)
            .header("Authorization", self.auth_header())
            .form(&form)
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::bad(format!(
                "set vm hardware {vmid} failed: {body}"
            )));
        }
        Ok(())
    }

    /// Grow the primary disk (`scsi0`) to at least `disk_gb` GiB (never shrinks).
    pub async fn grow_vm_disk(&self, node: &str, vmid: i64, disk_gb: i64) -> ApiResult<()> {
        if disk_gb < 1 {
            return Err(AppError::bad("disk_gb must be >= 1"));
        }
        let base = self.url.trim_end_matches('/');
        let url = format!("{base}/api2/json/nodes/{node}/qemu/{vmid}/resize");
        let size = format!("{disk_gb}G");
        let resp = self
            .client()?
            .put(&url)
            .header("Authorization", self.auth_header())
            .form(&[("disk", "scsi0"), ("size", size.as_str())])
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            // Already at/above size is often reported as an error — treat as soft ok.
            if body.contains("smaller") || body.contains("already") {
                return Ok(());
            }
            return Err(AppError::bad(format!(
                "resize disk {vmid} failed: {body}"
            )));
        }
        Ok(())
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
