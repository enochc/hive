
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// use btleplug::api::{UUID, Central, CentralEvent, BDAddr, AdapterManager, Peripheral};
use bluster::{
    gatt::{
        characteristic,
        characteristic::Characteristic,
        descriptor,
        descriptor::Descriptor,
        event::{Event, Response, NotifySubscribe},
        service::Service,
    },
    Peripheral as Peripheral_device, SdpShortUuid,
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{channel::mpsc::channel, prelude::*};
use futures::channel::mpsc;
use log::{ debug, info};
use uuid::Uuid;

use crate::bluetooth::{HIVE_CHAR_ID, HIVE_DESC_ID, HiveMessage, SERVICE_ID};
use crate::bluetooth::my_blurz::set_discoverable;
use crate::hive::{Receiver, Sender};
use crate::peer::SocketEvent;

// #[derive(Clone)]
pub struct Peripheral {
    // pub peripheral: Peripheral_device,
    // sender: Sender<Bytes>,
    // receiver: Arc<Receiver<Bytes>>,
    ble_name: String,
    event_sender: Sender<SocketEvent>,
    //TODO this feels silly, do a really need a mutex?
    address: Mutex<String>,

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

        return Peripheral { ble_name: name, event_sender, address:Mutex::new(String::new()) }
    }

    async fn get_peripheral() -> Peripheral_device {
        return Peripheral_device::new().await.expect("Failed to initialize peripheral");
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
            debug!("Starting notify loop");
            while let Some(bytes) = bytes_rx.next().await {
                debug!("notify: {:?}", bytes);
                let s = &*subs_clone.lock().unwrap();
                debug!("subs: {:?}", s.len());
                for (x, sub) in s.iter().enumerate() {
                    if sub.notification.is_closed() {
                        info!("<< Notification closed, removing notification..");
                        subs_clone.lock().unwrap().remove(x);
                    } else {
                        info!("<< Send notification: {:?}", bytes);
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
        let mut event_sender_clone = self.event_sender.clone();

        let characteristic_handler = async {
            let characteristic_value = Arc::new(Mutex::new(String::from("hi")));
            let mut rx = receiver_characteristic;
            while let Some(event) = rx.next().await {
                match event {
                    Event::ReadRequest(read_request) => {
                        info!(
                            "Characteristic got a read request with offset {}!",
                            read_request.offset
                        );
                        let value = characteristic_value.lock().unwrap().clone();
                        let mut bmut = BytesMut::new();
                        let conn = HiveMessage::CONNECTED;
                        bmut.put_u16(conn);
                        read_request
                            .response
                            .send(Response::Success(bmut.to_vec()))
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

                        match bm.get_u16() {
                            HiveMessage::CONNECTED => {
                                let msg = String::from_utf8(bm.to_vec()).unwrap();
                                println!("<<<<<<<<< CONNECTED: {:?}", msg);
                                if msg.len() <=0 {info!("no name or address")} else {
                                    let vec: Vec<&str> = msg.split(",").collect();
                                    let a = vec.get(1).unwrap().to_string();
                                    let mut addr = self.address.lock().unwrap();//.clone();
                                    *addr = a.clone();
                                    let event = SocketEvent::NewPeer {
                                        name: vec.get(0).unwrap().to_string(),
                                        stream: None,
                                        peripheral: Some(sender_clone.clone()),
                                        central: None,
                                        address: a,
                                    };
                                    &event_sender_clone.send(event).await.expect("Failed to send event");
                                }

                            },
                            _ => { eprintln!("Unknown message received, failed to process") }
                        }

                        write_request
                            .response
                            .send(Response::Success(vec![]))
                            .unwrap();
                    }
                    Event::NotifySubscribe(notify_subscribe) => {
                        info!("Characteristic got a notify subscription!");
                        subscriptions.lock().unwrap().push(notify_subscribe);

                    }
                    Event::NotifyUnsubscribe => {
                        info!("Characteristic got a notify unsubscribe!");
                    }
                };
            }
        };

        let mut event_sender_clone = self.event_sender.clone();

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
                        // *descriptor_value.lock().unwrap() = new_value;
                        let adr = &*self.address.lock().unwrap();
                        let se = SocketEvent::Message {
                            from: adr.clone(),
                            msg: new_value
                        };
                        event_sender_clone.send(se).await.expect("Failed to send event");

                        write_request
                            .response
                            .send(Response::Success(vec![]))
                            .expect("Failed to send response to descriptor write");
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


        let ble_name_clone = self.ble_name.clone();
        let event_sender_clone = self.event_sender.clone();
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

                while !event_sender_clone.is_closed() {
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
