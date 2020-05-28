
// use std::sync::mpsc::{Sender, Receiver};
// use std::sync::mpsc;
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use std::thread::sleep;
use failure::_core::time::Duration;
use std::fs;
use hive::hive::Hive;
use async_std::task;
use async_std::task::JoinHandle;

use async_std::prelude::*;
use futures::{SinkExt, StreamExt};

fn main() {

    // let foo: String = fs::read_to_string("examples/listen_3000.toml").unwrap().parse().unwrap();
    // let config: toml::Value = toml::from_str(&foo).unwrap();
    let (tx, mut rx): (UnboundedSender<i32>, UnboundedReceiver<i32>) = mpsc::unbounded();
    let mut txc = tx.clone();
    task::spawn(  async move{
        let h = Hive::new("examples/listen_3000.toml").run().await;
        // h.run().await;
        txc.send(1);
    });

    // sleep for a sec so listen hive is running
    // sleep(Duration::from_secs(1));
    let mut txc = tx.clone();
    task::spawn(async move {
        let mut h = Hive::new("examples/connect_3000.toml").run().await;
       // h.run().await;
        txc.send(2);
    });

    let mut x = 0;
    // while x != 1 {
    //     x = rx.recv().unwrap();
    //     println!("Process Ran: {:?}", x);
    // }
    async fn doit(mut receiver: UnboundedReceiver<i32>) {
        while let Some(msg) = receiver.next().await {
            println!("Process Ran: {}", msg);
        }
    };
    task::block_on(doit(rx));
    println!("Process Done");



}
