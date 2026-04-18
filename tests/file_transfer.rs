/// Integration test for Hive file transfer protocol.
///
/// Sets up a server and client Hive, sends a file from client to server,
/// and verifies the file_received signal fires with the correct data.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hive::hive::Hive;
use hive::init_logging;
use hive::LevelFilter;
use hive::CancellationToken;
use hive::file_transfer::ReceivedFile;

#[allow(unused_imports)]
use log::{debug, info};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_transfer_between_peers() {
    let result = tokio::time::timeout(Duration::from_millis(10_000), async {
        init_logging(Some(LevelFilter::Debug));

        let file_received = Arc::new(AtomicBool::new(false));
        let file_received_clone = file_received.clone();

        // Channel to notify when the file has been received and verified
        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<ReceivedFile>(1);

        // Server: listens on port 3200
        let server_props = r#"
            listen = "127.0.0.1:3200"
            name = "FileServer"
            [Properties]
            status = "ready"
        "#;
        let mut server_hive = Hive::new_from_str_unknown(server_props);

        // Register file_received callback on the server
        server_hive.file_received.connect(move |received: ReceivedFile| {
            info!(
                "SERVER received file: '{}' ({} bytes)",
                received.filename, received.data.len()
            );
            file_received_clone.store(true, Ordering::SeqCst);
            let _ = done_tx.try_send(received);
        });

        let cancel = CancellationToken::new();
        server_hive.go(true, cancel.clone()).await;

        // Client: connects to server
        let client_props = r#"
            connect = "127.0.0.1:3200"
            name = "FileClient"
        "#;
        let client_hive = Hive::new_from_str_unknown(client_props);
        let mut client_handler = client_hive.go(true, cancel.clone()).await;

        // Give the handshake a moment to complete
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Build a test payload (big enough to span multiple chunks with small chunk size)
        let test_data: Vec<u8> = (0..5000u16).flat_map(|i| i.to_le_bytes()).collect();
        info!("sending file: {} bytes", test_data.len());

        // Send the file from client to server
        client_handler.send_file("FileServer", "test_payload.bin", &test_data).await;

        // Wait for the server to receive and verify the file
        let received = tokio::time::timeout(
            Duration::from_secs(5),
            done_rx.recv(),
        ).await;

        assert!(received.is_ok(), "timed out waiting for file");
        let received = received.unwrap().expect("channel closed");

        assert_eq!(received.filename, "test_payload.bin");
        assert_eq!(received.data.len(), test_data.len());
        assert_eq!(received.data, test_data);
        assert!(file_received.load(Ordering::SeqCst));

        info!("file transfer integration test passed");
    }).await;

    if result.is_err() {
        panic!("Test timed out after 10 seconds");
    }
}
