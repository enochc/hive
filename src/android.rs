#![cfg(target_os = "android")]
#![allow(non_snake_case)]
use jni::sys::jstring;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use crate::hive::Hive;
use crate::newHive;
use std::ffi::CString;

use async_std::{
    // net::{TcpListener, TcpStream, ToSocketAddrs},
    task,
};

#[allow(clippy::similar_names)]
#[no_mangle]
// pub extern "system" fn Java_com_robertohuertas_rusty_1android_1lib_RustyKt_hello(
pub extern "system" fn Java_com_example_rustinandroid_HiveKt_newHive(
env: JNIEnv,
_: JClass,
input: JString,
// ) -> Hive {
) -> jstring {
    let java_str = env.get_string(input).expect("Couldn't get Java string!");
    // we call our generic func for iOS
    let java_str_ptr = java_str.as_ptr();
    let mut result = unsafe { newHive(java_str_ptr) };
    task::spawn(async move{
        result.run().await;
    });

    // freeing memory from CString in ios function
    // if we call hello_release we won't have access to the result
    // let result_ptr = unsafe { CString::from_raw(result) };
    // let result_str = result_ptr.to_str().unwrap();
    // let output = env
    //         .new_string(result_str)
    //         .expect("Couldn't create a Java string!");
    // output.into_inner()
    // let result_ptr = unsafe { CString::from_raw() };
    let result_str = "running hive 4";
    let output = env
        .new_string(result_str)
        .expect("Couldn't create a Java string!");
    output.into_inner()

}