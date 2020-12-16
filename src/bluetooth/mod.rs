pub mod my_blurz;

#[cfg(not(target_os="linux"))]
pub mod blurz_cross;
pub mod peripheral;
pub mod central;

pub const ADVERTISING_NAME:&str = "Hive";
pub const SERVICE_ID:u16 = 0x1234;
pub const HIVE_CHAR_ID:u16 = 0x1235;
pub const HIVE_DESC_ID:u16 = 0x1236;
//todo this needs to be learned then saves somewhere
pub const HIVE_UUID: &str = "B8:27:EB:1F:38:F0";