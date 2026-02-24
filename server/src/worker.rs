use crate::cooldown::CooldownArray;
use crate::master::PixelWrite;
use crate::spsc::SpscRingBuffer;
use crate::timing_wheel::TimingWheel;
use crate::transport::TransportState;
#[cfg(target_os = "linux")]
use io_uring::{IoUring, opcode, types};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

// Tag for completion events
const TAG_INCOMING_UDP: u64 = 1;
const TAG_OUTGOING_UDP: u64 = 2;

const PKT_BUF_SIZE: usize = 2048; // Max standard UDP (+QUIC) MTU size
const NUM_BUFFERS: u16 = 65535; // Maximum allowable provided buffers (u16 limit)
const TX_CAPACITY: usize = 65536; // Increased from 16384
const BGID: u16 = 0; // Buffer Group ID

pub struct TxItem {
    pub buf: [u8; 1500],
    pub addr: libc::sockaddr_in,
    pub iov: libc::iovec,
    pub msghdr: libc::msghdr,
}

pub struct WorkerCore {
    master_queue: Arc<SpscRingBuffer<PixelWrite>>,
    cooldown_master: CooldownArray,
    timing_wheel: Box<TimingWheel>,
    port: u16,
    buffer_slab: Vec<u8>,
    transport: TransportState,
    framing: Framing,
    last_broadcast_index: usize,
    tx_items: Box<[TxItem]>,
    tx_free_indices: Vec<usize>,
    msghdr: Box<libc::msghdr>,
    last_sent_canvas: Box<[u8; crate::canvas::CANVAS_SIZE]>,
    broadcast_ticks: u32,
    diff_buffer: Vec<u8>,
}

unsafe impl Send for WorkerCore {}
unsafe impl Sync for WorkerCore {}

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
        // namelen (padded to msghdr.msg_namelen): peer address
        // controllen (padded to msghdr.msg_controllen): ancillary data (IP_PKTINFO)
        // payloadlen: the actual data

        let namelen = u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as usize;
        let controllen = u32::from_ne_bytes(buf[4..8].try_into().unwrap()) as usize;
        let payloadlen = u32::from_ne_bytes(buf[8..12].try_into().unwrap()) as usize;

        // Constants matching WorkerCore msghdr configuration
        let msg_namelen_cap = std::mem::size_of::<libc::sockaddr_in>(); // 16
        let msg_controllen_cap = 64;

        let name_pos = 16;
        let control_pos = name_pos + msg_namelen_cap;
        let payload_pos = control_pos + msg_controllen_cap;

        // 1. Extract Peer Address
        let peer_addr =
            if namelen >= std::mem::size_of::<libc::sockaddr_in>() && namelen <= msg_namelen_cap {
                let sin: libc::sockaddr_in =
                    unsafe { std::ptr::read(buf[name_pos..].as_ptr() as *const _) };
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let port = u16::from_be(sin.sin_port);
                SocketAddr::V4(SocketAddrV4::new(ip, port))
            } else {
                "127.0.0.1:0".parse().unwrap()
            };

        // 2. Extract Local Address (Destination IP) from IP_PKTINFO
        let mut local_ip = Ipv4Addr::UNSPECIFIED;
        if controllen > 0 && controllen <= msg_controllen_cap {
            let mut cmsg_pos = control_pos;
            let cmsg_end = control_pos + controllen;
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

        let payload = &mut buf[payload_pos..payload_pos + payloadlen];

        RecvMsgFrame {
            peer_addr,
            local_addr,
            payload,
        }
    }
}

