//! The pinned client must not follow the platform verifier's root file.
//! This binary is its own process, so `SSL_CERT_FILE` cannot change roots
//! loaded by any other test. macOS and Windows do not consult that variable.
//! UNVERIFIED there, and this test does not observe OCSP.

use std::io::{Read, Write};
use std::sync::Arc;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios")))]
#[tokio::test]
async fn pinned_client_ignores_the_platform_root_file() {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(vec!["Pinned Test CA".into()]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec!["pinned.example".into()]).unwrap();
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
    let cert = rustls::pki_types::CertificateDer::from(leaf.der().to_vec());
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into());
    let mut server_config =
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
    server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let server_config = Arc::new(server_config);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); // DevSkim: ignore DS162092 because this fixture binds only to loopback.
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            tcp.set_write_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut conn = rustls::ServerConnection::new(Arc::clone(&server_config)).unwrap();
            let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
            let mut buf = [0u8; 2048];
            let mut seen = Vec::new();
            loop {
                match tls.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        seen.extend_from_slice(&buf[..n]);
                        if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => return,
                }
            }
            let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = tls.flush();
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca_cert.pem()).unwrap();
    let previous = std::env::var("SSL_CERT_FILE").ok();
    std::env::set_var("SSL_CERT_FILE", &ca_path);
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("SSL_CERT_FILE", value),
                None => std::env::remove_var("SSL_CERT_FILE"),
            }
        }
    }
    let restore = Restore(previous);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    // tls-verifier: platform-contrast
    let platform = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs("pinned.example", &[addr])
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let pinned = cgagentharness::server::web_search::pinned_client_builder_for_test(
        "pinned.example",
        &[addr],
        std::time::Duration::from_secs(3),
    )
    .build()
    .unwrap();
    drop(restore);
    let url = "https://pinned.example/";
    let platform_body = platform
        .get(url)
        .send()
        .await
        .expect("platform verifier trusts SSL_CERT_FILE");
    assert_eq!(platform_body.text().await.unwrap(), "ok");
    let rejected = pinned
        .get(url)
        .send()
        .await
        .expect_err("pinned client must reject the private CA");
    let detail = format!("{rejected:?}").to_ascii_lowercase();
    assert!(
        detail.contains("certificate") || detail.contains("unknownissuer") || detail.contains("unknown issuer"),
        "pinned rejection was not a certificate failure: {rejected:?}"
    );
    server.join().unwrap();
}
