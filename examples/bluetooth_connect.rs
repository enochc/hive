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
use std::time::Duration;


#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging();
    println!("<< println");
    debug!("<< debug");
    let props_str = r#"
    #bt_connect = "Hive_Peripheral"
    connect = "192.168.1.13:3000"
    name = "bt_client"
    "#;
    let mut server_hive = Hive::new_from_str( props_str);

    let mut p = server_hive.get_mut_property("pt").unwrap();

    p.on_changed.connect(move |value|{
        debug!("<<<< <<<< <<<< <<<< pt: {:?}", value);
    });

    let handler = server_hive.get_handler();

    task::block_on(async{server_hive.run().await});
    // task::spawn(async move {
    //     server_hive.run().await;
    // });
    // sleep(Duration::from_secs(2));
    //
    //
    // // task::spawn(async move {
    // //     handler.send_property_value("pt", Some(&666.into())).await;
    // // });
    // println!("sleeping...");
    // sleep(Duration::from_secs(10));



    // task::block_on(async {});

    println!("Done!! ");



}
