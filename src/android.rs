#![cfg(target_os = "android")]
#![allow(non_snake_case)]

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use crate::hive::Hive;
use crate::newHive;

#[allow(clippy::similar_names)]
#[no_mangle]
// pub extern "system" fn Java_com_robertohuertas_rusty_1android_1lib_RustyKt_hello(
pub extern "system" fn Java_com_example_rustinandroid_HiveKt_newHive(
env: JNIEnv,
_: JClass,
input: JString,
) -> Hive {
    let java_str = env.get_string(input).expect("Couldn't get Java string!");
    // we call our generic func for iOS
    let java_str_ptr = java_str.as_ptr();
    let result = unsafe { newHive(java_str_ptr) };
    // freeing memory from CString in ios function
    // if we call hello_release we won't have access to the result
    // let result_ptr = unsafe { CString::from_raw(result) };
    // let result_str = result_ptr.to_str().unwrap();
    // let output = env
    //         .new_string(result_str)
    //         .expect("Couldn't create a Java string!");
    // output.into_inner()
    result
}