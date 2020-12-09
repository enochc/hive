// use std::fs;


// use std::os::raw::c_char;
// use std::ffi::{CStr};
use log::{Metadata, Level, Record, LevelFilter};

mod hive_macros;
pub mod property;
pub mod signal;
pub mod hive;
pub mod peer;
pub mod handler;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;




// INIT LOGGING
pub struct SimpleLogger;
impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // println!("{:?}{:?}, {:?} - {}", record.file(), record.line(), record.level(), record.args());
            println!("{:?} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}
pub static LOGGER: SimpleLogger = SimpleLogger;
pub fn init_logging(){
    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Debug)).expect("failed to init logger");
}



// fn get_toml_config(file_path: &str) -> toml::Value{
//     let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
//     return toml::from_str(&foo).unwrap();
// }

