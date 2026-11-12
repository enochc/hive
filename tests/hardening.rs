//! Phase 1 hardening: a misbehaving or slow remote must never take down or
//! stall the hive for everyone else. Uses a raw TCP client that speaks the
//! Hive handshake by hand so it can send exactly the bytes a test needs.

use hive::hive::Hive;
use hive::property::Property;
use hive::CancellationToken;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PROPERTY: u8 = 0x11;
const IS_LONG: u8 = 0x15; // i32 value

async fn wait_for<F: Fn() -> bool>(f: F, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f()
}

/// Connect and complete the client side of the Hive handshake.
async fn raw_client(port: u16) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"HVEP\n\x78raw\n").await.unwrap();
    s
}

fn property_frame(name: &str, value: i32) -> Vec<u8> {
    let mut body = vec![PROPERTY];
    body.extend_from_slice(&Property::hash_id(name).to_be_bytes());
    body.push(IS_LONG);
    body.extend_from_slice(&value.to_be_bytes());
    frame(&body)
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

/// A server whose `m` property reports every change into `last`/`count`.
async fn server(port: u16) -> (hive::handler::Handler, Arc<AtomicI64>, Arc<AtomicUsize>) {
    let mut h = Hive::new_from_str_unknown(&format!(
        "listen = \"127.0.0.1:{port}\"\nname = \"Serv\"\nhandshake_timeout_ms = 2000\n[Properties]\nm = 0"
    ));
    let last = Arc::new(AtomicI64::new(-1));
    let count = Arc::new(AtomicUsize::new(0));
    let (l, c) = (last.clone(), count.clone());
    h.get_mut_property_by_name("m").unwrap().on_next(move |v| {
        l.store(v.val.as_integer().unwrap(), Ordering::SeqCst);
        c.fetch_add(1, Ordering::SeqCst);
    });
    let handler = h.go(true, CancellationToken::new()).await;
    (handler, last, count)
}

/// A hive client whose `m` property reports the latest value into `last`.
async fn hive_client(port: u16, name: &str) -> (hive::handler::Handler, Arc<AtomicI64>) {
    let mut c = Hive::new_from_str_unknown(&format!("connect = \"127.0.0.1:{port}\"\nname = \"{name}\""));
    let last = Arc::new(AtomicI64::new(-1));
    let l = last.clone();
    c.get_mut_property_by_name("m").unwrap().on_next(move |v| {
        l.store(v.val.as_integer().unwrap(), Ordering::SeqCst);
    });
    let h = c.go(false, CancellationToken::new()).await;
    (h, last)
}

#[tokio::test(flavor = "multi_thread")]
async fn frames_split_one_byte_at_a_time_and_bursts_arrive_intact() {
    let (_srv, last, count) = server(34701).await;
    let mut s = raw_client(34701).await;

    // One frame dribbled a byte at a time, so the length prefix is split
    // across several reads.
    for b in property_frame("m", 7) {
        s.write_all(&[b]).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(wait_for(|| last.load(Ordering::SeqCst) == 7, Duration::from_secs(2)).await);

    // A burst written as one buffer, so frame boundaries land anywhere
    // inside the server's read buffer.
    let mut burst = Vec::new();
    for v in 1..=5000 {
        burst.extend(property_frame("m", v));
    }
    let before = count.load(Ordering::SeqCst);
    s.write_all(&burst).await.unwrap();
    assert!(
        wait_for(|| last.load(Ordering::SeqCst) == 5000, Duration::from_secs(5)).await,
        "last value seen: {}",
        last.load(Ordering::SeqCst)
    );
    assert_eq!(count.load(Ordering::SeqCst) - before, 5000, "every frame should be decoded once");
}

#[tokio::test(flavor = "multi_thread")]
async fn silent_connection_does_not_block_other_clients() {
    let (srv, _, _) = server(34702).await;

    // Connects and never says a word.
    let _silent = TcpStream::connect("127.0.0.1:34702").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (_c, _) = hive_client(34702, "real").await;
    assert!(
        wait_for(|| srv.peer_count() == 1, Duration::from_secs(1)).await,
        "a real client should attach while the silent one is still pending"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_messages_drop_only_the_sender() {
    let (mut srv, _, _) = server(34703).await;
    let (_c, client_last) = hive_client(34703, "real").await;
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await);

    let mut bad = raw_client(34703).await;
    assert!(wait_for(|| srv.peer_count() == 2, Duration::from_secs(2)).await);

    // Unknown message type: ignored, connection stays up.
    bad.write_all(&frame(&[0x7f, 1, 2, 3])).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(srv.peer_count(), 2, "unknown type should be ignored, not fatal");

    // Truncated property: the sender gets dropped.
    bad.write_all(&frame(&[PROPERTY, 0, 0, 0])).await.unwrap();
    assert!(
        wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await,
        "malformed sender should be dropped"
    );

    // Its socket is actually closed.
    let mut sink = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(2), bad.read_to_end(&mut sink)).await;
    assert!(closed.is_ok(), "server should close the malformed peer's socket");

    // And the hive still serves everyone else.
    srv.set_property("m", Some(&42.into())).await;
    assert!(wait_for(|| client_last.load(Ordering::SeqCst) == 42, Duration::from_secs(2)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dead_peer_does_not_stop_broadcasts_to_others() {
    let (mut srv, _, _) = server(34704).await;
    let (_c, client_last) = hive_client(34704, "real").await;
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await);

    let dead = raw_client(34704).await;
    assert!(wait_for(|| srv.peer_count() == 2, Duration::from_secs(2)).await);
    // Reset rather than close, so the server's next writes to it fail.
    #[allow(deprecated)]
    dead.set_linger(Some(Duration::ZERO)).unwrap();
    drop(dead);

    for v in 100..200 {
        srv.set_property("m", Some(&v.into())).await;
    }
    assert!(
        wait_for(|| client_last.load(Ordering::SeqCst) == 199, Duration::from_secs(3)).await,
        "surviving client stopped receiving, last = {}",
        client_last.load(Ordering::SeqCst)
    );
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn long_strings_sync_between_hives() {
    let mut s = Hive::new_from_str_unknown("listen = \"127.0.0.1:34705\"\nname = \"Serv\"\n[Properties]\nt = \"x\"");
    let got = Arc::new(std::sync::Mutex::new(String::new()));
    let g = got.clone();
    s.get_mut_property_by_name("t").unwrap().on_next(move |v| {
        *g.lock().unwrap() = v.val.as_str().unwrap().to_string();
    });
    let srv = s.go(true, CancellationToken::new()).await;

    let (mut c, _) = hive_client(34705, "real").await;
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await);

    let long = "y".repeat(10_000);
    c.set_property("t", Some(&long.as_str().into())).await;
    assert!(wait_for(|| got.lock().unwrap().len() == 10_000, Duration::from_secs(2)).await);
    assert_eq!(*got.lock().unwrap(), long);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_stops_reading_is_dropped_and_others_keep_up() {
    let mut s = Hive::new_from_str_unknown("listen = \"127.0.0.1:34706\"\nname = \"Serv\"\n[Properties]\nt = \"x\"");
    let _ = s.get_mut_property_by_name("t");
    let mut srv = s.go(true, CancellationToken::new()).await;

    let mut c = Hive::new_from_str_unknown("connect = \"127.0.0.1:34706\"\nname = \"real\"");
    let got = Arc::new(std::sync::Mutex::new(String::new()));
    let g = got.clone();
    c.get_mut_property_by_name("t").unwrap().on_next(move |v| {
        *g.lock().unwrap() = v.val.as_str().unwrap().to_string();
    });
    let _c = c.go(false, CancellationToken::new()).await;

    // Handshakes, then never reads another byte.
    let _stalled = raw_client(34706).await;
    assert!(wait_for(|| srv.peer_count() == 2, Duration::from_secs(2)).await);

    // ~20 MB of updates: far more than the stalled peer's socket buffers
    // plus its send queue can absorb.
    let started = Instant::now();
    for i in 0..2000 {
        let v = format!("{:05}{}", i, "z".repeat(10_000));
        srv.set_property("t", Some(&v.as_str().into())).await;
    }
    assert!(
        wait_for(|| got.lock().unwrap().starts_with("01999"), Duration::from_secs(15)).await,
        "the reading client never got the last update"
    );
    assert!(
        wait_for(|| srv.peer_count() == 1, Duration::from_secs(5)).await,
        "the stalled client should have been dropped"
    );
    println!("stalled-client run took {:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn max_peers_refuses_extra_clients() {
    let s = Hive::new_from_str_unknown("listen = \"127.0.0.1:34707\"\nname = \"Serv\"\nmax_peers = 1\n[Properties]\nm = 0");
    let mut srv = s.go(true, CancellationToken::new()).await;
    let (_a, a_last) = hive_client(34707, "a").await;
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await);

    let (b, _) = hive_client(34707, "b").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(srv.peer_count(), 1, "max_peers = 1 let a second client in");
    assert_eq!(b.peer_count(), 0);

    srv.set_property("m", Some(&5.into())).await;
    assert!(wait_for(|| a_last.load(Ordering::SeqCst) == 5, Duration::from_secs(2)).await);
}

/// A burst from the server has to reach every client complete and in
/// order: nothing lost or doubled where one batch of writes ends and the
/// next begins, or where a frame straddles two reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_burst_reaches_every_client_once_and_in_order() {
    const N: i64 = 20_000;
    let (mut srv, _, _) = server(34708).await;

    let mut clients = Vec::new();
    for i in 0..3 {
        let mut c = Hive::new_from_str_unknown(&format!("connect = \"127.0.0.1:34708\"\nname = \"c{i}\""));
        let last = Arc::new(AtomicI64::new(-1));
        let gaps = Arc::new(AtomicUsize::new(0));
        let (l, g) = (last.clone(), gaps.clone());
        c.get_mut_property_by_name("m").unwrap().on_next(move |v| {
            let v = v.val.as_integer().unwrap();
            // The first value is the server's initial 0, then 1, 2, 3...
            if l.swap(v, Ordering::SeqCst) + 1 != v {
                g.fetch_add(1, Ordering::SeqCst);
            }
        });
        let h = c.go(false, CancellationToken::new()).await;
        clients.push((h, last, gaps));
    }
    assert!(wait_for(|| srv.peer_count() == 3, Duration::from_secs(2)).await);

    for v in 1..=N {
        srv.set_property("m", Some(&v.into())).await;
    }
    for (i, (_, last, gaps)) in clients.iter().enumerate() {
        assert!(
            wait_for(|| last.load(Ordering::SeqCst) == N, Duration::from_secs(10)).await,
            "client {} stopped at {}",
            i,
            last.load(Ordering::SeqCst)
        );
        assert_eq!(gaps.load(Ordering::SeqCst), 0, "client {} saw values skipped or out of order", i);
    }
    assert_eq!(srv.peer_count(), 3, "a healthy client was dropped during the burst");
}

/// One message bigger than a peer's whole send buffer (and than a read
/// chunk) still goes through, in both directions, and what was sent
/// after it arrives after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_larger_than_the_send_buffer_syncs_both_ways() {
    let mut s = Hive::new_from_str_unknown("listen = \"127.0.0.1:34709\"\nname = \"Serv\"\n[Properties]\nt = \"x\"");
    let srv_got = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let g = srv_got.clone();
    s.get_mut_property_by_name("t").unwrap().on_next(move |v| {
        g.lock().unwrap().push(v.val.as_str().unwrap().to_string());
    });
    let mut srv = s.go(true, CancellationToken::new()).await;

    let mut c = Hive::new_from_str_unknown("connect = \"127.0.0.1:34709\"\nname = \"real\"");
    let cli_got = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let g = cli_got.clone();
    c.get_mut_property_by_name("t").unwrap().on_next(move |v| {
        g.lock().unwrap().push(v.val.as_str().unwrap().to_string());
    });
    let mut cli = c.go(false, CancellationToken::new()).await;
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(2)).await);

    // 600 KB: over the 256 KiB send buffer, under the 1 MiB message cap.
    let up = "u".repeat(600_000);
    cli.set_property("t", Some(&up.as_str().into())).await;
    cli.set_property("t", Some(&"after-up".into())).await;
    assert!(
        wait_for(|| srv_got.lock().unwrap().last().is_some_and(|s| s == "after-up"), Duration::from_secs(5)).await,
        "server never saw the update that followed the large one"
    );
    {
        let got = srv_got.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert!(got[0] == up, "large message arrived as {} bytes", got[0].len());
    }

    let down = "d".repeat(600_000);
    srv.set_property("t", Some(&down.as_str().into())).await;
    srv.set_property("t", Some(&"after-down".into())).await;
    assert!(
        wait_for(|| cli_got.lock().unwrap().last().is_some_and(|s| s == "after-down"), Duration::from_secs(5)).await,
        "client never saw the update that followed the large one"
    );
    let got = cli_got.lock().unwrap();
    let n = got.len();
    assert!(n >= 2 && got[n - 2] == down, "large message missing or damaged on the way down");
}

/// todo.md 5.3: a peer that sends faster than the hive can handle is
/// slowed down by TCP instead of growing the hive's queue. The server's
/// `on_next` takes 1 ms per update, so it handles about 17 KB/s of these
/// frames; the raw client tries to write 64 MiB. With backpressure its
/// writes block once the hive's inbound budget (256 KiB) and the socket
/// buffers are full; without it `read_loop` keeps reading and queueing.
#[tokio::test(flavor = "multi_thread")]
async fn a_fast_sender_is_slowed_down_not_buffered() {
    let port = 34720;
    let mut h = Hive::new_from_str_unknown(&format!(
        "listen = \"127.0.0.1:{port}\"\nname = \"Serv\"\n[Properties]\nm = 0"
    ));
    let slow = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let s = slow.clone();
    h.get_mut_property_by_name("m").unwrap().on_next(move |_| {
        if s.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let cancel = CancellationToken::new();
    let _srv = h.go(true, cancel.clone()).await;

    let mut conn = raw_client(port).await;
    let mut chunk = Vec::new();
    while chunk.len() < 64 * 1024 {
        chunk.extend(property_frame("m", chunk.len() as i32));
    }
    let total = 64 * 1024 * 1024;
    let written = Arc::new(AtomicUsize::new(0));
    let w = written.clone();
    let writer = tokio::spawn(async move {
        while w.load(Ordering::Relaxed) < total {
            if conn.write_all(&chunk).await.is_err() {
                break;
            }
            w.fetch_add(chunk.len(), Ordering::Relaxed);
        }
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = written.load(Ordering::Relaxed);
    // Inbound budget + one read chunk + kernel socket buffers on both
    // ends; loopback buffers are a few MiB at most.
    assert!(
        after < 16 * 1024 * 1024,
        "client wrote {} MiB in 1.5 s to a hive handling ~17 KB/s: nothing pushed back",
        after / (1024 * 1024)
    );
    assert!(after > 0, "client couldn't write at all");
    slow.store(false, Ordering::Relaxed);
    writer.abort();
    cancel.cancel();
}

/// Same for a local producer: `Handler::set_property` waits once the
/// hive's handlers have its inbound budget queued, instead of queuing
/// without limit ahead of a slow event loop.
#[tokio::test(flavor = "multi_thread")]
async fn a_fast_local_producer_waits_for_the_hive() {
    // A listen address so the hive has an event loop running at all.
    let mut h = Hive::new_from_str_unknown("listen = \"127.0.0.1:34721\"\nname = \"Local\"\n[Properties]\nm = 0");
    let slow = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let s = slow.clone();
    h.get_mut_property_by_name("m").unwrap().on_next(move |_| {
        if s.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let cancel = CancellationToken::new();
    let mut handler = h.go(false, cancel.clone()).await;
    let sent = Arc::new(AtomicUsize::new(0));
    let n = sent.clone();
    let producer = tokio::spawn(async move {
        for v in 0..1_000_000i64 {
            handler.set_property("m", Some(&v.into())).await;
            n.fetch_add(1, Ordering::Relaxed);
        }
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // 256 KiB of ~22 byte updates is ~12k queued, plus ~1k handled.
    let after = sent.load(Ordering::Relaxed);
    assert!(after < 50_000, "{after} updates queued ahead of a hive handling ~1k/s");
    slow.store(false, Ordering::Relaxed);
    producer.abort();
    cancel.cancel();
}

/// todo.md 5.7: a hive that can't start used to leave `go(true, ..)`
/// waiting forever (the run task panicked, nothing woke the caller).
#[tokio::test(flavor = "multi_thread")]
async fn go_reports_a_hive_that_cannot_start() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let config = format!("listen = \"127.0.0.1:{port}\"\nname = \"Busy\"");

    let r = tokio::time::timeout(
        Duration::from_secs(5),
        Hive::new_from_str_unknown(&config).try_go(true, CancellationToken::new()),
    )
    .await
    .expect("try_go(true) hung on a port that is already in use");
    let err = r.err().expect("binding a taken port succeeded");
    assert!(err.to_string().to_lowercase().contains("address"), "unexpected error: {err}");

    // `go` turns the same failure into a panic in the caller.
    let go = tokio::spawn(async move { Hive::new_from_str_unknown(&config).go(true, CancellationToken::new()).await });
    let joined = tokio::time::timeout(Duration::from_secs(5), go).await.expect("go(true) hung");
    assert!(matches!(joined, Err(e) if e.is_panic()), "go(true) returned instead of panicking");
    drop(taken);
}
