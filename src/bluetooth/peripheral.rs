
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, atomic, Mutex};
use std::time::Duration;

// use btleplug::api::{UUID, Central, CentralEvent, BDAddr, AdapterManager, Peripheral};
use bluster::{
    gatt::{
        characteristic,
        characteristic::Characteristic,
        descriptor,
        descriptor::Descriptor,
        event::{Event, Response},
        service::Service,
    },
    Peripheral as Peripheral_device, SdpShortUuid,
};
// use bluster::gatt::event::Event::NotifySubscribe;
use bluster::gatt::event::NotifySubscribe;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{channel::mpsc::channel, prelude::*};
use futures::channel::mpsc;
use log::{ info};
use uuid::Uuid;

use crate::bluetooth::{HIVE_CHAR_ID, HIVE_DESC_ID, HiveMessage, SERVICE_ID};
use crate::bluetooth::my_blurz::set_discoverable;
use crate::hive::{Receiver, Sender};
use crate::peer::SocketEvent;

#[derive(Clone)]
pub struct Peripheral {
    // pub peripheral: Peripheral_device,
    // sender: Sender<Bytes>,
    // receiver: Arc<Receiver<Bytes>>,
    ble_name: String,
    event_sender: Sender<SocketEvent>,

}

impl Debug for Peripheral {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Peripheral")
            .field("name", &self.ble_name)
            .finish()
    }
}


impl Peripheral {
    pub async fn new(ble_name: &str, event_sender: Sender<SocketEvent>) -> Peripheral {

        // let peripheral = Peripheral_device::new().await.expect("Failed to initialize peripheral");
        let name = String::from(ble_name);

        return Peripheral { ble_name: name, event_sender }
    }

    async fn get_peripheral() -> Peripheral_device {
        return Peripheral_device::new().await.expect("Failed to initialize peripheral");
    }
    pub async fn process(mut bytes: BytesMut, mut event_sender: &Sender<SocketEvent>, perf_sender: Sender<Bytes>) -> Result<(), Box<dyn std::error::Error>> {
        // let mut sender = self.event_sender.clone();
        match bytes.get_u16() {
            HiveMessage::CONNECTED => {
                let msg = String::from_utf8(bytes.to_vec()).unwrap();
                println!("<<<<<<<<< CONNECTED: {:?}", msg);
                // let ss = msg.split(",").collect().unwrap();
                let vec: Vec<&str> = msg.split(",").collect();
                let event = SocketEvent::NewPeer {
                    name: vec.get(0).unwrap().to_string(),
                    stream: None,
                    peripheral: Some(perf_sender),
                    central: None,
                    address: Some(vec.get(1).unwrap().to_string()),
                };
                event_sender.send(event).await?;
            },
            _ => { eprintln!("Unknown message received, failed to process") }
        }
        Ok(())
    }

