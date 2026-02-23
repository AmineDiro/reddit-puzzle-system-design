use crate::cooldown::CooldownArray;
use crate::master::PixelWrite;
use crate::spsc::SpscRingBuffer;
use crate::timing_wheel::TimingWheel;
use crate::transport::{PixelDatagram, TransportState};
use io_uring::{IoUring, cqe, opcode, types};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

// Tag for completion events
const TAG_INCOMING_UDP: u64 = 1;
const TAG_OUTGOING_UDP: u64 = 2;

const PKT_BUF_SIZE: usize = 2048; // Max standard UDP (+QUIC) MTU size
const NUM_BUFFERS: u16 = 8192; // Max provided buffers in the slab
const BGID: u16 = 0; // Buffer Group ID

pub struct TxItem {
    pub buf: [u8; 1500],
    pub addr: libc::sockaddr_in,
}

pub struct WorkerCore {
    master_queue: Arc<SpscRingBuffer<PixelWrite>>,
    cooldown_master: CooldownArray,
    timing_wheel: TimingWheel,
    port: u16,
    buffer_slab: Vec<u8>,
    transport: TransportState,
    framing: Framing,
    last_broadcast_index: usize,
    tx_items: Box<[TxItem]>,
    tx_free_indices: Vec<usize>,
}

pub struct RecvMsgFrame<'a> {
    pub peer_addr: SocketAddr,
    pub local_addr: SocketAddr,
    pub payload: &'a mut [u8],
}

pub struct Framing {
    local_port: u16,
}

impl Framing {
    pub fn new(local_port: u16) -> Self {
        Self { local_port }
    }

    pub fn parse<'a>(&self, buf: &'a mut [u8]) -> RecvMsgFrame<'a> {
        // Layout of RecvMsgMulti buffer:
        // 16 bytes: io_uring_recvmsg_out
        // namelen: peer address
        // controllen: ancillary data (IP_PKTINFO)
        // payloadlen: the actual data
        let namelen = u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as usize;
        let controllen = u32::from_ne_bytes(buf[4..8].try_into().unwrap()) as usize;
        let payloadlen = u32::from_ne_bytes(buf[8..12].try_into().unwrap()) as usize;

        let mut pos = 16;

        // 1. Extract Peer Address
        let peer_addr = if namelen >= std::mem::size_of::<libc::sockaddr_in>() {
            let sin: libc::sockaddr_in = unsafe { std::ptr::read(buf[pos..].as_ptr() as *const _) };
            let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
            let port = u16::from_be(sin.sin_port);
            SocketAddr::V4(SocketAddrV4::new(ip, port))
        } else {
            "127.0.0.1:0".parse().unwrap()
        };
        pos += namelen;

        // 2. Extract Local Address (Destination IP) from IP_PKTINFO
        let mut local_ip = Ipv4Addr::UNSPECIFIED;
        if controllen > 0 {
            let mut cmsg_pos = pos;
            let cmsg_end = pos + controllen;
            while cmsg_pos + std::mem::size_of::<libc::cmsghdr>() <= cmsg_end {
                let cmsg: libc::cmsghdr =
                    unsafe { std::ptr::read(buf[cmsg_pos..].as_ptr() as *const _) };
                if cmsg.cmsg_level == libc::IPPROTO_IP && cmsg.cmsg_type == libc::IP_PKTINFO {
                    let info: libc::in_pktinfo =
                        unsafe { std::ptr::read(buf[cmsg_pos + 16..].as_ptr() as *const _) };
                    local_ip = Ipv4Addr::from(u32::from_be(info.ipi_addr.s_addr));
                    break;
                }
                let len = (cmsg.cmsg_len as usize + 7) & !7;
                cmsg_pos += len;
            }
        }
        let local_addr = SocketAddr::V4(SocketAddrV4::new(local_ip, self.local_port));
        pos += controllen;

        let payload = &mut buf[pos..pos + payloadlen];

        RecvMsgFrame {
            peer_addr,
            local_addr,
            payload,
        }
    }
}

