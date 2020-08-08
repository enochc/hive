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

    // let props_str = r#"
    // listen = "127.0.0.1:3000"
    // [Properties]
    // thingvalue= 1
    // is_active = true
    // lightValue = 0
    // thermostatName = "thermostat"
    // thermostatTarget_temp = 2
    // "#;
    let props_str = r#"
    listen = "127.0.0.1:3000"
    [Properties]
    moveup = false
    movedown = false
    speed = 1000
    "#;
    let mut server_hive = Hive::new_from_str("SERVE", props_str);

    server_hive.get_mut_property("moveup").unwrap().on_changed.connect( move|value|{
        println!("<<<< MOVE UP: {:?}", value);
        // let val = value.unwrap().as_bool().unwrap();
        // move_up_clone.store(val, Ordering::SeqCst);

    });

    server_hive.get_mut_property("movedown").unwrap().on_changed.connect(move |value|{
        println!("<<<< MOVE DOWN: {:?}", value);
        // let val = value.unwrap().as_bool().unwrap();
        // move_down.store(val, Ordering::SeqCst);
    });


    let (mut send_chan, mut receive_chan) = mpsc::unbounded();
    task::spawn(async move {
        server_hive.run().await;
        send_chan.send(true);
    });
    let done = block_on(receive_chan.next());
    println!("Done");



}
