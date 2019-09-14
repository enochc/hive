use {
    tokio::net::{TcpListener},
    //tokio::net::tcp::Incoming,
    tokio::codec::{Framed, LinesCodec},
    futures::stream::Stream,
    futures::future,
    futures::future::Future,
    futures::future::lazy,
    //failure::{format_err, Error},
    config::Config,
    std::net::{IpAddr, Ipv4Addr, SocketAddr},
    //std::sync::{Arc, Mutex, RwLock},
};
use std::thread::sleep;
use failure::_core::time::Duration;


fn client_requests(addr: &SocketAddr) -> Box<dyn Future<Item=(), Error=()> + Send> {
    println!("<<<< Listening on: {:?}", addr);
    let listener = TcpListener::bind(&addr).expect("Failed to bind address");
    let listener = listener.incoming()
        .map_err(|e| eprintln!("failed to accept socket; error = {:?}", e))
        .for_each(|socket| {
            println!("<<< for_each");
            //process_socket(socket);
            let (sink, stream) = Framed::new(socket, LinesCodec::new()).split();
            let frame_read = stream
                .for_each(move |frame| {
                    println!("{:?}", frame);
                    Ok(())
                }).map_err(|_| ());
            tokio::spawn(frame_read);
            future::ok(())
        });

    // Spawn tcplistener
    tokio::spawn(listener);
    Box::new(future::ok(()))
}

#[macro_use]
mod hive_macros;
pub mod signal;



pub fn run(config: &Config) {

    match config.get_int("listen") {
        Ok(port) => {
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port as u16);
            // accept connections and process them
            tokio::run(lazy(move || {
                client_requests(&addr)
            }));
        }
        _ => println!("No listen port specified, not listening")
    }
}

