// https://github.com/JojiiOfficial/LiveBudsCli/tree/master/src/daemon/bluetooth
// https://github.com/JojiiOfficial/LiveBudsCli/blob/master/src/daemon/bluetooth/rfcomm_connector.rs#L159

use std::str::FromStr;
use std::thread;
use std::time::Duration;

#[cfg(target_os = "linux")]
use blurz::{
    bluetooth_discovery_session::BluetoothDiscoverySession as DiscoverySession,
    bluetooth_gatt_characteristic::BluetoothGATTCharacteristic as Characteristic,
    bluetooth_gatt_descriptor::BluetoothGATTDescriptor as Descriptor,
    bluetooth_gatt_service::BluetoothGATTService,
    BluetoothAdapter,
    BluetoothDevice,
    BluetoothEvent::{self, Connected},
    BluetoothSession,
};
use bluster::SdpShortUuid;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use log::{debug, info};
use regex::Regex;
use uuid::Uuid;

use crate::bluetooth::{HIVE_CHAR_ID, HIVE_UUID, SERVICE_ID, HIVE_DESC_ID};
#[cfg(not(target_os = "linux"))]
use crate::bluetooth::blurz_cross::{BluetoothAdapter,
                                    BluetoothDevice,
                                    BluetoothDiscoverySession as DiscoverySession,
                                    BluetoothEvent,
                                    BluetoothGATTCharacteristic as Characteristic,
                                    BluetoothGATTDescriptor as Descriptor,
                                    BluetoothGATTService,
                                    BluetoothSession,
                                    BtAddr,
                                    BtProtocol,
                                    BtSocket,
};
use crate::hive::Sender;
use crate::peer::SocketEvent;
use async_std::sync::Arc;

const UUID_REGEX: &str = r"([0-9a-f]{8})-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}";
lazy_static! {
  static ref RE: Regex = Regex::new(UUID_REGEX).unwrap();
}


#[derive(Clone, Debug)]
pub struct Central {
    connect_to_name: String,
    sender: Sender<SocketEvent>,
    desc_sender: Option<UnboundedSender<String>>,
}

// lazy_static! {
//   // static ref RE: Regex = Regex::new(UUID_REGEX).unwrap();
//   static ref session:Arc<BluetoothSession> = Arc::new(BluetoothSession::create_session(None).unwrap());
// }
// static session:BluetoothSession = BluetoothSession::create_session(None).unwrap();


