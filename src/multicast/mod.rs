use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use async_std::task;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};


const PORT: u16 = 7645;

fn new_socket(addr: &SocketAddr) -> io::Result<Socket> {
    let domain = if addr.is_ipv4() {
        Domain::ipv4()
    } else {
        Domain::ipv6()
    };

    let socket = Socket::new(domain, Type::dgram(), Some(Protocol::udp()))?;

    // we're going to use read timeouts so that we don't hang waiting for packets
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;

    Ok(socket)
}

fn join_multicast(addr: SocketAddr) -> io::Result<UdpSocket> {
    let ip_addr = addr.ip();

    let socket = new_socket(&addr)?;

    // depending on the IP protocol we have slightly different work
    match ip_addr {
        IpAddr::V4(ref mdns_v4) => {
            // join to the multicast address, with all interfaces
            socket.join_multicast_v4(mdns_v4, &Ipv4Addr::new(0, 0, 0, 0))?;
        }
        IpAddr::V6(ref mdns_v6) => {
            // join to the multicast address, with all interfaces (ipv6 uses indexes not addresses)
            socket.join_multicast_v6(mdns_v6, 0)?;
            socket.set_only_v6(true)?;
        }
    };

    // bind us to the socket address.
    socket.bind(&SockAddr::from(addr))?;
    Ok(socket.into_udp_socket())
}

fn new_sender(addr: &SocketAddr) -> io::Result<UdpSocket> {
    let socket = new_socket(addr)?;

    if addr.is_ipv4() {
        socket.bind(&SockAddr::from(SocketAddr::new(
            Ipv4Addr::new(0, 0, 0, 0).into(),
            0,
        )))?;
    } else {
        socket.bind(&SockAddr::from(SocketAddr::new(
            Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0).into(),
            0,
        )))?;
    }

    Ok(socket.into_udp_socket())
}

pub struct McListener {
    pub IPV4: IpAddr,// = Ipv4Addr::new(224, 0, 0, 123).into();
    pub IPV6: IpAddr,// = Ipv6Addr::new(0xFF02, 0, 0, 0, 0, 0, 0, 0x0123).into();
}

impl McListener {
    pub fn new() -> McListener {
        return McListener {
            IPV4: Ipv4Addr::new(224, 0, 0, 123).into(),
            IPV6: Ipv6Addr::new(0xFF02, 0, 0, 0, 0, 0, 0, 0x0123).into()
        }
    }
    pub fn listen(&self, do_listen: Arc<AtomicBool>) {
        let addr = SocketAddr::new(self.IPV4, PORT);

        let t = task::spawn( async move {
            let listener = join_multicast(addr).unwrap();
            while do_listen.load(Relaxed) {
                println!("listening .....");
                let mut buf = [0u8; 64];
                match listener.recv_from(&mut buf) {
                    Ok((len, remote_addr)) => {
                        let data = &buf[..len];

                        println!(
                            "server: got data: {} from: {}",
                            String::from_utf8_lossy(data),
                            remote_addr
                        );

                        // create a socket to send the response
                        let responder = new_socket(&remote_addr)
                            .expect("failed to create responder")
                            .into_udp_socket();

                        // we send the response that was set at the method beginning
                        responder
                            .send_to("yup".as_bytes(), &remote_addr)
                            .expect("failed to respond");

                        println!("server: sent response to: {}", remote_addr);
                    }
                    Err(err) => {
                        println!("server: got an error: {}", err);
                    }
                }

            }
            println!("<< done listening");

        });


        // println!("{}:server: joined: {}", response, IPV4);
        println!("server: joined: {}", addr);
    }

    pub fn hello(&self) -> io::Result<UdpSocket> {
        println!("client: hello!!");

        let message = b"Hello from client!";

        // create the sending socket
        let addr = SocketAddr::new(self.IPV4, PORT);
        let socket = new_sender(&addr).expect("could not create sender!");
        socket
            .send_to(message, &addr)
            .expect("could not send_to!");

        Ok(socket)
    }
}