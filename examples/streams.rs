#![allow(unused_imports)]
use async_std::prelude::*;
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{SinkExt, StreamExt};
use hive::property::{Property, PropertyType};
// use async_std::task::sleep;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use async_std::task::block_on;
use std::thread::sleep;
use std::time::Duration;
use hive::init_logging;


#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging();
    let counter = Arc::new(AtomicUsize::new(0));
    let count1 = counter.clone();
    let count2 = counter.clone();
    let count3 = counter.clone();
    let count4 = counter.clone();

    let props_str = r#"
    listen = "127.0.0.1:3000"
    [bluetooth]
    btname = "Hive"
    [Properties]
    thingvalue= 1
    is_active = true
    lightValue = 0
    thermostatName = "orig therm name"
    thermostatTemperature= "too cold"
    thermostatTarget_temp = 1.45
    "#;
    let mut server_hive = Hive::new_from_str("SERVE", props_str);
    let prop = server_hive.get_mut_property("thermostatName").unwrap();
    server_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value| {
        println!("<<<< 222222222 SERV|| THERMOSTAT NAME CHANGED: {:?}", value);
        count1.fetch_add(1, Ordering::SeqCst);
    });
    server_hive.message_received.connect(move |message|{
        println!("<<<< ----------  MESSAGE {}", message);
        count4.fetch_add(1, Ordering::SeqCst);
    });


    let mut server_hand = server_hive.get_handler();
    let server_connected = server_hive.connected.clone();
    task::Builder::new().name("SERVER HIVE".to_string()).spawn(async move {
        server_hive.run().await;
    });

    let mut client_hive = Hive::new_from_str("client1", "connect = \"127.0.0.1:3000\"");
    client_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value| {
        println!("<<<< 1111111 CLIENT|| THERMOSTAT NAME CHANGED: {:?}", value);
        count3.fetch_add(1, Ordering::SeqCst);
    });
    client_hive.message_received.connect(move |message| {
        println!("<<<< ------------- MESSAGE {}", message);
        count2.fetch_add(1, Ordering::SeqCst);
    });

    client_hive.get_mut_property("thingvalue").unwrap().on_changed.connect(|value|{
        println!("thing value::::::::::::::::::::: {:?}", value);
    });

    let mut client_hand = client_hive.get_handler();

    task::spawn(async move {
       client_hive.run().await;
    });


    // wait a sec for the client to connect and sync properties
    // sleep(Duration::from_secs(1));

    let mut client_hive_2 = Hive::new_from_str("client2", "connect = \"127.0.0.1:3000\"");
    let mut client_2_handler = client_hive_2.get_handler();
    let mut clone_hand = client_hand.clone();

    let (mut sender, mut receiver) = mpsc::unbounded();
    task::spawn(async move {
        client_hive_2.run();
        task::sleep(Duration::from_millis(500)).await;
        if server_connected.load(Ordering::Relaxed) {
            server_hand.send_to_peer("client1", "hey you").await;
            task::sleep(Duration::from_millis(500)).await;
            clone_hand.send_to_peer("SERVE", "hey mr man").await;
            task::sleep(Duration::from_millis(500)).await;
            clone_hand.send_property_value("thermostatName", Some(&"Before".into())).await;
            task::sleep(Duration::from_millis(500)).await;
            server_hand.delete_property("thermostatName").await;
            task::sleep(Duration::from_millis(500)).await;
            server_hand.send_property_value("thingvalue", Some(&2.into())).await;
            task::sleep(Duration::from_millis(500)).await;
            // These should not be counted, because we deleted the ThermostatName
            server_hand.send_property_value("thermostatName", Some(&"After".into())).await;
            task::sleep(Duration::from_millis(500)).await;

        } else {
            println!("server is not connected");
        }
        sender.send(1).await;

    });

    let done = block_on(receiver.next());
    assert_eq!(counter.load(Ordering::Relaxed), 5);
    client_hand.hangup();

    println!("done with stuff");

}

