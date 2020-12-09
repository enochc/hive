#[allow(unused_imports)]
use log::{debug};

#[cfg(target_os = "linux")]
use blurz::{
    BluetoothAdapter, BluetoothDevice,
    BluetoothEvent::{self, Connected},
    BluetoothSession,
    bluetooth_discovery_session::BluetoothDiscoverySession as DiscoverySession,
    bluetooth_gatt_service::BluetoothGATTService,
    bluetooth_gatt_characteristic::BluetoothGATTCharacteristic as Characteristic,
    bluetooth_gatt_descriptor::BluetoothGATTDescriptor as Descriptor,

};

// #[cfg(not(target_os = "linux"))]
// use crate::blurz_cross::{BluetoothAdapter,
//                          BluetoothSession,
//                          BluetoothDiscoverySession as DiscoverySession,
//                          BluetoothDevice,
//                          BtSocket,
//                          BtProtocol,
//                          BtAddr,
//                          BluetoothGATTService,
//                          BluetoothGATTCharacteristic as Characteristic,
//                          BluetoothGATTDescriptor as Descriptor,
//                          BluetoothEvent,
#[cfg(not(target_os = "linux"))]
use crate::bluetooth::blurz_cross::{BluetoothAdapter,
                         BluetoothSession,
};
use std::error::Error;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub struct MyBluez{
    // session:&'a BluetoothSession,
    // adapter:Option<Arc<BluetoothAdapter<'a>>>
    // adapter: BluetoothAdapter<'a>
}
pub fn set_discoverable(discoverable:bool)->Result<()>{
    let sess = &BluetoothSession::create_session(None).unwrap();
    let adapter = BluetoothAdapter::init(
        sess
    );
    return match adapter {
        Ok(adapter) => {
            adapter.set_discoverable(true).map(|_|{ discoverable }).expect("failed to set discoverable");
            Ok(())
        }
        Err(err) => {
            Err(err)
        }
    }
}


