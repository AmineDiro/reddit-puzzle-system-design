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
    #[cfg(feature = "debug-logs")]
    println!("Client {} connecting to {}...", metrics.id, target);

    let conn: wtransport::Connection = match endpoint.connect(target).await {
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
    };

    loop {
        tokio::select! {
            result = conn.receive_datagram() => {
                match result {
                    Ok(dgram) => {
                        #[cfg(feature = "debug-logs")]
                        println!("Client {} received datagram of {} bytes", metrics.id, dgram.payload().len());
                        metrics.rx_datagrams.add(1);
                        metrics.rx_bytes.add(dgram.payload().len());
                    }
                    Err(e) => {
                        #[cfg(feature = "debug-logs")]
                        println!("Client {} connection error: {:?}", metrics.id, e);
                        break;
                    }
                }
            }
            _ = sleep(Duration::from_secs(rand::thread_rng().gen_range(1..10))) => {
                let payload = b"x:100,y:200,color:FFF";
                #[cfg(feature = "debug-logs")]
                println!("Client {} sending pixel datagram...", metrics.id);
                if conn.send_datagram(payload).is_ok() {
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
            // Stagger handshakes over 120 seconds to prevent thermal throttling
            let jitter = rand::thread_rng().gen_range(0..120_000);
            sleep(Duration::from_millis(jitter)).await;
            simulate_user(ep, target, m).await;
        });
    }

    std::future::pending::<()>().await;
}
