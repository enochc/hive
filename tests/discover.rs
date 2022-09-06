#![allow(unused_must_use, unused_variables, unused_mut, unused_imports, non_snake_case)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;
use log::{debug, info, LevelFilter};
use hive::hive::Hive;
use hive::init_logging;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Barrier};
use std::{io, thread};
use std::sync::atomic::Ordering::Relaxed;
use async_std::prelude::FutureExt;
use async_std::task;
use futures::executor::block_on;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use hive::multicast::McListener;
// use hive::multicast;



#[test]
fn main() {
    init_logging(Some(LevelFilter::Debug));
    // println!("<< println");
    // debug!("<< debug");
    // let props_str = r#"
    // name = "hi_there"
    // listen = "3215"
    // discover = "true"
    // [Properties]
    // prop = 7
    // "#;
    // let server_hive = Hive::new_from_str(props_str);
    // let _ = server_hive.go(true);

    // let other_props_str = r#"
    // name = "lost_one"
    // discover = "false"
    // "#;
    // let other_hive = Hive::new_from_str(other_props_str);

    // let cc = other_hive.connected.load(Ordering::Relaxed);

    let list = McListener::new();

    let hello_barrier = Arc::new(Barrier::new(2));
    let hb_clone = Arc::clone(&hello_barrier);
    let hb_clone2 = Arc::clone(&hello_barrier);

    let do_listen = Arc::new(AtomicBool::new(true));
    let do_listen2 = Arc::clone(&do_listen);

    let listener = list.listen(do_listen2);
    sleep(Duration::from_millis(700));

    let r = task::spawn( async move {
        // let list = McListener::new();
        let sender = list.hello();

        let mut buf = [0u8; 64]; // receive buffer
        match sender.unwrap().recv_from(&mut buf) {
            Ok((len, remote_addr)) => {
                let data = &buf[..len];
                let response = String::from_utf8_lossy(data);

                println!("client: got data: {}", response);

                // verify it's what we expected
                assert_eq!("test", response);
                hb_clone2.wait();


            }
            Err(err) => {
                println!("client: had a problem: {}", err);
            }
        }
        println!("<<< hello done");

    });
    // block_on(r.await);
    // hello_barrier.wait();

    // block_on(async {
    //     futures::join!(r);
    // });

    sleep(Duration::from_millis(500));
    do_listen.store(false, Relaxed);

    assert!(true);
    sleep(Duration::from_millis(100));

}
