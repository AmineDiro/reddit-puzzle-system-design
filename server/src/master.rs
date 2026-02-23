use crate::canvas::Canvas;
use crate::spsc::SpscRingBuffer;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub static CANVAS_SEQ: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub struct PixelWrite {
    pub x: u16,
    pub y: u16,
    pub color: u8,
}

pub struct MasterCore {
    workers: Vec<Arc<SpscRingBuffer<PixelWrite>>>,
    pub canvas: Arc<Canvas>,
}

impl MasterCore {
    pub fn new(workers: Vec<Arc<SpscRingBuffer<PixelWrite>>>, canvas: Arc<Canvas>) -> Self {
        Self { workers, canvas }
    }

    pub fn run(&self, core_id: usize) {
        // Pin to physical core using core_affinity
        if core_affinity::set_for_current(core_affinity::CoreId { id: core_id }) {
            // Successfully pinned
        }

        let mut ticks = 0u64;
        let mut last_broadcast = std::time::Instant::now();
        let broadcast_interval = std::time::Duration::from_millis(50);

        loop {
            let seq = CANVAS_SEQ.load(Ordering::Relaxed);

            // Sequence lock write begin (make odd)
            CANVAS_SEQ.store(seq.wrapping_add(1), Ordering::Release);

            for worker_queue in &self.workers {
                // Batch drain to minimize lock duration effectively
                for _ in 0..128 {
                    if let Some(pixel) = worker_queue.pop() {
                        self.canvas
                            .set_pixel(pixel.x as usize, pixel.y as usize, pixel.color);
                    } else {
                        break;
                    }
                }
            }

            // Sequence lock write end (make even)
            CANVAS_SEQ.store(seq.wrapping_add(1), Ordering::Release);

            ticks = ticks.wrapping_add(1);
            // if ticks & 2047 == 0 {
            //     let now = std::time::Instant::now();
            //     if now.duration_since(last_broadcast) >= broadcast_interval {
            //         let current_active = crate::canvas::ACTIVE_INDEX.load(Ordering::Relaxed);
            //         let next_active = (current_active + 1) & 15;

            //         self.canvas.snapshot_to_pool(next_active);
            //         crate::canvas::ACTIVE_INDEX.store(next_active, Ordering::Release);

            //         last_broadcast = now;
            //     }
            // }

            std::hint::spin_loop();
        }
    }
}
