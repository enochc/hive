use hive::hive::Hive;
use std::sync::atomic::{Ordering, AtomicUsize};
#[allow(unused_imports)]
use log::{info, debug};
use async_std::task;
use std::time::Duration;
use std::thread;
use hive::init_logging;
use futures::executor::block_on;
use async_std::sync::Arc;
use log::{LevelFilter};

// TODO somethimes this test works, sometimes it doesn't under identical circumstances!!!!!
// Sometimes the Client receives a response on the initial thing value of 1, sometimes not
// It doesn't really break stuff, but it's annoying as HELL!!!!!
#[test]
fn main(){
    init_logging(Some(LevelFilter::Info));

    let counter = Arc::new(AtomicUsize::new(0));
    let counter1 = counter.clone();
    let counter2 = counter.clone();

    let props_str = r#"
    listen="3000"
    name = "Server"
    [Properties]
    thing=1
    "#;
    let mut server_hive = Hive::new_from_str(props_str);
    server_hive.get_mut_property("thing", ).unwrap().on_changed.connect(move |value| {
        info!("SERVER ----------------------- thing changed: {:?}", value);
        counter1.fetch_add(1, Ordering::SeqCst);
    });

    // let mut server_hand = server_hive.get_handler();
    server_hive.go(true);


    let props_str = r#"
    name = "MiddleMan"
    connect="3000"
    listen="3001"
    "#;
    let mut middle_man = Hive::new_from_str(props_str);
    let mut middle_hand = middle_man.go(true);

    let props_str = r#"
    connect="3001"
    name = "Client"
    thing=1
    "#;
    let mut client_hive = Hive::new_from_str(props_str);
    client_hive.get_mut_property("thing", ).unwrap().on_changed.connect(move |value| {
        info!("CLIENT!! --------------------- thing changed: {:?}", value);
        // this gets changed on initial connection when the property first sinks
        counter2.fetch_add(1, Ordering::SeqCst);
    });

    let mut client_hand = client_hive.go(true);

    block_on(async{
        middle_hand.send_property_value("thing", Some(&4.into())).await;
        thread::sleep(Duration::from_millis(100));
        // assert_eq!(counter.load(Ordering::Relaxed), 3);
        client_hand.send_property_value("thing",Some(&5.into())).await;
        thread::sleep(Duration::from_millis(100));
        // assert_eq!(counter.load(Ordering::Relaxed), 5);

    });

}