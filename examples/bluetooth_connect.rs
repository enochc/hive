#![allow(unused_imports)]
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{SinkExt, StreamExt};
use hive::property::Property;
use futures::executor::block_on;
use std::thread::sleep;
use log::{Metadata, Level, Record, LevelFilter};
use hive::init_logging;
use log::{debug, info, error};
use async_std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use simple_signal::Signal;
use std::sync::{Mutex, Condvar};


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
    let mut server_hive = Hive::new_from_str( props_str);

    let mut p = server_hive.get_mut_property("pt").unwrap().on_next(move |value|{
        debug!("<<<< <<<< <<<< <<<< pt: {:?}", value);
    });

    let is_running: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(true), Condvar::new()));

    simple_signal::set_handler(&[Signal::Int, Signal::Term], {
        let run_clone = is_running.clone();

        move |_| {
            info!("Stopping...");
            let (lock, cvar) = &*run_clone;
            let mut running = lock.lock().unwrap();
            *running = false;
            cvar.notify_one();
        }
    });

    let handler = server_hive.go(true);

    let (lock, cvar) = &*is_running;
    let mut running = lock.lock().unwrap();
    while *running {
        println!("Running...");
        running = cvar.wait(running).unwrap();
    }


    println!("Done!! ");

}
