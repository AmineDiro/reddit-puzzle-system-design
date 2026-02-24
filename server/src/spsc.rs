use crate::const_settings::SPSC_CAPACITY;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

// https://docs.rs/crossbeam-utils/latest/src/crossbeam_utils/cache_padded.rs.html#148-150
#[repr(align(64))]
struct CachePadded<T>(T);

pub struct SpscRingBuffer<T> {
    tail: CachePadded<AtomicUsize>, // Written by Worker
    head: CachePadded<AtomicUsize>, // Written by Master
    buffer: [UnsafeCell<MaybeUninit<T>>; SPSC_CAPACITY],
}

// Ensure the struct is safely sendable and shareable based on `T` properties
unsafe impl<T: Send> Send for SpscRingBuffer<T> {}
unsafe impl<T: Send> Sync for SpscRingBuffer<T> {}

impl<T> SpscRingBuffer<T> {
    pub fn new() -> Self {
        let buffer = unsafe {
            MaybeUninit::<[UnsafeCell<MaybeUninit<T>>; SPSC_CAPACITY]>::uninit().assume_init()
        };

        Self {
            tail: CachePadded(AtomicUsize::new(0)),
            head: CachePadded(AtomicUsize::new(0)),
            buffer,
        }
    }

    #[inline(always)]
    pub fn push(&self, value: T) -> Result<(), T> {
        let current_tail = self.tail.0.load(Ordering::Relaxed);
        let next_tail = current_tail.wrapping_add(1);

        // Strict boundary check. We use wrapping arithmetic correctly.
        if current_tail.wrapping_sub(self.head.0.load(Ordering::Acquire)) >= SPSC_CAPACITY {
            // Buffer is full
            return Err(value);
        }

        let index = current_tail & (SPSC_CAPACITY - 1); // power of two modulo

        unsafe {
            (*self.buffer[index].get()).write(value);
        }

        // Matches the Acquire load in pop
        self.tail.0.store(next_tail, Ordering::Release);
        Ok(())
    }

    #[inline(always)]
    pub fn pop(&self) -> Option<T> {
        let current_head = self.head.0.load(Ordering::Relaxed);
        if current_head == self.tail.0.load(Ordering::Acquire) {
            // Buffer is empty
            return None;
        }

        let index = current_head & (SPSC_CAPACITY - 1);
        let value = unsafe { (*self.buffer[index].get()).assume_init_read() };

        // Matches the Release store of Head in push
        self.head
            .0
            .store(current_head.wrapping_add(1), Ordering::Release);

        Some(value)
    }
}

impl<T> Default for SpscRingBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spsc_ring_buffer() {
        let buffer = SpscRingBuffer::<usize>::new();
        assert_eq!(buffer.pop(), None);
        assert!(buffer.push(42).is_ok());
        assert_eq!(buffer.pop(), Some(42));
        assert_eq!(buffer.pop(), None);
    }

    #[test]
    fn test_spsc_ring_buffer_full() {
        let buffer = SpscRingBuffer::<usize>::new();
        for i in 0..SPSC_CAPACITY {
            assert!(buffer.push(i).is_ok());
        }
        assert!(buffer.push(SPSC_CAPACITY).is_err());
        assert_eq!(buffer.pop(), Some(0));
        assert!(buffer.push(SPSC_CAPACITY).is_ok());
        assert!(buffer.push(SPSC_CAPACITY + 1).is_err());

        for i in 1..=SPSC_CAPACITY {
            assert_eq!(buffer.pop(), Some(i));
        }
        assert_eq!(buffer.pop(), None);
    }
}
