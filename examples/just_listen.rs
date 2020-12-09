#![allow(unused_imports)]
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{SinkExt, StreamExt};
use hive::property::Property;
use futures::executor::block_on;
use std::thread::sleep;
use log::{Metadata, Level, Record};
use hive::init_logging;
use log::{debug, info, error};


#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging();
    println!("<< println");
    debug!("<< debug");
    let props_str = r#"
    listen = "192.168.5.45:3000"
    [bluetooth]
    btname = "Hive"
    [Properties]
    turn = 0
    speed = 1000
    pt = 2
    "#;
    let mut server_hive = Hive::new_from_str("SERVE", props_str);

    // server_hive.get_mut_property("moveup").unwrap().on_changed.connect( move|value|{
    //     println!("<<<< MOVE UP: {:?}", value);
    //     // let val = value.unwrap().as_bool().unwrap();
    //     // move_up_clone.store(val, Ordering::SeqCst);
    //
    // });

    // server_hive.get_mut_property("pt").unwrap().on_changed.connect( move|value|{
    //     println!("<<<< PT: {:?}", value);
    // });
    //
    server_hive.get_mut_property("turn").unwrap().on_changed.connect(move |value|{
        println!("<<<< TURN: {:?}", value);
    });


    let (mut send_chan, mut receive_chan) = mpsc::unbounded();
    task::spawn(async move {
        server_hive.run().await;
        send_chan.send(true).await;
    });
    let done = block_on(receive_chan.next());
    println!("Done {:?}",done);



}
