// Dependencies needed in Cargo.toml:
// tokio = { version = "1.32", features = ["full"] }
// quinn = "0.10.2"
// rand = "0.8"
// clap = { version = "4.4", features = ["derive"] }

use clap::Parser;
use quinn::Endpoint;
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

mod metrics;
mod tls;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    target: String,
    #[arg(long)]
    clients: usize,
    #[arg(long)]
    id: String,
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

async fn simulate_user(endpoint: Endpoint, target: String, metrics: Arc<metrics::LoadMetrics>) {
    let target_cleaned = target.replace("https://", "").replace("http://", "");
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

    let mut canvas = vec![0u8; 1_000_000];

    loop {
        tokio::select! {
            result = conn.read_datagram() => {
                match result {
                    Ok(dgram) => {
                        #[cfg(feature = "debug-logs")]
                        println!("Client {} received datagram of {} bytes", metrics.id, dgram.len());
                        metrics.rx_datagrams.add(1);
                        metrics.rx_bytes.add(dgram.len());

                        // In a real scenario, you'd reassemble these chunks before decompressing.
                        // Here we just show the decompression function works.
                        // rle_decompress(&dgram, &mut canvas);
                    }
                    Err(e) => {
                        #[cfg(feature = "debug-logs")]
                        println!("Client {} connection error: {:?}", metrics.id, e);
                        break;
                    }
                }
            }
            _ = sleep(Duration::from_secs(rand::thread_rng().gen_range(1..10))) => {
                let mut payload = [0u8; 5];
                payload[0..2].copy_from_slice(&100u16.to_ne_bytes());
                payload[2..4].copy_from_slice(&200u16.to_ne_bytes());
                payload[4] = 255;

                #[cfg(feature = "debug-logs")]
                println!("Client {} sending pixel datagram...", metrics.id);
                if conn.send_datagram(payload.to_vec().into()).is_ok() {
                    #[cfg(feature = "debug-logs")]
                    println!("Client {} pixel datagram sent successfully!", metrics.id);
                    metrics.tx_pixels.add(1);
                }
            }
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
        let target = args.target.clone();
        let m = metrics.clone();

        tokio::spawn(async move {
            let jitter = rand::thread_rng().gen_range(0..10_000);
            sleep(Duration::from_millis(jitter)).await;
            simulate_user(ep, target, m).await;
        });
    }

    std::future::pending::<()>().await;
}
