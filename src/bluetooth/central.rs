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
use futures::channel::mpsc::{ UnboundedSender};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use log::{debug, info};
use regex::Regex;
use uuid::Uuid;

use crate::bluetooth::{HIVE_CHAR_ID, HIVE_UUID, SERVICE_ID, HIVE_DESC_ID, HiveMessage, HIVE_PROPS_DESC_ID};
#[cfg(not(target_os = "linux"))]
use crate::bluetooth::blurz_cross::{BluetoothAdapter,
                                    BluetoothDevice,
                                    BluetoothDiscoverySession as DiscoverySession,
                                    BluetoothEvent,
                                    BluetoothGATTCharacteristic as Characteristic,
                                    BluetoothGATTDescriptor as Descriptor,
                                    BluetoothGATTService,
                                    BluetoothSession,
                                    // BtProtocol,
                                    // BtSocket,
};
use crate::hive::Sender;
use crate::peer::SocketEvent;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use futures::executor::block_on;


const UUID_REGEX: &str = r"([0-9a-f]{8})-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}";
lazy_static! {
  static ref RE: Regex = Regex::new(UUID_REGEX).unwrap();
  static ref HIVE_CHAR_UUID:String = Uuid::from_sdp_short_uuid(HIVE_CHAR_ID).to_string();
  static ref HIVE_PROPS_DESC_UUID: Uuid = Uuid::from_sdp_short_uuid(HIVE_PROPS_DESC_ID);
}


#[derive(Clone, Debug)]
pub struct Central {
    connect_to_name: String,
    sender: Sender<SocketEvent>,
    desc_sender: Option<UnboundedSender<BytesMut>>,
    pub connected: Arc<AtomicBool>,
    pub found_device: bool,
    char_object_path: Arc<Mutex<String>>,
    peer_address: Arc<Mutex<String>>
}

impl Central {
    pub fn new(name: &str, sender: Sender<SocketEvent>) -> Central {
        let central = Central {
            connect_to_name: String::from(name),
            sender,
            desc_sender: None,
            connected: Arc::new(AtomicBool::new(false)),
            found_device: false,
            char_object_path : Arc::from(Mutex::new(String::from(""))),
            peer_address : Arc::from(Mutex::new(String::from(""))),
        };
        central.listen_for_events();
        return central;
    }


    pub fn listen_for_events(&self){
        let char_path_clone = self.char_object_path.clone();
        let peer_address_clone = self.peer_address.clone();
        let mut sender_clone = self.sender.clone();
        async_std::task::spawn(async move{
            debug!("<<<<< LOPING OVER RECEIVER");
            loop {
                for event in BluetoothSession::create_session(None).unwrap().incoming(1000).map(BluetoothEvent::from) {
                    if event.is_some() {
                        let event = event.unwrap();
                        match event {
                            BluetoothEvent::Value { value, object_path } => {
                                let str_val = String::from_utf8(value.to_vec());

                                info!("<<<< {:?} VALUE: << {:?}", object_path, str_val);
                                info!("<<<< {:?} ", &char_path_clone.lock().unwrap());
                                let chr = &*char_path_clone.lock().unwrap().clone();
                                // descriptors start with the characteristics obeject path
                                if object_path.starts_with(chr) {
                                    let str = &*peer_address_clone.lock().unwrap();
                                    let se = SocketEvent::Message {
                                        from: str.clone(),
                                        msg: str_val.unwrap(),
                                    };
                                    info!("<<<<<< SENDING MESSAGE {:?}",se);
                                    block_on(sender_clone.send(se)).unwrap();
                                    // sender_clone.send(se).await.expect("failed to send message");
                                } else {
                                    info!("<<<<< not sending message!!!! ");
                                }

                            }
                            BluetoothEvent::Connected{object_path, connected} => {

                                info!("<<<< CONNECTED: {:?} {:?}", connected, object_path);
                                // let str = &*mydevice_id_clone.lock().unwrap();
                                // if object_path.eq(str){
                                //     debug!("<<<<<<<< THIS IS MY DEVICE");
                                // }
                            }
                            BluetoothEvent::None => {}
                            _ => info!("<<<< EVENT: {:?}", event),
                        }
                    }
                }
            }
        });
    }
    pub async fn send(& self, msg: BytesMut) {
        debug!("SEND <<<< {:?}", msg);
        if self.desc_sender.is_some() {
            self.desc_sender.as_ref().unwrap().send(msg).await.expect("Failed to send message!");
        }
    }

    fn fetch_properties(&self, session:&BluetoothSession, descriptors:&Vec<String>)-> Result<(), Box<dyn std::error::Error>>{

        debug!("<<<< fetch properties: {:?}",descriptors);
        for descriptor in descriptors {
            let desc = Descriptor::new(session, descriptor.clone());
            if desc.get_uuid().unwrap() == HIVE_PROPS_DESC_UUID.to_string(){
                debug!(".<< << << << << {:?}",desc);
                let _val = match desc.read_value(None){
                    Ok(_) => {
                        // Do nothing, the response is captured in the listen_for_events loop
                    }
                    Err(e) => {debug!("<< << << << << FAILED: {:?}", e)}
                };

            }
            //if desc.get_uuid().unwrap()
        }
        return Ok(())
    }

    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.found_device = false;
        let session: BluetoothSession = BluetoothSession::create_session(None).unwrap();

