//! Cluster PKI via rcgen (mint CA in-process, or reuse shared CA for HA join).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
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
    issuer: Issuer<'static, KeyPair>,
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
    let issuer = Issuer::from_ca_cert_pem(&ca_crt, KeyPair::from_pem(&ca_key)?)
        .context("build CA issuer")?;
    Ok(CaIssuer {
        issuer,
        ca_crt,
        ca_key,
    })
}

fn load_ca(ca_crt_pem: &str, ca_key_pem: &str) -> Result<CaIssuer> {
    let ca_crt = ca_crt_pem.trim().to_string();
    let ca_key = ca_key_pem.trim().to_string();
    let key = KeyPair::from_pem(&ca_key).context("parse caKey PEM")?;
    let issuer = Issuer::from_ca_cert_pem(&ca_crt, key).context("load existing CA for signing")?;
    Ok(CaIssuer {
        issuer,
        ca_crt,
        ca_key,
    })
}

fn load_or_mint_sa(sa_key_pem: Option<&str>) -> Result<(String, String)> {
    if let Some(pem) = sa_key_pem.map(str::trim).filter(|s| !s.is_empty()) {
        let key = KeyPair::from_pem(pem).context("parse saKey PEM")?;
        return Ok((key.serialize_pem(), key.public_key_pem()));
    }
    let key = KeyPair::generate()?;
    Ok((key.serialize_pem(), key.public_key_pem()))
}

fn unique_sans(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    for n in names {
        let t = n.trim().to_string();
        if t.is_empty() {
            continue;
        }
        if !out.iter().any(|e| e == &t) {
            out.push(t);
        }
    }
    out
}

fn apiserver_sans(
    advertise_ip: &str,
    hostname: &str,
    endpoint_host: &str,
    kubernetes_svc_ip: &str,
    extra_sans: &[String],
) -> Vec<String> {
    let mut names = vec![
        hostname.to_string(),
        "kubernetes".into(),
        "kubernetes.default".into(),
        "kubernetes.default.svc".into(),
        "kubernetes.default.svc.cluster.local".into(),
        "localhost".into(),
        advertise_ip.to_string(),
        endpoint_host.to_string(),
        "127.0.0.1".into(),
        kubernetes_svc_ip.to_string(),
    ];
    names.extend(extra_sans.iter().cloned());
    unique_sans(names)
}

fn etcd_sans(advertise_ip: &str, hostname: &str, extra_sans: &[String]) -> Vec<String> {
    let mut names = vec![
        hostname.to_string(),
        "localhost".into(),
        advertise_ip.to_string(),
        "127.0.0.1".into(),
    ];
    names.extend(extra_sans.iter().cloned());
    unique_sans(names)
}

fn issue_leaf(
    ca: &CaIssuer,
    sa_key: String,
    sa_pub: String,
    apiserver_names: &[String],
    etcd_names: &[String],
    hostname: &str,
) -> Result<ClusterPki> {
    let apiserver = issue_server(ca, "kube-apiserver", apiserver_names)?;
    let etcd = issue_server(ca, "etcd", etcd_names)?;
    let admin = issue_client(ca, "kubernetes-admin", &["system:masters"])?;
    let cm = issue_client(ca, "system:kube-controller-manager", &[])?;
    let sched = issue_client(ca, "system:kube-scheduler", &[])?;
    let kubelet = issue_client(ca, &format!("system:node:{hostname}"), &["system:nodes"])?;

    Ok(ClusterPki {
        ca_crt: ca.ca_crt.clone(),
        ca_key: ca.ca_key.clone(),
        sa_key,
        sa_pub,
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

/// Generate a full PKI tree for the first control plane (new CA + SA key).
///
/// `kubernetes_svc_ip` is the ClusterIP of the `default/kubernetes` Service.
/// `extra_sans` comes from `cluster.certSANs` (VIP, extra CP IPs, DNS).
pub fn generate_pki(
    advertise_ip: &str,
    hostname: &str,
    endpoint_host: &str,
    kubernetes_svc_ip: &str,
    extra_sans: &[String],
) -> Result<ClusterPki> {
    let ca = mint_ca()?;
    let (sa_key, sa_pub) = load_or_mint_sa(None)?;
    issue_leaf(
        &ca,
        sa_key,
        sa_pub,
        &apiserver_sans(
            advertise_ip,
            hostname,
            endpoint_host,
            kubernetes_svc_ip,
            extra_sans,
        ),
        &etcd_sans(advertise_ip, hostname, extra_sans),
        hostname,
    )
}

/// First CP or join: reuse shared CA/SA when PEMs are provided.
pub fn generate_pki_with_optional_existing(
    advertise_ip: &str,
    hostname: &str,
    endpoint_host: &str,
    kubernetes_svc_ip: &str,
    extra_sans: &[String],
    ca_crt: Option<&str>,
    ca_key: Option<&str>,
    sa_key: Option<&str>,
) -> Result<ClusterPki> {
    match (
        ca_crt.map(str::trim).filter(|s| !s.is_empty()),
        ca_key.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(crt), Some(key)) => generate_pki_from_existing(
            crt,
            key,
            sa_key,
            advertise_ip,
            hostname,
            endpoint_host,
            kubernetes_svc_ip,
            extra_sans,
        ),
        (None, None) => {
            if sa_key.map(str::trim).filter(|s| !s.is_empty()).is_some() {
                anyhow::bail!("saKey requires ca + caKey");
            }
            generate_pki(
                advertise_ip,
                hostname,
                endpoint_host,
                kubernetes_svc_ip,
                extra_sans,
            )
        }
        _ => anyhow::bail!("ca and caKey must both be set (or both omitted)"),
    }
}

/// Join / additional CP: reuse cluster CA + SA signing key; mint node-local leafs.
pub fn generate_pki_from_existing(
    ca_crt_pem: &str,
    ca_key_pem: &str,
    sa_key_pem: Option<&str>,
    advertise_ip: &str,
    hostname: &str,
    endpoint_host: &str,
    kubernetes_svc_ip: &str,
    extra_sans: &[String],
) -> Result<ClusterPki> {
    let ca = load_ca(ca_crt_pem, ca_key_pem)?;
    let (sa_key, sa_pub) = load_or_mint_sa(sa_key_pem)?;
    issue_leaf(
        &ca,
        sa_key,
        sa_pub,
        &apiserver_sans(
            advertise_ip,
            hostname,
            endpoint_host,
            kubernetes_svc_ip,
            extra_sans,
        ),
        &etcd_sans(advertise_ip, hostname, extra_sans),
        hostname,
    )
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
    let cert = params.signed_by(&key, &ca.issuer)?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn issue_client(ca: &CaIssuer, cn: &str, orgs: &[&str]) -> Result<(String, String)> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.distinguished_name.push(DnType::CommonName, cn);
    for o in orgs {
        params.distinguished_name.push(DnType::OrganizationName, *o);
    }
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(365);
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, &ca.issuer)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pki_from_existing_reuses_ca() {
        let first = generate_pki("10.0.0.1", "cp-1", "10.0.0.100", "10.96.0.1", &[]).unwrap();
        let join = generate_pki_from_existing(
            &first.ca_crt,
            &first.ca_key,
            Some(&first.sa_key),
            "10.0.0.2",
            "cp-2",
            "10.0.0.100",
            "10.96.0.1",
            &["10.0.0.100".into()],
        )
        .unwrap();
        assert_eq!(first.ca_crt.trim(), join.ca_crt.trim());
        assert_eq!(first.sa_key.trim(), join.sa_key.trim());
        assert_ne!(first.apiserver_crt, join.apiserver_crt);
    }
}
