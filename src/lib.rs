// use std::fs;
#![warn(rust_2018_idioms)]

use std::ffi::CStr;
use chrono::Local;
use log::{Metadata, Level, Record};
pub use log::LevelFilter;
pub use tokio_util::sync::CancellationToken;
pub use futures;
use std::os::raw::c_char;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use crate::hive::Hive;

#[cfg(feature = "gui")]
pub mod hive_gui;
mod hive_macros;
pub mod property;
pub mod signal;
pub mod hive;
pub mod peer;
pub mod handler;

#[cfg(feature = "websock")]
pub mod websocket;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;


#[cfg(feature="bluetooth")]
#[macro_use]
extern crate lazy_static;

#[cfg(target_os = "android")]
mod android;

// INIT LOGGING
pub struct SimpleLogger;
impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            // I currently only care about the minute, second and millisecond
            let n = Local::now().format("%M:%S.%6f");
            println!("{} {:?} {:?}:{:?} - {}",n, record.level(), record.file().unwrap(), record.line().unwrap(), record.args());
        }
    }

    fn flush(&self) {}
}
pub static LOGGER: SimpleLogger = SimpleLogger;

pub fn init_logging(level:Option<LevelFilter>){
    let my_level = level.unwrap_or_else(|| LevelFilter::Trace);
    log::set_logger(&LOGGER).map(|()| log::set_max_level(my_level)).expect("failed to init logger");
}




#[no_mangle]
pub unsafe extern "C" fn newHive(props: *const c_char) -> Hive {
    let c_str = CStr::from_ptr(props);
    let prop_str_pointer = c_str.to_str().unwrap_or_else(|_| "you");

    Hive::new_from_str("Hive1", prop_str_pointer )
}

// fn get_toml_config(file_path: &str) -> toml::Value{
//     let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
//     return toml::from_str(&foo).unwrap();
// }

pub fn panic_after<T, F>(d: Duration, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T,
    F: Send + 'static,
{
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let val = f();
        done_tx.send(()).expect("Unable to send completion signal");
        val
    });

    match done_rx.recv_timeout(d) {
        Ok(_) => handle.join().expect("Thread panicked"),
        Err(_) => panic!("Thread took too long"),
    }
}