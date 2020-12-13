pub mod my_blurz;

#[cfg(not(target_os="linux"))]
pub mod blurz_cross;
pub mod peripheral;

pub const ADVERTISING_NAME:&str = "Hive";
pub const SERVICE_ID:u16 = 0x1234;
pub const HIVE_CHAR_ID:u16 = 0x1235;