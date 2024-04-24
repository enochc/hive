use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_std::task;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use futures::executor::block_on;
use futures::task::SpawnExt;
use log::{debug, error, info, LevelFilter, warn};
use slint::private_unstable_api::debug;

use hive::hive::Hive;
use hive::init_logging;
use hive::property::{Property, PropertyValue};

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
#[test]
fn main()-> Result<(), Box<dyn std::error::Error>> {
    init_logging(Some(LevelFilter::Debug));
    async_std::task::block_on(async {
        do1().await;
    });
    Ok(())
}

async fn do1(){
    let props_str = r#"
    listen = "127.0.0.1:3000"
    name = "Server"
    [Properties]
    thingvalue= 1
    "#;
    let mut server_hive = Hive::new_from_str(props_str);
    let mut prop: &mut Property = server_hive.get_mut_property_by_name("thingvalue").unwrap().with_backoff(&1_000);
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_2 = counter.clone();
    let mut stream = prop.stream.clone();
    let things = Arc::new((Mutex::new(true), Condvar::new()));
    let things2 = things.clone();
    async_std::task::spawn(async move {
        loop {
            let (x, y) = &*things2;
            y.notify_one();
             match stream.next().await {
                None => { warn!("noithin...")}
                Some(p) => {
                    error!("it here: {:?}", p);
                    counter_2.fetch_add(1, Ordering::Relaxed);
                }
            }
        };
    });

    let (x, y) = &*things;
    let lock = x.lock().unwrap();
    let _thing = y.wait(lock);
    prop.set_value(6.into());
    task::sleep(Duration::from_millis(100)).await;
    prop.set_value(7.into()); // this update is skipped because of the 1 second backoff
    task::sleep(Duration::from_millis(100)).await;
    prop.set_value(8.into());
    task::sleep(Duration::from_millis(100)).await;
    let pn = prop.value.read();

    let num = counter.load(Ordering::Relaxed);
    assert_eq!(1, num);
    debug!("<<<< {:?} {}", pn, counter.load(Ordering::Relaxed));
    task::sleep(Duration::from_millis(1_000)).await;

    let pn = prop.value.read();
    debug!("<<<< {:?} {}", pn, counter.load(Ordering::Relaxed));
    let num = counter.load(Ordering::Relaxed);
    assert_eq!(2, num);
}
