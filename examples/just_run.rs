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
use log::{warn, Level, LevelFilter, Metadata, Record};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::sleep;

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging(Some(LevelFilter::Debug));
    println!("<< println");
    debug!("<< debug");
    let args: Vec<String> = env::args().collect();
    debug!("<< debug {:?}", args[1]);
    let mut server_hive = Hive::new(&args[1]);

    server_hive.get_mut_property_by_name("turn").unwrap().on_next(move |value| {
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
    if res.is_err() {
        error!("<<<<Error setting Ctrl-C handler");
    }
    let (lock, cvar) = &*is_running;

    let handler = server_hive.go(true, false);

    let mut running = lock.lock().unwrap();
    while *running {
        running = cvar.wait(running).unwrap();
    }

    debug!("Done!! ");
}
