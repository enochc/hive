// use std::fs;
#![warn(rust_2018_idioms)]

use std::ffi::CStr;
use chrono::Local;
use log::{Metadata, Level, Record, LevelFilter};

use std::os::raw::c_char;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
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
    let prop_str_pointer = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => "you",
    };

    Hive::new_from_str("Hive1", prop_str_pointer )
        // .unwrap()
        // .into_raw()
}

// fn get_toml_config(file_path: &str) -> toml::Value{
//     let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
//     return toml::from_str(&foo).unwrap();
// }

