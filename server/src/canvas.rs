use crate::const_settings::{CANVAS_BUFFER_POOL_SIZE, CANVAS_HEIGHT, CANVAS_SIZE, CANVAS_WIDTH};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Copy)]
pub struct CanvasBuffer {
    pub data: [u8; CANVAS_SIZE],
}

impl CanvasBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; CANVAS_SIZE],
        }
    }
}

// Pre-allocated statically to avoid allocations later on. Lives in .bss segment.
pub static mut BUFFER_POOL: [CanvasBuffer; CANVAS_BUFFER_POOL_SIZE] =
    [CanvasBuffer::new(); CANVAS_BUFFER_POOL_SIZE];

// Compressed buffers can be up to 2x the original size in worst case RLE
#[derive(Clone, Copy)]
pub struct CompressedBuffer {
    pub data: [u8; CANVAS_SIZE * 2],
}

impl CompressedBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; CANVAS_SIZE * 2],
        }
    }
}

pub static mut COMPRESSED_BUFFER_POOL: [CompressedBuffer; CANVAS_BUFFER_POOL_SIZE] =
    [CompressedBuffer::new(); CANVAS_BUFFER_POOL_SIZE];
pub static mut COMPRESSED_LENS: [usize; CANVAS_BUFFER_POOL_SIZE] = [0; CANVAS_BUFFER_POOL_SIZE];

// The currently active buffer index that workers read from.
// RCU like without atomic pointers, just offsets of fixed size array
pub static ACTIVE_INDEX: AtomicUsize = AtomicUsize::new(0);

pub struct Canvas {
    pub pixels: Box<[u8; CANVAS_SIZE]>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    pub fn new() -> Self {
        Self {
            pixels: vec![0; CANVAS_SIZE].into_boxed_slice().try_into().unwrap(),
        }
    }

    #[inline(always)]
    pub fn set_pixel(&self, x: usize, y: usize, color: u8) {
        if x < CANVAS_WIDTH && y < CANVAS_HEIGHT {
            let index = y * CANVAS_WIDTH + x;
            unsafe {
                let pixels_ptr = self.pixels.as_ptr() as *mut u8;
                *pixels_ptr.add(index) = color;
            }
        }
    }

    pub fn snapshot_to_pool(&self, target_index: usize) {
        unsafe {
            let src = self.pixels.as_ptr();
            let dst = BUFFER_POOL[target_index].data.as_mut_ptr();
            std::ptr::copy_nonoverlapping(src, dst, CANVAS_SIZE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canvas_snapshot() {
        let canvas = Canvas::new();
        canvas.set_pixel(10, 10, 255);

        canvas.snapshot_to_pool(1);

        unsafe {
            let buffer = &BUFFER_POOL[1];
            assert_eq!(buffer.data[10 * CANVAS_WIDTH + 10], 255);
            assert_eq!(buffer.data[0], 0); // other pixels are unaffected
        }
    }
}