impl WorkerCore {
    pub fn new(master_queue: Arc<SpscRingBuffer<PixelWrite>>, port: u16) -> Self {
        let tx_capacity = 4096;
        let mut tx_items = Vec::with_capacity(tx_capacity);
        let mut tx_free_indices = Vec::with_capacity(tx_capacity);
        for i in 0..tx_capacity {
            tx_items.push(TxItem {
                buf: [0; 1500],
                addr: unsafe { std::mem::zeroed() },
            });
            tx_free_indices.push(i);
        }

        Self {
            master_queue,
            cooldown_master: CooldownArray::new(),
            timing_wheel: TimingWheel::new(),
            port,
            buffer_slab: vec![0; PKT_BUF_SIZE * (NUM_BUFFERS as usize)],
            transport: TransportState::new(),
            framing: Framing::new(port),
            last_broadcast_index: 0,
            tx_items: tx_items.into_boxed_slice(),
            tx_free_indices,
        }
    }

    pub fn run(mut self, core_id: usize) {
        if core_affinity::set_for_current(core_affinity::CoreId { id: core_id }) {
            // pinned
        }

        #[cfg(target_os = "linux")]
        self.run_linux();

        #[cfg(not(target_os = "linux"))]
        println!("Worker core only supported via io_uring on Linux.");
    }

    #[cfg(target_os = "linux")]
    fn setup_socket(&self) -> Socket {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket.set_reuse_port(true).unwrap();

        unsafe {
            let opt: libc::c_int = 1;
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                libc::IP_PKTINFO,
                &opt as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }

        let addr: std::net::SocketAddr = format!("0.0.0.0:{}", self.port).parse().unwrap();
        socket.bind(&addr.into()).unwrap();
        socket
    }

    #[cfg(target_os = "linux")]
    fn setup_io_uring(&self) -> IoUring {
        IoUring::builder()
            .setup_coop_taskrun()
            .setup_single_issuer()
            .build(256)
            .expect("Failed to create io_uring")
    }

    #[cfg(target_os = "linux")]
    fn provide_initial_buffers(&mut self, ring: &mut IoUring) {
        let provide_bufs_sqe = opcode::ProvideBuffers::new(
            self.buffer_slab.as_mut_ptr(),
            PKT_BUF_SIZE as i32,
            NUM_BUFFERS as u16,
            BGID,
            0,
        )
        .build()
        .user_data(0);

        unsafe {
            ring.submission().push(&provide_bufs_sqe).unwrap();
        }
        ring.submit_and_wait(1).unwrap();
        ring.completion().next();
    }

    #[cfg(target_os = "linux")]
    fn handle_tick(&mut self, last_tick_sec: &mut u64) {
        // TODO: use something faster to get time, this could be slow
        let now_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if now_sec > *last_tick_sec {
            // Execute O(1) tick mass eviction
            self.timing_wheel.tick(&mut self.cooldown_master);
            *last_tick_sec = now_sec;
        }
    }

