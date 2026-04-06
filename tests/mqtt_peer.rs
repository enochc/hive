/// Integration test for the MQTT peer.
///
/// Requires an MQTT broker running on 127.0.0.1:1883.
/// Skip by setting env var: SKIP_MQTT_TESTS=1
///
/// Quick broker setup:
///     docker run -it -p 1883:1883 eclipse-mosquitto:2 \
///         mosquitto -c /mosquitto-no-auth.conf

#![allow(unused_imports)]
#![cfg(feature = "mqtt")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hive::hive::Hive;
use hive::futures::{SinkExt, StreamExt};
use hive::init_logging;
use hive::property::PropertyValue;
use hive::LevelFilter;
use hive::CancellationToken;
use log::{debug, info};

/// Drain the notification channel looking for a message that matches `target`.
async fn wait_for(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>, target: &str) {
    let timeout = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(val) = rx.recv().await {
            info!("wait_for({}) got: {:?}", target, val);
            if val == target {
                return;
            }
        }
        panic!("Channel closed before receiving {:?}", target);
    });
    timeout.await.expect(&format!("Timed out waiting for {:?}", target));
}

fn should_skip() -> bool {
    std::env::var("SKIP_MQTT_TESTS").unwrap_or_default() == "1"
}

/// Test that two Hive instances connected to the same MQTT broker can
/// synchronize property values through MQTT topics.
///
/// Hive A publishes a property change → MQTT broker → Hive B receives it.
#[tokio::test(flavor = "multi_thread")]
async fn mqtt_property_sync() {
    if should_skip() {
        eprintln!("Skipping MQTT test (SKIP_MQTT_TESTS=1)");
        return;
    }

    init_logging(Some(LevelFilter::Debug));
    info!("=== mqtt_property_sync ===");

    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        // --- Hive A: publishes lightValue changes ---
        let config_a = r#"
            name = "MqttHiveA"

            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-test-a"
            topic_prefix = "hive/test"
            qos = 1
            keep_alive = 30
            subscribe = ["lightValue"]

            [Properties]
            lightValue = 0
        "#;

        let hive_a = Hive::new_from_str_unknown(config_a);
        let cancel_a = CancellationToken::new();
        let mut handler_a = hive_a.go(false, cancel_a.clone()).await;

        // --- Hive B: subscribes to lightValue ---
        let config_b = r#"
            name = "MqttHiveB"

            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-test-b"
            topic_prefix = "hive/test"
            qos = 1
            keep_alive = 30
            subscribe = ["lightValue"]

            [Properties]
            lightValue = 0
        "#;

        let mut hive_b = Hive::new_from_str_unknown(config_b);

        // Watch for lightValue changes on Hive B
        let ntx = notify_tx.clone();
        hive_b.get_mut_property_by_name("lightValue")
            .unwrap()
            .on_next(move |value| {
                info!("Hive B lightValue on_next: {:?}", value);
                let _ = ntx.send(value.to_string());
            });

        let cancel_b = CancellationToken::new();
        let handler_b = hive_b.go(false, cancel_b.clone()).await;

        // Give both MQTT connections time to establish and subscribe
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Hive A publishes a property change
        info!("Setting lightValue = 42 on Hive A");
        handler_a.set_property("lightValue", Some(&42.into())).await;

        // Hive B should receive it via MQTT
        wait_for(&mut notify_rx, "42").await;
        info!("Hive B received lightValue = 42 via MQTT");

        // Cleanup
        cancel_a.cancel();
        cancel_b.cancel();

    }).await;

    if result.is_err() {
        panic!("mqtt_property_sync timed out after 30 seconds");
    }
}

/// Test that a single Hive instance receives MQTT messages published
/// externally (simulated by using the MqttPeerHandle directly).
#[tokio::test(flavor = "multi_thread")]
async fn mqtt_external_publish_received() {
    if should_skip() {
        eprintln!("Skipping MQTT test (SKIP_MQTT_TESTS=1)");
        return;
    }

    init_logging(Some(LevelFilter::Debug));
    info!("=== mqtt_external_publish_received ===");

    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let config_str = r#"
            name = "MqttReceiver"

            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-test-recv"
            topic_prefix = "hive/ext_test"
            qos = 1
            keep_alive = 30
            subscribe = ["sensor"]

            [Properties]
            sensor = "none"
        "#;

        let mut hive = Hive::new_from_str_unknown(config_str);

        let ntx = notify_tx.clone();
        hive.get_mut_property_by_name("sensor")
            .unwrap()
            .on_next(move |value| {
                info!("sensor on_next: {:?}", value);
                let _ = ntx.send(value.to_string());
            });

        let cancel = CancellationToken::new();
        let handler = hive.go(false, cancel.clone()).await;

        // Give MQTT time to connect and subscribe
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Simulate an external MQTT publish using a separate rumqttc client
        let mut opts = rumqttc::MqttOptions::new("external-publisher", "127.0.0.1", 1883);
        opts.set_keep_alive(Duration::from_secs(5));
        let (client, mut eventloop) = rumqttc::AsyncClient::new(opts, 10);

        // Drive the event loop in background
        tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(_) => {}
                    Err(e) => {
                        debug!("external publisher eventloop error: {:?}", e);
                        break;
                    }
                }
            }
        });

        // Small delay for the external client to connect
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Publish as if we're an external device
        info!("External publish: hive/ext_test/sensor = 99");
        client
            .publish("hive/ext_test/sensor", rumqttc::QoS::AtLeastOnce, false, b"99")
            .await
            .expect("external publish failed");

        // Hive should receive it
        wait_for(&mut notify_rx, "99").await;
        info!("Hive received external MQTT sensor = 99");

        cancel.cancel();

    }).await;

    if result.is_err() {
        panic!("mqtt_external_publish_received timed out after 20 seconds");
    }
}
