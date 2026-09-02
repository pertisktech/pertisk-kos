//! Standalone ESXi (HostAgent) client over SOAP vim25.
//!
//! Auth is username/password → session cookie (`vmware_soap_session`).
//! Provider columns map: token_id=user, token_secret=password, node=host,
//! storage=datastore, bridge=network/portgroup.

use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{ApiResult, AppError};
use crate::proxmox::{
    HypervisorCapacity, ProbeResult, ProxmoxNode, ProxmoxStorage, StorageValidation, TestResult,
    VmIdCheck, VmIdConflict,
};

#[derive(Debug, Clone)]
pub struct VsphereClient {
    pub url: String,
    pub username: String,
    pub password: String,
    pub insecure: bool,
    /// Cached session cookie value (with or without quotes).
    session: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone)]
pub struct VsphereVm {
    pub moref: String,
    pub name: String,
    pub power_state: Option<String>,
    #[allow(dead_code)]
    pub num_cpu: Option<i64>,
    #[allow(dead_code)]
    pub memory_mb: Option<i64>,
    #[allow(dead_code)]
    pub mac: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct HostCap {
    name: String,
    cpu_cores: Option<f64>,
    cpu_mhz: Option<f64>,
    cpu_used_mhz: Option<f64>,
    mem_bytes: Option<f64>,
    mem_used_mb: Option<f64>,
}

#[derive(Debug, Clone)]
struct Inventory {
    #[allow(dead_code)]
    version: String,
    hosts: Vec<ProxmoxNode>,
    host_caps: Vec<HostCap>,
    datastores: Vec<ProxmoxStorage>,
    #[allow(dead_code)]
    networks: Vec<String>,
    vms: Vec<VsphereVm>,
    /// Datacenter inventory path for datastore browser (usually `ha-datacenter`).
    #[allow(dead_code)]
    dc_path: String,
    #[allow(dead_code)]
    vm_folder: String,
    #[allow(dead_code)]
    resource_pool: String,
    #[allow(dead_code)]
    host_moref: String,
}

impl VsphereClient {
    #[allow(dead_code)]
    pub fn new(url: String, username: String, password: String, insecure: bool) -> Self {
        Self {
            url,
            username,
            password,
            insecure,
            session: Arc::new(Mutex::new(None)),
        }
    }

    fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    fn sdk_url(&self) -> String {
        format!("{}/sdk", self.base())
    }

    fn http(&self) -> ApiResult<reqwest::Client> {
        let mut b = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(15))
            .cookie_store(true)
            .pool_max_idle_per_host(0);
        if self.insecure {
            b = b
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
        b.build().map_err(|e| AppError::Anyhow(e.into()))
    }

    fn map_req_err(&self, e: reqwest::Error) -> AppError {
        let mut msg = format!("vsphere request failed: {e}");
        if let Some(src) = std::error::Error::source(&e) {
            msg.push_str(&format!(" ({src})"));
        }
        if !self.insecure {
            msg.push_str(
                " — tip: enable Insecure TLS for lab self-signed certificates (edit provider)",
            );
        } else {
            msg.push_str(
                " — check URL reachability from this host, credentials, and that the ESXi API is up",
            );
        }
        AppError::bad(msg)
    }

