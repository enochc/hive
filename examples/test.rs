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

macro_rules! peer_by_address {
    ($addr:tt, $close:tt) => {
        println!("{:?}", $addr);
        $close(5)
    };
}

fn main(){
    peer_by_address!("what", (|x| {println!("blahh: {:?}",x)}));
}

