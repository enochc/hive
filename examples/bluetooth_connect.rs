#![allow(unused_imports)]
use futures::channel::{mpsc, mpsc::UnboundedReceiver, mpsc::UnboundedSender};
use futures::executor::block_on;
use hive::{SinkExt, StreamExt};
use hive::hive::Hive;
use hive::init_logging;
use hive::property::Property;
use log::{debug, error, info};
use log::{Level, LevelFilter, Metadata, Record};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::sleep;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging(Some(LevelFilter::Debug));
    println!("<< println");
    debug!("<< debug");
    let props_str = r#"
    bt_connect = "Hive_Peripheral"
    #connect = "192.168.1.13:3000"
    name = "bt_client"
    "#;
    let mut server_hive = Hive::new_from_str_unknown(props_str);

    let mut p = server_hive
        .get_mut_property_by_name("pt")
        .unwrap()
        .on_next(move |value| {
            debug!("<<<< <<<< <<<< <<<< pt: {:?}", value);
        });

    let is_running: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(true), Condvar::new()));

    let run_clone = is_running.clone();
    let cancellation_token = CancellationToken::new();
    let cancellation_token_clone = cancellation_token.clone();
    let res = ctrlc::set_handler(move || {
        info!("Stopping...");
        let (lock, cvar) = &*run_clone;
        let mut running = lock.lock().unwrap();
        *running = false;
        cvar.notify_one();
        cancellation_token_clone.cancelled();
    });

    let handler = server_hive.go(true, cancellation_token);

    let (lock, cvar) = &*is_running;
    let mut running = lock.lock().unwrap();
    while *running {
        println!("Running...");
        running = cvar.wait(running).unwrap();
    }

    println!("Done!! ");
}