    async fn soap(&self, action: &str, body: &str) -> ApiResult<String> {
        let client = self.http()?;
        let mut req = client
            .post(self.sdk_url())
            .header("Content-Type", "text/xml; charset=UTF-8")
            .header("SOAPAction", action)
            .body(envelope(body));
        if let Some(cookie) = self.session.lock().await.as_ref() {
            req = req.header("Cookie", format!("vmware_soap_session={cookie}"));
        }
        let resp = req.send().await.map_err(|e| self.map_req_err(e))?;
        // Capture session cookie from Set-Cookie if present.
        if let Some(sc) = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                extract_soap_session(s)
            })
        {
            *self.session.lock().await = Some(sc);
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::bad(format!("vsphere {status}: {text}")));
        }
        if text.contains("Fault>") || text.contains(":Fault") {
            let fault = xml_text(&text, "faultstring")
                .or_else(|| xml_text(&text, "faultString"))
                .unwrap_or_else(|| text.chars().take(400).collect());
            return Err(AppError::bad(format!("vsphere fault: {fault}")));
        }
        Ok(text)
    }

    pub async fn login(&self) -> ApiResult<()> {
        *self.session.lock().await = None;
        let user = xml_escape(&self.username);
        let pass = xml_escape(&self.password);
        let body = format!(
            r#"<Login xmlns="urn:vim25">
  <_this type="SessionManager">ha-sessionmgr</_this>
  <userName>{user}</userName>
  <password>{pass}</password>
</Login>"#
        );
        let resp = self.soap("urn:vim25/8.0.3.0", &body).await?;
        if xml_text(&resp, "key").is_none() && !resp.contains("LoginResponse") {
            return Err(AppError::bad("vsphere login failed"));
        }
        // Some ESXi builds only set cookie via Set-Cookie (handled in soap()).
        // Fallback: use session key as cookie if store still empty.
        if self.session.lock().await.is_none() {
            if let Some(key) = xml_text(&resp, "key") {
                *self.session.lock().await = Some(format!("\"{key}\""));
            }
        }
        if self.session.lock().await.is_none() {
            return Err(AppError::bad(
                "vsphere login succeeded but no session cookie was returned",
            ));
        }
        Ok(())
    }

    pub async fn ensure_login(&self) -> ApiResult<()> {
        if self.session.lock().await.is_some() {
            return Ok(());
        }
        self.login().await
    }

    async fn retrieve_service_content(&self) -> ApiResult<String> {
        let body = r#"<RetrieveServiceContent xmlns="urn:vim25">
  <_this type="ServiceInstance">ServiceInstance</_this>
</RetrieveServiceContent>"#;
        self.soap("urn:vim25/8.0.3.0", body).await
    }

    async fn inventory(&self) -> ApiResult<Inventory> {
        self.ensure_login().await?;
        let about = self.retrieve_service_content().await?;
        let version = xml_text(&about, "fullName")
            .or_else(|| xml_text(&about, "version"))
            .unwrap_or_else(|| "ESXi".into());

        let body = TRAVERSAL_RETRIEVE;
        let xml = self.soap("urn:vim25/8.0.3.0", body).await?;

        let mut hosts = Vec::new();
        let mut host_caps = Vec::new();
        let mut datastores = Vec::new();
        let mut networks = Vec::new();
        let mut vms = Vec::new();
        let mut dc_path = "ha-datacenter".to_string();
        let mut vm_folder = "ha-folder-vm".to_string();
        let mut resource_pool = "ha-root-pool".to_string();
        let mut host_moref = "ha-host".to_string();

        for obj in split_objects(&xml) {
            let typ = obj_type(&obj).unwrap_or_default();
            let moref = obj_id(&obj).unwrap_or_default();
            let props = obj_props(&obj);
            match typ.as_str() {
                "HostSystem" => {
                    host_moref = moref.clone();
                    let name = props.get("name").cloned().unwrap_or_else(|| moref.clone());
                    hosts.push(ProxmoxNode {
                        node: name.clone(),
                        status: props.get("runtime.connectionState").cloned(),
                    });
                    let cores = props
                        .get("summary.hardware.numCpuCores")
                        .and_then(|s| s.parse().ok());
                    let mhz = props
                        .get("summary.hardware.cpuMhz")
                        .and_then(|s| s.parse().ok());
                    let used_mhz = props
                        .get("summary.quickStats.overallCpuUsage")
                        .and_then(|s| s.parse().ok());
                    let mem_bytes = props
                        .get("summary.hardware.memorySize")
                        .and_then(|s| s.parse().ok());
                    let mem_used_mb = props
                        .get("summary.quickStats.overallMemoryUsage")
                        .and_then(|s| s.parse().ok());
                    host_caps.push(HostCap {
                        name,
                        cpu_cores: cores,
                        cpu_mhz: mhz,
                        cpu_used_mhz: used_mhz,
                        mem_bytes,
                        mem_used_mb,
                    });
                }
                "Datastore" => {
                    let name = props.get("name").cloned().unwrap_or_else(|| moref.clone());
                    let accessible = props
                        .get("summary.accessible")
                        .map(|s| s == "true")
                        .unwrap_or(true);
                    datastores.push(ProxmoxStorage {
                        storage: name,
                        type_: props.get("summary.type").cloned(),
                        content: Some("images".into()),
                        active: Some(if accessible { 1 } else { 0 }),
                        enabled: Some(1),
                        avail: props.get("summary.freeSpace").and_then(|s| s.parse().ok()),
                        total: props.get("summary.capacity").and_then(|s| s.parse().ok()),
                    });
                }
                "Network" | "OpaqueNetwork" => {
                    if let Some(n) = props.get("name") {
                        networks.push(n.clone());
                    }
                }
                "VirtualMachine" => {
                    vms.push(VsphereVm {
                        moref: moref.clone(),
                        name: props.get("name").cloned().unwrap_or_else(|| moref.clone()),
                        power_state: props.get("runtime.powerState").cloned(),
                        num_cpu: props
                            .get("config.hardware.numCPU")
                            .and_then(|s| s.parse().ok()),
                        memory_mb: props
                            .get("config.hardware.memoryMB")
                            .and_then(|s| s.parse().ok()),
                        mac: None,
                    });
                }
                "Datacenter" => {
                    if let Some(n) = props.get("name") {
                        dc_path = n.clone();
                    }
                    if let Some(f) = props.get("vmFolder") {
                        vm_folder = f.clone();
                    }
                }
                "ResourcePool"
                    if moref.contains("root")
                        || props.get("name").map(|n| n == "Resources").unwrap_or(false) =>
                {
                    resource_pool = moref.clone();
                }
                _ => {}
            }
        }

        // Fixed ESXi MoRefs when traversal didn't surface them.
        if hosts.is_empty() {
            hosts.push(ProxmoxNode {
                node: "localhost.lan".into(),
                status: Some("connected".into()),
            });
        }

        let _ = (&dc_path, &vm_folder, &resource_pool, &host_moref);

        Ok(Inventory {
            version,
            hosts,
            host_caps,
            datastores,
            networks,
            vms,
            dc_path,
            vm_folder,
            resource_pool,
            host_moref,
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

    /// Fast API reachability (login only, 3s cap).
    pub async fn ping(&self) -> bool {
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(3), self.login()).await,
            Ok(Ok(()))
        )
    }

    pub async fn list_storage(&self, _node: &str) -> ApiResult<Vec<ProxmoxStorage>> {
        Ok(self.inventory().await?.datastores)
    }

    pub async fn host_capacity(&self, node: &str, storage: &str) -> ApiResult<HypervisorCapacity> {
        let inv = self.inventory().await?;
        let want = node.trim();
        let mut cpu_used = 0.0;
        let mut cpu_total = 0.0;
        let mut mem_used = 0.0;
        let mut mem_total = 0.0;
        let mut any = false;
        let mut node_name = want.to_string();
        for h in &inv.host_caps {
            if !want.is_empty() && !h.name.eq_ignore_ascii_case(want) && inv.host_caps.len() > 1 {
                continue;
            }
            any = true;
            if node_name.is_empty() {
                node_name = h.name.clone();
            }
            let cores = h.cpu_cores.unwrap_or(0.0);
            cpu_total += cores;
            if let (Some(used_mhz), Some(mhz)) = (h.cpu_used_mhz, h.cpu_mhz) {
                if mhz > 0.0 {
                    cpu_used += (used_mhz / mhz).clamp(0.0, cores.max(used_mhz / mhz));
                }
            }
            mem_total += h.mem_bytes.unwrap_or(0.0);
            mem_used += h.mem_used_mb.unwrap_or(0.0) * 1024.0 * 1024.0;
        }
        if !any {
            for h in &inv.host_caps {
                let cores = h.cpu_cores.unwrap_or(0.0);
                cpu_total += cores;
                if let (Some(used_mhz), Some(mhz)) = (h.cpu_used_mhz, h.cpu_mhz) {
                    if mhz > 0.0 {
                        cpu_used += used_mhz / mhz;
                    }
                }
                mem_total += h.mem_bytes.unwrap_or(0.0);
                mem_used += h.mem_used_mb.unwrap_or(0.0) * 1024.0 * 1024.0;
            }
            if node_name.is_empty() {
                node_name = inv
                    .hosts
                    .first()
                    .map(|h| h.node.clone())
                    .unwrap_or_default();
            }
        }
        let mut cap = HypervisorCapacity {
            cpu_used: (cpu_total > 0.0).then_some(cpu_used.min(cpu_total)),
            cpu_total: (cpu_total > 0.0).then_some(cpu_total),
            mem_used_bytes: (mem_total > 0.0).then_some(mem_used.min(mem_total)),
            mem_total_bytes: (mem_total > 0.0).then_some(mem_total),
            node: node_name,
            storage: storage.to_string(),
            ..HypervisorCapacity::default()
        };
        let st = inv
            .datastores
            .iter()
            .find(|s| s.storage == storage)
            .or(inv.datastores.first());
        if let Some(st) = st {
            cap.disk_total_bytes = st.total.map(|v| v as f64);
            cap.disk_avail_bytes = st.avail.map(|v| v as f64);
            cap.disk_used_bytes = match (st.total, st.avail) {
                (Some(t), Some(a)) => Some((t - a).max(0) as f64),
                _ => None,
            };
            cap.storage = st.storage.clone();
        }
        Ok(cap)
    }

    #[allow(dead_code)]
    pub async fn list_networks(&self) -> ApiResult<Vec<String>> {
        Ok(self.inventory().await?.networks)
    }

    pub async fn list_vms(&self) -> ApiResult<Vec<VsphereVm>> {
        Ok(self.inventory().await?.vms)
    }

    pub async fn validate_storage(
        &self,
        node: &str,
        storage: &str,
    ) -> ApiResult<StorageValidation> {
        let inv = self.inventory().await?;
        let available: Vec<String> = inv.datastores.iter().map(|s| s.storage.clone()).collect();
        let Some(found) = inv.datastores.iter().find(|s| s.storage == storage) else {
            return Ok(StorageValidation {
                ok: false,
                storage: storage.to_string(),
                node: node.to_string(),
                type_: None,
                content: None,
                active: false,
                enabled: false,
                message: format!(
                    "datastore `{storage}` not found — available: {}",
                    if available.is_empty() {
                        "(none)".into()
                    } else {
                        available.join(", ")
                    }
                ),
                available,
            });
        };
        let active = found.active.unwrap_or(1) != 0;
        let ok = active;
        let message = if ok {
            format!(
                "datastore `{storage}` ok (type={})",
                found.type_.as_deref().unwrap_or("?")
            )
        } else {
            format!("datastore `{storage}` is not accessible")
        };
        Ok(StorageValidation {
            ok,
            storage: storage.to_string(),
            node: node.to_string(),
            type_: found.type_.clone(),
            content: found.content.clone(),
            active,
            enabled: true,
            message,
            available,
        })
    }

    /// Name scheme (legacy): `{prefix}-{vmid}`. New creates use `{prefix}-cp-N` /
    /// `{prefix}-wk-N` (same as Proxmox); prefer `delete_vm_by_name` with the DB node name.
    pub fn vm_name(prefix: Option<&str>, vmid: i64) -> String {
        match prefix.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => format!("{p}-{vmid}"),
            None => vmid.to_string(),
        }
    }

    /// Resolve a VM for a numeric id: exact legacy name, bare id, `*-{vmid}` suffix,
    /// or (when prefix+index known) role names are handled by callers via `delete_vm_by_name`.
    async fn find_vm_for_vmid(
        &self,
        prefix: Option<&str>,
        vmid: i64,
    ) -> ApiResult<Option<VsphereVm>> {
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
                "VM names {start}–{end} free on ESXi (prefix={})",
                prefix.unwrap_or("")
            )
        } else {
            let detail = conflicts
                .iter()
                .map(|c| format!("{} ({})", c.vmid, c.name.as_deref().unwrap_or("unnamed")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("VM names already in use on ESXi: {detail}")
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
                node_message = "host is required".into();
            } else if !inv.hosts.iter().any(|x| x.node == n) {
                // On standalone ESXi the host name may be localhost.lan while
                // operators pass the URL hostname — accept any single host if
                // only one exists and names differ only by convention.
                if inv.hosts.len() == 1 {
                    node_ok = true;
                    node_message = format!(
                        "host ok (requested `{n}`, inventory `{}`)",
                        inv.hosts[0].node
                    );
                } else {
                    node_ok = false;
                    let names: Vec<_> = inv.hosts.iter().map(|x| x.node.as_str()).collect();
                    node_message = format!(
                        "host `{n}` not found — available: {}",
                        if names.is_empty() {
                            "(none)".into()
                        } else {
                            names.join(", ")
                        }
                    );
                }
            } else {
                node_message = format!("host `{n}` ok");
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
            if !inv.networks.iter().any(|n| n == net) {
                let avail = if inv.networks.is_empty() {
                    "(none)".to_string()
                } else {
                    inv.networks.join(", ")
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
        // Standalone ESXi is almost always x86_64; leave override to the provider form.
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

    async fn find_vm(&self, name: &str) -> ApiResult<Option<VsphereVm>> {
        Ok(self.list_vms().await?.into_iter().find(|v| v.name == name))
    }

    async fn wait_task(&self, task_moref: &str) -> ApiResult<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        while std::time::Instant::now() < deadline {
            let body = format!(
                r#"<RetrieveProperties xmlns="urn:vim25">
  <_this type="PropertyCollector">ha-property-collector</_this>
  <specSet>
    <propSet>
      <type>Task</type>
      <all>false</all>
      <pathSet>info</pathSet>
    </propSet>
    <objectSet>
      <obj type="Task">{task}</obj>
      <skip>false</skip>
    </objectSet>
  </specSet>
</RetrieveProperties>"#,
                task = xml_escape(task_moref)
            );
            let xml = self.soap("urn:vim25/8.0.3.0", &body).await?;
            let state = xml
                .find("<state>")
                .and_then(|i| {
                    let rest = &xml[i + 7..];
                    rest.split('<').next().map(|s| s.to_string())
                })
                .or_else(|| xml_text(&xml, "state"));
            if state.as_deref() == Some("success") {
                return Ok(());
            }
            if state.as_deref() == Some("error") {
                let msg = xml_text(&xml, "localizedMessage")
                    .or_else(|| xml_text(&xml, "message"))
                    .unwrap_or_else(|| "task failed".into());
                return Err(AppError::bad(format!("vsphere task error: {msg}")));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(AppError::bad(format!(
            "vsphere task {task_moref} timed out"
        )))
    }

    pub async fn power_off(&self, name: &str) -> ApiResult<()> {
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(());
        };
        if vm.power_state.as_deref() == Some("poweredOff") {
            return Ok(());
        }
        let body = format!(
            r#"<PowerOffVM_Task xmlns="urn:vim25">
  <_this type="VirtualMachine">{}</_this>
</PowerOffVM_Task>"#,
            xml_escape(&vm.moref)
        );
        let resp = self.soap("urn:vim25/8.0.3.0", &body).await?;
        if let Some(task) =
            xml_attr_moref(&resp, "returnval").or_else(|| xml_text(&resp, "returnval"))
        {
            self.wait_task(&task).await?;
        }
        Ok(())
    }

    pub async fn power_on(&self, name: &str) -> ApiResult<()> {
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        if vm.power_state.as_deref() == Some("poweredOn") {
            return Ok(());
        }
        // Keep BIOS UUID so a generated NIC MAC is not recomputed on power-on.
        let _ = self.ensure_uuid_keep(&vm.moref).await;
        let body = format!(
            r#"<PowerOnVM_Task xmlns="urn:vim25">
  <_this type="VirtualMachine">{}</_this>
</PowerOnVM_Task>"#,
            xml_escape(&vm.moref)
        );
        let resp = self.soap("urn:vim25/8.0.3.0", &body).await?;
        if let Some(task) =
            xml_attr_moref(&resp, "returnval").or_else(|| xml_text(&resp, "returnval"))
        {
            self.wait_task(&task).await?;
        }
        Ok(())
    }

    /// Persist `uuid.action=keep` so ESXi does not mint a new BIOS UUID (and
    /// therefore a new generated MAC / DHCP address) on each power-on.
    async fn ensure_uuid_keep(&self, moref: &str) -> ApiResult<()> {
        let body = format!(
            r#"<ReconfigVM_Task xmlns="urn:vim25">
  <_this type="VirtualMachine">{}</_this>
  <spec>
    <extraConfig xsi:type="OptionValue" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      <key>uuid.action</key>
      <value xsi:type="xsd:string" xmlns:xsd="http://www.w3.org/2001/XMLSchema">keep</value>
    </extraConfig>
  </spec>
</ReconfigVM_Task>"#,
            xml_escape(moref)
        );
        let resp = self.soap("urn:vim25/8.0.3.0", &body).await?;
        if let Some(task) =
            xml_attr_moref(&resp, "returnval").or_else(|| xml_text(&resp, "returnval"))
        {
            self.wait_task(&task).await?;
        }
        Ok(())
    }

    async fn wait_power_state(&self, name: &str, want: &str) -> ApiResult<()> {
        for _ in 0..30 {
            if let Some(vm) = self.find_vm(name).await? {
                if vm.power_state.as_deref() == Some(want) {
                    return Ok(());
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Err(AppError::bad(format!(
            "VM `{name}` did not reach {want} in time"
        )))
    }

    pub async fn restart_vm_by_name(&self, name: &str) -> ApiResult<()> {
        let _ = self.power_off(name).await;
        let _ = self.wait_power_state(name, "poweredOff").await;
        self.power_on(name).await
    }

    pub async fn delete_vm_by_name(&self, name: &str) -> ApiResult<()> {
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(());
        };
        let _ = self.power_off(name).await;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if let Some(v) = self.find_vm(name).await? {
                if v.power_state.as_deref() == Some("poweredOff") {
                    break;
                }
            } else {
                return Ok(());
            }
        }
        let body = format!(
            r#"<Destroy_Task xmlns="urn:vim25">
  <_this type="VirtualMachine">{}</_this>
</Destroy_Task>"#,
            xml_escape(&vm.moref)
        );
        let resp = self.soap("urn:vim25/8.0.3.0", &body).await?;
        if let Some(task) =
            xml_attr_moref(&resp, "returnval").or_else(|| xml_text(&resp, "returnval"))
        {
            self.wait_task(&task).await?;
        }
        Ok(())
    }

    /// Delete by numeric id. Tries legacy `{prefix}-{vmid}` and any inventory name
    /// ending in `-{vmid}`. Prefer `delete_vm_by_name` with `{prefix}-cp-N` when known.
    pub async fn delete_vm(&self, prefix: Option<&str>, vmid: i64) -> ApiResult<()> {
        self.ensure_login().await?;
        let Some(vm) = self.find_vm_for_vmid(prefix, vmid).await? else {
            return Ok(());
        };
        self.delete_vm_by_name(&vm.name).await
    }

    /// Set CPU/memory. ESXi rejects live `numCPUs`/`memoryMB` unless CPU/memory
    /// hot-plug is enabled for the guest OS — Pertisk VMs use `otherLinux64Guest`,
    /// which typically does not. Power off first; the resize job powers back on.
    pub async fn set_vm_hardware(
        &self,
        name: &str,
        cores: Option<i64>,
        memory_mb: Option<i64>,
    ) -> ApiResult<()> {
        if cores.is_none() && memory_mb.is_none() {
            return Ok(());
        }
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        let was_on = vm.power_state.as_deref() == Some("poweredOn");
        if was_on {
            self.power_off(name).await?;
            self.wait_power_state(name, "poweredOff").await?;
        }
        let mut spec = String::from("<spec>");
        if let Some(c) = cores {
            spec.push_str(&format!("<numCPUs>{c}</numCPUs>"));
        }
        if let Some(m) = memory_mb {
            spec.push_str(&format!("<memoryMB>{m}</memoryMB>"));
        }
        spec.push_str("</spec>");
        let body = format!(
            r#"<ReconfigVM_Task xmlns="urn:vim25">
  <_this type="VirtualMachine">{}</_this>
  {spec}
</ReconfigVM_Task>"#,
            xml_escape(&vm.moref)
        );
        let resp = match self.soap("urn:vim25/8.0.3.0", &body).await {
            Ok(r) => r,
            Err(e) => {
                if was_on {
                    let _ = self.power_on(name).await;
                }
                return Err(e);
            }
        };
        if let Some(task) =
            xml_attr_moref(&resp, "returnval").or_else(|| xml_text(&resp, "returnval"))
        {
            if let Err(e) = self.wait_task(&task).await {
                if was_on {
                    let _ = self.power_on(name).await;
                }
                return Err(e);
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn vm_mac(&self, name: &str) -> ApiResult<Option<String>> {
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(None);
        };
        let body = format!(
            r#"<RetrieveProperties xmlns="urn:vim25">
  <_this type="PropertyCollector">ha-property-collector</_this>
  <specSet>
    <propSet>
      <type>VirtualMachine</type>
      <all>false</all>
      <pathSet>config.hardware.device</pathSet>
    </propSet>
    <objectSet>
      <obj type="VirtualMachine">{}</obj>
      <skip>false</skip>
    </objectSet>
  </specSet>
</RetrieveProperties>"#,
            xml_escape(&vm.moref)
        );
        let xml = self.soap("urn:vim25/8.0.3.0", &body).await?;
        Ok(extract_mac_from_devices(&xml))
    }

    /// IPv4 reported by VMware Tools (`guest.ipAddress` / `guest.net`). Empty without tools.
    pub async fn vm_guest_ipv4(&self, name: &str) -> ApiResult<Option<String>> {
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Ok(None);
        };
        let body = format!(
            r#"<RetrieveProperties xmlns="urn:vim25">
  <_this type="PropertyCollector">ha-property-collector</_this>
  <specSet>
    <propSet>
      <type>VirtualMachine</type>
      <all>false</all>
      <pathSet>guest.ipAddress</pathSet>
      <pathSet>guest.net</pathSet>
    </propSet>
    <objectSet>
      <obj type="VirtualMachine">{}</obj>
      <skip>false</skip>
    </objectSet>
  </specSet>
</RetrieveProperties>"#,
            xml_escape(&vm.moref)
        );
        let xml = self.soap("urn:vim25/8.0.3.0", &body).await?;
        Ok(first_guest_ipv4_from_xml(&xml))
    }

    /// Guest IPv4s VMware Tools last reported (empty for powered-off VMs without tools data).
    pub async fn all_guest_ipv4s(&self) -> Vec<String> {
        let vms = self.list_vms().await.unwrap_or_default();
        let futs = vms.into_iter().map(|vm| {
            let this = self.clone();
            async move {
                this.vm_guest_ipv4(&vm.name)
                    .await
                    .ok()
                    .flatten()
            }
        });
        let mut ips: Vec<String> = futures::future::join_all(futs)
            .await
            .into_iter()
            .flatten()
            .collect();
        ips.sort();
        ips.dedup();
        ips
    }

    pub async fn grow_vm_disk(&self, name: &str, disk_gb: i64) -> ApiResult<()> {
        if disk_gb < 1 {
            return Err(AppError::bad("disk_gb must be >= 1"));
        }
        self.ensure_login().await?;
        let Some(vm) = self.find_vm(name).await? else {
            return Err(AppError::bad(format!("VM `{name}` not found")));
        };
        // Fetch disk backing + capacity; grow via ReconfigVM with capacityInKB.
        let body = format!(
            r#"<RetrieveProperties xmlns="urn:vim25">
  <_this type="PropertyCollector">ha-property-collector</_this>
  <specSet>
    <propSet>
      <type>VirtualMachine</type>
      <all>false</all>
      <pathSet>config.hardware.device</pathSet>
    </propSet>
    <objectSet>
      <obj type="VirtualMachine">{}</obj>
      <skip>false</skip>
    </objectSet>
  </specSet>
</RetrieveProperties>"#,
            xml_escape(&vm.moref)
        );
        let xml = self.soap("urn:vim25/8.0.3.0", &body).await?;
        let Some((key, capacity_kb)) = extract_first_disk(&xml) else {
            return Err(AppError::bad(format!(
                "VM `{name}` has no virtual disk to grow"
            )));
        };
        let want_kb = disk_gb.saturating_mul(1024 * 1024);
        if capacity_kb >= want_kb {
            return Ok(());
        }
        let reconfig = format!(
            r#"<ReconfigVM_Task xmlns="urn:vim25">
  <_this type="VirtualMachine">{}</_this>
  <spec>
    <deviceChange>
      <operation>edit</operation>
      <device xsi:type="VirtualDisk" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <key>{key}</key>
        <capacityInKB>{want_kb}</capacityInKB>
      </device>
    </deviceChange>
  </spec>
</ReconfigVM_Task>"#,
            xml_escape(&vm.moref)
        );
        let resp = self.soap("urn:vim25/8.0.3.0", &reconfig).await?;
        if let Some(task) =
            xml_attr_moref(&resp, "returnval").or_else(|| xml_text(&resp, "returnval"))
        {
            self.wait_task(&task).await?;
        }
        Ok(())
    }
}

// --- SOAP / XML helpers ---

fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:vim25="urn:vim25">
  <soapenv:Body>
    {body}
  </soapenv:Body>
</soapenv:Envelope>"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn extract_soap_session(set_cookie: &str) -> Option<String> {
    for part in set_cookie.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("vmware_soap_session=") {
            return Some(rest.to_string());
        }
    }
    None
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut start = 0;
    while let Some(i) = xml[start..].find(&open) {
        let abs = start + i;
        let after = &xml[abs + open.len()..];
        let content_start = after.find('>')? + 1;
        if after.as_bytes().get(content_start.saturating_sub(1)) == Some(&b'/') {
            start = abs + open.len();
            continue;
        }
        let rest = &after[content_start..];
        if let Some(end) = rest.find(&close) {
            let val = rest[..end].trim();
            // Skip nested-only empty wrappers.
            if !val.is_empty() && !val.starts_with('<') {
                return Some(val.to_string());
            }
            // Nested: try inner text of first child for about/name etc.
            if let Some(inner) = rest[..end].find('>') {
                let inner_rest = &rest[inner + 1..];
                if let Some(e2) = inner_rest.find('<') {
                    let v = inner_rest[..e2].trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
        start = abs + open.len();
    }
    None
}

fn xml_attr_moref(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let i = xml.find(&open)?;
    let after = &xml[i..];
    let end = after.find('>')?;
    let attrs = &after[..end];
    // <returnval type="Task">task-123</returnval>
    let content_start = end + 1;
    if let Some(close_at) = after[content_start..].find('<') {
        let val = after[content_start..content_start + close_at].trim();
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    let _ = attrs;
    None
}

fn split_objects(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<objects>") {
        rest = &rest[i + 9..];
        if let Some(j) = rest.find("</objects>") {
            out.push(rest[..j].to_string());
            rest = &rest[j + 10..];
        } else {
            break;
        }
    }
    // RetrieveProperties (non-Ex) uses <returnval> wrappers.
    if out.is_empty() {
        rest = xml;
        while let Some(i) = rest.find("<returnval>") {
            rest = &rest[i + 11..];
            if let Some(j) = rest.find("</returnval>") {
                let chunk = &rest[..j];
                if chunk.contains("<obj ") || chunk.contains("<obj>") {
                    out.push(chunk.to_string());
                }
                rest = &rest[j + 12..];
            } else {
                break;
            }
        }
    }
    out
}

fn obj_type(obj: &str) -> Option<String> {
    let i = obj.find("<obj")?;
    let slice = &obj[i..];
    let type_key = "type=\"";
    let t = slice.find(type_key)? + type_key.len();
    let end = slice[t..].find('"')?;
    Some(slice[t..t + end].to_string())
}

fn obj_id(obj: &str) -> Option<String> {
    let i = obj.find("<obj")?;
    let slice = &obj[i..];
    let gt = slice.find('>')?;
    let rest = &slice[gt + 1..];
    let end = rest.find('<')?;
    Some(rest[..end].trim().to_string())
}

fn obj_props(obj: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut rest = obj;
    while let Some(i) = rest.find("<name>") {
        rest = &rest[i + 6..];
        let Some(ne) = rest.find("</name>") else {
            break;
        };
        let name = rest[..ne].trim().to_string();
        rest = &rest[ne + 7..];
        if let Some(vi) = rest.find("<val") {
            rest = &rest[vi..];
            if let Some(gt) = rest.find('>') {
                let self_closing = rest.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/');
                rest = &rest[gt + 1..];
                if self_closing {
                    continue;
                }
                if let Some(ve) = rest.find("</val>") {
                    let val = rest[..ve].trim().to_string();
                    // Flatten nested text (e.g. MoRef content).
                    let flat = if val.contains('<') {
                        val.split('>')
                            .nth(1)
                            .and_then(|s| s.split('<').next())
                            .unwrap_or("")
                            .trim()
                            .to_string()
                    } else {
                        val
                    };
                    map.insert(name, flat);
                    rest = &rest[ve + 6..];
                }
            }
        }
    }
    map
}

#[allow(dead_code)]
fn extract_mac_from_devices(xml: &str) -> Option<String> {
    // Look for macAddress elements in VirtualEthernetCard devices.
    let mut rest = xml;
    while let Some(i) = rest.find("<macAddress>") {
        rest = &rest[i + 12..];
        if let Some(e) = rest.find("</macAddress>") {
            let mac = rest[..e].trim();
            if mac.len() >= 11 {
                return Some(mac.to_ascii_lowercase());
            }
            rest = &rest[e + 13..];
        } else {
            break;
        }
    }
    None
}

fn extract_first_disk(xml: &str) -> Option<(i64, i64)> {
    // Find VirtualDisk key + capacityInKB.
    let mut search = xml;
    while let Some(i) = search.find("VirtualDisk") {
        let slice = &search[i..];
        let end = slice.find("</val>").unwrap_or(slice.len().min(8000));
        let block = &slice[..end];
        if let (Some(key), Some(cap)) = (
            xml_text(block, "key").and_then(|s| s.parse().ok()),
            xml_text(block, "capacityInKB").and_then(|s| s.parse().ok()),
        ) {
            return Some((key, cap));
        }
        search = &search[i + 11..];
    }
    None
}

/// First non-loopback IPv4 in guest.ipAddress / guest.net SOAP.
pub fn first_guest_ipv4_from_xml(xml: &str) -> Option<String> {
    for token in xml.split(|c: char| !c.is_ascii_digit() && c != '.') {
        if !usable_guest_ipv4(token) {
            continue;
        }
        return Some(token.to_string());
    }
    None
}

fn usable_guest_ipv4(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    addr.is_private()
}

const TRAVERSAL_RETRIEVE: &str = r#"<RetrievePropertiesEx xmlns="urn:vim25">
  <_this type="PropertyCollector">ha-property-collector</_this>
  <specSet>
    <propSet>
      <type>HostSystem</type>
      <all>false</all>
      <pathSet>name</pathSet>
      <pathSet>runtime.connectionState</pathSet>
      <pathSet>summary.hardware.numCpuCores</pathSet>
      <pathSet>summary.hardware.cpuMhz</pathSet>
      <pathSet>summary.hardware.memorySize</pathSet>
      <pathSet>summary.quickStats.overallCpuUsage</pathSet>
      <pathSet>summary.quickStats.overallMemoryUsage</pathSet>
    </propSet>
    <propSet>
      <type>Datastore</type>
      <all>false</all>
      <pathSet>name</pathSet>
      <pathSet>summary.capacity</pathSet>
      <pathSet>summary.freeSpace</pathSet>
      <pathSet>summary.type</pathSet>
      <pathSet>summary.accessible</pathSet>
    </propSet>
    <propSet>
      <type>Network</type>
      <all>false</all>
      <pathSet>name</pathSet>
    </propSet>
    <propSet>
      <type>VirtualMachine</type>
      <all>false</all>
      <pathSet>name</pathSet>
      <pathSet>runtime.powerState</pathSet>
      <pathSet>config.hardware.numCPU</pathSet>
      <pathSet>config.hardware.memoryMB</pathSet>
    </propSet>
    <propSet>
      <type>Datacenter</type>
      <all>false</all>
      <pathSet>name</pathSet>
      <pathSet>vmFolder</pathSet>
    </propSet>
    <propSet>
      <type>ResourcePool</type>
      <all>false</all>
      <pathSet>name</pathSet>
    </propSet>
    <objectSet>
      <obj type="Folder">ha-folder-root</obj>
      <skip>false</skip>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>visitFolders</name>
        <type>Folder</type>
        <path>childEntity</path>
        <skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
        <selectSet><name>dcToHf</name></selectSet>
        <selectSet><name>dcToVmf</name></selectSet>
        <selectSet><name>dcToDs</name></selectSet>
        <selectSet><name>dcToNet</name></selectSet>
        <selectSet><name>crToH</name></selectSet>
        <selectSet><name>crToRp</name></selectSet>
        <selectSet><name>crToDs</name></selectSet>
        <selectSet><name>crToNet</name></selectSet>
        <selectSet><name>HToVm</name></selectSet>
        <selectSet><name>rpToRp</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>dcToHf</name><type>Datacenter</type><path>hostFolder</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>dcToVmf</name><type>Datacenter</type><path>vmFolder</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>dcToDs</name><type>Datacenter</type><path>datastoreFolder</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>dcToNet</name><type>Datacenter</type><path>networkFolder</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>crToH</name><type>ComputeResource</type><path>host</path><skip>false</skip>
        <selectSet><name>HToVm</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>crToRp</name><type>ComputeResource</type><path>resourcePool</path><skip>false</skip>
        <selectSet><name>rpToRp</name></selectSet>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>crToDs</name><type>ComputeResource</type><path>datastore</path><skip>false</skip>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>crToNet</name><type>ComputeResource</type><path>network</path><skip>false</skip>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>HToVm</name><type>HostSystem</type><path>vm</path><skip>false</skip>
      </selectSet>
      <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
        <name>rpToRp</name><type>ResourcePool</type><path>resourcePool</path><skip>false</skip>
        <selectSet><name>rpToRp</name></selectSet>
      </selectSet>
    </objectSet>
  </specSet>
  <options></options>
</RetrievePropertiesEx>"#;

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct VsphereProbeExtra {
    pub networks: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_guest_ip_from_soap() {
        let xml = r#"<RetrievePropertiesResponse>
  <returnval>
    <propSet><name>guest.ipAddress</name><val xsi:type="xsd:string">10.1.1.40</val></propSet>
  </returnval>
</RetrievePropertiesResponse>"#;
        assert_eq!(first_guest_ipv4_from_xml(xml).as_deref(), Some("10.1.1.40"));
        let xml6 = r#"<ipAddress><string>fe80::1</string><string>10.1.1.55</string></ipAddress>"#;
        assert_eq!(
            first_guest_ipv4_from_xml(xml6).as_deref(),
            Some("10.1.1.55")
        );
    }
}
