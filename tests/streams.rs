#![allow(unused_imports)]
use hive::futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use hive::futures::{SinkExt, StreamExt};
use hive::property::{Property, PropertyValue};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use hive::init_logging;
use log::{debug, info, error};
use hive::LevelFilter;

use hive::CancellationToken;

/// Drain the notification channel looking for a message that matches `target`.
/// Every notification is queued, so we never lose one.
async fn wait_for(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>, target: &str) {
    while let Some(val) = rx.recv().await {
        info!("wait_for({}) got: {:?}", target, val);
        if val == target {
            return;
        }
    }
    panic!("Channel closed before receiving {:?}", target);
}

/// Drain the notification channel until `counter` exceeds `min_count`.
async fn wait_for_count(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    counter: &AtomicUsize,
    min_count: usize,
) {
    loop {
        if counter.load(Ordering::SeqCst) > min_count {
            return;
        }
        match rx.recv().await {
            Some(val) => {
                info!("wait_for_count({}) got: {:?}, counter={}", min_count, val, counter.load(Ordering::SeqCst));
            }
            None => panic!("Channel closed before count reached {}", min_count),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_test() {
    init_logging(Some(LevelFilter::Debug));
    info!("RUNNING HIVE_HANDLER");
    let result = tokio::time::timeout(Duration::from_millis(50_000), async {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_1 = counter.clone();
        let counter_2 = counter.clone();
        let counter_3 = counter.clone();
        let counter_4 = counter.clone();
        let counter_6 = counter.clone();
        let counter_7 = counter.clone();
        let counter_8 = counter.clone();

        let thing_value = Arc::new(AtomicUsize::new(0));
        let thing_value_1 = thing_value.clone();

        // Use an async mpsc channel instead of Condvar.
        // Every callback sends a string notification here; the test task
        // reads them sequentially so nothing is ever lost.
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let props_str = r#"
    listen = "127.0.0.1:3000"
    name = "Server"
    [bluetooth]
    btname = "Hive"
    [Properties]
    thingvalue= 1
    is_active = true
    lightValue = 0
    thermostatName = "orig therm name"
    thermostatTemperature= "too cold"
    thermostatTarget_temp = 1.45
    blahh = 1000
    longNum = -1003987654
    "#;
        let mut server_hive = Hive::new_from_str_unknown(props_str);
        let prop = server_hive.get_mut_property_by_name("thermostatName").unwrap();

        let v = prop.value.read().unwrap().as_ref().unwrap().to_string();
        let matches = v.eq("\"orig therm name\"");
        info!("maths: {}", matches);
        assert_eq!(v, "\"orig therm name\"".to_string());

        // Track how many times the thermostat-name stream fires (server + client combined).
        let messages_received = Arc::new(AtomicUsize::new(0));
        let mr_clone = messages_received.clone();
        let mr_clone2 = messages_received.clone();

        // --- server thermostat-name stream listener ---
        let mut ff = server_hive.get_mut_property_by_name("thermostatName").unwrap().stream.clone();
        let ntx = notify_tx.clone();
        tokio::task::spawn(async move {
            while let Some(x) = ff.next().await {
                info!("+++++++++++++++++++++ SERV|| THERMOSTAT NAME CHANGED: {:?}", x);
                counter_1.fetch_add(1, Ordering::SeqCst);
                mr_clone.fetch_add(1, Ordering::SeqCst);
                let _ = ntx.send(x.to_string());
            }
        });

        // --- server message_received handler ---
        let ntx = notify_tx.clone();
        server_hive.message_received.connect(move |message| {
            info!("+++++++++++++++++++++  server MESSAGE {}", message);
            counter_4.fetch_add(1, Ordering::SeqCst);
            let newc = counter_4.load(Ordering::SeqCst);
            info!("<<<< {:?}", newc);
            let _ = ntx.send(message);
        });

        let cancellation_token = CancellationToken::new();
        let mut server_hand = server_hive.go(true, cancellation_token.clone()).await;

        let mut client_hive = Hive::new_from_str_unknown("connect = \"127.0.0.1:3000\"\nname=\"client1\"");

        let client_thermostat_name_property = client_hive.get_mut_property_by_name("thermostatName").unwrap();
        let mut stream = client_thermostat_name_property.stream.clone();

        // --- client thermostat-name stream listener ---
        let ntx = notify_tx.clone();
        tokio::task::spawn(async move {
            let mut count = 1;
            while let Some(x) = stream.next().await {
                info!("+++++++++++++++++++++ CLIENT THERMOSTAT NAME CHANGED: {:?}", x);
                if count == 1 {
                    assert_eq!(x, PropertyValue::from("orig therm name"));
                    count += 1;
                } else if count == 2 {
                    assert_eq!(x, PropertyValue::from("Before"));
                    count += 1;
                } else if count == 3 {
                    panic!("verify that the property has been deleted");
                }
                counter_3.fetch_add(1, Ordering::SeqCst);
                mr_clone2.fetch_add(1, Ordering::SeqCst);
                let _ = ntx.send(x.to_string());
            }
        });

        // --- client message_received handler ---
        let ntx = notify_tx.clone();
        client_hive.message_received.connect(move |message| {
            info!("+++++++++++++++++++++ client MESSAGE {}", message);
            counter_2.fetch_add(1, Ordering::SeqCst);
            let _ = ntx.send(message);
        });

        // --- client thingvalue handler ---
        let ntx = notify_tx.clone();
        client_hive.get_mut_property_by_name("thingvalue").unwrap().on_next(move |value| {
            info!(" +++++++++++++++++++++ CLIENT thing value::::::::::::::::::::: {:?}", value);
            let t2 = value.val.as_integer().unwrap() as usize;
            thing_value_1.store(t2, Ordering::Relaxed);
            let _ = ntx.send(value.to_string());
        });

        client_hive.get_mut_property_by_name("thermostatTarget_temp").unwrap().on_next(move |value| {
            assert_eq!(value.val.as_float().unwrap(), 1.45);
            counter_6.fetch_add(1, Ordering::Relaxed);
        });

        client_hive.get_mut_property_by_name("blahh").unwrap().on_next(move |value| {
            assert_eq!(value.val.as_integer().unwrap(), 1000);
        });

        client_hive.get_mut_property_by_name("is_active").unwrap().on_next(move |value| {
            assert_eq!(value.val.as_bool().unwrap(), true);
            counter_7.fetch_add(1, Ordering::Relaxed);
        });
        client_hive.get_mut_property_by_name("longNum").unwrap().on_next(move |value| {
            assert_eq!(value.val.as_integer().unwrap(), -1003987654);
            counter_8.fetch_add(1, Ordering::Relaxed);
        });

        let mut client_hand = client_hive.go(true, cancellation_token).await;

        let (mut sender, mut receiver) = mpsc::unbounded();
        tokio::spawn(async move {
            // Wait for the initial property sync to complete.
            // When the client receives "orig therm name" from the server,
            // it means the server has registered the client peer AND
            // the full handshake (including property transfer) is done.
            wait_for_count(&mut notify_rx, &messages_received, 0).await;
            info!("Initial property sync confirmed, proceeding with test");

            let sent1 = server_hand.send_to_peer("client1", "hey you").await; //+ 1
            info!("send: {:?}", sent1);
            // test sometimes failing here, client appears to disconnect
            wait_for(&mut notify_rx, "hey you").await;

            let sent2 = client_hand.send_to_peer("Server", "hey mr man").await; // + 1
            info!("send: {:?}", sent2);
            wait_for(&mut notify_rx, "hey mr man").await;

            info!("SENT thermostatName = Before");
            client_hand.set_property("thermostatName", Some(&"Before".into())).await; // +2
            // Wait until BOTH the server and client thermostat streams have fired.
            // Initial sync gives 1 (client gets "orig therm name"), then "Before"
            // fires on both client and server streams → total 3.
            wait_for_count(&mut notify_rx, &messages_received, 2).await;
            info!("thermostatName update confirmed");

            info!("DELETE thermostatName");
            server_hand.delete_property("thermostatName").await;
            server_hand.set_property("thingvalue", Some(&10.into())).await; // +1
            wait_for(&mut notify_rx, "10").await;

            client_hand.hangup().await;

            // These should not be counted, because we deleted the ThermostatName
            server_hand.set_property("thermostatName", Some(&"After".into())).await;

            sender.send(1i32).await.expect("TODO: panic message");
        });

        let done = receiver.next().await;

        let c6 = thing_value.load(Ordering::Relaxed);
        assert_eq!(c6, 10);

        let c = counter.load(Ordering::Relaxed);
        assert_eq!(c, 8);

        info!("done with stuff");
    }).await;

    if result.is_err() {
        panic!("Test timed out after 50 seconds");
    }
}