    #[cfg(target_os = "linux")]
    fn handle_broadcast(&mut self) {
        let current_active = crate::canvas::ACTIVE_INDEX.load(std::sync::atomic::Ordering::Relaxed);
        if current_active != self.last_broadcast_index {
            self.last_broadcast_index = current_active;

            // Grab the newly compressed buffer without allocating or locking
            let buffer_slice = unsafe { &crate::canvas::BUFFER_POOL[current_active].data[..1200] };

            for conn in self.transport.connections.values_mut() {
                // NOTE:
                // Just puts in quiche state, not sending yet !!
                let _ = conn.dgram_send(buffer_slice);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn handle_incoming_cqe(&mut self, ring: &mut IoUring, flags: u32, fd_types: types::Fd) {
        let buffer_id = match io_uring::cqueue::buffer_select(flags) {
            Some(id) => id,
            None => return,
        };

        let offset = (buffer_id as usize) * PKT_BUF_SIZE;
        let buf = &mut self.buffer_slab[offset..offset + PKT_BUF_SIZE];

        let frame = self.framing.parse(buf);

        if let Some(pixels) =
            self.transport
                .handle_incoming(frame.payload, frame.peer_addr, frame.local_addr)
        {
            for p in pixels {
                let user_id = p.color as u32;
                if !self.cooldown_master.is_on_cooldown(user_id) {
                    self.cooldown_master.set_cooldown(user_id);
                    self.timing_wheel.add_cooldown(user_id);
                    let _ = self.master_queue.push(PixelWrite {
                        x: p.x as usize,
                        y: p.y as usize,
                        color: p.color,
                    });
                }
            }
        }

        // Replenish buffer back to kernel
        let replenish_sqe = opcode::ProvideBuffers::new(
            self.buffer_slab[offset..].as_mut_ptr(),
            PKT_BUF_SIZE as i32,
            1,
            BGID,
            buffer_id as u16,
        )
        .build()
        .user_data(0);

        unsafe {
            ring.submission().push(&replenish_sqe).unwrap();
        }

        if !cqe::more(flags) {
            let recv = opcode::RecvMsgMulti::new(fd_types, BGID)
                .build()
                .user_data(TAG_INCOMING_UDP);
            unsafe {
                ring.submission().push(&recv).unwrap();
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn flush_outgoing(&mut self, ring: &mut IoUring, fd_types: types::Fd) -> usize {
        let mut sqes_added = 0;
        for conn in self.transport.connections.values_mut() {
            while let Some(idx) = self.tx_free_indices.pop() {
                let item = &mut self.tx_items[idx];
                match conn.send(&mut item.buf) {
                    Ok((len, send_info)) => {
                        let dest_addr = match send_info.to {
                            SocketAddr::V4(v4) => v4,
                            _ => {
                                self.tx_free_indices.push(idx);
                                continue;
                            }
                        };

                        item.addr.sin_family = libc::AF_INET as u16;
                        item.addr.sin_port = dest_addr.port().to_be();
                        item.addr.sin_addr.s_addr = u32::from(dest_addr.ip()).to_be();

                        let send_sqe = opcode::SendTo::new(
                            fd_types,
                            item.buf.as_ptr(),
                            len as u32,
                            &item.addr as *const _ as *const libc::sockaddr,
                            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                        )
                        .build()
                        .user_data(TAG_OUTGOING_UDP | ((idx as u64) << 8));

                        unsafe {
                            ring.submission().push(&send_sqe).expect("SQ full");
                        }
                        sqes_added += 1;
                    }
                    Err(_) => {
                        self.tx_free_indices.push(idx);
                        break;
                    }
                }
            }
        }
        sqes_added
    }

    #[cfg(target_os = "linux")]
    fn maintain_connections(&mut self) {
        for conn in self.transport.connections.values_mut() {
            conn.on_timeout();
        }

        self.transport
            .connections
            .retain(|_, conn| !conn.is_closed());
    }

    #[cfg(target_os = "linux")]

    fn run_linux(&mut self) {
        let mut ring = self.setup_io_uring();
        let socket = self.setup_socket();
        let fd = socket.as_raw_fd();

        self.provide_initial_buffers(&mut ring);

        let fd_types = types::Fd(fd);
        let recv = opcode::RecvMsgMulti::new(fd_types, BGID)
            .build()
            .user_data(TAG_INCOMING_UDP);

        unsafe {
            ring.submission().push(&recv).unwrap();
        }
        ring.submit().unwrap();

        let mut last_tick_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.last_broadcast_index =
            crate::canvas::ACTIVE_INDEX.load(std::sync::atomic::Ordering::Relaxed);

        loop {
            // One syscall to sleep until data arrives
            ring.submit_and_wait(1).unwrap();

            self.handle_tick(&mut last_tick_sec);
            self.handle_broadcast();

            let mut cqes_processed = 0;
            let mut pending_cqes = [(0u64, 0i32, 0u32); 256];
            let mut parsed_count = 0;

            let mut completion = ring.completion();
            while let Some(cqe) = completion.next() {
                cqes_processed += 1;
                if parsed_count < 256 {
                    pending_cqes[parsed_count] = (cqe.user_data(), cqe.result(), cqe.flags());
                    parsed_count += 1;
                }
            }
            drop(completion);

            for i in 0..parsed_count {
                let (user_data, result, flags) = pending_cqes[i];
                if user_data & 0xFF == TAG_OUTGOING_UDP {
                    let idx = (user_data >> 8) as usize;
                    self.tx_free_indices.push(idx);
                } else if user_data == TAG_INCOMING_UDP && result >= 0 {
                    self.handle_incoming_cqe(&mut ring, flags, fd_types);
                }
            }

            let sqes_added = self.flush_outgoing(&mut ring, fd_types);

            if cqes_processed > 0 || sqes_added > 0 {
                ring.submission().sync(); // Wake up kernel if SQEs pending
            }

            self.maintain_connections();
        }
    }
}
