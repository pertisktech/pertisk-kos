//! Cluster PKI via rcgen (mint CA in-process; do not re-load for signing).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType,
};
use time::{Duration, OffsetDateTime};

pub struct ClusterPki {
    pub ca_crt: String,
    pub ca_key: String,
    pub sa_key: String,
    pub sa_pub: String,
    pub apiserver_crt: String,
    pub apiserver_key: String,
    pub etcd_crt: String,
    pub etcd_key: String,
    pub admin_crt: String,
    pub admin_key: String,
    pub cm_crt: String,
    pub cm_key: String,
    pub sched_crt: String,
    pub sched_key: String,
    pub kubelet_crt: String,
    pub kubelet_key: String,
}

struct CaIssuer {
    cert: Certificate,
    key: KeyPair,
    ca_crt: String,
    ca_key: String,
}

fn mint_ca() -> Result<CaIssuer> {
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "pertisk-ca");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(3650);
    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    let ca_crt = cert.pem();
    let ca_key = key.serialize_pem();
    Ok(CaIssuer {
        cert,
        key,
        ca_crt,
        ca_key,
    })
}

/// Generate a full PKI tree for the first control plane.
pub fn generate_pki(
    advertise_ip: &str,
    hostname: &str,
    endpoint_host: &str,
) -> Result<ClusterPki> {
    let ca = mint_ca()?;
    let sa = KeyPair::generate()?;

    let apiserver = issue_server(
        &ca,
        "kube-apiserver",
        &[
            hostname.to_string(),
            "kubernetes".into(),
            "kubernetes.default".into(),
            "kubernetes.default.svc".into(),
            "kubernetes.default.svc.cluster.local".into(),
            "localhost".into(),
            advertise_ip.to_string(),
            endpoint_host.to_string(),
            "127.0.0.1".into(),
        ],
    )?;

    let etcd = issue_server(
        &ca,
        "etcd",
        &[
            hostname.to_string(),
            "localhost".into(),
            advertise_ip.to_string(),
            "127.0.0.1".into(),
        ],
    )?;

    let admin = issue_client(&ca, "kubernetes-admin", &["system:masters"])?;
    let cm = issue_client(&ca, "system:kube-controller-manager", &[])?;
    let sched = issue_client(&ca, "system:kube-scheduler", &[])?;
    let kubelet = issue_client(
        &ca,
        &format!("system:node:{hostname}"),
        &["system:nodes"],
    )?;

    Ok(ClusterPki {
        ca_crt: ca.ca_crt,
        ca_key: ca.ca_key,
        sa_key: sa.serialize_pem(),
        sa_pub: sa.public_key_pem(),
        apiserver_crt: apiserver.0,
        apiserver_key: apiserver.1,
        etcd_crt: etcd.0,
        etcd_key: etcd.1,
        admin_crt: admin.0,
        admin_key: admin.1,
        cm_crt: cm.0,
        cm_key: cm.1,
        sched_crt: sched.0,
        sched_key: sched.1,
        kubelet_crt: kubelet.0,
        kubelet_key: kubelet.1,
    })
}

fn issue_server(ca: &CaIssuer, cn: &str, names: &[String]) -> Result<(String, String)> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.distinguished_name.push(DnType::CommonName, cn);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    // kubeadm dual-purpose: apiserver→etcd and apiserver→kubelet both present
    // these certs as TLS clients; ServerAuth alone makes Go reject the handshake.
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(365);
    for n in names {
        if n.is_empty() {
            continue;
        }
        if let Ok(ip) = n.parse::<std::net::IpAddr>() {
            params.subject_alt_names.push(SanType::IpAddress(ip));
        } else {
            params
                .subject_alt_names
                .push(SanType::DnsName(n.clone().try_into()?));
        }
    }
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, &ca.cert, &ca.key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn issue_client(ca: &CaIssuer, cn: &str, orgs: &[&str]) -> Result<(String, String)> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.distinguished_name.push(DnType::CommonName, cn);
    for o in orgs {
        params
            .distinguished_name
            .push(DnType::OrganizationName, *o);
    }
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(365);
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, &ca.cert, &ca.key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

pub fn write_pki(dir: &Path, etcd_dir: &Path, pki: &ClusterPki) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::create_dir_all(etcd_dir)?;
    write(dir.join("ca.crt"), &pki.ca_crt)?;
    write(dir.join("ca.key"), &pki.ca_key)?;
    write(dir.join("sa.key"), &pki.sa_key)?;
    write(dir.join("sa.pub"), &pki.sa_pub)?;
    write(dir.join("apiserver.crt"), &pki.apiserver_crt)?;
    write(dir.join("apiserver.key"), &pki.apiserver_key)?;
    write(dir.join("admin.crt"), &pki.admin_crt)?;
    write(dir.join("admin.key"), &pki.admin_key)?;
    write(etcd_dir.join("ca.crt"), &pki.ca_crt)?;
    write(etcd_dir.join("server.crt"), &pki.etcd_crt)?;
    write(etcd_dir.join("server.key"), &pki.etcd_key)?;
    write(etcd_dir.join("peer.crt"), &pki.etcd_crt)?;
    write(etcd_dir.join("peer.key"), &pki.etcd_key)?;
    write(dir.join("front-proxy-ca.crt"), &pki.ca_crt)?;
    write(dir.join("front-proxy-ca.key"), &pki.ca_key)?;
    write(dir.join("front-proxy-client.crt"), &pki.admin_crt)?;
    write(dir.join("front-proxy-client.key"), &pki.admin_key)?;
    write(dir.join("controller-manager.crt"), &pki.cm_crt)?;
    write(dir.join("controller-manager.key"), &pki.cm_key)?;
    write(dir.join("scheduler.crt"), &pki.sched_crt)?;
    write(dir.join("scheduler.key"), &pki.sched_key)?;
    write(dir.join("kubelet.crt"), &pki.kubelet_crt)?;
    write(dir.join("kubelet.key"), &pki.kubelet_key)?;
    Ok(())
}

fn write(path: impl AsRef<Path>, data: &str) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, data).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.extension().and_then(|e| e.to_str()) == Some("key") {
            0o600
        } else {
            0o644
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}
