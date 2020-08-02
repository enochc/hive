#![allow(unused_imports)]
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{SinkExt, StreamExt};
use hive::property::Property;
use futures::executor::block_on;
use std::thread::sleep;
use failure::_core::time::Duration;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {

    let counter = Arc::new(AtomicUsize::new(0));
    let count1 = Arc::clone(&counter);
    let count2 = count1.clone();
    let count3 = count2.clone();

    let props_str = r#"
    listen = "127.0.0.1:3000"
    [Properties]
    thingvalue= 1
    is_active = true
    lightValue = 0
    thermostatName = "thermostat"
    thermostatTemperature= "too cold"
    thermostatTarget_temp = 1.45
    "#;
    let mut server_hive = Hive::new_from_str("SERVE", props_str);
    server_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value|{
        println!("<<<< SERV|| THERMOSTAT NAME CHANGED: {:?}", value);
        count1.fetch_add(1, Ordering::SeqCst);
    });


    let mut server_hand = server_hive.get_handler();
    task::spawn(  async move{
        server_hive.run().await;
    });

    let mut client_hive = Hive::new_from_str("client1", "connect = \"127.0.0.1:3000\"");
    client_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value|{
        count3.fetch_add(1, Ordering::SeqCst);
       println!("<<<< CLIENT|| THERMOSTAT NAME CHANGED: {:?}", value);
    });
    client_hive.message_received.connect(move |message|{
        println!("MESSAGE {}", message);
        count2.fetch_add(1, Ordering::SeqCst);
    });
    let mut client_hand = client_hive.get_handler();

    task::spawn(async move {
        client_hive.run().await;
    });


    // wait a sec for the client to connect and sync properties
    sleep(Duration::from_millis(500));

    let mut client_hive_2 = Hive::new_from_str("client2", "connect = \"127.0.0.1:3000\"");
    let mut client_2_handler = client_hive_2.get_handler();
    task::spawn(async move {
        client_hive_2.run().await;
    });
    sleep(Duration::from_millis(500));

    block_on(server_hand.send_to_peer("client1", "hey you"));


    //TODO this works for the server hand, make it work for the client hand
    block_on(client_hand.send_property_string("thermostatName", "Before"));

    sleep(Duration::from_millis(500));

    block_on(server_hand.delete_property("thermostatName"));
    sleep(Duration::from_secs(1));

    block_on( server_hand.send_property_string("thermostatName", "After"));
    assert_eq!(counter.load(Ordering::Relaxed), 3);

    // sleep a few seconds then call it quits
    sleep(Duration::from_millis(500));
    client_hand.hangup();
    println!("done with stuff");
    // server_hand.hangup();
    // block_on(server_hand.send_property_string("thermostatName", "late"));

    //sleep(Duration::from_secs(20));
    // let (tx, mut rx) = mpsc::unbounded();
    //
    // let result:i32 = block_on(rx.next()).unwrap();
    // println!("<<<< this wasn't supposed to happen");



}
