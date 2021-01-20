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
use log::{debug, info, error};


#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
#[test]
fn main() {
    init_logging();
    let counter = Arc::new(AtomicUsize::new(0));
    let count1 = counter.clone();
    let count2 = counter.clone();
    let count3 = counter.clone();
    let count4 = counter.clone();
    let count5 = counter.clone();

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
    "#;
    let mut server_hive = Hive::new_from_str(props_str);
    let prop = server_hive.get_mut_property("thermostatName").unwrap();
    server_hive.get_mut_property("thermostatName", ).unwrap().on_changed.connect(move |value| {
        info!("++++ 222222222 SERV|| THERMOSTAT NAME CHANGED: {:?}", value);
        count1.fetch_add(1, Ordering::SeqCst);
    });
    server_hive.message_received.connect(move |message|{
        info!("++++ ----------  MESSAGE {}", message);
        count4.fetch_add(1, Ordering::SeqCst);
    });


    let mut server_hand = server_hive.get_handler();
    let server_connected = server_hive.connected.clone();
    task::Builder::new().name("SERVER HIVE".to_string()).spawn(async move {
        server_hive.run().await;

    });

    let mut client_hive = Hive::new_from_str("connect = \"127.0.0.1:3000\"\nname=\"client1\"");
    client_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value| {
        info!("++++ 1111111 CLIENT THERMOSTAT NAME CHANGED: {:?}", value);
        count3.fetch_add(1, Ordering::SeqCst);
    });
    client_hive.message_received.connect(move |message| {
        info!("++++ ------------- MESSAGE {}", message);
        count2.fetch_add(1, Ordering::SeqCst);
    });

    client_hive.get_mut_property("thingvalue").unwrap().on_changed.connect(move |value|{
        info!(" ++++ 1111111 CLIENT thing value::::::::::::::::::::: {:?}", value);
        count5.fetch_add(1, Ordering::SeqCst);
    });

    let mut client_hand = client_hive.get_handler();

    task::spawn(async move {
       client_hive.run().await;
    });


    let mut client_hive_2 = Hive::new_from_str("connect = \"127.0.0.1:3000\"\nname=\"client2\"");
    // let mut client_2_handler = client_hive_2.get_handler();
    let mut clone_hand = client_hand.clone();

    let (mut sender, mut receiver) = mpsc::unbounded();
    task::spawn(async move {
        client_hive_2.run();
        task::sleep(Duration::from_millis(500)).await;
        let sleep_dur = Duration::from_millis(50);
        if server_connected.load(Ordering::Relaxed) {  //+2 on prop initialization
            // task::sleep(Duration::from_secs(1)).await;
            server_hand.send_to_peer("client1", "hey you").await; //+ 1
            task::sleep(sleep_dur).await;
            clone_hand.send_to_peer("Server", "hey mr man").await; // + 1
            task::sleep(sleep_dur).await;
            debug!("SENT thermostatName = Before");
            clone_hand.send_property_value("thermostatName", Some(&"Before".into())).await; // +2
            task::sleep(sleep_dur).await;
            debug!("DELETE thermostatName");
            server_hand.delete_property("thermostatName").await;
            task::sleep(sleep_dur).await;
            server_hand.send_property_value("thingvalue", Some(&2.into())).await; // +1
            task::sleep(sleep_dur).await;
            // These should not be counted, because we deleted the ThermostatName
            server_hand.send_property_value("thermostatName", Some(&"After".into())).await;
            task::sleep(sleep_dur).await;

        } else {
            println!("server is not connected");
        }
        sender.send(1 as i32).await;

    });

    let done = block_on(receiver.next());
    assert_eq!(counter.load(Ordering::Relaxed), 7);
    client_hand.hangup();

    println!("done with stuff");

}

