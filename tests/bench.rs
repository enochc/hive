//! Loopback throughput and latency for property updates (todo.md 3.6).
//! Ignored by default; run in release with output:
//!
//!   cargo test --release --test bench -- --ignored --nocapture --test-threads=1
//!
//! Numbers go in PERF_NOTES.md. They're for before/after comparisons on
//! the same machine, not absolute claims.

use hive::handler::Handler;
use hive::hive::Hive;
use hive::CancellationToken;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn wait_for<F: Fn() -> bool>(f: F, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::task::yield_now().await;
    }
    f()
}

struct Net {
    server: Handler,
    sender: Handler,
    receivers: Vec<Arc<AtomicI64>>,
    _cancel: CancellationToken,
}

/// Extra config for the server and for every client: plain TCP by
/// default, TLS (and optionally auth) for the security benchmarks.
#[derive(Clone, Default)]
struct Sec {
    server: String,
    client: String,
}

/// A server plus `n_receivers` clients watching `m`, plus one more client
/// that does the sending.
async fn net(port: u16, n_receivers: usize, sec: &Sec) -> Net {
    let cancel = CancellationToken::new();
    let s = Hive::new_from_str_unknown(&format!(
        "listen = \"127.0.0.1:{port}\"\nname = \"Serv\"\n{}\n[Properties]\nm = 0",
        sec.server
    ));
    let server = s.go(true, cancel.clone()).await;

    let mut receivers = Vec::new();
    for i in 0..n_receivers {
        let mut c = Hive::new_from_str_unknown(&format!(
            "connect = \"127.0.0.1:{port}\"\nname = \"r{i}\"\n{}",
            sec.client
        ));
        let last = Arc::new(AtomicI64::new(-1));
        let l = last.clone();
        c.get_mut_property_by_name("m").unwrap().on_next(move |v| {
            l.store(v.val.as_integer().unwrap(), Ordering::Relaxed);
        });
        c.go(false, cancel.clone()).await;
        receivers.push(last);
    }
    let c = Hive::new_from_str_unknown(&format!(
        "connect = \"127.0.0.1:{port}\"\nname = \"sender\"\n{}",
        sec.client
    ));
    let sender = c.go(false, cancel.clone()).await;
    assert!(
        wait_for(|| server.peer_count() == n_receivers + 1, Duration::from_secs(5)).await,
        "clients never attached"
    );
    Net { server, sender, receivers, _cancel: cancel }
}

async fn throughput(label: &str, port: u16, n_receivers: usize, updates: i64, sec: &Sec) {
    let mut n = net(port, n_receivers, sec).await;
    let start = Instant::now();
    for v in 1..=updates {
        n.sender.set_property("m", Some(&v.into())).await;
    }
    let recv = n.receivers.clone();
    let ok = wait_for(
        || recv.iter().all(|r| r.load(Ordering::Relaxed) == updates),
        Duration::from_secs(60),
    )
    .await;
    let elapsed = start.elapsed();
    assert!(ok, "not every receiver saw the last update");
    println!(
        "{:<6} throughput  1 sender -> server -> {:>2} receivers: {:>7} updates in {:>8.1?}  = {:>9.0} updates/s",
        label,
        n_receivers,
        updates,
        elapsed,
        updates as f64 / elapsed.as_secs_f64()
    );
    let _ = n.server.peer_count();
}

async fn latency(label: &str, port: u16, n_receivers: usize, rounds: i64, sec: &Sec) {
    let mut n = net(port, n_receivers, sec).await;
    let r = n.receivers[0].clone();
    let mut samples = Vec::with_capacity(rounds as usize);
    for v in 1..=rounds {
        let t = Instant::now();
        n.sender.set_property("m", Some(&v.into())).await;
        assert!(wait_for(|| r.load(Ordering::Relaxed) == v, Duration::from_secs(5)).await);
        samples.push(t.elapsed());
    }
    samples.sort();
    let pct = |p: f64| samples[((samples.len() as f64 - 1.0) * p) as usize];
    println!(
        "{:<6} latency     sender -> server -> receiver ({:>2} receivers): p50 {:>8.1?}  p99 {:>8.1?}",
        label,
        n_receivers,
        pct(0.50),
        pct(0.99)
    );
}

/// Updates per throughput run. 50,000 finishes in tens of milliseconds;
/// set `HIVE_BENCH_UPDATES` higher for a steadier number or to profile.
fn updates() -> i64 {
    std::env::var("HIVE_BENCH_UPDATES").ok().and_then(|v| v.parse().ok()).unwrap_or(50_000)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_property_updates() {
    // cargo prints "test bench_property_updates ... " with no newline.
    println!();
    if std::env::var("HIVE_BENCH_LOG").is_ok() {
        hive::init_logging(Some(hive::LevelFilter::Warn));
    }
    let plain = Sec::default();
    throughput("plain", 34801, 1, updates(), &plain).await;
    throughput("plain", 34802, 8, updates(), &plain).await;
    latency("plain", 34803, 1, 2_000, &plain).await;
    latency("plain", 34804, 8, 2_000, &plain).await;
}

/// Same runs over TLS 1.3 (ring, AES-GCM on this CPU). Needs
/// `--features tls`.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_property_updates_tls() {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    println!();
    let dir = std::env::temp_dir().join(format!("hive-bench-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cap = CertificateParams::new(Vec::<String>::new()).unwrap();
    cap.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca = cap.self_signed(&ca_key).unwrap();
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["127.0.0.1".to_string()])
        .unwrap()
        .signed_by(&key, &ca, &ca_key)
        .unwrap();
    let (ca_p, cert_p, key_p) = (dir.join("ca.pem"), dir.join("s.pem"), dir.join("s.key"));
    std::fs::write(&ca_p, ca.pem()).unwrap();
    std::fs::write(&cert_p, leaf.pem()).unwrap();
    std::fs::write(&key_p, key.serialize_pem()).unwrap();

    let tls = Sec {
        server: format!("[TLS]\ncert = \"{}\"\nkey = \"{}\"", cert_p.display(), key_p.display()),
        client: format!("[TLS]\nca = \"{}\"", ca_p.display()),
    };
    throughput("tls", 34811, 1, updates(), &tls).await;
    throughput("tls", 34812, 8, updates(), &tls).await;
    latency("tls", 34813, 1, 2_000, &tls).await;
    latency("tls", 34814, 8, 2_000, &tls).await;
    let _ = std::fs::remove_dir_all(&dir);
}
