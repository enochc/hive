#![allow(unused_imports)]
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{SinkExt, StreamExt};
use hive::property::Property;
use futures::executor::block_on;
use std::thread::sleep;
use failure::_core::time::Duration;

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {

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
    server_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(|value|{
        println!("<<<< SERV|| THERMOSTAT NAME CHANGED: {:?}", value);
    });

    let mut server_hand = server_hive.get_handler();
    task::spawn(  async move{
        server_hive.run().await;
    });

    let mut client_hive = Hive::new_from_str("CLI", "connect = \"127.0.0.1:3000\"");

    client_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value|{
       println!("<<<< CLIENT|| THERMOSTAT NAME CHANGED: {:?}", value);
    });
    let mut client_hand = client_hive.get_handler();



    task::spawn(async move {
        client_hive.run().await;
    });

    // wait a sec for the client to connect and sync properties
    sleep(Duration::from_secs(1));

    //TODO this works for the server hand, make it work for the client hand
    block_on(client_hand.send_property_string("thermostatName", "dumb thermostat"));
    sleep(Duration::from_secs(1));
    block_on( server_hand.send_property_string("thermostatName", "hip hip"));
    // sleep a few seconds then call it quits
    sleep(Duration::from_secs(1));
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
