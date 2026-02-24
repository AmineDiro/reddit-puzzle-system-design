use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct AtomicTime {
    time_ms: AtomicU64,
}

impl AtomicTime {
    pub fn new() -> Arc<Self> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let clock = Arc::new(Self {
            time_ms: AtomicU64::new(now_ms),
        });

        let clock_clone = clock.clone();
        thread::spawn(move || {
            loop {
                // core spin waiting or use advanced timing, but 1ms sleep is
                // perfectly fine OK to avoid VDSO hit on your main worker loops
                thread::sleep(std::time::Duration::from_millis(1));

                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;

                clock_clone.time_ms.store(now_ms, Ordering::Relaxed);
            }
        });

        clock
    }

    #[inline(always)]
    pub fn now_ms(&self) -> u64 {
        self.time_ms.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn now_sec(&self) -> u64 {
        self.now_ms() / 1000
    }
}
