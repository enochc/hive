#![allow(unused_imports)]
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
    sync::Arc,
};
use futures::executor::block_on;
use failure::_core::time::Duration;
use std::thread::sleep;


fn main(){
    let five = 5;
    let p = Arc::new(five);
   let r = block_on(dothign(p));
    println!("return:: {:?}", r);
}

async fn dothign(s: Arc<i32>) -> i32{

    let p = s.clone();
    let new = task::spawn(async move {
        sleep(Duration::from_secs(1));
        *p+1
        // println!("done:: {:?}", s);
    }).await;
    println!("done:: {:?}", new);
    // return new
    *s+2
}
// impl Solution {
//     pub fn kids_with_candies(candies: Vec<i32>, extra_candies: i32) -> Vec<bool> {
//
//     }
// }
