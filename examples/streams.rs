
// use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;

use futures::{SinkExt, StreamExt};
use hive::property::Property;

#[allow(unused_must_use)]
fn main() {

    // let (tx, rx) = spmc::channel();// ch::unbounded();
    // let mut txc = tx.clone();
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
    let mut server_hive = Hive::new_from_str("SERVE", props_str);
    // let server_p = server_hive.get_mut_property("thermostatTarget_temp").unwrap();
    // println!("PROPERTIES 1 {:?}", &server_hive.properties);
    let mut server_hand = server_hive.get_handler();
    task::spawn(  async move{
        server_hive.run().await;
    });

    // let mut txc = tx.clone();
    let mut client_hive = Hive::new_from_str("CLI", "connect = \"127.0.0.1:3000\"");
    let client_p = client_hive.get_mut_property("thermostatTarget_temp").unwrap();
    client_p.on_changed.connect(|value|{
        println!("|||| <<<< |||| target_temp: {:?}", value);
    });


    server_hand.send_property("THE OTHER THING");
    task::block_on(client_hive.run());






}
