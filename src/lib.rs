
use log::{Metadata, Level, Record, LevelFilter};

mod hive_macros;
pub mod property;
pub mod signal;
pub mod hive;
pub mod peer;
pub mod handler;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;


#[cfg(feature="bluetooth")]
#[macro_use]
extern crate lazy_static;

// INIT LOGGING
pub struct SimpleLogger;
impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            println!("{:?}:{:?} {:?} - {}", record.file().unwrap(), record.line().unwrap(), record.level(), record.args());
            // println!("{:?} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}
pub static LOGGER: SimpleLogger = SimpleLogger;
pub fn init_logging(){
    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace)).expect("failed to init logger");
}

// use futures::channel::mpsc::{UnboundedSender};
// use futures::Future;
// use futures::{SinkExt, AsyncReadExt, AsyncWriteExt};
use async_std::{
    prelude::*,
};




// futures_io::AsyncWrite

// pub trait HiveSocket {
//     // fn read(bytes:&[u8]){}
//     fn do_write(& self, bytes:&[u8]) {}
//     fn do_read(& self, bytes:&[u8]){}
// }
//
// impl HiveSocket for async_std::net::TcpStream {
//     fn do_write(& self, bytes:&[u8]) {
//         println!("writing");
//         async_std::task::block_on(async{
//             let mut  s = self;
//             s.write(bytes).await;
//         });
//     }
// }


// fn get_toml_config(file_path: &str) -> toml::Value{
//     let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
//     return toml::from_str(&foo).unwrap();
// }

