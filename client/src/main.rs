// Dependencies needed in Cargo.toml:
// tokio = { version = "1.32", features = ["full"] }
// quinn = "0.10.2"
// rand = "0.8"
// clap = { version = "4.4", features = ["derive"] }

use bytes::Bytes;
use clap::Parser;
use quinn::Endpoint;
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

mod metrics;
mod tls;

#[derive(Parser, Debug, Clone)]
struct Args {
    #[arg(long)]
    target: String,
    #[arg(long)]
    clients: usize,
    #[arg(long)]
    id: String,
    #[arg(long, default_value_t = 10000)]
    max_conn_jitter: u64,
    #[arg(long, default_value_t = 1000)]
    min_pixel_wait: u64,
    #[arg(long, default_value_t = 10000)]
    max_pixel_wait: u64,
}

pub fn rle_decompress(src: &[u8], dst: &mut [u8]) -> usize {
    let mut src_idx = 0;
    let mut dst_idx = 0;
    while src_idx + 1 < src.len() {
        let count = src[src_idx] as usize;
        let color = src[src_idx + 1];
        src_idx += 2;
        for _ in 0..count {
            if dst_idx < dst.len() {
                dst[dst_idx] = color;
                dst_idx += 1;
            }
        }
    }
    dst_idx
}

async fn simulate_user(endpoint: Endpoint, metrics: Arc<metrics::LoadMetrics>, args: Args) {
    let target_cleaned = args.target.replace("https://", "").replace("http://", "");
    let addr = target_cleaned
        .parse::<std::net::SocketAddr>()
        .expect("Invalid target format");

    #[cfg(feature = "debug-logs")]
    println!("Client {} connecting to {}...", metrics.id, addr);

    let conn: quinn::Connection = match endpoint.connect(addr, "localhost") {
        Ok(connecting) => match connecting.await {
            Ok(c) => {
                #[cfg(feature = "debug-logs")]
                println!("Client {} connected successfully!", metrics.id);
                metrics.active.add(1);
                c
            }
            Err(e) => {
                #[cfg(feature = "debug-logs")]
                println!("Client {} failed to connect: {:?}", metrics.id, e);
                metrics.failed.add(1);
                return;
            }
        },
        Err(e) => {
            #[cfg(feature = "debug-logs")]
            println!("Client {} endpoint connect error: {:?}", metrics.id, e);
            metrics.failed.add(1);
            return;
        }
    };

    // Spawn RX task
    let conn_rx = conn.clone();
    let metrics_rx = metrics.clone();
    tokio::spawn(async move {
        while let Ok(dgram) = conn_rx.read_datagram().await {
            metrics_rx.rx_datagrams.add(1);
            metrics_rx.rx_bytes.add(dgram.len());
        }
        metrics_rx.active.add(usize::MAX); // Subtract 1 (wrapping) to indicate disconnection
    });

    // TX Loop
    let mut payload = [0u8; 5];
    payload[0..2].copy_from_slice(&100u16.to_ne_bytes());
    payload[2..4].copy_from_slice(&200u16.to_ne_bytes());
    payload[4] = 255;

    // Pre-build payload as Bytes for zero-copy cloning
    let payload_bytes = Bytes::copy_from_slice(&payload);

    loop {
        let pixel_wait = if args.min_pixel_wait >= args.max_pixel_wait {
            args.min_pixel_wait
        } else {
            rand::thread_rng().gen_range(args.min_pixel_wait..args.max_pixel_wait)
        };

        if pixel_wait > 0 {
            sleep(Duration::from_millis(pixel_wait)).await;
        } else {
            // Yield to allow other tasks to run if we are in a tight loop
            tokio::task::yield_now().await;
        }

        if conn.send_datagram(payload_bytes.clone()).is_ok() {
            metrics.tx_pixels.add(1);
        } else {
            // If the connection is closed, exit the TX loop
            break;
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let config = tls::build_optimized_config();

    let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(config);

    let metrics = metrics::LoadMetrics::new(args.id.clone());

    metrics::spawn_csv_exporter(metrics.clone(), args.id.clone());

    println!(
        "Starting worker {} ramping up {} clients...",
        args.id, args.clients
    );

    for _ in 0..args.clients {
        let ep = endpoint.clone();
        let m = metrics.clone();
        let a = args.clone();

        tokio::spawn(async move {
            let jitter = if a.max_conn_jitter == 0 {
                0
            } else {
                rand::thread_rng().gen_range(0..a.max_conn_jitter)
            };
            sleep(Duration::from_millis(jitter)).await;
            simulate_user(ep, m, a).await;
        });
    }

    std::future::pending::<()>().await;
}
