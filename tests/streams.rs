#![allow(unused_imports)]

use std::io::{Error, ErrorKind};
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{io, SinkExt, StreamExt};
use hive::property::{Property, PropertyValue};
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use async_std::task::block_on;
use std::thread::sleep;
use hive::init_logging;
use log::{debug, info, error, LevelFilter};

use std::sync::{Condvar, Mutex};
use simple_signal::Signal;
use std::ops::Index;
use anyhow::anyhow;
use tokio::time::{self, Duration};
use futures::{FutureExt, select};

#[derive(Debug)]
enum Control {
    Stop,
    Pause,
    Continue,
}
async fn primary_task(mut value: i32) -> io::Result<i32> {
    println!("Primary task started with value: {}", value);

    for _ in 0..8 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        value += 1;
        // println!("Tick... value = {}", value);
    }

    println!("Primary task finished with value: {}", value);
    Ok(value)
}
#[tokio::test]
async fn test_thing() -> io::Result<()> {
    let val = test_thing_main(10).await;
    println!("Done: val = {:?}", val);
    Ok(())
}
async fn test_thing_main(start_val: i32) -> io::Result<i32> {
    let (tx, mut rx) = mpsc::channel::<Control>(10);
    // Spawn a task to send a Stop message after a short delay
    tokio::spawn({
        let mut tx = tx.clone();
        async move {

            time::sleep(Duration::from_secs(2)).await;
            println!("Sending Continue command");
            tx.send(Control::Continue).await.unwrap();

            time::sleep(Duration::from_secs(2)).await;
            println!("Sending Continue command");
            tx.send(Control::Continue).await.unwrap();

            time::sleep(Duration::from_secs(2)).await;
            println!("Sending stop command");
            tx.send(Control::Stop).await.unwrap();
            //
            // time::sleep(Duration::from_secs(2)).await;
            // println!("Sending Stop command");
            // tx.send(Control::Stop).await.unwrap();
        }
    });
    let mut handle = tokio::spawn(primary_task(start_val));
    tokio::pin!(handle);
    let final_result;
    loop {
        println!("looping");
        tokio::select! {
            result = &mut handle => {
                match result {
                    Ok(Ok(val)) => {
                        println!("Primary task completed.");
                        final_result = val;
                    }
                   Ok(Err(e)) => {
                        println!("Primary task returned an error: {}", e);
                        return Err(e);
                    }
                    Err(join_err) => {
                        println!("Primary task panicked or was aborted: {}", join_err);
                        return Err(Error::new(ErrorKind::Interrupted, join_err));
                    }
                }
                break;
            }

            msg = rx.next() => {
                match msg {
                    Some(Control::Stop) => {
                        println!("Received Stop command.");
                        handle.abort(); // Abort the running task
                        let _ = handle.await; // Ensure cleanup
                        return Ok(start_val);
                    }
                    Some(other) => {
                        println!("Received other control command: {:?}", other);
                    }
                    None => {
                        println!("Control channel closed.");
                         return Ok(start_val);
                    }
                }
            }
        }
    }

    println!("Exited.");
    assert!(true);
    Ok((final_result))
}

