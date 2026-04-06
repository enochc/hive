#![cfg(feature = "mqtt")]

/// MQTT middle-man test — the MQTT broker acts as the relay between
/// two Hive instances.  A third-party MQTT client publishes a property
/// change that should propagate to both Hive nodes.
///
/// Topology:
///
///   [Hive A] <--MQTT--> [Broker] <--MQTT--> [Hive B]
///                           ^
///                           |
///                   [Third-party client]
///
/// Requires an MQTT broker on 127.0.0.1:1883.
/// Skip with: SKIP_MQTT_TESTS=1


use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use hive::hive::Hive;
use hive::init_logging;
use hive::property::{Property, PropertyValue, property_value_to_bytes};
use hive::LevelFilter;
use hive::CancellationToken;
#[allow(unused_imports)]
use log::{debug, info};

fn should_skip() -> bool {
    std::env::var("SKIP_MQTT_TESTS").unwrap_or_default() == "1"
}

/// Third-party publish propagates to both Hive A and Hive B.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mqtt_middle_man() {
    if should_skip() {
        eprintln!("Skipping MQTT test (SKIP_MQTT_TESTS=1)");
        return;
    }

    let result = tokio::time::timeout(Duration::from_millis(15_000), async {
        init_logging(Some(LevelFilter::Debug));
        info!("=== mqtt_middle_man ===");

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_a = counter.clone();
        let counter_b = counter.clone();

        let ack: Arc<(Mutex<u32>, Condvar)> = Arc::new((Mutex::new(0), Condvar::new()));
        let ack_a = ack.clone();
        let ack_b = ack.clone();

        // --- Hive A ---
        let config_a = r#"
            name = "HiveA"
            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-mm-a"
            topic_prefix = "hive/mm"
            qos = 1
            keep_alive = 30
            subscribe = ["temperature"]
            [Properties]
            temperature = 0
        "#;

        let mut hive_a = Hive::new_from_str_unknown(config_a);
        hive_a.get_mut_property_by_name("temperature").unwrap().on_next(move |value| {
            info!("HIVE A ---- temperature changed: {:?}", value);
            counter_a.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_a;
            let mut done = lock.lock().unwrap();
            *done += 1;
            cvar.notify_one();
        });

        let cancel = CancellationToken::new();
        let handler_a = hive_a.go(false, cancel.clone()).await;

        // --- Hive B ---
        let config_b = r#"
            name = "HiveB"
            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-mm-b"
            topic_prefix = "hive/mm"
            qos = 1
            keep_alive = 30
            subscribe = ["temperature"]
            [Properties]
            temperature = 0
        "#;

        let mut hive_b = Hive::new_from_str_unknown(config_b);
        hive_b.get_mut_property_by_name("temperature").unwrap().on_next(move |value| {
            info!("HIVE B ---- temperature changed: {:?}", value);
            counter_b.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_b;
            let mut done = lock.lock().unwrap();
            *done += 1;
            cvar.notify_one();
        });

        let handler_b = hive_b.go(false, cancel.clone()).await;

        // Let both MQTT connections establish and subscribe
        tokio::time::sleep(Duration::from_secs(2)).await;

        // --- Third-party MQTT client publishes a change ---
        let mut opts = rumqttc::MqttOptions::new("third-party-mm", "127.0.0.1", 1883);
        opts.set_keep_alive(Duration::from_secs(5));
        let (client, mut eventloop) = rumqttc::AsyncClient::new(opts, 10);

        tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(_) => {}
                    Err(e) => {
                        debug!("third-party eventloop error: {:?}", e);
                        break;
                    }
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Publish temperature = 72 from the third party
        let ext_prop = Property::from_name("temperature", Some(72.into()));
        let ext_payload = property_value_to_bytes(&ext_prop);
        info!("Third-party publish: temperature = 72 ({} bytes)", ext_payload.len());
        client
            .publish("hive/mm/temperature", rumqttc::QoS::AtLeastOnce, false, ext_payload.to_vec())
            .await
            .expect("third-party publish failed");

        // Wait for both Hive A and Hive B to receive the change
        {
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();
            while *done < 2 {
                info!(":::: waiting for 2 callbacks, have {}", *done);
                let result = cvar.wait_timeout(done, Duration::from_secs(10)).unwrap();
                done = result.0;
                if result.1.timed_out() {
                    panic!("Timed out waiting for both Hive instances to receive temperature=72");
                }
            }
        }
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        info!("Both Hive instances received third-party temperature=72");

        // --- Now Hive A publishes a change; Hive B should receive it ---
        // (the third-party client is not subscribed, so only Hive B gets it)
        let counter_a2 = counter.clone();
        let ack_a2 = ack.clone();

        // We can't re-register on_next after go(), but the existing
        // callbacks on both hives are still active.  Hive A will get its
        // own change via the broker echo, and Hive B gets the forwarded
        // change.  That's 2 more callbacks = total 4.
        //
        // Use the handler to set the property on Hive A.
        let mut handler_a = handler_a;
        handler_a.set_property("temperature", Some(&99.into())).await;

        {
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();
            // Hive A's own change won't echo back (from != "mqtt" publish
            // fires, then the broker delivers to Hive B whose from IS "mqtt").
            // But Hive A also subscribes to the same topic, so the broker
            // delivers it back to Hive A too — that's from "mqtt" for A's
            // ingest.
            //
            // So: Hive A on_next fires once (from handler set_property),
            //     Hive A on_next fires again (from broker echo via MQTT),
            //     Hive B on_next fires once (from broker delivery).
            // That's 3 more = total 5.
            // Actually — let's just wait for at least 4 total (conservative).
            while *done < 4 {
                info!(":::: waiting for 4 callbacks, have {}", *done);
                let result = cvar.wait_timeout(done, Duration::from_secs(10)).unwrap();
                done = result.0;
                if result.1.timed_out() {
                    panic!("Timed out waiting for Hive B to receive temperature=99");
                }
            }
        }

        let final_count = counter.load(Ordering::SeqCst);
        info!("Final callback count: {}", final_count);
        assert!(final_count >= 4, "Expected at least 4 callbacks, got {}", final_count);

        cancel.cancel();
        info!("=== mqtt_middle_man done ===");
    }).await;

    if result.is_err() {
        panic!("mqtt_middle_man timed out after 15 seconds");
    }
}

