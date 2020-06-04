
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;

use futures::{SinkExt, StreamExt};

#[allow(unused_must_use)]
fn main() {

    let (tx, rx): (UnboundedSender<i32>, UnboundedReceiver<i32>) = mpsc::unbounded();
    let mut txc = tx.clone();
    let mut hive = Hive::new("examples/listen_3000.toml");
    println!("PROPERTIES 1 {:?}", hive.properties);
    task::spawn(  async move{
        hive.run().await;
        txc.send(1).await;
    });

    let mut txc = tx.clone();
    task::spawn(async move {
        let mut hive = Hive::new("examples/connect_3000.toml");
        hive.run().await;
        println!("PROPERTIES 2 {:?}", hive.properties);
        txc.send(2).await;
    });


    async fn doit(mut receiver: UnboundedReceiver<i32>) {
        while let Some(msg) = receiver.next().await {
            println!("<<<<<<<<<<<<<<<<<<<<  Process Ran: {}", msg);
        }
    };
    task::block_on(doit(rx));
    println!("Process Done");

}
