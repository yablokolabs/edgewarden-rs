//! School start-time connection storm generator.
//!
//! At T=0 launches N simultaneous clients against the proxy, each doing
//! M request/response roundtrips of P bytes, then reports connections/sec,
//! throughput, p50/p95/p99 latency, errors, and local RSS. Results are
//! written as machine-readable JSON.

use anyhow::Context;
use clap::Parser;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Parser)]
#[command(name = "storm", about = "EdgeWarden connection-storm load generator")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:18080")]
    target: SocketAddr,
    #[arg(long, default_value_t = 200)]
    clients: usize,
    #[arg(long, default_value_t = 5)]
    rounds: usize,
    #[arg(long, default_value_t = 256)]
    payload_bytes: usize,
    #[arg(long, default_value = "storm-results.json")]
    out: String,
}

#[derive(Debug, Serialize)]
struct Results {
    clients: usize,
    rounds: usize,
    payload_bytes: usize,
    total_requests: u64,
    errors: u64,
    elapsed_secs: f64,
    connections_per_sec: f64,
    throughput_mbps: f64,
    latency_ms_p50: f64,
    latency_ms_p95: f64,
    latency_ms_p99: f64,
    rss_bytes: u64,
}

fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|c| {
            c.lines().find_map(|l| {
                l.strip_prefix("VmRSS:")
                    .and_then(|r| r.split_whitespace().next()?.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
            })
        })
        .unwrap_or(0)
}

fn percentile(sorted: &mut [u128], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_unstable();
    let idx = ((p * sorted.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[idx] as f64 / 1_000_000.0
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let args = Args::parse();
    let payload = vec![0xABu8; args.payload_bytes];
    let errors = Arc::new(AtomicU64::new(0));
    let mut latencies: Vec<u128> = Vec::with_capacity(args.clients * args.rounds);

    let start = Instant::now();
    // T=0 thundering herd: all clients spawn simultaneously.
    let mut handles = Vec::with_capacity(args.clients);
    for _ in 0..args.clients {
        let payload = payload.clone();
        let target = args.target;
        let rounds = args.rounds;
        let errors = errors.clone();
        handles.push(tokio::spawn(async move {
            let mut mine = Vec::with_capacity(rounds);
            let mut stream = match TcpStream::connect(target).await {
                Ok(s) => s,
                Err(_) => {
                    errors.fetch_add(rounds as u64, Ordering::Relaxed);
                    return mine;
                }
            };
            let mut buf = vec![0u8; payload.len()];
            for _ in 0..rounds {
                let t0 = Instant::now();
                if stream.write_all(&payload).await.is_err() {
                    errors.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                if tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
                    .await
                    .is_err()
                {
                    errors.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                mine.push(t0.elapsed().as_nanos());
            }
            mine
        }));
    }
    for h in handles {
        let mine = h.await.context("client task")?;
        latencies.extend(mine);
    }
    let elapsed = start.elapsed();
    let total = latencies.len() as u64;
    let errs = errors.load(Ordering::Relaxed);
    let p50 = percentile(&mut latencies.clone(), 0.50);
    let p95 = percentile(&mut latencies.clone(), 0.95);
    let p99 = percentile(&mut latencies.clone(), 0.99);
    let total_bytes = total * args.payload_bytes as u64 * 2; // req + resp
    let results = Results {
        clients: args.clients,
        rounds: args.rounds,
        payload_bytes: args.payload_bytes,
        total_requests: total,
        errors: errs,
        elapsed_secs: elapsed.as_secs_f64(),
        connections_per_sec: args.clients as f64 / elapsed.as_secs_f64().max(1e-9),
        throughput_mbps: (total_bytes as f64 * 8.0) / elapsed.as_secs_f64().max(1e-9) / 1e6,
        latency_ms_p50: p50,
        latency_ms_p95: p95,
        latency_ms_p99: p99,
        rss_bytes: rss_bytes(),
    };
    let json = serde_json::to_string_pretty(&results)?;
    println!("{json}");
    std::fs::write(&args.out, &json)?;
    eprintln!("wrote {}", args.out);
    Ok(())
}
