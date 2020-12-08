// use libusb;
// use usb_device::prelude::{UsbDeviceBuilder, UsbVidPid};
// use usbd_serial::{USB_CLASS_CDC, UsbError, SerialPort};
// use log::{Metadata, Level, Record, LevelFilter, error};

fn main() {
    // let mut context = libusb::Context::new().unwrap();
    //
    // for mut device in context.devices().unwrap().iter() {
    //     let device_desc = device.device_descriptor().unwrap();
    //
    //     println!("Bus {:?} Device {:03} ID {:04x}:{:04x}", // println!("Bus {:?} Device {:03} ID {:04x}:{:04x}",
    //              device.bus_number(),
    //              device.address(),
    //              device_desc.vendor_id(),
    //              device_desc.product_id());
    // }
    //
    // let mut serial = SerialPort::new(&usb_bus);
    // let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
    //         .product("Serial port")
    //         .device_class(USB_CLASS_CDC)
    //         .build();
    //
    //     loop {
    //         if !usb_dev.poll(&mut [&mut serial]) {
    //             continue;
    //         }
    //
    //         let mut buf = [0u8; 64];
    //
    //         match serial.read(&mut buf[..]) {
    //             Ok(count) => {
    //                 // count bytes were read to &buf[..count]
    //                 prin
    //             },
    //             Err(UsbError::WouldBlock) => {
    //                 // Err(err) => // An error occurred
    //             },// No data received
    //             _ => {error!("Something else happened!");}
    //
    //         };
    //
    //         match serial.write(&[0x3a, 0x29]) {
    //             Ok(count) => {
    //                 // count bytes were written
    //             },
    //             Err(err) => {
    //                 error!("Usb error occurred");
    //             }// No data could be written (buffers full)
    //                 // Err(e) => // An error occurred
    //         };
    //     }
}