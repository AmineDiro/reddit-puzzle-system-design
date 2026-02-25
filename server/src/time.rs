use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct AtomicTime {
    time_ms: AtomicU64,
}

// SAFETY: AtomicU64 is Send + Sync, so AtomicTime is too.
unsafe impl Sync for AtomicTime {}

pub static CLOCK: AtomicTime = AtomicTime {
    time_ms: AtomicU64::new(0),
};

impl AtomicTime {
    /// Call once at startup to set the initial time and spawn the background updater thread.
    pub fn init(&self) {
        self.time_ms.store(now_ms_system(), Ordering::Relaxed);

        thread::spawn(|| {
            loop {
                // 1ms sleep is perfectly fine to avoid VDSO hit on main worker loops
                thread::sleep(std::time::Duration::from_millis(1));
                CLOCK.time_ms.store(now_ms_system(), Ordering::Relaxed);
            }
        });
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

fn now_ms_system() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
