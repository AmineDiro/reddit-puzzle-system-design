use quiche::{Connection, RecvInfo};
use rustc_hash::FxHashMap;
use std::net::SocketAddr;

#[repr(C, packed)]
pub struct PixelDatagram {
    pub x: u16,
    pub y: u16,
    pub color: u8,
}

pub struct TransportState {
    // Map of QUIC Source Connection ID -> Active Connection (Thread local)
    pub connections: FxHashMap<Vec<u8>, Connection>,

    // Quiche backend config
    pub config: quiche::Config,
}

impl TransportState {
    pub fn new() -> Self {
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

        // Load WebTransport configurations
        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();

        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_unidi(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_unidi(100);
        config.set_disable_active_migration(true);

        // Required for WebTransport / Datagrams
        config.enable_dgram(true, 1000, 1000);

        // Ideally load TLS certs here
        // config.load_cert_chain_from_pem_file("cert.crt").unwrap();
        // config.load_priv_key_from_pem_file("key.key").unwrap();

        Self {
            connections: FxHashMap::with_capacity_and_hasher(10000, Default::default()),
            config,
        }
    }

    pub fn accept_connection(
        &mut self,
        scid: &[u8],
        odcid: Option<&[u8]>,
        local: SocketAddr,
        peer: SocketAddr,
    ) -> Result<&mut Connection, quiche::Error> {
        let conn = quiche::accept(scid, odcid, local, peer, &mut self.config)?;

        println!("Accepted new QUIC connection ID: {:?}", scid);
        self.connections.insert(scid.to_vec(), conn);
        Ok(self.connections.get_mut(scid).unwrap())
    }

    pub fn handle_incoming(
        &mut self,
        buf: &mut [u8],
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Option<Vec<PixelDatagram>> {
        let mut hdr = match quiche::Header::parse(buf, None) {
            Ok(v) => v,
            Err(_) => return None,
        };

        let conn = if !self.connections.contains_key(&hdr.dcid) {
            // New connection? Handle version negotiation/handshake
            if hdr.ty != quiche::Type::Initial {
                return None;
            }
            match self.accept_connection(&hdr.dcid, None, local, peer) {
                Ok(c) => c,
                Err(_) => return None,
            }
        } else {
            self.connections.get_mut(&hdr.dcid).unwrap()
        };

        let recv_info = RecvInfo {
            from: peer,
            to: local,
        };
        let _ = conn.recv(buf, recv_info);

        // Extract WebTransport datagrams (Pixels)
        let mut pixels = Vec::new();
        if conn.is_established() {
            // In a real WebTransport setup, we'd use h3 to poll dgrams
            // Mocking datagram extraction for the TRD flow
            while let Ok(len) = conn.dgram_recv(buf) {
                if len == std::mem::size_of::<PixelDatagram>() {
                    let pixel: PixelDatagram = unsafe { std::ptr::read(buf.as_ptr() as *const _) };
                    pixels.push(pixel);
                }
            }
        }

        if pixels.is_empty() {
            None
        } else {
            Some(pixels)
        }
    }
}
