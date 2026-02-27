use quinn::ClientConfig;
use rustls::client::{ServerCertVerified, ServerCertVerifier};
use rustls::{Certificate, Error, ServerName};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug)]
struct RecklessVerifier;

impl ServerCertVerifier for RecklessVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &Certificate,
        _intermediates: &[Certificate],
        _server_name: &ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: SystemTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }
}

pub fn build_optimized_config() -> ClientConfig {
    let mut crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(RecklessVerifier))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"h3".to_vec()];

    let mut config = ClientConfig::new(Arc::new(crypto));

    let mut transport = quinn::TransportConfig::default();
    // 1 min timeout
    transport.max_idle_timeout(Some(std::time::Duration::from_secs(60).try_into().unwrap()));

    // Default receive_window is 16MB (!)
    // We only receive small broadcast diffs, so 64KB MAAX
    transport.receive_window(65_536u32.into());
    transport.send_window(65_536);

    // Stream windows — we don't use streams at all, just datagrams
    transport.stream_receive_window(4_096u32.into());

    // Datagram buffer — enough for a few broadcast chunks
    transport.datagram_receive_buffer_size(Some(65_536));
    transport.datagram_send_buffer_size(4_096);

    config.transport_config(Arc::new(transport));

    config
}
