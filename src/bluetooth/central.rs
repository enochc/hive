// https://github.com/JojiiOfficial/LiveBudsCli/tree/master/src/daemon/bluetooth
// https://github.com/JojiiOfficial/LiveBudsCli/blob/master/src/daemon/bluetooth/rfcomm_connector.rs#L159

use log::{debug,info};
use std::str::FromStr;
use std::thread;
use std::time::Duration;

#[cfg(target_os = "linux")]
use blurz::{
    bluetooth_discovery_session::BluetoothDiscoverySession as DiscoverySession, bluetooth_gatt_characteristic::BluetoothGATTCharacteristic as Characteristic,
    bluetooth_gatt_descriptor::BluetoothGATTDescriptor as Descriptor,
    bluetooth_gatt_service::BluetoothGATTService,
    BluetoothAdapter,
    BluetoothDevice,
    BluetoothEvent::{self, Connected},
    BluetoothSession,
};
use bluster::SdpShortUuid;
use bytes::{BufMut, BytesMut};
use regex::Regex;
use uuid::Uuid;

use crate::bluetooth::{HIVE_CHAR_ID, HIVE_UUID, SERVICE_ID};
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

const UUID_REGEX: &str = r"([0-9a-f]{8})-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}";
lazy_static! {
  static ref RE: Regex = Regex::new(UUID_REGEX).unwrap();
}


pub struct Central {
    connect_to_name: String,
}

