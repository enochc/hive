
pub mod my_blurz;

#[cfg(not(target_os="linux"))]
pub mod blurz_cross;
pub mod peripheral;
pub mod central;

pub const ADVERTISING_NAME:&str = "Hive";
pub const SERVICE_ID:u16 = 0x1234;
pub const HIVE_CHAR_ID:u16 = 0x1235;
pub const HIVE_DESC_ID:u16 = 0x1236;
pub const HIVE_PROPS_DESC_ID:u16 = 0x1237;
//todo this needs to be learned then saves somewhere
// pub const HIVE_UUID: &str = "B8:27:EB:1F:38:F0";
pub const HIVE_UUID: &str = "B8:27:EB:6D:A3:66";


pub struct HiveMessage {
}

impl HiveMessage {
    pub const CONNECTED:u16 = 0x9876 as u16;
    // pub const CONNECTED = 0x9876;
}
