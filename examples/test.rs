#![allow(unused_imports)]
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
    sync::Arc,
};
use futures::executor::block_on;
use std::thread::sleep;
use hive::hive::Hive;
use std::time::Duration;
use hive::init_logging;
use std::sync::atomic::Ordering;

macro_rules! peer_by_address {
    ($addr:tt, $close:tt) => {
        println!("{:?}", $addr);
        $close(5)
    };
}

fn main(){
    init_logging();
    peer_by_address!("what", (|x| {println!("blahh: {:?}",x)}));
    // let props_str = r#"
    // connect = "127.0.0.1:3000"
    // [Properties]
    // thingvalue= 1
    // is_active = true
    // lightValue = 0
    // thermostatName = "orig therm name"
    // thermostatTemperature= "too cold"
    // thermostatTarget_temp = 1.45
    // "#;
    // let mut server_hive = Hive::new_from_str("SERVE", props_str);
    // // let handle = server_hive.get_handler();
    // let connected = server_hive.connected.clone();
    // task::Builder::new().name("SERVER HIVE".to_string()).spawn(async move {
    //     server_hive.run().await;
    // });
    // loop {
    //     println!("---- {:?}", connected.load(Ordering::Relaxed));
    //     sleep(Duration::from_secs(1));
    // }
}

