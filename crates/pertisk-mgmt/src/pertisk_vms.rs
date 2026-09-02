//! pertisk-vms (pertiskd) client over REST `/v1`.
//!
//! Auth is username/password → Bearer token. Provider columns map:
//! token_id=user, token_secret=password, node=cluster member name,
//! storage=`replica`|`rbd`, bridge=network name or Linux bridge (`vmbr0`).
//!
//! Default URL: `https://<host>:7443` (HTTPS) or `http://<host>:7480`.

use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{ApiResult, AppError};
use crate::proxmox::{
    json_f64_val, normalize_host_arch, HypervisorCapacity, ProbeResult, ProxmoxNode, ProxmoxStorage,
    StorageValidation, TestResult, VmIdCheck, VmIdConflict,
};

#[derive(Debug)]
pub struct PertiskVmsClient {
    pub url: String,
    pub username: String,
    pub password: String,
    pub insecure: bool,
    token: Mutex<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct PertiskVm {
    pub id: String,
    pub name: String,
    pub state: Option<String>,
    pub ip: Option<String>,
    pub volume_id: Option<String>,
}

impl PertiskVmsClient {
    pub fn new(url: String, username: String, password: String, insecure: bool) -> Self {
        Self {
            url,
            username,
            password,
            insecure,
            token: Mutex::new(None),
        }
    }

    fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    fn api(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}/{}", self.base(), path)
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
        let mut msg = format!("pertisk-vms request failed: {e}");
        if let Some(src) = std::error::Error::source(&e) {
            msg.push_str(&format!(" ({src})"));
        }
        if !self.insecure {
            msg.push_str(
                " — tip: enable Insecure TLS for lab self-signed certificates (edit provider)",
            );
        } else {
            msg.push_str(
                " — check URL reachability (port 7443/7480), credentials, and that pertiskd is up",
            );
        }
        AppError::bad(msg)
    }

