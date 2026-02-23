use io_uring::{IoUring, opcode, types};
use socket2::{Domain, Protocol, Socket, Type};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use crate::cooldown::CooldownArray;
use crate::master::PixelWrite;
use crate::spsc::SpscRingBuffer;
use crate::timing_wheel::TimingWheel;

// Tag for completion events
const TAG_INCOMING_UDP: u64 = 1;

pub struct WorkerCore {
    master_queue: Arc<SpscRingBuffer<PixelWrite>>,
    cooldown_master: CooldownArray,
    timing_wheel: TimingWheel,
    port: u16,
}

impl WorkerCore {
    pub fn new(master_queue: Arc<SpscRingBuffer<PixelWrite>>, port: u16) -> Self {
        Self {
            master_queue,
            cooldown_master: CooldownArray::new(),
            timing_wheel: TimingWheel::new(),
            port,
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
    fn run_linux(&mut self) {
        let mut ring = IoUring::builder()
            .setup_coop_taskrun()
            .setup_single_issuer()
            .build(256)
            .expect("Failed to create io_uring");

        // Bind UDP socket using SO_REUSEPORT to shard load across workers
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket.set_reuse_port(true).unwrap();

        let addr: std::net::SocketAddr = format!("0.0.0.0:{}", self.port).parse().unwrap();
        socket.bind(&addr.into()).unwrap();
        let fd = socket.as_raw_fd();

        // Register provided buffer slab (mocked here for illustration)
        // ... (kernel buffer prep)

        let fd_types = types::Fd(fd);
        let recv = opcode::RecvMulti::new(fd_types, 0)
            .buf_group(0)
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

        loop {
            // One syscall to sleep until data arrives
            ring.submit_and_wait(1).unwrap();

            let now_sec = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if now_sec > last_tick_sec {
                // Execute O(1) tick mass eviction
                self.timing_wheel.tick(&mut self.cooldown_master);
                last_tick_sec = now_sec;
            }

            let mut cqes_processed = 0;
            let mut completion = ring.completion();

            while let Some(cqe) = completion.next() {
                cqes_processed += 1;
                if cqe.user_data() == TAG_INCOMING_UDP {
                    let _buffer_id = io_uring::cqueue::buffer_select(cqe.flags());
                    // Route to quiche state machine
                    // let packet = get_provided_buffer(buffer_id, cqe.result() as usize);
                    // process_quic_packet(packet);

                    // Demo pixel write
                    let user_local_id = 42;
                    if !self.cooldown_master.is_on_cooldown(user_local_id) {
                        self.cooldown_master.set_cooldown(user_local_id);
                        self.timing_wheel.add_cooldown(user_local_id);

                        let _ = self.master_queue.push(PixelWrite {
                            x: 10,
                            y: 10,
                            color: 1,
                        });
                    }

                    // Replenish buffer (not submitted yet)
                    // replenish_provided_buffer(&mut ring, buffer_id)
                }
            }
            drop(completion);

            if cqes_processed > 0 {
                ring.submission().sync(); // Update tail pointer, zero syscalls
            }
        }
    }
}
