//! Exact certificate trust for an owned loopback portal. This module is also
//! compiled into the native owner; it is never used for internet content reads.
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    CertificateError, DigitallySignedStruct, Error, SignatureScheme,
};
use std::sync::Arc;
use x509_parser::prelude::{FromDer, X509Certificate};

#[derive(Debug)]
struct OwnedCertificate {
    certificate: Vec<u8>,
    host: String,
}

impl ServerCertVerifier for OwnedCertificate {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        _chain: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let refused = || Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure);
        if leaf.as_ref() != self.certificate || name.to_str() != self.host {
            return Err(refused());
        }
        let (_, cert) = X509Certificate::from_der(leaf.as_ref()).map_err(|_| refused())?;
        let now = i64::try_from(now.as_secs()).map_err(|_| refused())?;
        if now < cert.validity().not_before.timestamp() || now > cert.validity().not_after.timestamp() || cert.is_ca() {
            return Err(refused());
        }
        let san = cert
            .subject_alternative_name()
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        let named = san.value.general_names.iter().any(|n| match n {
            x509_parser::extensions::GeneralName::DNSName(n) => *n == self.host,
            x509_parser::extensions::GeneralName::IPAddress(bytes) => match self.host.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(ip)) => *bytes == ip.octets(),
                Ok(std::net::IpAddr::V6(ip)) => *bytes == ip.octets(),
                _ => false,
            },
            _ => false,
        });
        if !named
            || cert
                .extended_key_usage()
                .map_err(|_| refused())?
                .is_some_and(|u| !u.value.server_auth)
            || cert
                .key_usage()
                .map_err(|_| refused())?
                .is_some_and(|u| !u.value.digital_signature())
        {
            return Err(refused());
        }
        // The private pipe/owned private file is the trust authority for this
        // exact leaf. Rustls verifies possession of its key below before HTTP
        // bytes can be sent; no CA-issued sibling certificate can substitute.
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn client_config(certificate: Vec<u8>, host: &str) -> Result<rustls::ClientConfig, String> {
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") || certificate.is_empty() || certificate.len() > 8192 {
        return Err("invalid owned loopback certificate".into());
    }
    let verifier = OwnedCertificate {
        certificate,
        host: host.into(),
    };
    let name = ServerName::try_from(host.to_string()).map_err(|_| "invalid loopback name")?;
    verifier
        .verify_server_cert(
            &CertificateDer::from(verifier.certificate.as_slice()),
            &[],
            &name,
            &[],
            UnixTime::now(),
        )
        .map_err(|_| "owned certificate has invalid name, validity, or key usage")?;
    Ok(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| "TLS protocols unavailable")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_leaf_pin_checks_name_time_usage_and_identity() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        assert!(client_config(cert.der().to_vec(), "127.0.0.1").is_ok());
        assert!(client_config(cert.der().to_vec(), "localhost").is_err());
        assert!(client_config(cert.der().to_vec(), "example.com").is_err());
        let verifier = OwnedCertificate {
            certificate: cert.der().to_vec(),
            host: "127.0.0.1".into(),
        };
        let other = params.self_signed(&rcgen::KeyPair::generate().unwrap()).unwrap();
        assert!(verifier
            .verify_server_cert(
                other.der(),
                &[],
                &ServerName::try_from("127.0.0.1").unwrap(),
                &[],
                UnixTime::now()
            )
            .is_err());
        params.not_before = rcgen::date_time_ymd(2020, 1, 1);
        params.not_after = rcgen::date_time_ymd(2020, 1, 2);
        assert!(client_config(params.self_signed(&key).unwrap().der().to_vec(), "127.0.0.1").is_err());
    }
}