impl Central {
    pub fn new(name: &str, sender: Sender<SocketEvent>) -> Central {
        return Central {
            connect_to_name: String::from(name),
            sender,
            desc_sender: None,
        };
    }
    pub async fn send(&mut self, msg: &str) {
        if self.desc_sender.is_some() {
            self.desc_sender.as_ref().unwrap().send(String::from(msg)).await;
        }
    }
    pub async fn connect(&mut self) {
        let session: BluetoothSession = BluetoothSession::create_session(None).unwrap();

        // #[cfg(target_os = "linux")]
        {
            // thread::spawn(move || {
            async_std::task::spawn(async move{
                loop {
                    for event in BluetoothSession::create_session(None).unwrap().incoming(1000).map(BluetoothEvent::from) {
                        if event.is_some() {
                            let event = event.unwrap();
                            match event {
                                BluetoothEvent::Value { value, object_path } => {
                                    let str_val = String::from_utf8(value.to_vec());
                                    info!("{:?} VALUE: << {:?}", object_path, str_val)
                                }
                                BluetoothEvent::None => {}
                                _ => info!("EVENT: {:?}", event),
                            }
                        }
                    }
                }
            });
        }


        let adapter = BluetoothAdapter::init(&session);
        if let Err(err) = adapter {
            // Thanks blurz for implementing usable error types
            let s = err.to_string();

            // On adapter not found, wait and try to connect to it again
            // maybe the user inserted the adapter later on
            if s.contains("Bluetooth adapter not found") {
                debug!("{:?}", err);
                std::process::exit(1);
            } else {
                // Every other error should be treated as fatal error
                debug!("Bluetooth error: {}", err);
                std::process::exit(1);
            }
        }

        let adapter = adapter.unwrap();

        let get_device = || {
            for p in adapter.get_device_list().unwrap() {
                let d = BluetoothDevice::new(&session, String::from(p));
                match d.get_address() {
                    Ok(addr) => {
                        if addr.eq(HIVE_UUID) {
                            debug!("FOUND ADDRESS: {:?}, {:?}", HIVE_UUID, d.get_name());
                            return Some(d);
                        }
                    }
                    _ => {}
                }
                match d.get_name() {
                    Ok(name) => {
                        if name.eq(&self.connect_to_name) {
                            debug!("FOUND NAME: {:?}, {:?}", name, d.get_address());
                            return Some(d);
                        }
                    }
                    Err(_) => {}
                }
            }
            return None;
        };

        let mut hive_device: Option<BluetoothDevice> = None;
        match get_device() {
            Some(d) => {
                if d.is_connected().unwrap() {
                    debug!("FOUND paired device");
                    hive_device = Some(d);
                } else {
                    let conn = d.connect(5000);
                    debug!("<CONNECTED:: {:?}", conn);
                    if conn.is_ok() {
                        hive_device = Some(d);
                    }
                }
            }
            _ => {
                'scan_loop: for _ in 0..5 {
                    scan_for_devices(&session, adapter.clone()).expect("Failed to scan for devices");
                    let device = get_device();
                    if device.is_some() {
                        let d = device.unwrap();
                        debug!("FOUND device in search");

                        let mut counter = 0;
                        'connect_loop: loop {
                            counter += 1;

                            let conn = d.connect(5000);
                            debug!("CONNECTED:: {:?}, {:?}", conn, d.is_connected());
                            if d.is_connected().unwrap() {
                                hive_device = Some(d);
                                break 'connect_loop;
                            }

                            match conn {
                                Ok(_) => {
                                    hive_device = Some(d);
                                    break 'connect_loop;
                                }
                                Err(_) => {
                                    if counter >= 3 {
                                        break;
                                    }
                                    thread::sleep(Duration::from_millis(500));
                                }
                            }
                        }

                        break 'scan_loop;
                    }
                }
            }
        }
        if hive_device.is_some() {
            // B8:27:EB:6D:A3:66
            // debug!("<< moving on...my id: {:?}, {:?}", adapter.get_name(), adapter.get_address());
            let device = hive_device.unwrap();
            let services = device.get_gatt_services().unwrap();
            // debug!("<<<< SERVICES: {:?}", device.get_service_data());
            // debug!("<< connected:{:?}", device.is_connected());
            // debug!("<< paired:{:?}", device.is_paired());
            // debug!("<< services: {:?}", services);

            // let services = try!(device.get_gatt_services());
            for service in services {
                let s = BluetoothGATTService::new(&session, service.clone());
                let uuid = s.get_uuid().unwrap();
                let ff = Uuid::from_sdp_short_uuid(SERVICE_ID);
                let tp_serv = Uuid::from_str(&uuid).unwrap();
                let my_service = tp_serv == ff;
                let mut buf = BytesMut::with_capacity(16);
                buf.put_u16(SERVICE_ID);

                if my_service {
                    let characteristics = s.get_gatt_characteristics().expect("Failed get characteristics");
                    for characteristic in characteristics {
                        // let sess = session.clone();
                        let c = Characteristic::new(&session, characteristic.clone());
                        let hive_char_id = Uuid::from_sdp_short_uuid(HIVE_CHAR_ID);
                        // let char_id = Uuid::from_str(&c.get_uuid().unwrap());
                        // debug!("<<<< char char: {:?}", char_id);
                        // debug!("<<<< hive char: {:?}", hive_char_id);
                        if hive_char_id == Uuid::from_str(&c.get_uuid().unwrap()).unwrap() {
                            debug!("Found Characteristic!!!!!");
                            let se = SocketEvent::NewPeer {
                                name: device.get_name().unwrap(),
                                stream: None,
                                peripheral: None,
                                central: Some(self.clone()),
                                address: None,
                            };
                            let mut sender = &self.sender;
                            sender.send(se).await.expect("failed to send peer");

                            debug!("<< {:?} == {:?}", c.get_value(), c);
                            debug!("<< {:?}", c.get_uuid());
                            debug!("<< characteristic: {:?}", String::from_utf8(c.read_value(None).unwrap()));
                            let msg = "you whats up mama?".to_string().into_bytes();
                            c.write_value(msg, None).expect("Failed to write to characteristic");

                            let descriptors = c.get_gatt_descriptors().expect("Failed to get descriptors");
                            for descriptor in descriptors {
                                let d = Descriptor::new(&session, descriptor.clone());
                                debug!("<< d: {:?}", d);
                                let (tx, mut rx) = mpsc::unbounded();
                                self.desc_sender = Some(tx.clone());
                                let hive_desc_id = Uuid::from_sdp_short_uuid(HIVE_DESC_ID);
                                if d.get_uuid().unwrap() == hive_desc_id.to_string(){
                                    debug!("<<<< MY DESCRIPTOR");
                           
                                    let sender_clone = self.sender.clone();
                                    let sender_clone2 = tx.clone();
                                    let cc = c.get_id();
                                    async_std::task::spawn(async move{
                                        let session: BluetoothSession = BluetoothSession::create_session(None).unwrap();
                                        let c = Characteristic::new(&session, cc);
                                        let notify = c.start_notify().expect("failed to start notify");
                                        while !sender_clone.is_closed(){
                                            async_std::task::sleep(Duration::from_secs(1));
                                        }
                                        debug!("<< STOPPING");

                                        c.stop_notify().expect("failed to cancel bt notify");
                                        sender_clone2.close_channel();
                                    });

                                    while !tx.is_closed(){
                                        debug!("<< Looping over send");
                                        // loop here forever to send messages via descriptor updates
                                        let msg = rx.next().await;
                                        match msg {
                                            Some(m) => {
                                                debug!("<<< Send message: {:?}", m);
                                                d.write_value(m.into_bytes(), None);

                                            },
                                            _=>{}
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn scan_for_devices(bt_session: &BluetoothSession,
                        adapter: BluetoothAdapter, ) -> Result<(), Box<dyn std::error::Error>> {
    debug!("<<<< SCANNING FOR DEVICES...");
    let disc_session = DiscoverySession::create_session(bt_session, adapter.get_id())?;
    disc_session.start_discovery()?;
    thread::sleep(Duration::from_millis(5000));


    return disc_session.stop_discovery();
}


// todo this was expirimental work, might just remove it
// #[cfg(target_os = "linux")]
// use bluetooth_serial_port::{BtAddr, BtProtocol, BtSocket};
//
// #[derive(Debug)]
// pub struct HiveConnection {
//     pub addr: String,
//     pub socket: BtSocket,
//     pub fd: i32,
// }
//
// // #[cfg(target_os = "linux")]
// pub fn connect_rfcomm(addr: &str) -> Result<HiveConnection, String> {
//     debug!("one");
//     let mut socket = BtSocket::new(BtProtocol::RFCOMM).map_err(|e| e.to_string())?;
//     debug!("two");
//     let address = BtAddr::from_str(addr.as_ref()).unwrap();
//     debug!("three");
//     socket.connect(address).map_err(|e| e.to_string())?;
//     debug!("four");
//     // let fd = socket.get_fd();
//     // debug!("five");
//
//     Ok(HiveConnection {
//         addr: addr.to_owned(),
//         socket,
//         fd: 6,// fd,
//     })
// }