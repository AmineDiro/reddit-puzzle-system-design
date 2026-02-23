// Dependencies needed in Cargo.toml:
// tokio = { version = "1.32", features = ["full"] }
// wtransport = "0.1"
// rand = "0.8"
// clap = { version = "4.4", features = ["derive"] }

use clap::Parser;
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use wtransport::Endpoint;

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

async fn simulate_user(
    endpoint: Arc<Endpoint<wtransport::endpoint::endpoint_side::Client>>,
    target: String,
    metrics: Arc<metrics::LoadMetrics>,
) {
    let conn: wtransport::Connection = match endpoint.connect(target).await {
        Ok(c) => {
            metrics.active.add(1);
            c
        }
        Err(_) => {
            metrics.failed.add(1);
            return;
        }
    };

    loop {
        tokio::select! {
            Ok(dgram) = conn.receive_datagram() => {
                metrics.rx_datagrams.add(1);
                metrics.rx_bytes.add(dgram.payload().len());
            }
            _ = sleep(Duration::from_secs(rand::thread_rng().gen_range(280..320))) => {
                if conn.send_datagram(b"x:100,y:200,color:FFF").is_ok() {
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
    let endpoint = Arc::new(Endpoint::client(config).unwrap());
    let metrics = metrics::LoadMetrics::new();

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
            // Stagger handshakes over 120 seconds to prevent thermal throttling
            let jitter = rand::thread_rng().gen_range(0..120_000);
            sleep(Duration::from_millis(jitter)).await;
            simulate_user(ep, target, m).await;
        });
    }

    std::future::pending::<()>().await;
}
