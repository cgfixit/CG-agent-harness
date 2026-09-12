//! Shared loopback transport. The private PEM bundle is the atomic authority;
//! exported certificate text and desktop pins contain no private key material.
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::{Extension, Router};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::common::{
    config::AppConfig,
    errors::{HarnessError, Result},
    home::Home,
};

#[derive(Debug, Clone, Copy)]
pub struct ListenerScheme(pub &'static str);

pub struct Transport {
    config: Option<Arc<rustls::ServerConfig>>,
    pub certificate_der: Vec<u8>,
    pub certificate_pem: String,
}

fn tls_error(message: &str) -> HarnessError {
    HarnessError::new("TLS_CONFIGURATION", message)
}

pub fn read_material(path: &Path, private: bool) -> Result<Vec<u8>> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = opts
        .open(path)
        .map_err(|_| tls_error("TLS material is missing or unreadable; check configured certificate/key paths"))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 65_536 {
        return Err(tls_error("TLS material must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 || private && (meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0) {
            return Err(tls_error(
                "TLS private material must be owned by this user, mode 0600, without hard links",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(tls_error("TLS material exceeds limit"));
    }
    Ok(bytes)
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(tls_error("TLS directory must not be a symlink"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(tls_error("TLS directory must be owned by this user with mode 0700"));
        }
    }
    Ok(())
}

fn generate(days: i64) -> Result<(String, String)> {
    let key = rcgen::KeyPair::generate().map_err(|_| tls_error("could not generate TLS private key"))?;
    let mut params = rcgen::CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])
        .map_err(|_| tls_error("invalid local certificate names"))?;
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(days);
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "CG Agent Harness local portal");
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    let cert = params
        .self_signed(&key)
        .map_err(|_| tls_error("could not sign local TLS certificate"))?;
    Ok((cert.pem(), key.serialize_pem()))
}

impl Transport {
    pub fn load(home: &Home, cfg: &AppConfig, host: &str) -> Result<Self> {
        if !cfg.flag_is_true("tls.enabled") {
            return Ok(Self {
                config: None,
                certificate_der: Vec::new(),
                certificate_pem: String::new(),
            });
        }
        let cert_path = cfg.str_opt("tls.cert_file").unwrap_or_default();
        let key_path = cfg.str_opt("tls.key_file").unwrap_or_default();
        let (cert, key) = if !cert_path.is_empty() || !key_path.is_empty() {
            if cert_path.is_empty() || key_path.is_empty() {
                return Err(tls_error("set both tls.cert_file and tls.key_file"));
            }
            (
                read_material(&home.anchor(&cert_path), false)?,
                read_material(&home.anchor(&key_path), true)?,
            )
        } else {
            private_directory(&home.root.join("tls"))?;
            let bundle = home.root.join("tls/server.pem");
            match std::fs::symlink_metadata(&bundle) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if !cfg.flag_is_true("tls.auto_generate") {
                        return Err(tls_error("TLS material is missing and tls.auto_generate is disabled"));
                    }
                    Self::renew(home, cfg)?;
                }
                Err(e) => return Err(e.into()),
                Ok(_) => {}
            }
            let bundle = read_material(&bundle, true)?;
            (bundle.clone(), bundle)
        };
        Self::from_material(&cert, &key, host)
    }

    /// Explicit offline operation while holding the home lock; never renew
    /// operator-provided material or silently replace a corrupt existing bundle.
    pub fn renew(home: &Home, cfg: &AppConfig) -> Result<()> {
        if cfg.str_opt("tls.cert_file").is_some_and(|s| !s.is_empty())
            || cfg.str_opt("tls.key_file").is_some_and(|s| !s.is_empty())
        {
            return Err(tls_error(
                "operator-provided certificates must be renewed by their owner",
            ));
        }
        let days = match cfg.get("tls.certificate_days") {
            None => 90,
            Some(v) => v
                .as_i64()
                .filter(|n| (1..=365).contains(n))
                .ok_or_else(|| tls_error("tls.certificate_days must be 1..=365"))?,
        };
        private_directory(&home.root.join("tls"))?;
        let (cert, key) = generate(days)?;
        // Validate before replacing. One atomic file prevents mixed key/cert pairs.
        Self::from_material(cert.as_bytes(), key.as_bytes(), "127.0.0.1")?;
        crate::common::atomic::write_atomic(
            &home.root.join("tls/server.pem"),
            format!("{cert}{key}").as_bytes(),
            Some(0o600),
        )?;
        Ok(())
    }

    pub fn from_material(cert: &[u8], key: &[u8], host: &str) -> Result<Self> {
        let certs: Vec<_> = CertificateDer::pem_slice_iter(cert)
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| tls_error("certificate PEM is invalid"))?;
        if certs.is_empty() || certs.len() > 8 {
            return Err(tls_error("certificate chain is empty or too long"));
        }
        let leaf = certs[0].as_ref().to_vec();
        let (_, parsed) = X509Certificate::from_der(&leaf).map_err(|_| tls_error("certificate DER is invalid"))?;
        if !parsed.validity().is_valid() {
            return Err(tls_error(
                "certificate is expired or not yet valid; renew explicitly or correct the system clock",
            ));
        }
        let san = parsed
            .subject_alternative_name()
            .map_err(|_| tls_error("certificate SAN is invalid"))?
            .ok_or_else(|| tls_error("certificate has no SAN"))?;
        let has_host = san.value.general_names.iter().any(|name| match name {
            x509_parser::extensions::GeneralName::DNSName(name) => host == *name,
            x509_parser::extensions::GeneralName::IPAddress(bytes) => match host.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(ip)) => *bytes == ip.octets(),
                Ok(std::net::IpAddr::V6(ip)) => *bytes == ip.octets(),
                _ => false,
            },
            _ => false,
        });
        if !has_host {
            return Err(tls_error("certificate SAN does not cover the requested loopback host"));
        }
        let key = PrivateKeyDer::from_pem_slice(key).map_err(|_| tls_error("private key PEM is invalid"))?;
        let mut config =
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| tls_error("TLS protocol setup failed"))?
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|_| tls_error("private key is invalid or does not match the certificate"))?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&leaf);
        let certificate_pem = format!("-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----\n");
        Ok(Self {
            config: Some(Arc::new(config)),
            certificate_der: leaf,
            certificate_pem,
        })
    }

    pub fn scheme(&self) -> &'static str {
        if self.config.is_some() {
            "https"
        } else {
            "http"
        }
    }
    pub async fn serve(self, listener: tokio::net::TcpListener, app: Router) -> std::io::Result<()> {
        if !listener.local_addr()?.ip().is_loopback() {
            return Err(std::io::Error::other("loopback transport required"));
        }
        let app = app.layer(Extension(ListenerScheme(self.scheme())));
        if let Some(config) = self.config {
            axum_server::from_tcp_rustls(
                listener.into_std()?,
                axum_server::tls_rustls::RustlsConfig::from_config(config),
            )?
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
        } else {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn owned_client_accepts_operator_issued_leaf_and_refuses_substitution_before_http() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut ca = rcgen::CertificateParams::new(vec!["fixture authority".into()]).unwrap();
        ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let issuer = rcgen::Issuer::new(ca, rcgen::KeyPair::generate().unwrap());
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
        let cert = params.signed_by(&key, &issuer).unwrap();
        let transport =
            Transport::from_material(cert.pem().as_bytes(), key.serialize_pem().as_bytes(), "127.0.0.1").unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        let app = Router::new().route(
            "/",
            axum::routing::post(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async { "owned" }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(transport.serve(listener, app));
        let client = reqwest::Client::builder()
            .no_proxy()
            .use_preconfigured_tls(crate::common::local_tls::client_config(cert.der().to_vec(), "127.0.0.1").unwrap())
            .build()
            .unwrap();
        assert_eq!(client.post(&url).send().await.unwrap().text().await.unwrap(), "owned");
        let other = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
            .unwrap()
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap();
        let wrong = reqwest::Client::builder()
            .no_proxy()
            .use_preconfigured_tls(crate::common::local_tls::client_config(other.der().to_vec(), "127.0.0.1").unwrap())
            .build()
            .unwrap();
        assert!(wrong.post(&url).body("must never reach server").send().await.is_err());
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        task.abort();
    }
    #[tokio::test]
    async fn fresh_tls_persists_verifies_names_and_refuses_wrong_trust_or_keys() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().into());
        let cfg = AppConfig::from_str(
            "tls: {enabled: true, auto_generate: true, certificate_days: 90}",
            &dir.path().join("config.yaml"),
        )
        .unwrap();
        let first = Transport::load(&home, &cfg, "127.0.0.1").unwrap();
        let again = Transport::load(&home, &cfg, "localhost").unwrap();
        assert_eq!(first.certificate_der, again.certificate_der);
        assert!(Transport::load(&home, &cfg, "::1").is_ok());
        assert!(Transport::load(&home, &cfg, "wrong.invalid").is_err());
        let pinned = reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(reqwest::Certificate::from_der(&first.certificate_der).unwrap())
            .build()
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/",
            axum::routing::get(|Extension(scheme): Extension<ListenerScheme>| async move { scheme.0 }),
        );
        let task = tokio::spawn(first.serve(listener, app));
        let url = format!("https://{address}/");
        assert_eq!(pinned.get(&url).send().await.unwrap().text().await.unwrap(), "https");
        assert!(reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(&url)
            .send()
            .await
            .is_err());
        let original = read_material(&home.root.join("tls/server.pem"), true).unwrap();
        let (_, wrong_key) = generate(90).unwrap();
        assert!(Transport::from_material(&original, wrong_key.as_bytes(), "127.0.0.1").is_err());
        let (expired, key) = generate(-1).unwrap();
        assert!(Transport::from_material(expired.as_bytes(), key.as_bytes(), "127.0.0.1").is_err());
        std::fs::write(home.root.join("tls/server.pem"), b"broken key").unwrap();
        assert!(Transport::load(&home, &cfg, "127.0.0.1").is_err());
        assert_eq!(std::fs::read(home.root.join("tls/server.pem")).unwrap(), b"broken key");
        Transport::renew(&home, &cfg).unwrap();
        assert_ne!(
            Transport::load(&home, &cfg, "127.0.0.1").unwrap().certificate_der,
            again.certificate_der
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, MetadataExt};
            assert_eq!(
                std::fs::metadata(home.root.join("tls/server.pem")).unwrap().mode() & 0o777,
                0o600
            );
            std::fs::remove_file(home.root.join("tls/server.pem")).unwrap();
            symlink(dir.path().join("missing"), home.root.join("tls/server.pem")).unwrap();
            assert!(Transport::load(&home, &cfg, "127.0.0.1").is_err());
            assert!(!dir.path().join("missing").exists());
        }
        task.abort();
    }
}
