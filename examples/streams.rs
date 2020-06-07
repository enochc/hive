
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;

use futures::{SinkExt, StreamExt};

#[allow(unused_must_use)]
fn main() {

    let (tx, rx): (UnboundedSender<i32>, UnboundedReceiver<i32>) = mpsc::unbounded();
    let mut txc = tx.clone();
    let props_str = r#"
    listen = "127.0.0.1:3000"
    [Properties]
    thingvalue= 1
    is_active = true
    lightValue = 0
    thermostatName = "thermostat"
    thermostatTemperature= "too cold"
    thermostatTarget_temp = 1.45
    "#;
    print!("{:?}",props_str);

    let mut hive = Hive::newFromStr("SERVE", props_str);
    println!("PROPERTIES 1 {:?}", hive.properties);
    task::spawn(  async move{
        hive.run().await;
        txc.send(1).await;
    });

    let mut txc = tx.clone();
    task::spawn(async move {
        let mut hive = Hive::newFromStr("CLI", "connect = \"127.0.0.1:3000\"");
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