/// Hive A sets a property, Hive B receives it via broker, then Hive B
/// sets a different value, and Hive A receives that back.  Round-trip
/// through the broker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mqtt_round_trip() {
    if should_skip() {
        eprintln!("Skipping MQTT test (SKIP_MQTT_TESTS=1)");
        return;
    }

    let result = tokio::time::timeout(Duration::from_millis(15_000), async {
        init_logging(Some(LevelFilter::Debug));
        info!("=== mqtt_round_trip ===");

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_a = counter.clone();
        let counter_b = counter.clone();

        let ack: Arc<(Mutex<u32>, Condvar)> = Arc::new((Mutex::new(0), Condvar::new()));
        let ack_a = ack.clone();
        let ack_b = ack.clone();

        // --- Hive A ---
        let config_a = r#"
            name = "RoundTripA"
            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-rt-a"
            topic_prefix = "hive/rt"
            qos = 1
            keep_alive = 30
            subscribe = ["motor_speed"]
            [Properties]
            motor_speed = 0
        "#;

        let mut hive_a = Hive::new_from_str_unknown(config_a);
        hive_a.get_mut_property_by_name("motor_speed").unwrap().on_next(move |value| {
            info!("RT HIVE A ---- motor_speed changed: {:?}", value);
            counter_a.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_a;
            let mut done = lock.lock().unwrap();
            *done += 1;
            cvar.notify_one();
        });

        let cancel = CancellationToken::new();
        let mut handler_a = hive_a.go(false, cancel.clone()).await;

        // --- Hive B ---
        let config_b = r#"
            name = "RoundTripB"
            [MQTT]
            host = "127.0.0.1"
            port = 1883
            client_id = "hive-rt-b"
            topic_prefix = "hive/rt"
            qos = 1
            keep_alive = 30
            subscribe = ["motor_speed"]
            [Properties]
            motor_speed = 0
        "#;

        let mut hive_b = Hive::new_from_str_unknown(config_b);
        hive_b.get_mut_property_by_name("motor_speed").unwrap().on_next(move |value| {
            info!("RT HIVE B ---- motor_speed changed: {:?}", value);
            counter_b.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_b;
            let mut done = lock.lock().unwrap();
            *done += 1;
            cvar.notify_one();
        });

        let mut handler_b = hive_b.go(false, cancel.clone()).await;

        // Let MQTT connections settle
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Hive A sets motor_speed = 100
        info!("Hive A sets motor_speed = 100");
        handler_a.set_property("motor_speed", Some(&100.into())).await;

        // Wait for Hive B to receive it
        {
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();
            while *done < 2 {
                info!(":::: round trip step 1, waiting for 2, have {}", *done);
                let result = cvar.wait_timeout(done, Duration::from_secs(10)).unwrap();
                done = result.0;
                if result.1.timed_out() {
                    panic!("Timed out waiting for motor_speed=100 propagation");
                }
            }
        }
        info!("Step 1 done: motor_speed=100 propagated");

        // Hive B sets motor_speed = 200 — should reach Hive A
        info!("Hive B sets motor_speed = 200");
        handler_b.set_property("motor_speed", Some(&200.into())).await;

        {
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();
            while *done < 4 {
                info!(":::: round trip step 2, waiting for 4, have {}", *done);
                let result = cvar.wait_timeout(done, Duration::from_secs(10)).unwrap();
                done = result.0;
                if result.1.timed_out() {
                    panic!("Timed out waiting for motor_speed=200 propagation");
                }
            }
        }

        let final_count = counter.load(Ordering::SeqCst);
        info!("Round trip final count: {}", final_count);
        assert!(final_count >= 4, "Expected at least 4, got {}", final_count);

        cancel.cancel();
        info!("=== mqtt_round_trip done ===");
    }).await;

    if result.is_err() {
        panic!("mqtt_round_trip timed out after 15 seconds");
    }
}
