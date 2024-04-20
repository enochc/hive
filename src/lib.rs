
#![warn(rust_2018_idioms)]
use chrono::Local;
use log::{Metadata, Level, Record, LevelFilter};

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
pub mod backoff;


#[cfg(feature="bluetooth")]
#[macro_use]
extern crate lazy_static;

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
