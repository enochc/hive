
use std::sync::mpsc::{Sender, Receiver};
use std::sync::mpsc;
use std::thread::sleep;
use failure::_core::time::Duration;
use std::fs;
use hive::hive::Hive;
use async_std::task;
use async_std::task::JoinHandle;

fn main() {

    // let foo: String = fs::read_to_string("examples/listen_3000.toml").unwrap().parse().unwrap();
    // let config: toml::Value = toml::from_str(&foo).unwrap();
    let (tx, rx): (Sender<i32>, Receiver<i32>) = mpsc::channel();
    let tc = tx.clone();
    task::spawn(  async move{
        let h = Hive::new("examples/listen_3000.toml").run().await;
        // h.run().await;
        tc.send(1);
    });




    // sleep for a sec so listen hive is running
    // sleep(Duration::from_secs(1));
    let tx = tx.clone();
    task::spawn(async move {
        let mut h = Hive::new("examples/connect_3000.toml");
       // h.run().await;
        tx.send(2);
    });

    let mut x = 0;
    while x != 1 {
        x = rx.recv().unwrap();
        println!("Process Ran: {:?}", x);
    }
    println!("Process Done");



}
