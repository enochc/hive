pub mod blurz;

#[cfg(not(target_os="linux"))]
pub mod blurz_cross;
pub mod advertise;

pub const ADVERTISING_NAME:&str = "Hive";
pub const SERVICE_ID:u16 = 0x1234;