impl WorkerCore {
    pub fn new(master_queue: Arc<SpscRingBuffer<PixelWrite>>, port: u16) -> Self {
        let mut tx_items = Vec::with_capacity(TX_CAPACITY);
        let mut tx_free_indices = Vec::with_capacity(TX_CAPACITY);
        for i in 0..TX_CAPACITY {
            tx_items.push(TxItem {
                buf: [0; 1500],
                addr: unsafe { std::mem::zeroed() },
                iov: unsafe { std::mem::zeroed() },
                msghdr: unsafe { std::mem::zeroed() },
            });
            tx_free_indices.push(i);
        }

        Self {
            master_queue,
            cooldown_master: CooldownArray::new(),
            timing_wheel: Box::new(TimingWheel::new()),
            port,
            buffer_slab: vec![0; PKT_BUF_SIZE * (NUM_BUFFERS as usize)],
            transport: TransportState::new(),
            framing: Framing::new(port),
            last_broadcast_index: 0,
            tx_items: tx_items.into_boxed_slice(),
            tx_free_indices,
            msghdr: Box::new(unsafe {
                let mut msghdr: libc::msghdr = std::mem::zeroed();
                msghdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as _;
                msghdr.msg_controllen = 64; // Enough for IP_PKTINFO
                msghdr
            }),
            last_sent_canvas: vec![0; crate::canvas::CANVAS_SIZE]
                .into_boxed_slice()
                .try_into()
                .unwrap(),
            broadcast_ticks: 0,
            diff_buffer: Vec::with_capacity(1024),
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
        unsafe {
            let opt: libc::c_int = 1;
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_REUSEPORT,
                &opt as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                &opt as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }

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

        // Increase Kernel UDP buffers
        let rcv_buf = 32 * 1024 * 1024; // 32MB
        let snd_buf = 32 * 1024 * 1024; // 32MB
        socket.set_recv_buffer_size(rcv_buf).unwrap();
        socket.set_send_buffer_size(snd_buf).unwrap();

        socket.bind(&addr.into()).unwrap();
        socket
    }

    #[cfg(target_os = "linux")]
    fn setup_io_uring(&self) -> IoUring {
        IoUring::builder()
            .setup_coop_taskrun()
            .setup_single_issuer()
            .build(32768)
            .or_else(|_| {
                println!("Warning: Failed to create io_uring of size 32768, falling back to 16384");
                IoUring::builder()
                    .setup_coop_taskrun()
                    .setup_single_issuer()
                    .build(16384)
            })
            .or_else(|_| {
                println!("Warning: Failed to create io_uring of size 16384, falling back to 8192");
                IoUring::builder()
                    .setup_coop_taskrun()
                    .setup_single_issuer()
                    .build(8192)
            })
            .or_else(|_| {
                println!("Warning: Failed to create io_uring of size 8192, falling back to 4096");
                IoUring::builder()
                    .setup_coop_taskrun()
                    .setup_single_issuer()
                    .build(4096)
            })
            .or_else(|_| {
                println!("Warning: Failed to create io_uring of size 4096, falling back to 2048");
                IoUring::builder()
                    .setup_coop_taskrun()
                    .setup_single_issuer()
                    .build(2048)
            })
            .or_else(|_| {
                println!("Warning: Failed to create io_uring of size 2048, falling back to 1024");
                IoUring::builder()
                    .setup_coop_taskrun()
                    .setup_single_issuer()
                    .build(1024)
            })
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
            self.broadcast_ticks += 1;

            if self.broadcast_ticks == 1 || self.broadcast_ticks % 60 == 0 {
                // Send full RLE rarely
                unsafe {
                    let compressed_len = crate::canvas::COMPRESSED_LENS[current_active];
                    let buffer_slice = &crate::canvas::COMPRESSED_BUFFER_POOL[current_active].data
                        [..compressed_len];

                    for (_, conn) in self.transport.connections.values_mut() {
                        #[cfg(feature = "debug-logs")]
                        println!(
                            "Worker: broadcasting {} bytes of FULL RLE data to client",
                            compressed_len
                        );

                        // Send compressed data in MTU-sized chunks. 1200 is safe for most networks.
                        for chunk in buffer_slice.chunks(1200) {
                            let _ = conn.dgram_send(chunk);
                        }
                    }

                    // Sync last_sent_canvas
                    let new_canvas = &crate::canvas::BUFFER_POOL[current_active].data;
                    self.last_sent_canvas.copy_from_slice(new_canvas);
                }
            } else {
                // Send very compressed diff
                self.diff_buffer.clear();
                let new_canvas = unsafe { &crate::canvas::BUFFER_POOL[current_active].data };

                for i in 0..crate::canvas::CANVAS_SIZE {
                    let new_pixel = new_canvas[i];
                    if self.last_sent_canvas[i] != new_pixel {
                        // Changed cell: [u32 index, u8 color]
                        self.diff_buffer
                            .extend_from_slice(&(i as u32).to_le_bytes());
                        self.diff_buffer.push(new_pixel);

                        self.last_sent_canvas[i] = new_pixel;
                    }
                }

                if !self.diff_buffer.is_empty() {
                    #[cfg(feature = "debug-logs")]
                    println!(
                        "Worker: broadcasting {} bytes of DIFF data to client",
                        self.diff_buffer.len()
                    );
                    for (_, conn) in self.transport.connections.values_mut() {
                        for chunk in self.diff_buffer.chunks(1200) {
                            let _ = conn.dgram_send(chunk);
                        }
                    }
                }
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

        if let Some((user_id, pixels)) =
            self.transport
                .handle_incoming(frame.payload, frame.peer_addr, frame.local_addr)
        {
            for p in pixels {
                if !self.cooldown_master.is_on_cooldown(user_id) {
                    self.cooldown_master.set_cooldown(user_id);
                    self.timing_wheel.add_cooldown(user_id);
                    let _ = self.master_queue.push(PixelWrite {
                        x: p.x,
                        y: p.y,
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
            if ring.submission().push(&replenish_sqe).is_err() {
                ring.submit().unwrap();
                ring.submission().push(&replenish_sqe).unwrap();
            }
        }

        if !io_uring::cqueue::more(flags) {
            let recv = opcode::RecvMsgMulti::new(fd_types, self.msghdr.as_ref() as *const _, BGID)
                .build()
                .user_data(TAG_INCOMING_UDP);
            unsafe {
                if ring.submission().push(&recv).is_err() {
                    ring.submit().unwrap();
                    ring.submission().push(&recv).unwrap();
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn flush_outgoing(&mut self, ring: &mut IoUring, fd_types: types::Fd) -> usize {
        let mut sqes_added = 0;
        for (_, conn) in self.transport.connections.values_mut() {
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
                        item.addr.sin_addr.s_addr = u32::from(dest_addr.ip().clone()).to_be();

                        item.iov.iov_base = item.buf.as_mut_ptr() as *mut _;
                        item.iov.iov_len = len as _;

                        item.msghdr.msg_name = &mut item.addr as *mut _ as *mut _;
                        item.msghdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as _;
                        item.msghdr.msg_iov = &mut item.iov;
                        item.msghdr.msg_iovlen = 1;

                        let send_sqe = opcode::SendMsg::new(fd_types, &item.msghdr)
                            .build()
                            .user_data(TAG_OUTGOING_UDP | ((idx as u64) << 8));

                        unsafe {
                            if ring.submission().push(&send_sqe).is_err() {
                                // flush the pending items to the Linux kernel, making room for the new job, and then retry pushing it.
                                ring.submit().unwrap();
                                ring.submission().push(&send_sqe).unwrap();
                            }
                        }
                        sqes_added += 1;
                    }
                    Err(_e) => {
                        self.tx_free_indices.push(idx);
                        break;
                    }
                }
            }
        }
        sqes_added
    }

    #[cfg(target_os = "linux")]
    fn maintain_connections(&mut self, last_timeout_ms: &mut u128) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        // Throttle to every 20ms to save massive CPU overhead on 40k+ connections
        if now_ms - *last_timeout_ms >= 20 {
            for (_, conn) in self.transport.connections.values_mut() {
                conn.on_timeout();
            }

            self.transport
                .connections
                .retain(|_, (_, conn)| !conn.is_closed());

            *last_timeout_ms = now_ms;
        }
    }

    #[cfg(target_os = "linux")]

    fn run_linux(&mut self) {
        let mut ring = self.setup_io_uring();
        let socket = self.setup_socket();
        let fd = socket.as_raw_fd();

        self.provide_initial_buffers(&mut ring);

        let fd_types = types::Fd(fd);
        let recv = opcode::RecvMsgMulti::new(fd_types, self.msghdr.as_ref() as *const _, BGID)
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

        let mut last_timeout_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        self.last_broadcast_index =
            crate::canvas::ACTIVE_INDEX.load(std::sync::atomic::Ordering::Relaxed);

        loop {
            // One syscall to sleep until data arrives
            ring.submit_and_wait(1).unwrap();

            self.handle_tick(&mut last_tick_sec);
            self.handle_broadcast();

            let mut cqes_processed = 0;
            let mut pending_cqes = Box::new([(0u64, 0i32, 0u32); 65536]);
            let mut parsed_count = 0;

            let mut completion = ring.completion();
            while let Some(cqe) = completion.next() {
                cqes_processed += 1;
                if parsed_count < 65536 {
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
                } else if user_data == TAG_INCOMING_UDP {
                    if result >= 0 {
                        self.handle_incoming_cqe(&mut ring, flags, fd_types);
                    } else {
                        #[cfg(feature = "debug-logs")]
                        println!("CQE error in RecvMsgMulti: {}", result);

                        if !io_uring::cqueue::more(flags) {
                            let recv = opcode::RecvMsgMulti::new(
                                fd_types,
                                self.msghdr.as_ref() as *const _,
                                BGID,
                            )
                            .build()
                            .user_data(TAG_INCOMING_UDP);
                            unsafe {
                                if ring.submission().push(&recv).is_err() {
                                    ring.submit().unwrap();
                                    ring.submission().push(&recv).unwrap();
                                }
                            }
                        }
                    }
                }
            }

            let sqes_added = self.flush_outgoing(&mut ring, fd_types);

            if cqes_processed > 0 || sqes_added > 0 {
                ring.submission().sync(); // Wake up kernel if SQEs pending
            }

            self.maintain_connections(&mut last_timeout_ms);
        }
    }
}
