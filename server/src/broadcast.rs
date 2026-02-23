use crate::canvas::{ACTIVE_INDEX, BUFFER_POOL, CANVAS_SIZE};
use crate::master::CANVAS_SEQ;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub struct BroadcastCore {
    interval: Duration,
}

fn rle_compress(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }

    let mut compressed = Vec::with_capacity(CANVAS_SIZE / 4);
    let mut last_val = data[0];
    let mut count: u8 = 1;

    for &val in data.iter().skip(1) {
        if val == last_val && count < 255 {
            count += 1;
        } else {
            compressed.push(count);
            compressed.push(last_val);
            last_val = val;
            count = 1;
        }
    }
    compressed.push(count);
    compressed.push(last_val);
    compressed
}

impl BroadcastCore {
    pub fn new() -> Self {
        Self {
            interval: Duration::from_secs(5),
        }
    }

    pub fn run(self, core_id: usize, canvas: std::sync::Arc<crate::canvas::Canvas>) {
        if core_affinity::set_for_current(core_affinity::CoreId { id: core_id }) {
            // Successfully pinned to physical core
            println!("Broadcast core pinned to core {}", core_id);
        }

        loop {
            std::thread::sleep(self.interval);

            let mut seq = 0;
            loop {
                seq = CANVAS_SEQ.load(Ordering::Acquire);
                if seq & 1 != 0 {
                    std::hint::spin_loop();
                    continue; // Write in progress, spin and wait
                }

                // Perform the read/copy
                // Determine next buffer index
                let current_active = ACTIVE_INDEX.load(Ordering::Relaxed);

                let next_active = (current_active + 1) % 16;
                canvas.snapshot_to_pool(next_active);

                // Verify sequence hasn't changed during our copy
                let seq_end = CANVAS_SEQ.load(Ordering::Acquire);

                // NOTE: Success! No master writes occurred during our snapshot.
                if seq == seq_end {
                    let target_buffer_slice = unsafe { &BUFFER_POOL[next_active].data };

                    // Fast RLE approximation
                    let compressed = Self::rle_compress(target_buffer_slice);

                    // Optional Zstd compression around the RLE result
                    let mut final_compressed = Vec::with_capacity(compressed.len());
                    let _ = zstd::stream::copy_encode(&compressed[..], &mut final_compressed, 3);

                    // Swap the pointer for Zero-Allocation RCU semantics
                    // Workers doing Scatter-Gather I/O will instantly pick up the new index.
                    ACTIVE_INDEX.store(next_active, Ordering::Release);

                    break;
                }
            }
        }
    }
}
