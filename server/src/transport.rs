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

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct SourceConnectionId(pub Vec<u8>);

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct DestinationConnectionId(pub Vec<u8>);

pub const MAX_CONNECTIONS: usize = crate::cooldown::COOLDOWN_ARRAY_LEN * 64;

pub struct TransportState {
    // Map of QUIC Source Connection ID -> Active Connection (Thread local)
    pub connections: FxHashMap<SourceConnectionId, (u32, Connection, DestinationConnectionId)>,
    pub cid_map: FxHashMap<DestinationConnectionId, SourceConnectionId>,
    pub free_user_ids: Vec<u32>,

    // Quiche backend config
    pub config: quiche::Config,
}

impl Default for TransportState {
    fn default() -> Self {
        Self::new()
    }
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

        let mut free_user_ids: Vec<u32> = (0..MAX_CONNECTIONS as u32).collect();

        Self {
            connections: FxHashMap::with_capacity_and_hasher(MAX_CONNECTIONS, Default::default()),
            cid_map: FxHashMap::with_capacity_and_hasher(MAX_CONNECTIONS, Default::default()),
            free_user_ids,
            config,
        }
    }

    pub fn accept_connection(
        &mut self,
        scid: &[u8],
        dcid: &[u8],
        odcid: Option<&[u8]>,
        local: SocketAddr,
        peer: SocketAddr,
    ) -> Result<(), quiche::Error> {
        if self.free_user_ids.is_empty() {
            {
                #[cfg(feature = "debug-logs")]
                println!("Worker at capacity, rejecting connection from {:?}", peer);
            }
            return Err(quiche::Error::Done);
        }

        let scid_val = quiche::ConnectionId::from_ref(scid);
        let odcid_val = odcid.map(quiche::ConnectionId::from_ref);
        let conn = quiche::accept(&scid_val, odcid_val.as_ref(), local, peer, &mut self.config)?;

        let user_id = self.free_user_ids.pop().unwrap();

        #[cfg(feature = "debug-logs")]
        println!(
            "Accepted new QUIC connection ID: {:?} (user_id: {})",
            scid_val, user_id
        );

        self.connections.insert(
            SourceConnectionId(scid.to_vec()),
            (user_id, conn, DestinationConnectionId(dcid.to_vec())),
        );
        Ok(())
    }

    fn resolve_connection_id(
        &mut self,
        dcid: &[u8],
        ty: quiche::Type,
        local: SocketAddr,
        peer: SocketAddr,
    ) -> Option<SourceConnectionId> {
        let process_id = self
            .cid_map
            .get(&DestinationConnectionId(dcid.to_vec()))
            .map_or_else(|| SourceConnectionId(dcid.to_vec()), |id| id.clone());

        if self.connections.contains_key(&process_id) {
            return Some(process_id);
        }

        if ty != quiche::Type::Initial {
            return None;
        }

        // else new connection has arrived, accept it
        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        rand::thread_rng().fill(&mut scid);

        match self.accept_connection(&scid[..], dcid, None, local, peer) {
            Ok(_) => {
                let source_cid = SourceConnectionId(scid.to_vec());
                self.cid_map
                    .insert(DestinationConnectionId(dcid.to_vec()), source_cid.clone());
                Some(source_cid)
            }
            Err(_e) => {
                #[cfg(feature = "debug-logs")]
                println!("Failed to accept connection: {:?}", _e);
                None
            }
        }
    }

    fn process_datagrams(conn: &mut Connection) -> Vec<PixelDatagram> {
        let mut pixels = Vec::new();
        if !conn.is_established() {
            return pixels;
        }

        // TODO: use h3 to poll dgrams
        // In a real WebTransport setup, we'd use h3 to poll dgrams
        let mut dgram_buf = [0; 1500];
        // Securely copies the decrypted, verified WebTransport datagram
        // out of quiche's internal state machine into our local variable dgram_buf
        while let Ok(len) = conn.dgram_recv(&mut dgram_buf) {
            if len == std::mem::size_of::<PixelDatagram>() {
                pixels.push(PixelDatagram {
                    x: u16::from_ne_bytes([dgram_buf[0], dgram_buf[1]]),
                    y: u16::from_ne_bytes([dgram_buf[2], dgram_buf[3]]),
                    color: dgram_buf[4],
                });
            } else {
                #[cfg(feature = "debug-logs")]
                println!(
                    "Received datagram of incorrect size: {} (expected {})",
                    len,
                    std::mem::size_of::<PixelDatagram>()
                );
            }
        }
        pixels
    }

    pub fn handle_incoming(
        &mut self,
        buf: &mut [u8],
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Option<(u32, Vec<PixelDatagram>)> {
        let hdr = quiche::Header::from_slice(buf, quiche::MAX_CONN_ID_LEN).ok()?;

        let process_id = self.resolve_connection_id(&hdr.dcid[..], hdr.ty, local, peer)?;

        let tuple = self.connections.get_mut(&process_id)?;
        let user_id = tuple.0;
        let conn = &mut tuple.1;

        let recv_info = RecvInfo {
            from: peer,
            to: local,
        };
        let _ = conn.recv(buf, recv_info);

        let pixels = Self::process_datagrams(conn);

        if pixels.is_empty() {
            None
        } else {
            #[cfg(feature = "debug-logs")]
            println!("Received {} pixels from {:?}", pixels.len(), peer);
            Some((user_id, pixels))
        }
    }

    pub fn cleanup_connections(&mut self) {
        let mut freed_ids = Vec::new();
        let mut freed_dcids = Vec::new();

        self.connections.retain(|_, (id, conn, dcid)| {
            if conn.is_closed() {
                freed_ids.push(*id);
                freed_dcids.push(dcid.clone());
                false
            } else {
                true
            }
        });

        for dcid in freed_dcids {
            self.cid_map.remove(&dcid);
        }

        self.free_user_ids.extend(freed_ids);
    }
}
