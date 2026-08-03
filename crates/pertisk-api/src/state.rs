//! Shared node state for the management API and init supervisor.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pertisk_disk::DEFAULT_CONFIG_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    None,
    Reboot,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub version: String,
    pub api_version: String,
    pub platform: String,
    pub state_root: PathBuf,
    pub config_path: PathBuf,
    /// Ed25519 public key used to verify OS upgrade bundles.
    pub trust_public_key: PathBuf,
    pub containerd: String,
    pub kubelet: String,
    pub containerd_pid: u32,
    pub kubelet_pid: u32,
    pub power: PowerAction,
    /// Set by ApplyConfiguration — supervise loop reloads config + starts kubelet.
    pub config_reload: bool,
    /// Set after Bootstrap — supervise loop restarts kubelet with cert kubeconfig.
    pub kubelet_reload: bool,
    pub ready: bool,
    pub message: String,
}

impl NodeState {
    pub fn new(state_root: PathBuf, trust_public_key: PathBuf) -> Self {
        let config_path = state_root.join(DEFAULT_CONFIG_NAME);
        Self {
            version: pertisk_config::release_version().to_string(),
            api_version: "v1alpha1".into(),
            platform: std::env::consts::OS.to_string(),
            state_root,
            config_path,
            trust_public_key,
            containerd: "absent".into(),
            kubelet: "absent".into(),
            containerd_pid: 0,
            kubelet_pid: 0,
            power: PowerAction::None,
            config_reload: false,
            kubelet_reload: false,
            ready: true,
            message: "booting".into(),
        }
    }

    pub fn set_runtime_status(
        &mut self,
        containerd: impl Into<String>,
        kubelet: impl Into<String>,
        containerd_pid: u32,
        kubelet_pid: u32,
    ) {
        self.containerd = containerd.into();
        self.kubelet = kubelet.into();
        self.containerd_pid = containerd_pid;
        self.kubelet_pid = kubelet_pid;
        self.message = format!("containerd={} kubelet={}", self.containerd, self.kubelet);
        self.ready = self.power == PowerAction::None;
    }

    /// Point shared state at the real STATE volume once it is mounted.
    pub fn bind_state(&mut self, state_root: PathBuf, trust_public_key: PathBuf) {
        self.config_path = state_root.join(DEFAULT_CONFIG_NAME);
        self.state_root = state_root;
        self.trust_public_key = trust_public_key;
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }
}

pub type SharedState = Arc<Mutex<NodeState>>;

pub fn shared(state_root: PathBuf, trust_public_key: PathBuf) -> SharedState {
    Arc::new(Mutex::new(NodeState::new(state_root, trust_public_key)))
}
