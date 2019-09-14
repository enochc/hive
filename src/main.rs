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
mod signal;
#[derive(Debug, Default)]
struct Foo{

}
impl Foo {
    fn gotValue(&self, v:i32) {
        println!("got value: {:?}", v)
    }
}

//macro_rules! property {
//    ( $x:expr, i32 ) => {
//        {
//            signal::Property{
//                value: $x,
//                value_changed: Signal<$t>
//            }
//            p
//        }
//    };
//}

fn main() {
    let mut config = Config::new();
    config.merge(config::File::with_name("hive.yaml"));
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

    //let mut a = signal::Counter::default();

    let mut p: signal::Property<i32> = Default::default();


    p.on_changed.connect(|v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_secs(2))
    });

    p.on_changed.connect(|v|{
        println!("also Inside signal: {:?}", v);
    });

    p.set_value(3);
    p.set_value(4);

    //println!("{}", b.read().expect("failed to get read").value());
    println!("Done: {:?}", p.value);
}

