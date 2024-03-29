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
use simple_signal::{self, Signal};
use std::sync::{Mutex, Condvar};
use std::env;

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
fn main() {
    init_logging(Some(LevelFilter::Debug));
    println!("<< println");
    debug!("<< debug");
    let args: Vec<String> = env::args().collect();
    debug!("<< debug {:?}", args[1]);
    let mut server_hive = Hive::new(&args[1]);

    use simple_signal::{self, Signal};

    server_hive.get_mut_property("turn").unwrap().on_next(move |value|{
        debug!("<<<< TURN: {:?}", value);
    });

    let advertising = server_hive.get_advertising();

    let is_running: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(true), Condvar::new()));

    // listens for termination signal: (ctrl+c)
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
    let (lock, cvar) = &*is_running;

    let handler = server_hive.go(true);

    let mut running = lock.lock().unwrap();
    while *running {
        running = cvar.wait(running).unwrap();
    }


    debug!("Done!! ");

}
