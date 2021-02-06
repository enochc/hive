#![allow(unused_imports)]
// use async_std::prelude::*;
use futures::channel::{mpsc, mpsc::UnboundedSender, mpsc::UnboundedReceiver};
use hive::hive::Hive;
use async_std::task;
use futures::{SinkExt, StreamExt};
use hive::property::{Property, PropertyType};
// use async_std::task::sleep;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use async_std::task::block_on;
use std::thread::sleep;
use std::time::Duration;
use hive::init_logging;
use log::{debug, info, error, LevelFilter};

use std::sync::{Condvar, Mutex};
use simple_signal::Signal;
// use async_std::sync::{Condvar, Mutex};
// use parking_lot::{Condvar, Mutex};



#[allow(unused_must_use, unused_variables, unused_mut, unused_imports)]
#[test]
fn main() {
    init_logging(Some(LevelFilter::Debug));
    let counter = Arc::new(AtomicUsize::new(0));
    let count1 = counter.clone();
    let count2 = counter.clone();
    let count3 = counter.clone();
    let count4 = counter.clone();
    let count5 = counter.clone();
    let count6 = counter.clone();

    let ack: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(false), Condvar::new()));

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
    "#;
    let mut server_hive = Hive::new_from_str(props_str);
    let prop = server_hive.get_mut_property("thermostatName").unwrap();

    let ack_clone = ack.clone();
    server_hive.get_mut_property("thermostatName", ).unwrap().on_changed.connect(move |value| {
        debug!("+++++++++++++++++++++ SERV|| THERMOSTAT NAME CHANGED: {:?}", value);
        count1.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*ack_clone;
        cvar.notify_one();
    });

    let ack_clone = ack.clone();
    server_hive.message_received.connect(move |message|{
        debug!("+++++++++++++++++++++  server MESSAGE {}", message);
        count4.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*ack_clone;
        let mut ack = lock.lock().unwrap();
        *ack = true;
        cvar.notify_one();
    });


    let server_connected = server_hive.connected.clone();
    let mut server_hand = server_hive.go(true);

    let ack_clone = ack.clone();
    let mut client_hive = Hive::new_from_str("connect = \"127.0.0.1:3000\"\nname=\"client1\"");
    client_hive.get_mut_property("thermostatName").unwrap().on_changed.connect(move |value| {
        debug!("+++++++++++++++++++++ CLIENT THERMOSTAT NAME CHANGED: {:?}", value);
        count3.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*ack_clone;
        cvar.notify_one();
    });

    let ack_clone = ack.clone();
    client_hive.message_received.connect(move |message| {
        debug!("+++++++++++++++++++++ client MESSAGE {}", message);
        count2.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*ack_clone;
        cvar.notify_one();
    });

    let ack_clone = ack.clone();
    client_hive.get_mut_property("thingvalue").unwrap().on_changed.connect(move |value|{
        debug!(" +++++++++++++++++++++ CLIENT thing value::::::::::::::::::::: {:?}", value);
        let (lock, cvar) = &*ack_clone;
        // let t1 = value.unwrap().as_integer().unwrap();
        let t2 = value.unwrap().as_integer().unwrap() as usize;
        count5.store(t2, Ordering::Relaxed);
        cvar.notify_one();
    });

    let mut client_hand = client_hive.go(true);

    let ack_clone = ack.clone();
    simple_signal::set_handler(&[Signal::Int, Signal::Term], {

        move |_| {
            debug!("...... kill signal received stop Condvar mutex");
            let (lock, cvar) = &*ack_clone;
            cvar.notify_all();
        }

    });


    let mut client_hive_2 = Hive::new_from_str("connect = \"127.0.0.1:3000\"\nname=\"client2\"");
    let mut client_2_handler = client_hive_2.go(true);


    let mut client_handl_clone = client_hand.clone();

    let (mut sender, mut receiver) = mpsc::unbounded();
    task::spawn(async move {

        if server_connected.load(Ordering::Relaxed) {  //+2 on prop initialization
            let (lock, cvar) = &*ack;
            server_hand.send_to_peer("client1", "hey you").await; //+ 1
            {
                let kk = lock.lock().unwrap();
                debug!("!!!!!!!! Waiting for Ack");
                cvar.wait(kk).unwrap();
                debug!("                ACK: {:?}", ack);
            }
            client_handl_clone.send_to_peer("Server", "hey mr man").await; // + 1
            {
                let kk = lock.lock().unwrap();
                debug!("!!!!!!!! Waiting for Ack");
                cvar.wait(kk).unwrap();
                debug!("                ACK2: {:?}", ack);
            }
            debug!("SENT thermostatName = Before");
            count6.store(0, Ordering::Relaxed);
            client_handl_clone.send_property_value("thermostatName", Some(&"Before".into())).await; // +2
            {
                let mut done = false;
                while !done {
                    debug!("NOT DONE!!");
                    let kk = lock.lock().unwrap();
                    cvar.wait(kk).unwrap();
                    if count6.load(Ordering::Relaxed) == 2 {
                        done = true;
                    }
                }
                debug!("                ACK3: {:?}", ack);
            }
            debug!("DELETE thermostatName");
            //TODO there is curently no validation for the delete method
            server_hand.delete_property("thermostatName").await;
            server_hand.send_property_value("thingvalue", Some(&10.into())).await; // +1
            {
                let kk = lock.lock().unwrap();
                debug!("!!!!!!!! Waiting for Ack");
                cvar.wait(kk).unwrap();
                assert_eq!(count6.load(Ordering::Relaxed), 10);
                debug!("                ACK4: {:?}", ack);
            }

            // These should not be counted, because we deleted the ThermostatName
            // server_hand.send_property_value("thermostatName", Some(&"After".into())).await;

        } else {
            debug!("server is not connected");
        }
        sender.send(1 as i32).await;

    });

    let done = block_on(receiver.next());
    // assert_eq!(counter.load(Ordering::Relaxed), 7);
    client_hand.hangup();

    debug!("done with stuff");

}