    async fn login(&self) -> ApiResult<String> {
        let client = self.http()?;
        let resp = client
            .post(self.api("v1/login"))
            .json(&serde_json::json!({
                "username": self.username,
                "password": self.password,
            }))
            .send()
            .await
            .map_err(|e| self.map_req_err(e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::bad(format!(
                "pertisk-vms login {status}: {}",
                text.chars().take(400).collect::<String>()
            )));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| {
            AppError::bad(format!("pertisk-vms login JSON: {e}"))
        })?;
        let token = v
            .get("token")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::bad("pertisk-vms login: missing token"))?;
        *self.token.lock().await = Some(token.to_string());
        Ok(token.to_string())
    }

    async fn bearer(&self) -> ApiResult<String> {
        if let Some(t) = self.token.lock().await.clone() {
            return Ok(t);
        }
        self.login().await
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> ApiResult<Value> {
        self.request_allow_empty(method, path, body, false).await
    }

    async fn request_allow_empty(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        allow_empty: bool,
    ) -> ApiResult<Value> {
        let mut token = self.bearer().await?;
        for attempt in 0..2 {
            let client = self.http()?;
            let mut req = client
                .request(method.clone(), self.api(path))
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/json");
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await.map_err(|e| self.map_req_err(e))?;
            let status = resp.status();
            if status.as_u16() == 401 && attempt == 0 {
                token = self.login().await?;
                continue;
            }
            if status.as_u16() == 404 {
                return Ok(Value::Null);
            }
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 204 || (allow_empty && text.trim().is_empty()) {
                return Ok(Value::Null);
            }
            if !status.is_success() {
                let detail = if text.is_empty() {
                    status.to_string()
                } else {
                    text.chars().take(600).collect()
                };
                return Err(AppError::bad(format!("pertisk-vms {status}: {detail}")));
            }
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text).map_err(|e| {
                AppError::bad(format!(
                    "pertisk-vms JSON parse failed: {e} ({})",
                    text.chars().take(200).collect::<String>()
                ))
            });
        }
        Err(AppError::bad("pertisk-vms: unauthorized"))
    }

    async fn get(&self, path: &str) -> ApiResult<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }

    pub async fn test_connection(&self) -> ApiResult<TestResult> {
        let _ = self.login().await?;
        let host = self.get("v1/host").await?;
        let cluster = self.get("v1/cluster").await.unwrap_or(Value::Null);
        let version = host
            .get("driver")
            .and_then(|v| v.as_str())
            .unwrap_or("pertisk-vms")
            .to_string();
        Ok(TestResult {
            ok: true,
            version,
            nodes: cluster_members_as_nodes(&cluster),
            insecure: self.insecure,
            url: self.url.clone(),
        })
    }

    pub async fn ping(&self) -> bool {
        let fut = async {
            let client = self.http()?;
            let resp = client
                .get(self.api("v1/health"))
                .send()
                .await
                .map_err(|e| self.map_req_err(e))?;
            Ok::<_, AppError>(resp.status().is_success())
        };
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), fut).await,
            Ok(Ok(true))
        )
    }

    pub async fn list_storage(&self, _node: &str) -> ApiResult<Vec<ProxmoxStorage>> {
        let host = self.get("v1/host").await?;
        Ok(storage_rows_from_host(&host))
    }

    pub async fn host_capacity(&self, node: &str, storage: &str) -> ApiResult<HypervisorCapacity> {
        let cluster = self.get("v1/cluster").await.unwrap_or(Value::Null);
        let members = cluster.get("members").and_then(|m| m.as_array());
        let want = node.trim();
        let mut cpu_used = 0.0;
        let mut cpu_total = 0.0;
        let mut mem_used = 0.0;
        let mut mem_total = 0.0;
        let mut node_name = want.to_string();
        let mut any = false;
        if let Some(members) = members {
            for m in members {
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if !want.is_empty()
                    && !name.eq_ignore_ascii_case(want)
                    && members.len() > 1
                {
                    continue;
                }
                any = true;
                if node_name.is_empty() {
                    node_name = name.to_string();
                }
                cpu_total += json_f64_val(m.get("cpus").unwrap_or(&Value::Null)).unwrap_or(0.0);
                cpu_used += json_f64_val(m.get("used_vcpus").unwrap_or(&Value::Null)).unwrap_or(0.0);
                mem_total += json_f64_val(m.get("memory_mib").unwrap_or(&Value::Null)).unwrap_or(0.0)
                    * 1024.0
                    * 1024.0;
                mem_used += json_f64_val(m.get("used_memory_mib").unwrap_or(&Value::Null))
                    .unwrap_or(0.0)
                    * 1024.0
                    * 1024.0;
            }
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
        if let Ok(vols) = self.get("v1/volumes").await {
            if let Some(arr) = vols.as_array() {
                let used: f64 = arr
                    .iter()
                    .filter_map(|v| json_f64_val(v.get("size_bytes").unwrap_or(&Value::Null)))
                    .sum();
                if used > 0.0 {
                    cap.disk_used_bytes = Some(used);
                }
            }
        }
        Ok(cap)
    }

    pub fn vm_name(prefix: Option<&str>, vmid: i64) -> String {
        match prefix.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => format!("{p}-{vmid}"),
            None => vmid.to_string(),
        }
    }

    pub async fn list_vms(&self) -> ApiResult<Vec<PertiskVm>> {
        let v = self.get("v1/vms").await?;
        Ok(parse_vms(&v))
    }

    async fn find_vm(&self, name: &str) -> ApiResult<Option<PertiskVm>> {
        Ok(self.list_vms().await?.into_iter().find(|v| v.name == name))
    }

    async fn find_vm_for_vmid(
        &self,
        prefix: Option<&str>,
        vmid: i64,
    ) -> ApiResult<Option<PertiskVm>> {
        let want = Self::vm_name(prefix, vmid);
        let suffix = format!("-{vmid}");
        let id_s = vmid.to_string();
        Ok(self.list_vms().await?.into_iter().find(|v| {
            v.name == want
                || v.name == id_s
                || v.name.ends_with(&suffix)
                || v.id == id_s
        }))
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
            let id_s = vmid.to_string();
            if let Some(vm) = existing.iter().find(|v| {
                v.name == want
                    || v.name.ends_with(&suffix)
                    || v.name == id_s
                    || v.id == id_s
            }) {
                conflicts.push(VmIdConflict {
                    vmid,
                    name: Some(vm.name.clone()),
                    status: vm.state.clone(),
                });
            } else {
                free.push(vmid);
            }
        }
        let ok = conflicts.is_empty();
        let message = if ok {
            format!(
                "VMIDs {start}–{end} free on pertisk-vms (prefix={})",
                prefix.unwrap_or("")
            )
        } else {
            let detail = conflicts
                .iter()
                .map(|c| format!("{} ({})", c.vmid, c.name.as_deref().unwrap_or("unnamed")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("VM names/IDs already in use on pertisk-vms: {detail}")
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
        let host = self.get("v1/host").await?;
        let cluster = self.get("v1/cluster").await.unwrap_or(Value::Null);
        let nets = self.get("v1/networks").await.unwrap_or(Value::Null);
        let members = cluster_members_as_nodes(&cluster);
        let cluster_name = cluster
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut node_ok = true;
        let mut node_message = String::new();
        if let Some(n) = node {
            if n.is_empty() {
                node_ok = false;
                node_message = "node name is required".into();
            } else if members.iter().any(|x| x.node == n) || (!cluster_name.is_empty() && cluster_name == n)
            {
                node_message = format!("node `{n}` ok");
            } else if members.len() <= 1 {
                node_ok = true;
                let have = members
                    .first()
                    .map(|x| x.node.as_str())
                    .unwrap_or("local");
                node_message = format!("host ok (requested `{n}`, inventory `{have}`)");
            } else {
                node_ok = false;
                let names: Vec<_> = members.iter().map(|x| x.node.as_str()).collect();
                node_message = format!("node `{n}` not found — available: {}", names.join(", "));
            }
        }

        let storage_check = match (node, storage) {
            (Some(n), Some(s)) if node_ok && !s.is_empty() => {
                Some(validate_storage(&host, n, s))
            }
            _ => None,
        };
        let storage_ok = storage_check.as_ref().map(|s| s.ok).unwrap_or(true);

        if let Some(net) = network.filter(|s| !s.is_empty()) {
            let names = network_names(&nets);
            if names.is_empty() {
                if node_message.is_empty() {
                    node_message =
                        format!("network `{net}` not in inventory (will be created on first VM)");
                }
            } else if !names.iter().any(|n| n == net) {
                node_ok = false;
                let avail = names.join(", ");
                if node_message.is_empty() {
                    node_message = format!("network `{net}` not found — available: {avail}");
                } else {
                    node_message =
                        format!("{node_message}; network `{net}` not found — available: {avail}");
                }
            }
        }

        let arch = host
            .get("arch")
            .and_then(|v| v.as_str())
            .map(normalize_host_arch);
        let driver = host
            .get("driver")
            .and_then(|v| v.as_str())
            .unwrap_or("pertisk-vms");
        let ok = node_ok && storage_ok;
        Ok(ProbeResult {
            ok,
            version: driver.to_string(),
            nodes: members,
            insecure: self.insecure,
            url: self.url.clone(),
            node_ok,
            node_message,
            storage: storage_check,
            arch,
        })
    }

    pub async fn vm_ipv4(&self, name: &str) -> ApiResult<Option<String>> {
        Ok(self.find_vm(name).await?.and_then(|v| v.ip))
    }

    pub async fn delete_vm_by_name(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(());
        };
        let _ = self
            .request_allow_empty(
                reqwest::Method::POST,
                &format!("v1/vms/{}/stop", vm.id),
                None,
                true,
            )
            .await;
        let _ = self
            .request_allow_empty(
                reqwest::Method::DELETE,
                &format!("v1/vms/{}", vm.id),
                None,
                true,
            )
            .await?;
        Ok(())
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
        let mut body = serde_json::Map::new();
        if let Some(c) = cores {
            let v = u8::try_from(c).unwrap_or(u8::MAX).max(1);
            body.insert("vcpus".into(), Value::from(v));
        }
        if let Some(m) = memory_mb {
            let v = u32::try_from(m.max(64)).unwrap_or(u32::MAX);
            body.insert("memory_mib".into(), Value::from(v));
        }
        self.request(
            reqwest::Method::PATCH,
            &format!("v1/vms/{}", vm.id),
            Some(&Value::Object(body)),
        )
        .await?;
        Ok(())
    }

    pub async fn grow_vm_disk(&self, name: &str, disk_gb: i64) -> ApiResult<()> {
        if disk_gb < 1 {
            return Err(AppError::bad("disk_gb must be >= 1"));
        }
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        let Some(vol_id) = vm.volume_id.clone() else {
            return Err(AppError::bad(format!("VM `{name}` has no volume to grow")));
        };
        let size_bytes = disk_gb.saturating_mul(1024 * 1024 * 1024);
        self.request(
            reqwest::Method::POST,
            &format!("v1/volumes/{vol_id}/resize"),
            Some(&serde_json::json!({ "size_bytes": size_bytes })),
        )
        .await?;
        Ok(())
    }

    pub async fn restart_vm_by_name(&self, name: &str) -> ApiResult<()> {
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        self.request_allow_empty(
            reqwest::Method::POST,
            &format!("v1/vms/{}/restart", vm.id),
            None,
            true,
        )
        .await?;
        Ok(())
    }
}

