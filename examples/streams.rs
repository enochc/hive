
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

    let (tx, mut rx): (UnboundedSender<i32>, UnboundedReceiver<i32>) = mpsc::unbounded();
    let mut txc = tx.clone();
    task::spawn(  async move{
        let h = Hive::new("examples/listen_3000.toml").run().await;
        txc.send(1);
    });

    let mut txc = tx.clone();
    task::spawn(async move {
        let mut h = Hive::new("examples/connect_3000.toml").run().await;
        txc.send(2);
    });


    async fn doit(mut receiver: UnboundedReceiver<i32>) {
        while let Some(msg) = receiver.next().await {
            println!("<<<<<<<<<<<<<<<<<<<<  Process Ran: {}", msg);
        }
    };
    task::block_on(doit(rx));
    println!("Process Done");

}
