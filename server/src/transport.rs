use quiche::{Connection, RecvInfo};
use rand::Rng;
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
    pub connections: FxHashMap<Vec<u8>, (u32, Connection)>,
    pub next_user_id: u32,

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
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);

        // Required for WebTransport / Datagrams
        config.enable_dgram(true, 1000, 1000);

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        std::fs::write("cert.crt", cert.cert.pem()).unwrap();
        std::fs::write("key.key", cert.key_pair.serialize_pem()).unwrap();

        config.load_cert_chain_from_pem_file("cert.crt").unwrap();
        config.load_priv_key_from_pem_file("key.key").unwrap();

        Self {
            connections: FxHashMap::with_capacity_and_hasher(10000, Default::default()),
            next_user_id: 0,
            config,
        }
    }

    pub fn accept_connection(
        &mut self,
        scid: &[u8],
        odcid: Option<&[u8]>,
        local: SocketAddr,
        peer: SocketAddr,
    ) -> Result<(u32, Connection), quiche::Error> {
        let scid_val = quiche::ConnectionId::from_ref(scid);
        let odcid_val = odcid.map(quiche::ConnectionId::from_ref);
        let conn = quiche::accept(&scid_val, odcid_val.as_ref(), local, peer, &mut self.config)?;

        // Modulo 65536 to fit within CooldownArray (1024 * 64 bits)
        let user_id = self.next_user_id % (crate::cooldown::COOLDOWN_ARRAY_LEN as u32 * 64);
        self.next_user_id = self.next_user_id.wrapping_add(1);

        #[cfg(feature = "debug-logs")]
        println!("Accepted new QUIC connection ID: {:?}", scid_val);
        self.connections.insert(scid.to_vec(), (user_id, conn));
        Ok((user_id, conn))
    }

    pub fn handle_incoming(
        &mut self,
        buf: &mut [u8],
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Option<(u32, Vec<PixelDatagram>)> {
        let mut hdr = match quiche::Header::from_slice(buf, quiche::MAX_CONN_ID_LEN) {
            Ok(v) => v,
            Err(_) => return None,
        };

        let (user_id, conn) = if !self.connections.contains_key(&hdr.dcid[..]) {
            // New connection? Handle version negotiation/handshake
            if hdr.ty != quiche::Type::Initial {
                return None;
            }

            let mut scid = [0; quiche::MAX_CONN_ID_LEN];
            rand::thread_rng().fill(&mut scid);

            match self.accept_connection(&scid[..], Some(&hdr.dcid[..]), local, peer) {
                Ok(tuple) => (tuple.0, &mut tuple.1),
                Err(e) => {
                    #[cfg(feature = "debug-logs")]
                    println!("Failed to accept connection: {:?}", e);
                    return None;
                }
            }
        } else {
            let tuple = self.connections.get_mut(&hdr.dcid[..]).unwrap();
            (tuple.0, &mut tuple.1)
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
            #[cfg(feature = "debug-logs")]
            println!("Received {} pixels from {:?}", pixels.len(), peer);
            Some((user_id, pixels))
        }
    }
}