fn cluster_members_as_nodes(cluster: &Value) -> Vec<ProxmoxNode> {
    cluster
        .get("members")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let node = m.get("name").and_then(|v| v.as_str())?.to_string();
                    let online = m.get("online").and_then(|v| v.as_bool()).unwrap_or(true);
                    Some(ProxmoxNode {
                        node,
                        status: Some(if online { "online".into() } else { "offline".into() }),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn storage_rows_from_host(host: &Value) -> Vec<ProxmoxStorage> {
    let backend = host
        .get("storage_backend")
        .and_then(|v| v.as_str())
        .unwrap_or("replica");
    let rbd = host.get("rbd").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut rows = vec![ProxmoxStorage {
        storage: "replica".into(),
        type_: Some("dir".into()),
        content: Some("images".into()),
        active: Some(1),
        enabled: Some(1),
        avail: None,
        total: None,
    }];
    if rbd || backend == "rbd" {
        rows.push(ProxmoxStorage {
            storage: "rbd".into(),
            type_: Some("rbd".into()),
            content: Some("images".into()),
            active: Some(1),
            enabled: Some(1),
            avail: None,
            total: None,
        });
    }
    rows
}

fn validate_storage(host: &Value, node: &str, storage: &str) -> StorageValidation {
    let rows = storage_rows_from_host(host);
    let available: Vec<String> = rows.iter().map(|s| s.storage.clone()).collect();
    let found = rows.iter().find(|s| s.storage.eq_ignore_ascii_case(storage));
    if let Some(found) = found {
        StorageValidation {
            ok: true,
            storage: found.storage.clone(),
            node: node.to_string(),
            type_: found.type_.clone(),
            content: found.content.clone(),
            active: true,
            enabled: true,
            message: format!("storage `{storage}` ok"),
            available,
        }
    } else {
        StorageValidation {
            ok: false,
            storage: storage.to_string(),
            node: node.to_string(),
            type_: None,
            content: None,
            active: false,
            enabled: false,
            message: format!(
                "storage `{storage}` not found — available: {}",
                if available.is_empty() {
                    "(none)".into()
                } else {
                    available.join(", ")
                }
            ),
            available,
        }
    }
}

fn network_names(nets: &Value) -> Vec<String> {
    let Some(arr) = nets.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for n in arr {
        if let Some(name) = n.get("name").and_then(|v| v.as_str()) {
            out.push(name.to_string());
        }
        if let Some(br) = n.get("bridge").and_then(|v| v.as_str()) {
            if !out.iter().any(|x| x == br) {
                out.push(br.to_string());
            }
        }
    }
    out
}

fn json_id(v: &Value) -> Option<String> {
    v.as_u64()
        .map(|n| n.to_string())
        .or_else(|| v.as_i64().map(|n| n.to_string()))
        .or_else(|| v.as_str().map(|s| s.to_string()))
}

fn parse_vms(v: &Value) -> Vec<PertiskVm> {
    let arr = v
        .as_array()
        .cloned()
        .or_else(|| {
            v.get("vms")
                .and_then(|x| x.as_array())
                .cloned()
        })
        .unwrap_or_default();
    arr.iter()
        .filter_map(|vm| {
            let id = json_id(vm.get("id")?)?;
            let spec = vm.get("spec").cloned().unwrap_or(Value::Null);
            let name = spec
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() && id.is_empty() {
                return None;
            }
            let ip = spec
                .get("nets")
                .and_then(|n| n.as_array())
                .and_then(|nets| {
                    nets.iter().find_map(|nic| {
                        nic.get("ip")
                            .and_then(|i| i.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                    })
                });
            let volume_id = spec
                .get("disks")
                .and_then(|d| d.as_array())
                .and_then(|disks| {
                    disks.iter().find_map(|disk| {
                        if disk.get("cdrom").and_then(|c| c.as_bool()).unwrap_or(false) {
                            return None;
                        }
                        disk.get("volume_id")
                            .and_then(json_id)
                            .filter(|s| !s.is_empty())
                    })
                });
            Some(PertiskVm {
                id,
                name,
                state: vm
                    .get("state")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string()),
                ip,
                volume_id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_name_prefix() {
        assert_eq!(PertiskVmsClient::vm_name(Some("lab"), 210), "lab-210");
        assert_eq!(PertiskVmsClient::vm_name(None, 210), "210");
    }

    #[test]
    fn parse_vm_list_numeric_id() {
        let v = serde_json::json!([{
            "id": 210,
            "state": "running",
            "spec": {
                "name": "lab-cp-1",
                "disks": [{"volume_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "cdrom": false}],
                "nets": [{"ip": "10.1.1.50"}]
            }
        }]);
        let vms = parse_vms(&v);
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].id, "210");
        assert_eq!(vms[0].name, "lab-cp-1");
        assert_eq!(vms[0].ip.as_deref(), Some("10.1.1.50"));
        assert!(vms[0].volume_id.is_some());
    }

    #[test]
    fn storage_rows_include_rbd_when_flagged() {
        let host = serde_json::json!({"storage_backend": "replica", "rbd": true});
        let rows = storage_rows_from_host(&host);
        assert!(rows.iter().any(|s| s.storage == "replica"));
        assert!(rows.iter().any(|s| s.storage == "rbd"));
        let v = validate_storage(&host, "n1", "replica");
        assert!(v.ok);
        let bad = validate_storage(&host, "n1", "local-lvm");
        assert!(!bad.ok);
    }

    #[test]
    fn network_names_include_bridge() {
        let nets = serde_json::json!([{"name": "lan", "bridge": "vmbr0"}]);
        let names = network_names(&nets);
        assert!(names.contains(&"lan".into()));
        assert!(names.contains(&"vmbr0".into()));
    }
}