    pub async fn run(&mut self, do_advertise: bool) -> Result<(), Box<dyn Error>> {
        let (sender_characteristic, receiver_characteristic) = channel(1);
        let (sender_descriptor, receiver_descriptor) = channel(1);

        let sender_characteristic_clone = sender_characteristic.clone();
        let sender_descriptor_clone = sender_descriptor.clone();

        let (bytes_tx, mut bytes_rx): (Sender<Bytes>, Receiver<Bytes>) = mpsc::unbounded();

        let subscriptions: Arc<Mutex<Vec<NotifySubscribe>>> = Arc::new(Mutex::new(vec![]));//Vec::new();
        let subs_clone = subscriptions.clone();

        async_std::task::spawn(async move {
            while let Some(bytes) = bytes_rx.next().await {
                let s = &*subs_clone.lock().unwrap();
                for (x, sub) in s.iter().enumerate() {
                    if sub.notification.is_closed() {
                        subs_clone.lock().unwrap().remove(x);
                    } else {
                        sub.clone()
                            .notification
                            .try_send(bytes.to_vec())
                            .unwrap();
                    }
                }
            }
        });


        let mut characteristics: HashSet<Characteristic> = HashSet::new();
        characteristics.insert(Characteristic::new(
            Uuid::from_sdp_short_uuid(HIVE_CHAR_ID),
            characteristic::Properties::new(
                Some(characteristic::Read(characteristic::Secure::Insecure(
                    sender_characteristic_clone.clone(),
                ))),
                Some(characteristic::Write::WithResponse(
                    characteristic::Secure::Insecure(sender_characteristic_clone.clone()),
                )),
                Some(sender_characteristic_clone),
                None,
            ),
            Some("Hive_Char".as_bytes().to_vec()),
            {
                let mut descriptors = HashSet::<Descriptor>::new();
                descriptors.insert(Descriptor::new(
                    Uuid::from_sdp_short_uuid(HIVE_DESC_ID),
                    descriptor::Properties::new(
                        Some(descriptor::Read(descriptor::Secure::Insecure(
                            sender_descriptor_clone.clone(),
                        ))),
                        Some(descriptor::Write(descriptor::Secure::Insecure(
                            sender_descriptor_clone,
                        ))),
                    ),
                    Some("Hive_Desc".as_bytes().to_vec()),
                ));
                descriptors
            },
        ));

        let sender_clone = bytes_tx.clone();
        let event_sender_clone = self.event_sender.clone();

        let characteristic_handler = async {
            let characteristic_value = Arc::new(Mutex::new(String::from("hi")));
            let notifying = Arc::new(atomic::AtomicBool::new(false));
            let mut rx = receiver_characteristic;
            while let Some(event) = rx.next().await {
                match event {
                    Event::ReadRequest(read_request) => {
                        info!(
                            "Characteristic got a read request with offset {}!",
                            read_request.offset
                        );
                        let value = characteristic_value.lock().unwrap().clone();
                        read_request
                            .response
                            .send(Response::Success(value.clone().into()))
                            .unwrap();
                        info!("Characteristic responded with \"{}\"", value);
                    }
                    Event::WriteRequest(write_request) => {
                        let mut bm = BytesMut::new();
                        bm.put_slice(&write_request.data);
                        info!(
                            "Characteristic got a write request with offset {} and data {:?}!",
                            write_request.offset, bm
                        );
                        //HiveMessage::process(bm);
                        Peripheral::process(
                            bm,
                            &event_sender_clone,
                            sender_clone.clone()).await.expect("Failed to precess message");

                        write_request
                            .response
                            .send(Response::Success(vec![]))
                            .unwrap();
                    }
                    Event::NotifySubscribe(notify_subscribe) => {
                        info!("Characteristic got a notify subscription!");
                        let notifying = Arc::clone(&notifying);
                        notifying.store(true, atomic::Ordering::Relaxed);
                        // let mut self_clone = self.clone();
                        let mut s = subscriptions.lock().unwrap();
                        s.push(notify_subscribe);
                        // subscriptions.push(notify_subscribe);


                        // loop {
                        // if !(&notifying).load(atomic::Ordering::Relaxed) {
                        //     break;
                        // };
                        // count += 1;
                        // debug!("Characteristic notifying \"hi {}\"!", count);
                        // notify_subscribe
                        //     .clone()
                        //     .notification
                        //     .try_send(format!("hi {}", count).into())
                        //     .unwrap();
                        // thread::sleep(Duration::from_secs(2));
                        // }
                        // });
                    }
                    Event::NotifyUnsubscribe => {
                        info!("Characteristic got a notify unsubscribe!");
                        notifying.store(false, atomic::Ordering::Relaxed);
                    }
                };
            }
        };

        let descriptor_handler = async {
            let descriptor_value = Arc::new(Mutex::new(String::from("hi")));
            let mut rx = receiver_descriptor;
            while let Some(event) = rx.next().await {
                match event {
                    Event::ReadRequest(read_request) => {
                        info!(
                            "Descriptor got a read request with offset {}!",
                            read_request.offset
                        );
                        let value = descriptor_value.lock().unwrap().clone();
                        read_request
                            .response
                            .send(Response::Success(value.clone().into()))
                            .unwrap();
                        info!("Descriptor responded with \"{}\"", value);
                    }
                    Event::WriteRequest(write_request) => {
                        let new_value = String::from_utf8(write_request.data).unwrap();
                        info!(
                            "Descriptor got a write request with offset {} and data {}!",
                            write_request.offset, new_value,
                        );
                        *descriptor_value.lock().unwrap() = new_value;
                        write_request
                            .response
                            .send(Response::Success(vec![]))
                            .unwrap();
                    }
                    _ => info!("Event not supported for Descriptors!"),
                };
            }
        };

        let peripheral = Peripheral::get_peripheral().await;

        peripheral.add_service(&Service::new(
            Uuid::from_sdp_short_uuid(SERVICE_ID),
            true,
            characteristics,
        )).unwrap();


        let self_clone = self.clone();
        let ble_name_clone = self_clone.ble_name.clone();
        let main_fut = async move {
            info!("ONE");

            let powered = peripheral.is_powered().await;
            info!(":::::: {:?}", powered);
            while !peripheral.is_powered().await.expect("Failed to check if powered") {}
            info!("Peripheral powered on");
            peripheral.register_gatt().await.unwrap();

            if do_advertise {
                set_discoverable(true).expect("Failed to set discoverable");
                peripheral.start_advertising(&ble_name_clone, &[]).await
                    .expect("Failed to start_advertising");
                while !peripheral.is_advertising().await.unwrap() {}
                info!("Peripheral started advertising");

                while !self_clone.event_sender.is_closed() {
                    tokio::time::delay_for(Duration::from_secs(1)).await;
                }

                peripheral.stop_advertising().await.unwrap();
                set_discoverable(false).expect("failed to stop being discovered");
                info!("Peripheral stopped advertising");
            }
        };

        let sender_characteristic_clone = sender_characteristic.clone();
        let mut sender_descriptor_clone = sender_descriptor.clone();
        let fut_stop = async {
            // we pretty much wait here for a long time
            while !self.event_sender.is_closed() {
                tokio::time::delay_for(Duration::from_secs(1)).await;
            }
            &sender_characteristic_clone.clone().close_channel();
            &sender_descriptor_clone.close_channel();
        };

        let fut = futures::future::join4(characteristic_handler, descriptor_handler, main_fut, fut_stop);
        fut.await;
        // thread::spawn(move ||{
        //     let fut = futures::future::join(characteristic_handler, descriptor_handler);
        //     block_on(fut);
        // });
        // main_fut.await;
        info!("<< Peripheral stopped!");
        Ok(())
    }
}
