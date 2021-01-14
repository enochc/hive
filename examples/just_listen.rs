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
use async_std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// blasuhhh
#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging();
    println!("<< println");
    debug!("<< debug");
    let props_str = r#"
    #listen = "192.168.5.45:3000"
    listen="3000"
    name= "listener"
    bt_listen = "Hive_Peripheral"
    [Properties]
    turn = 0
    speed = 1000
    pt = 2
    "#;
    let mut server_hive = Hive::new_from_str(props_str);

    use simple_signal::{self, Signal};

    server_hive.get_mut_property("turn").unwrap().on_changed.connect(move |value|{
        println!("<<<< TURN: {:?}", value);
    });

    let advertising = server_hive.get_advertising();

    task::block_on(async {server_hive.run().await});

    println!("Done!! ");



}
