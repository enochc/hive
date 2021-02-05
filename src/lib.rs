
#![warn(rust_2018_idioms)]

use log::{Metadata, Level, Record, LevelFilter};

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

// INIT LOGGING

pub struct SimpleLogger;
impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            println!("{:?}:{:?} {:?} - {}", record.file().unwrap(), record.line().unwrap(), record.level(), record.args());
            // println!("{:?} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}
pub static LOGGER: SimpleLogger = SimpleLogger;

pub fn init_logging(level:Option<LevelFilter>){
    let myLevel = match level {
        Some(l) => l,
        None => LevelFilter::Trace
    };
    log::set_logger(&LOGGER).map(|()| log::set_max_level(myLevel)).expect("failed to init logger");
}
