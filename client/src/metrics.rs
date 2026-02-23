use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, sleep};

#[repr(align(64))]
pub struct AlignedAtomic(AtomicUsize);

impl AlignedAtomic {
    pub const fn new(val: usize) -> Self {
        Self(AtomicUsize::new(val))
    }
    #[inline(always)]
    pub fn add(&self, val: usize) {
        self.0.fetch_add(val, Ordering::Relaxed);
    }
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct LoadMetrics {
    pub id: String,
    pub active: AlignedAtomic,
    pub failed: AlignedAtomic,
    pub tx_pixels: AlignedAtomic,
    pub rx_datagrams: AlignedAtomic,
    pub rx_bytes: AlignedAtomic,
}

impl LoadMetrics {
    pub fn new(id: String) -> Arc<Self> {
        Arc::new(Self {
            id,
            active: AlignedAtomic::new(0),
            failed: AlignedAtomic::new(0),
            tx_pixels: AlignedAtomic::new(0),
            rx_datagrams: AlignedAtomic::new(0),
            rx_bytes: AlignedAtomic::new(0),
        })
    }
}

pub fn spawn_csv_exporter(metrics: Arc<LoadMetrics>, worker_id: String) {
    tokio::spawn(async move {
        // We will just create local metrics so docker can map it properly, or if we run locally
        let path = format!("/metrics/{}_data.csv", worker_id);
        // Fallback for non-docker runs to `./test_results/` might be nice, but to match blueprint, we stick to /metrics
        let file_res = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .await;

        let mut file = match file_res {
            Ok(f) => Some(f),
            Err(_e) => {
                // Fallback for local testing maybe?
                let fallback = format!("{}_data.csv", worker_id);
                if let Ok(f) = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&fallback)
                    .await
                {
                    Some(f)
                } else {
                    eprintln!(
                        "Could not open metrics file at {} or fallback {}, ignoring metrics reporting.",
                        path, fallback
                    );
                    None
                }
            }
        };

        if let Some(ref mut f) = file {
            let _ = f
                .write_all(b"timestamp,active,failed,tx_pixels,rx_dgram_s,rx_mbps\n")
                .await;
        }

        let (mut last_dgrams, mut last_bytes) = (0, 0);

        loop {
            sleep(Duration::from_secs(1)).await;
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let current_dgrams = metrics.rx_datagrams.get();
            let current_bytes = metrics.rx_bytes.get();

            let dps = current_dgrams - last_dgrams;
            let mbps = ((current_bytes - last_bytes) as f64 * 8.0) / 1_000_000.0;

            let row = format!(
                "{},{},{},{},{},{:.3}\n",
                ts,
                metrics.active.get(),
                metrics.failed.get(),
                metrics.tx_pixels.get(),
                dps,
                mbps
            );

            if let Some(ref mut f) = file {
                let _ = f.write_all(row.as_bytes()).await;
            }

            last_dgrams = current_dgrams;
            last_bytes = current_bytes;
        }
    });
}
