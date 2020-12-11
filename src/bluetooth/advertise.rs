use log::{debug,info};
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
use futures::{channel::mpsc::channel, prelude::*};
use std::sync::{Arc, atomic, Mutex};
use std::collections::HashSet;
use std::thread;
use std::time::Duration;
use uuid::Uuid;
use crate::bluetooth::my_blurz::set_discoverable;
use crate::bluetooth::{ADVERTISING_NAME, SERVICE_ID};
use std::sync::atomic::AtomicBool;

use std::error::Error;

pub struct Peripheral{
    peripheral: Peripheral_device
}

impl Peripheral {

    pub async fn new()->Peripheral {
        let peripheral = Peripheral_device::new().await.expect("Failed to initialize peripheral");
        return Peripheral{peripheral}
    }

    pub async fn run(&self, listening:Arc<AtomicBool>) -> Result<(), Box<dyn Error>> {
        let (sender_characteristic, receiver_characteristic) = channel(1);
        let (sender_descriptor, receiver_descriptor) = channel(1);

        let mut characteristics: HashSet<Characteristic> = HashSet::new();
        characteristics.insert(Characteristic::new(
            Uuid::from_sdp_short_uuid(0x2A3D as u16),
            characteristic::Properties::new(
                Some(characteristic::Read(characteristic::Secure::Insecure(
                    sender_characteristic.clone(),
                ))),
                Some(characteristic::Write::WithResponse(
                    characteristic::Secure::Insecure(sender_characteristic.clone()),
                )),
                Some(sender_characteristic),
                None,
            ),
            None,
            {
                let mut descriptors = HashSet::<Descriptor>::new();
                descriptors.insert(Descriptor::new(
                    Uuid::from_sdp_short_uuid(0x2A3D as u16),
                    descriptor::Properties::new(
                        Some(descriptor::Read(descriptor::Secure::Insecure(
                            sender_descriptor.clone(),
                        ))),
                        Some(descriptor::Write(descriptor::Secure::Insecure(
                            sender_descriptor,
                        ))),
                    ),
                    None,
                ));
                descriptors
            },
        ));

        let characteristic_handler = async {
            let characteristic_value = Arc::new(Mutex::new(String::from("hi")));
            let notifying = Arc::new(atomic::AtomicBool::new(false));
            let mut rx = receiver_characteristic;
            while let Some(event) = rx.next().await {
                match event {
                    Event::ReadRequest(read_request) => {
                        info!(
                            "GATT server got a read request with offset {}!",
                            read_request.offset
                        );
                        let value = characteristic_value.lock().unwrap().clone();
                        read_request
                            .response
                            .send(Response::Success(value.clone().into()))
                            .unwrap();
                        info!("GATT server responded with \"{}\"", value);
                    }
                    Event::WriteRequest(write_request) => {
                        let new_value = String::from_utf8(write_request.data).unwrap();
                        info!(
                            "GATT server got a write request with offset {} and data {}!",
                            write_request.offset, new_value,
                        );
                        *characteristic_value.lock().unwrap() = new_value;
                        write_request
                            .response
                            .send(Response::Success(vec![]))
                            .unwrap();
                    }
                    Event::NotifySubscribe(notify_subscribe) => {
                        info!("GATT server got a notify subscription!");
                        let notifying = Arc::clone(&notifying);
                        notifying.store(true, atomic::Ordering::Relaxed);
                        thread::spawn(move || {
                            let mut count = 0;
                            loop {
                                if !(&notifying).load(atomic::Ordering::Relaxed) {
                                    break;
                                };
                                count += 1;
                                debug!("GATT server notifying \"hi {}\"!", count);
                                notify_subscribe
                                    .clone()
                                    .notification
                                    .try_send(format!("hi {}", count).into())
                                    .unwrap();
                                thread::sleep(Duration::from_secs(2));
                            }
                        });
                    }
                    Event::NotifyUnsubscribe => {
                        info!("GATT server got a notify unsubscribe!");
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
                            "GATT server got a read request with offset {}!",
                            read_request.offset
                        );
                        let value = descriptor_value.lock().unwrap().clone();
                        read_request
                            .response
                            .send(Response::Success(value.clone().into()))
                            .unwrap();
                        info!("GATT server responded with \"{}\"", value);
                    }
                    Event::WriteRequest(write_request) => {
                        let new_value = String::from_utf8(write_request.data).unwrap();
                        info!(
                            "GATT server got a write request with offset {} and data {}!",
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

        // let peripheral = Peripheral_device::new().await.unwrap();

        self.peripheral.add_service(&Service::new(
            Uuid::from_sdp_short_uuid(SERVICE_ID),
            true,
            characteristics,
        )).unwrap();
        let main_fut = async move {
            info!("ONE");

            let powered = self.peripheral.is_powered().await;
            info!(":::::: {:?}", powered);
            while !self.peripheral.is_powered().await.expect("Failed to check if powered") {}
            info!("Peripheral powered on");
            self.peripheral.register_gatt().await.unwrap();

            //set_discoverable(true).expect("Failed to set discoverable");
            self.peripheral
                .start_advertising(ADVERTISING_NAME, &[]).await
                .expect("Failed to start_advertising");
            while !self.peripheral.is_advertising().await.unwrap() {}

            info!("Peripheral started advertising");

            while listening.load(atomic::Ordering::Relaxed) {
                tokio::time::delay_for(Duration::from_secs(1));
                // thread::sleep(Duration::from_secs(1));
            }

            debug!("Stopping Peripheral from being discoverable");

            self.peripheral.stop_advertising().await.unwrap();

            //set_discoverable(false).expect("failed to stop being discovered");
            info!("Peripheral stopped advertising");
        };
        let fut = futures::future::join3(characteristic_handler, descriptor_handler, main_fut);
        fut.await;
        Ok(())
        // futures::join!(characteristic_handler, descriptor_handler, main_fut);
    }

}
