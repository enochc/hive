// use std::fs;

use crate::hive::Hive;
use std::os::raw::c_char;
use std::ffi::{CStr};

mod hive_macros;
pub mod property;
pub mod signal;
pub mod hive;
pub mod peer;
pub mod handler;

#[cfg(target_os = "android")]
mod android;

#[no_mangle]
pub unsafe extern "C" fn newHive(props: *const c_char) -> Hive {
    let c_str = CStr::from_ptr(props);
    let prop_str_pointer = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => "you",
    };

    Hive::new_from_str("Hive1", prop_str_pointer )
        // .unwrap()
        // .into_raw()
}


// fn get_toml_config(file_path: &str) -> toml::Value{
//     let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
//     return toml::from_str(&foo).unwrap();
// }

