pub mod canvas;
pub mod cooldown;
pub mod master;
pub mod spsc;
pub mod timing_wheel;
pub mod transport;
pub mod worker;

use crate::canvas::Canvas;
use crate::master::{MasterCore, PixelWrite};
use crate::spsc::SpscRingBuffer;
use crate::worker::WorkerCore;
use std::sync::Arc;

#[cfg(target_os = "linux")]
fn maximize_memlock() {
    unsafe {
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        if libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) != 0 {
            println!("Warning: Failed to set RLIMIT_MEMLOCK to infinity. io_uring may fail.");
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    maximize_memlock();

    println!("Bare-metal canvas server initializing...");

    let port = 4433;
    let args: Vec<String> = std::env::args().collect();
    let num_workers_arg = args
        .iter()
        .position(|r| r == "-w" || r == "--workers")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|val| val.parse::<usize>().ok());

    let core_ids = core_affinity::get_core_ids().expect("Failed to get core IDs");
    let num_cores = core_ids.len();

    let num_workers = num_workers_arg.unwrap_or(num_cores.saturating_sub(1));

    if num_workers == 0 {
        panic!("At least 1 worker is required. Use -w <num> to specify.");
    }

    if num_cores < 2 && num_workers_arg.is_none() {
        panic!(
            "Single core system detected. At least 2 cores are recommended, or force number of workers with -w 1"
        );
    }

    // Partition Cores
    // Core 0: Master (Primary writer + Broadcast)
    // Cores 1+: Workers (Ingress/Validation)
    let master_core_id = core_ids[0].id;

    let worker_cores: Vec<usize> = (0..num_workers)
        .map(|i| core_ids[(i + 1) % num_cores].id)
        .collect();

    println!(
        "Topology: 1 Master (Core {}), {} Workers assigned to cores {:?}",
        master_core_id,
        worker_cores.len(),
        worker_cores
    );

    let canvas = Arc::new(Canvas::new());
    let mut worker_queues = Vec::with_capacity(worker_cores.len());
    let mut workers = Vec::with_capacity(worker_cores.len());

    // Initialize Workers
    for &core_id in &worker_cores {
        let queue = Arc::new(SpscRingBuffer::<PixelWrite>::new());
        worker_queues.push(queue.clone());
        workers.push((WorkerCore::new(queue, port), core_id));
    }

    let master = MasterCore::new(worker_queues, canvas.clone());
    // (BroadcastCore removed)

    // Spawn Threads
    let mut handles = Vec::new();

    // 1. Spawn Workers
    for (mut worker, core_id) in workers {
        handles.push(std::thread::spawn(move || {
            worker.run(core_id);
        }));
    }

    // 2. Run Master on main thread
    println!("Starting Master loop on core {}...", master_core_id);
    master.run(master_core_id);

    // Join threads (Master loop is infinite, so this part is technically unreachable)
    for handle in handles {
        let _ = handle.join();
    }
}