#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
#[test]
fn main()-> Result<(), Box<dyn std::error::Error>> {
    println!("one<<<<");
    init_logging(Some(LevelFilter::Debug));

    println!("one<<<<");

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_1 = counter.clone();
    let counter_2 = counter.clone();
    let counter_3 = counter.clone();
    let counter_4 = counter.clone();
    let counter_5 = counter.clone();
    let counter_6 = counter.clone();
    let counter_7 = counter.clone();
    let counter_8 = counter.clone();

    let thing_value = Arc::new(AtomicUsize::new(0));
    let thing_value_1 = thing_value.clone();
    // let count6 = counter.clone();

    let ack: Arc<(Mutex<String>, Condvar)> = Arc::new((Mutex::new("".into()), Condvar::new()));

    let props_str = r#"
    listen = "127.0.0.1:3000"
    name = "Server"
    [bluetooth]
    btname = "Hive"
    [Properties]
    thingvalue= 1
    is_active = true
    lightValue = 0
    thermostatName = "orig therm name"
    thermostatTemperature= "too cold"
    thermostatTarget_temp = 1.45
    blahh = 1000
    longNum = -1003987654
    "#;
    let mut server_hive = Hive::new_from_str(props_str);
    let prop = server_hive.get_mut_property_by_name("thermostatName").unwrap();

    let v = prop.value.read().unwrap().as_ref().unwrap().to_string();
    let matches = v.eq("\"orig therm name\"");
    info!("maths: {}",matches);
    assert_eq!(v, "\"orig therm name\"".to_string());
    let ack_clone = ack.clone();

    let messages_received = Arc::new(Mutex::new(0));
    let mr_clone = messages_received.clone();
    let mr_clone2 = messages_received.clone();
    let mut ff = server_hive.get_mut_property_by_name("thermostatName", ).unwrap().stream.clone();
    async_std::task::spawn(async move {
        while let Some(x) = ff.next().await {
            info!("+++++++++++++++++++++ SERV|| THERMOSTAT NAME CHANGED: {:?}", x);
            counter_1.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_clone;
            let mut ack = lock.lock().unwrap();
            *ack = x.to_string();
            *mr_clone.lock().unwrap() += 1;
            cvar.notify_one();
        }
    });


    let ack_clone = ack.clone();
    server_hive.message_received.connect(move |message|{
        info!("+++++++++++++++++++++  server MESSAGE {}", message);
        counter_4.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*ack_clone;
        let mut ack = lock.lock().unwrap();
        *ack = message.clone();
        cvar.notify_one();
    });


    let server_connected = server_hive.connected.clone();
    let mut server_hand = server_hive.go(true);

    let ack_clone = ack.clone();
    let mut client_hive = Hive::new_from_str("connect = \"127.0.0.1:3000\"\nname=\"client1\"");

    let client_thermostat_name_property =  client_hive.get_mut_property_by_name("thermostatName").unwrap();
    let mut client_therm_prop_clone = client_thermostat_name_property.clone();
    let mut stream = client_thermostat_name_property.stream.clone();
    async_std::task::spawn(async move {
        let mut count = 1;
        while let Some(x) = stream.next().await {
            info!("+++++++++++++++++++++ CLIENT THERMOSTAT NAME CHANGED: {:?}", x);
            if count == 1 {
                assert_eq!(x, PropertyValue::from("orig therm name"));
                count += 1;
            } else if count == 2 {
                assert_eq!(x, PropertyValue::from("Before"));
                count += 1;
            } else if count == 3 {
                panic!("verify that the property has been deleted");
            }
            counter_3.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_clone;
            let mut ack = lock.lock().unwrap();
            *ack = x.to_string();
            *mr_clone2.lock().unwrap() += 1;
            cvar.notify_one();

        }
    });

    let ack_clone = ack.clone();
    client_hive.message_received.connect(move |message| {
        info!("+++++++++++++++++++++ client MESSAGE {}", message);
        counter_2.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*ack_clone;
        let mut ack = lock.lock().unwrap();
        *ack = message;
        cvar.notify_one();
    });

    let ack_clone = ack.clone();
    // client_hive.get_mut_property("thingvalue").unwrap().on_changed.connect(move |value|{
    client_hive.get_mut_property_by_name("thingvalue").unwrap().on_next( move |value|{
        info!(" +++++++++++++++++++++ CLIENT thing value::::::::::::::::::::: {:?}", value);
        let (lock, cvar) = &*ack_clone;
        let t2 = value.val.as_integer().unwrap() as usize;
        thing_value_1.store(t2, Ordering::Relaxed);
        let mut ack = lock.lock().unwrap();
        *ack = value.to_string();
        cvar.notify_one();
    });

    client_hive.get_mut_property_by_name("thermostatTarget_temp").unwrap().on_next(move |value|{
        assert_eq!(value.val.as_float().unwrap(), 1.45);
        counter_6.fetch_add(1, Ordering::Relaxed);
    });

    client_hive.get_mut_property_by_name("blahh").unwrap().on_next(move |value|{
        assert_eq!(value.val.as_integer().unwrap(), 1000);
    });

    client_hive.get_mut_property_by_name("is_active").unwrap().on_next(move |value|{
        assert_eq!(value.val.as_bool().unwrap(), true);
        counter_7.fetch_add(1, Ordering::Relaxed);
    });
    client_hive.get_mut_property_by_name("longNum").unwrap().on_next(move |value|{
        assert_eq!(value.val.as_integer().unwrap(), -1003987654);
        counter_8.fetch_add(1, Ordering::Relaxed);
    });


    let mut client_hand = client_hive.go(true);

    let ack_clone = ack.clone();
    simple_signal::set_handler(&[Signal::Int, Signal::Term], {

        move |_| {
            info!("...... kill signal received stop Condvar mutex");
            let (lock, cvar) = &*ack_clone;
            cvar.notify_all();
        }

    });


    // let mut client_hive_2 = Hive::new_from_str("connect = \"127.0.0.1:3000\"\nname=\"client2\"");
    // let mut client_2_handler = client_hive_2.go(true);

    // let mut client_handl_clone = client_hand.clone();

    let (mut sender, mut receiver) = mpsc::unbounded();
    task::spawn(async move {

        //+2 on prop initialization
        let (lock, cvar) = &*ack;

        server_hand.send_to_peer("client1", "hey you").await; //+ 1
        {
            let mut got = lock.lock().unwrap();
            while &*got != "hey you"{
                info!(":::::::::::::::::::: Wait for hey you: {:?}", got);
                got = cvar.wait(got).unwrap();
                info!("                ACK hey you: {:?}", got);
            }
        }
            client_hand.send_to_peer("Server", "hey mr man").await; // + 1
        {
            let mut got = lock.lock().unwrap();
            while &*got != "hey mr man" {
                info!(":::::::::::::::::::: Wait for hey mr man");
                got = cvar.wait(got).unwrap();
                info!("                ACK mr man: {:?}", got);
            }
        }

        info!("SENT thermostatName = Before");
        client_hand.set_property("thermostatName", Some(&"Before".into())).await; // +2
        {
            let mut done = lock.lock().unwrap();
            while *messages_received.lock().unwrap() <= 2 {
                info!(":::::::::::::::::::: Wait for thermostat name update {:?}, {:?}",done, messages_received);
                done = cvar.wait(done).unwrap();
            }
            info!("                ACK thermostatName: {:?}", done);
        }
        info!("DELETE thermostatName");

        server_hand.delete_property("thermostatName").await;
        server_hand.set_property("thingvalue", Some(&10.into())).await; // +1
        {
            let mut done = lock.lock().unwrap();
            while &*done != "10" {
                info!("Wait for thingvalue update to 10");
                done = cvar.wait(done).unwrap();
            }
        }

        client_hand.hangup();

        // These should not be counted, because we deleted the ThermostatName
        server_hand.set_property("thermostatName", Some(&"After".into())).await;

        sender.send(1 as i32).await;

    });

    let done = block_on(receiver.next());

    let c6 = thing_value.load(Ordering::Relaxed);
    assert_eq!(c6, 10);

    let c = counter.load(Ordering::Relaxed);
    assert_eq!(c, 8);

    info!("done with stuff");

    Ok(())

}

