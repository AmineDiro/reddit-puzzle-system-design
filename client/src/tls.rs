use std::sync::Arc;
use std::time::Duration;
use wtransport::ClientConfig;
use wtransport::tls::rustls::ClientConfig as RustlsClientConfig;
use wtransport::tls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use wtransport::tls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};

#[derive(Debug)]
struct RecklessVerifier;

impl ServerCertVerifier for RecklessVerifier {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, wtransport::tls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &wtransport::tls::rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, wtransport::tls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &wtransport::tls::rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, wtransport::tls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<wtransport::tls::rustls::SignatureScheme> {
        wtransport::tls::rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn build_optimized_config() -> ClientConfig {
    let mut crypto = RustlsClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(RecklessVerifier))
        .with_no_client_auth();
    crypto.enable_early_data = true;

    ClientConfig::builder()
        .with_bind_address("0.0.0.0:0".parse().unwrap())
        .with_custom_tls(crypto)
        .keep_alive_interval(Some(Duration::from_secs(15)))
        .max_idle_timeout(Some(Duration::from_secs(600)))
        .unwrap()
        .build()
}
