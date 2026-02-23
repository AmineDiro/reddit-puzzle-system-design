pub mod broadcast;
pub mod canvas;
pub mod cooldown;
pub mod master;
pub mod spsc;
pub mod timing_wheel;
pub mod worker;

use crate::broadcast::BroadcastCore;
use crate::canvas::Canvas;
use crate::master::{MasterCore, PixelWrite};
use crate::spsc::SpscRingBuffer;
use crate::worker::WorkerCore;
use std::sync::Arc;

fn main() {
    println!("Bare-metal canvas server initializing...");

    let core_ids = core_affinity::get_core_ids().expect("Failed to get core IDs");
    let num_cores = core_ids.len();

    if num_cores < 2 {
        panic!("Single core system not supported for bare-metal architecture");
    }

    // Partition Cores
    // Core 0: Master (Primary writer)
    // Core 1: Broadcast (Compressor/Publisher)
    // Cores 2+: Workers (Ingress/Validation)
    let master_core_id = core_ids[0].id;
    let broadcast_core_id = core_ids[1].id;

    let worker_cores = core_ids.iter().skip(2).map(|c| c.id).collect();

    println!(
        "Topology: 1 Master (Core {}), 1 Broadcast (Core {}), {} Workers",
        master_core_id,
        broadcast_core_id,
        worker_cores.len()
    );

    let canvas = Arc::new(Canvas::new());
    let mut worker_queues = Vec::with_capacity(worker_cores.len());
    let mut workers = Vec::with_capacity(worker_cores.len());

    // Initialize Workers
    for &core_id in &worker_cores {
        let queue = Arc::new(SpscRingBuffer::<PixelWrite>::new());
        worker_queues.push(queue.clone());
        workers.push((WorkerCore::new(queue, 8080), core_id));
    }

    let master = MasterCore::new(worker_queues, canvas.clone());
    let broadcast = BroadcastCore::new();

    // Spawn Threads
    let mut handles = Vec::new();

    // 1. Spawn Workers
    for (mut worker, core_id) in workers {
        handles.push(std::thread::spawn(move || {
            worker.run(core_id);
        }));
    }

    // 2. Spawn Broadcast
    let canvas_for_broadcast = canvas.clone();
    handles.push(std::thread::spawn(move || {
        broadcast.run(broadcast_core_id, canvas_for_broadcast);
    }));

    // 3. Run Master on main thread
    println!("Starting Master loop on core {}...", master_core_id);
    master.run(master_core_id);

    // Join threads (Master loop is infinite, so this part is technically unreachable)
    for handle in handles {
        let _ = handle.join();
    }
}