        let adapter = match BluetoothAdapter::init(&session) {
            Err(err) =>{
                let s = err.to_string();
                //todo
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
            },
            Ok(a) => a
        };

        let get_device = || {
            for p in adapter.get_device_list().expect("Failed to get devices") {
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
                if d.is_connected()? {
                    debug!("FOUND paired device");
                    hive_device = Some(d);
                } else {
                    let conn = d.connect(5000);
                    debug!("<< CONNECTED:: {:?}", conn);
                    if conn.is_ok() {
                        hive_device = Some(d);
                    }
                    // else {
                    //     debug!("waiting for connect");
                    //     while !d.is_connected()?{
                    //         debug!(".");
                    //         thread::sleep(Duration::from_secs(1))
                    //     }
                    //
                    // }
                }
            }
            _ => {
                'scan_loop: for _ in 0..5 {
                    scan_for_devices(&session, adapter.clone())?;
                    let device = get_device();
                    if device.is_some() {
                        let d = device.unwrap();
                        debug!("FOUND device in search");

                        let mut counter = 0;
                        'connect_loop: loop {
                            counter += 1;

                            let conn = d.connect(5000);
                            debug!("CONNECTED:: {:?}, {:?}", conn, d.is_connected());

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
            debug!("<< moving on...my id: {:?}, {:?}", adapter.get_name(), adapter.get_address());
            let device = hive_device.unwrap();
            let services = device.get_gatt_services()?;

            debug!("<< connected: {:?}", device.is_connected());
            debug!("<< paired:    {:?}", device.is_paired());
            debug!("<< services:  {:?}", services);
            //let mut found = false;
            for service in services {
                let s = BluetoothGATTService::new(&session, service.clone());

                let uuid = s.get_uuid()?;
                let ff = Uuid::from_sdp_short_uuid(SERVICE_ID);
                let tp_serv = Uuid::from_str(&uuid)?;
                self.found_device = tp_serv == ff;
                debug!("<< SERVICE: {:?}, {:?}",s, self.found_device);
                let mut buf = BytesMut::with_capacity(16);
                buf.put_u16(SERVICE_ID);

                if self.found_device {
                    let characteristics = s.get_gatt_characteristics().expect("Failed get characteristics");
                    for characteristic in characteristics {
                        let c = Characteristic::new(&session, characteristic.clone());

                        if *HIVE_CHAR_UUID == c.get_uuid()? {
                            debug!("<< Found Characteristic {:?}, {:?}, {:?}", c, c.get_value(), c.get_uuid());

                            let descriptors = c.get_gatt_descriptors().expect("Failed to get descriptors");

                            for descriptor in &descriptors {
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
                                    *self.char_object_path.lock().unwrap() = cc.clone();
                                    async_std::task::spawn(async move{
                                        let session: BluetoothSession = BluetoothSession::create_session(None).unwrap();
                                        let c = Characteristic::new(&session, cc);

                                        c.start_notify().expect("failed to start notify");
                                        while !sender_clone.is_closed(){
                                            std::thread::sleep(Duration::from_secs(1));
                                        }
                                        debug!("<< STOPPING");

                                        c.stop_notify().expect("failed to cancel bt notify");
                                        sender_clone2.close_channel();
                                    });
                                    // Send connect message
                                    let conn_message = HiveMessage::CONNECTED;
                                    let mut bytes = BytesMut::new();//::from(conn_message);
                                    bytes.put_u16(conn_message);
                                    bytes.put_slice(format!("{},", adapter.get_name()?).as_bytes());
                                    bytes.put_slice(adapter.get_address()?.as_bytes());
                                    c.write_value(bytes.to_vec(), None)?;
                                    *self.peer_address.lock().unwrap() = device.get_address()?.clone();

                                    let se = SocketEvent::NewPeer {
                                        name: device.get_name().unwrap(),
                                        stream: None,
                                        peripheral: None,
                                        central: Some(self.clone()),
                                        address: device.get_address()?,
                                    };
                                    let mut sender = &self.sender;
                                    sender.send(se).await.expect("failed to send peer");
                                    self.connected.store(true, Ordering::Relaxed);
                                    self.fetch_properties(&session, &descriptors)?;

                                    debug!("<< Looping over send");
                                    // loop here forever to send messages via descriptor updates
                                    while !tx.is_closed(){
                                        let msg = rx.next().await;
                                        match msg {
                                            Some(m) => {
                                                debug!("<<< Send message: {:?}", m);
                                                d.write_value(m.to_vec(), None)?;

                                            },
                                            _=>{}
                                        };
                                    }
                                    self.connected.store(false, Ordering::Relaxed);
                                    debug!("Central stopped");
                                }
                            }
                        }
                    }
                }
            }
            if !self.found_device{
                // our service wasn't found, not sure what entirely causes this, try removing
                // the device and search again.
                // adapter.remove_device(device.get_id())?;
            }
        }else {
            debug!("Failed to find device.");
        }
        Ok(())
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


// todo this was experimental work, might just remove it
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