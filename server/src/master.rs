use crate::canvas::Canvas;
use crate::const_settings::{BROADCAST_INTERVAL_MS, CANVAS_BUFFER_POOL_MASK, MASTER_BATCH_DRAIN};
use crate::spsc::SpscRingBuffer;
use crate::time::AtomicTime;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub static CANVAS_SEQ: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub struct PixelWrite {
    pub x: u16,
    pub y: u16,
    pub color: u8,
}

#[inline(always)]
pub fn rle_compress(src: &[u8], dst: &mut [u8]) -> usize {
    if src.is_empty() {
        return 0;
    }
    let mut src_idx = 0;
    let mut dst_idx = 0;
    let len = src.len();

    while src_idx < len {
        let color = src[src_idx];
        let mut count = 1;
        src_idx += 1;

        // SIMD optimization for x86_64
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                unsafe {
                    use std::arch::x86_64::*;
                    let color_vec = _mm256_set1_epi8(color as i8);
                    while src_idx + 32 <= len && count + 32 <= 255 {
                        let chunk = _mm256_loadu_si256(src.as_ptr().add(src_idx) as *const __m256i);
                        let mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(chunk, color_vec)) as u32;
                        if mask == 0xFFFFFFFF {
                            count += 32;
                            src_idx += 32;
                        } else {
                            let matching = (!mask).trailing_zeros() as usize;
                            count += matching;
                            src_idx += matching;
                            break;
                        }
                    }
                }
            } else if is_x86_feature_detected!("sse2") {
                unsafe {
                    use std::arch::x86_64::*;
                    let color_vec = _mm_set1_epi8(color as i8);
                    while src_idx + 16 <= len && count + 16 <= 255 {
                        let chunk = _mm_loadu_si128(src.as_ptr().add(src_idx) as *const __m128i);
                        let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(chunk, color_vec)) as u32;
                        if mask == 0xFFFF {
                            count += 16;
                            src_idx += 16;
                        } else {
                            let matching = (!mask).trailing_zeros() as usize;
                            count += matching;
                            src_idx += matching;
                            break;
                        }
                    }
                }
            }
        }

        while src_idx < len && src[src_idx] == color && count < 255 {
            count += 1;
            src_idx += 1;
        }

        dst[dst_idx] = count as u8;
        dst[dst_idx + 1] = color;
        dst_idx += 2;
    }
    dst_idx
}

pub struct MasterCore {
    workers: Vec<Arc<SpscRingBuffer<PixelWrite>>>,
    pub canvas: Arc<Canvas>,
    clock: Arc<AtomicTime>,
}

impl MasterCore {
    pub fn new(
        workers: Vec<Arc<SpscRingBuffer<PixelWrite>>>,
        canvas: Arc<Canvas>,
        clock: Arc<AtomicTime>,
    ) -> Self {
        Self {
            workers,
            canvas,
            clock,
        }
    }

    pub fn run(&self, core_id: usize) {
        // Pin to physical core using core_affinity
        if core_affinity::set_for_current(core_affinity::CoreId { id: core_id }) {
            // Successfully pinned
        }
        // Use AtomicTime for high-performance timing without syscall overhead
        let mut last_broadcast_time = self.clock.now_ms();
        let broadcast_threshold_ms = BROADCAST_INTERVAL_MS;

        loop {
            let seq = CANVAS_SEQ.load(Ordering::Relaxed);

            // Sequence lock write begin (make odd)
            CANVAS_SEQ.store(seq.wrapping_add(1), Ordering::Release);

            for worker_queue in &self.workers {
                // Batch drain to minimize lock duration effectively
                for _ in 0..MASTER_BATCH_DRAIN {
                    if let Some(pixel) = worker_queue.pop() {
                        self.canvas
                            .set_pixel(pixel.x as usize, pixel.y as usize, pixel.color);
                    } else {
                        break;
                    }
                }
            }

            // Sequence lock write end (make even)
            CANVAS_SEQ.store(seq.wrapping_add(2), Ordering::Release);

            let now = self.clock.now_ms();
            if now.wrapping_sub(last_broadcast_time) >= broadcast_threshold_ms {
                let current_active = crate::canvas::ACTIVE_INDEX.load(Ordering::Relaxed);
                let next_active = (current_active + 1) & CANVAS_BUFFER_POOL_MASK;

                self.canvas.snapshot_to_pool(next_active);

                // Compress the snapshot
                unsafe {
                    let src = &crate::canvas::BUFFER_POOL[next_active].data;
                    let dst = &mut crate::canvas::COMPRESSED_BUFFER_POOL[next_active].data;
                    let compressed_len = rle_compress(src, dst);
                    crate::canvas::COMPRESSED_LENS[next_active] = compressed_len;
                }

                crate::canvas::ACTIVE_INDEX.store(next_active, Ordering::Release);

                last_broadcast_time = now;
            }

            std::hint::spin_loop();
        }
    }
}
