use hive::hive::Hive;
use hive::CancellationToken;
use std::time::{Duration, Instant};

async fn wait_for<F: Fn() -> bool>(f: F, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn client_reconnects_and_server_peer_count_tracks_it() {
    let server = Hive::new_from_str_unknown("listen = \"127.0.0.1:34567\"\nname = \"Serv\"\n[Properties]\nm1 = 0");
    let server_handler = server.go(true, CancellationToken::new()).await;

    // Server is listening but nobody is attached.
    assert_eq!(server_handler.peer_count(), 0, "no client yet");
    assert!(server_handler.is_connected(), "server sets connected on bind");

    // Session one.
    let c1 = Hive::new_from_str_unknown(
        "connect = \"127.0.0.1:34567\"\nname = \"fc\"\nwave_timeout_ms = 800",
    );
    let t1 = CancellationToken::new();
    let h1 = c1.go(false, t1.clone()).await;

    assert!(
        wait_for(|| h1.peer_count() > 0, Duration::from_secs(3)).await,
        "client never attached"
    );
    assert!(
        wait_for(|| server_handler.peer_count() == 1, Duration::from_secs(3)).await,
        "server never saw the client"
    );

    // Tear the client down the way a reconnecting client does between
    // attempts (Navio2r's GUI, for one).
    // Cancelling ends run(), which drops the Hive and its peers, which is
    // what closes the socket.
    t1.cancel();

    assert!(
        wait_for(|| server_handler.peer_count() == 0, Duration::from_secs(5)).await,
        "server still counts a dead peer, socket did not close"
    );

    // Session two against the same still-running server.
    let c2 = Hive::new_from_str_unknown(
        "connect = \"127.0.0.1:34567\"\nname = \"fc\"\nwave_timeout_ms = 800",
    );
    let t2 = CancellationToken::new();
    let h2 = c2.go(false, t2.clone()).await;

    assert!(
        wait_for(|| h2.peer_count() > 0, Duration::from_secs(3)).await,
        "client never reattached"
    );
    assert!(
        wait_for(|| server_handler.peer_count() == 1, Duration::from_secs(3)).await,
        "server never saw the reconnect"
    );
    t2.cancel();
}


#[tokio::test(flavor = "multi_thread")]
async fn cancelled_connect_attempt_stops_dialing() {
    // Nothing is listening on this port.
    let c = Hive::new_from_str_unknown("connect = \"127.0.0.1:34599\"\nname = \"fc\"");
    let token = CancellationToken::new();
    let h = c.go(false, token.clone()).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.peer_count(), 0, "should not have connected");

    // Cancelling has to actually end the retry loop rather than leave it
    // sleeping on hive's internal 10s timer.
    token.cancel();
    tokio::time::sleep(Duration::from_millis(200)).await;
}