impl Central {
    pub fn new(name: &str) -> Central {
        return Central {
            connect_to_name: String::from(name)
        };
    }
    pub fn connect(&self) {
        let session = &BluetoothSession::create_session(None).unwrap();

        // #[cfg(target_os = "linux")]
        {
            thread::spawn(move || {
                loop {
                    for event in BluetoothSession::create_session(None).unwrap().incoming(1000).map(BluetoothEvent::from) {
                        if event.is_some() {
                            let event = event.unwrap();
                            match event {
                                BluetoothEvent::Value { value, object_path } => {
                                    let str_val = String::from_utf8(value.to_vec());
                                    info!("VALUE << {:?}", str_val)
                                }
                                BluetoothEvent::None => {}
                                _ => info!("EVENT <<<<<<<< {:?}", event),
                            }
                        }
                    }
                }
            });
        }


        let adapter = BluetoothAdapter::init(session);
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

        // start bo removing the device, do a clean add
        // adapter.remove_device(String::from(ADVERTISING_NAME)).expect("Failed to remove device");

        let get_device = || {
            for p in adapter.get_device_list().unwrap() {
                let d = BluetoothDevice::new(&session, String::from(p));
                match d.get_address() {
                    Ok(addr) => {
                        if addr.eq(HIVE_UUID) {
                            debug!("<< FOUND ADDRESS: {:?} = {:?} = {:?} found", d.get_name(), d.get_uuids(), d);
                            return Some(d);
                        }
                    }
                    _ => {}
                }
                match d.get_name() {
                    Ok(name) => {
                        debug!("<<FOUND NAME: {:?} = {:?} = {:?} found", name, d.get_uuids(), d.get_address());
                        if name.eq(&self.connect_to_name) {
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
                    debug!("<< FOUND 1");
                    hive_device = Some(d);
                } else {
                    let conn = d.connect(5000);
                    debug!("<<< CONNECTED:: {:?}", conn);
                    if conn.is_ok() {
                        hive_device = Some(d);
                    }
                }
            }
            _ => {
                'scan_loop: for _ in 0..5 {
                    scan_for_devices(session, adapter.clone()).expect("Failed to scan for devices");
                    let device = get_device();
                    if device.is_some() {
                        let d = device.unwrap();
                        debug!("<< FOUND 2");

                        let mut counter = 0;
                        'connect_loop: loop {
                            counter += 1;

                            let conn = d.connect(5000);
                            debug!("<<< CONNECTED:: {:?}, {:?}", conn, d.is_connected());
                            if d.is_connected().unwrap() {
                                hive_device = Some(d);
                                break 'connect_loop;
                            }

                            match conn {
                                // _ => {break;}
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
            let services = device.get_gatt_services().unwrap();
            debug!("<<<< SERVICES: {:?}", device.get_service_data());
            debug!("<< connected:{:?}", device.is_connected());
            debug!("<< paired:{:?}", device.is_paired());
            debug!("<< services: {:?}", services);

            // let services = try!(device.get_gatt_services());
            for service in services {
                let s = BluetoothGATTService::new(session, service.clone());
                let uuid = s.get_uuid().unwrap();
                let assigned_number = RE.captures(&uuid).unwrap().get(1).map_or("", |m| m.as_str());
                let ff = Uuid::from_sdp_short_uuid(SERVICE_ID);
                let tp_serv = Uuid::from_str(&uuid).unwrap();
                let my_service = tp_serv == ff;
                debug!("SERVICE: {:?} == {:?}, {:?}", s, uuid, assigned_number);
                let num_num = u16::from_str(assigned_number).unwrap();

                debug!("<< MY SERVICE {:?} == {:?}, {:?}", ff, tp_serv, my_service);
                debug!("assigned number str: {:?}, {:?}", assigned_number, num_num as u16);
                let mut buf = BytesMut::with_capacity(16);
                // buf.put(&b"hello world"[..]);
                buf.put_u16(SERVICE_ID);

                if my_service {
                    let characteristics = s.get_gatt_characteristics().expect("Failed get characteristics");
                    // let characteristics = try!(s.get_gatt_characteristics());
                    for characteristic in characteristics {
                        let c = Characteristic::new(session, characteristic.clone());

                        let hive_char_id = Uuid::from_sdp_short_uuid(HIVE_CHAR_ID);
                        let char_id = Uuid::from_str(&c.get_uuid().unwrap());
                        debug!("<<<< char char: {:?}", char_id);
                        debug!("<<<< hive char: {:?}", hive_char_id);
                        if hive_char_id == Uuid::from_str(&c.get_uuid().unwrap()).unwrap() {
                            debug!("<<<<<<<<<<<< MYCHAR!!!!!");


                            debug!("<< {:?} == {:?}", c.get_value(), c);
                            debug!("<< {:?}", c.get_uuid());
                            debug!("<< characteristic: {:?}", String::from_utf8(c.read_value(None).unwrap()));
                            let msg = "you whats up mama?".to_string().into_bytes();
                            c.write_value(msg, None).expect("Failed to write to characteristic");

                            let descriptors = c.get_gatt_descriptors().expect("Failed to get descriptors");
                            for descriptor in descriptors {
                                let d = Descriptor::new(session, descriptor.clone());
                                let bytes = d.read_value(None).unwrap();
                                let val = String::from_utf8(bytes);
                                debug!("<<<< descriptor: {:?}", val);
                            }
                            let notify = c.start_notify();
                            if notify.is_ok() {
                                thread::sleep(Duration::from_secs(5));
                                c.stop_notify().unwrap();
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
    let session =
        DiscoverySession::create_session(bt_session, adapter.get_id())?;
    session.start_discovery()?;
    // for _ in 0..5 {
    //     let devices = adapter.get_device_list()?;
    //     if !devices.is_empty() {
    //         break;
    //     }
    //
    // }
    thread::sleep(Duration::from_millis(5000));


    return session.stop_discovery();
}

#[cfg(target_os = "linux")]
use bluetooth_serial_port::{BtAddr, BtProtocol, BtSocket};

#[derive(Debug)]
pub struct HiveConnection {
    pub addr: String,
    pub socket: BtSocket,
    pub fd: i32,
}

// #[cfg(target_os = "linux")]
pub fn connect_rfcomm(addr: &str) -> Result<HiveConnection, String> {
    debug!("one");
    let mut socket = BtSocket::new(BtProtocol::RFCOMM).map_err(|e| e.to_string())?;
    debug!("two");
    let address = BtAddr::from_str(addr.as_ref()).unwrap();
    debug!("three");
    socket.connect(address).map_err(|e| e.to_string())?;
    debug!("four");
    // let fd = socket.get_fd();
    // debug!("five");

    Ok(HiveConnection {
        addr: addr.to_owned(),
        socket,
        fd: 6,// fd,
    })
}