#![allow(unused_imports)]
use async_std::sync::Arc;
use async_std::task;
use futures::channel::{mpsc, mpsc::UnboundedReceiver, mpsc::UnboundedSender};
use futures::executor::block_on;
use futures::{SinkExt, StreamExt};
use hive::hive::Hive;
use hive::init_logging;
use hive::property::Property;
use log::{debug, error, info};
use log::{Level, LevelFilter, Metadata, Record};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::sleep;

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging(Some(LevelFilter::Debug));
    println!("<< println");
    debug!("<< debug");
    let props_str = r#"
    # listen = "192.168.5.45:3000"
    # if were using the localhost public address, then the ip can be omitted
    listen="3000"
    name= "bossman"
    bt_listen = "Hive_Peripheral"
    # Because we can get many properties with a single get, it's listed outside of the properties
    # rest_get = "http://127.0.0.1:8000/hive/get"

    [Properties]
    turn = {val=0, rest_set="http://127.0.0.1:8000/hive/save/turn/{val}/?type=int"}
    speed = 1000
    pt = 2

    #[REST]
    #get = "http://127.0.0.1:8000/hive/get"

    "#;
    let mut server_hive = Hive::new_from_str_unknown(props_str);

    server_hive
        .get_mut_property(&Property::hash_id("turn"))
        .unwrap()
        .on_next(move |value| {
            debug!("<<<< TURN: {:?}", value);
        });

    let advertising = server_hive.get_advertising();

    let is_running: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(true), Condvar::new()));

    let run_clone = is_running.clone();
    
    let res = ctrlc::set_handler(move || {
        info!("Stopping...");
        let (lock, cvar) = &*run_clone;
        let mut running = lock.lock().unwrap();
        *running = false;
        cvar.notify_one();
    });

    let (lock, cvar) = &*is_running;

    let handler = server_hive.go(true, true);

    let mut running = lock.lock().unwrap();
    while *running {
        running = cvar.wait(running).unwrap();
    }

    debug!("Done!! ");
}
