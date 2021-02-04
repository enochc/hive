// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
use std::{collections::HashMap, fs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::Arc,
    task,
};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use futures::AsyncWriteExt;
use futures::channel::mpsc;
use log::{debug, error, info, trace};
use toml;

#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::handler::Handler;
use crate::peer::{Peer, SocketEvent};
use crate::property::{properties_to_bytes, Property};
use crate::signal::Signal;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type Sender<T> = mpsc::UnboundedSender<T>;
pub type Receiver<T> = mpsc::UnboundedReceiver<T>;

// #[derive(Debug)]
pub struct Hive {
    properties: HashMap<String, Property>,
    sender: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    connect_to: Option<String>,
    bt_connect_to: Option<String>,
    bt_listen: Option<String>,
    listen_port: Option<String>,
    pub name: String,
    peers: Vec<Peer>,
    pub message_received: Signal<String>,
    pub connected: Arc<AtomicBool>,
    advertising: Arc<AtomicBool>,
}

pub(crate) const PROPERTIES: u8 = 0x10;
pub(crate) const PROPERTY: u8 = 0x11;
pub(crate) const DELETE: u8 = 0x12;
pub(crate) const PEER_MESSAGE: u8 = 0x13;
pub(crate) const HEADER: u8 = 0x14;
pub(crate) const PEER_MESSAGE_DIV: &str = "\n";
pub(crate) const PONG: u8 = 0x61;
pub(crate) const HANGUP: u8 = 0x62;
pub(crate) const PING: u8 = 0x63;

// pub(crate) const NEW_PEER:u8 = 0x64;
pub(crate) const PEER_REQUESTS: u8 = 0x65;
pub(crate) const PEER_RESPONSE: u8 = 0x66;
pub(crate) const HEADER_NAME: u8 = 0x67;
pub(crate) const HIVE_PROTOCOL: &str = "HVEP";


#[cfg(feature = "bluetooth")]
fn spawn_bluetooth_listener(do_advertise: bool, sender: Sender<SocketEvent>, ble_name: String) -> Result<()> {
    // let str = ble_name.clone();
    std::thread::spawn(move || {
        let sender_clone = sender.clone();
        let mut rt = tokio::runtime::Builder::new()
            // .basic_scheduler()
            .threaded_scheduler()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut perf = crate::bluetooth::peripheral::Peripheral::new(&ble_name, sender_clone).await;
            perf.run(do_advertise).await.expect("Failed to run peripheral");
        });

        println!("Bluetooth no longer listening");
        sender.close_channel();
    });
    Ok(())
}

#[cfg(feature = "bluetooth")]
async fn spawn_bluetooth_central(ble_name: String, sender: Sender<SocketEvent>) -> Result<()> {
    std::thread::spawn(move || {
        let sender_clone = sender.clone();
        async_std::task::block_on(async {
            let mut central = Central::new(&ble_name, sender_clone);
            while !central.found_device {
                match central.connect().await {
                    Err(err) => error!("Failed to connect bt Central {:?}", err),
                    Ok(_) => {}
                }
                if !central.found_device {
                    debug!("No device found try again later");
                    async_std::task::sleep(Duration::from_secs(10)).await;
                }
            }
        });
        println!("Bluetooth no longer listening");
        sender.close_channel();
    });
    Ok(())
}


impl Hive {
    pub fn is_sever(&self) -> bool {
        self.listen_port.is_some() || self.bt_listen.is_some()
    }

    pub fn is_connected(&self) -> bool {
        return self.connected.load(Ordering::Relaxed);
    }

    pub fn new_from_str(properties: &str) -> Hive {
        let config: toml::Value = toml::from_str(properties).unwrap();
        let prop = |p: &str| {
            return match config.get(p) {
                // Some(v) => Some(v.to_string()),
                Some(v) => Some(String::from(v.as_str().unwrap())),
                _ => None
            };
        };
        let name = prop("name").or_else(|| {
            Some("Unnamed".to_string())
        }).unwrap();

        let connect_to = prop("connect");
        let listen_port = prop("listen");
        let bt_listen = prop("bt_listen");
        let bt_connect_to = prop("bt_connect");


        let props: HashMap::<String, Property> = HashMap::new();

        let (send_chan, receive_chan) = mpsc::unbounded();
        let mut hive = Hive {
            properties: props,
            sender: send_chan,
            receiver: receive_chan,
            connect_to,
            bt_connect_to,
            bt_listen,
            listen_port,
            name,
            peers: Vec::new(),
            message_received: Default::default(),
            connected: Arc::new(AtomicBool::new(false)),
            advertising: Arc::new(AtomicBool::new(false)),
        };

        let properties = config.get("Properties");
        if !properties.is_none() {
            match properties {
                Some(p) => hive.parse_properties(p),
                _ => ()
            }
        };

        return hive;
    }

    pub fn get_handler(&self) -> Handler {
        return Handler {
            sender: self.sender.clone(),
            from_name: self.name.clone(),
        };
    }

    pub fn new(toml_path: &str) -> Hive {
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        Hive::new_from_str(foo.as_str())
    }

    pub fn get_mut_property(&mut self, key: &str) -> Option<&mut Property> {
        if !self.properties.contains_key(key) {
            let p = Property::new(key, None);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
        return op;
    }

    pub fn get_property(&self, key: &str) -> Option<&Property> {
        let op = self.properties.get(key);
        return op;
    }

    fn has_property(&self, key: &str) -> bool {
        return self.properties.contains_key(key);
    }

    fn set_property(&mut self, property: Property) {
        debug!("SET PROPERTY:: {:?}={:?}", property.get_name(), property.value);
        let name = property.get_name().clone();
        if self.has_property(name) {
            /*
            This calls emit on the existing property
             */
            self.get_mut_property(name).unwrap().set_from_prop(property);
        } else {
            // TODO when added for first time, no change event is emitted, EMIT CHANGE!!
            //  De couple add_property and get_mut_property
            self.properties.insert(String::from(name), property);
        }
    }

    fn parse_property(&mut self, key: &str, property: Option<&toml::Value>) {
        let p = Property::from_toml(key, property);
        self.set_property(p);
    }

    fn parse_properties(&mut self, properties: &toml::Value) {
        let p_val = properties.as_table().unwrap();
        for key in p_val.keys() {
            let val = p_val.get(key);
            self.parse_property(key, val)
        }
    }

    /*
        Socket Communication:
            -- handshake / authentication
            -- transfer config /toml

            message [message size, message]
                message size = u32 unsigned 32 bite number, 4 bytes in length
     */

    // servers run this to accept client connections
    async fn accept_loop(mut sender: Sender<SocketEvent>, addr: impl ToSocketAddrs) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            info!("Accepting from: {}", stream.peer_addr()?);
            match stream.peer_addr() {
                Ok(addr) => {
                    let se = SocketEvent::NewPeer {
                        name: "unnamed client".to_string(),
                        stream: Some(stream),
                        peripheral: None,
                        central: None,
                        address: addr.to_string(),
                    };
                    sender.send(se).await.expect("failed to send message");
                }
                Err(e) => eprintln!("No peer address: {:?}", e),
            }
        }

        Ok(())
    }

    pub fn stop(&self) {
        debug!("Stopped called.");
        self.sender.close_channel();
    }

    pub fn get_advertising(&self) -> Arc<AtomicBool> {
        return self.advertising.clone();
    }

    fn setup_stop_listener(&self) {
        let sender = self.sender.clone();
        simple_signal::set_handler(&[
            simple_signal::Signal::Int,
            simple_signal::Signal::Term], {
                                       move |s| {
                                           println!("Signal {:?} received.", s);
                                           sender.close_channel();
                                       }
                                   });
    }


    pub async fn run(&mut self) -> Result<()> {
        self.setup_stop_listener();
        debug!("<< RUN {:?} :: {:?}", self.listen_port, self.bt_connect_to);
        self.advertising.store(true, Ordering::Relaxed);

        // I'm a bluetooth client
        #[cfg(feature = "bluetooth")]
        if self.bt_connect_to.is_some() {
            info!("Connect bluetooth to :{:?}", self.bt_connect_to);
            let name = self.bt_connect_to.as_ref().unwrap().clone();
            spawn_bluetooth_central(name, self.sender.clone()).await
                .expect("Failed to spawn bluetooth central");
            self.connected.store(true, Ordering::Relaxed);
        }

        // I'm a TCP client
        if self.connect_to.is_some() {
            info!("Connect To: {:?}", self.connect_to);
            let address = self.connect_to.as_ref().unwrap().to_string().clone();
            let send_chan = self.sender.clone();
            let (tx, mut rx) = mpsc::unbounded();
            let mut connected = false;
            /* loop indefinitely to connect to remote server */
            while !connected {
                let addr = address.clone();
                let send_chan_clone = send_chan.clone();
                let mut tx_clone = tx.clone();
                task::spawn(async move {
                    let stream = TcpStream::connect(&addr).await;
                    match stream {
                        Ok(s) => {
                            match s.peer_addr() {
                                Ok(_addr) => {
                                    let se = SocketEvent::NewPeer {
                                        name: String::from("unnamed server"),
                                        stream: Some(s.clone()),
                                        peripheral: None,
                                        central: None,
                                        address: addr,
                                    };
                                    send_chan_clone.clone().send(se).await.expect("failed to send peer");


                                    tx_clone.send(true).await.expect("Failed to send connected signal");
                                }
                                Err(e) => error!("No peer address: {:?}", e),
                            }
                        }
                        Err(e) => {
                            error!("Nope:: {:?}", e);
                            tx_clone.send(false).await.expect("Failed to send connect failed signal");
                        }
                    };
                });

                connected = rx.next().await.unwrap();
                if !connected {
                    debug!("failed to connect, retry in a moment");
                    task::sleep(Duration::from_secs(10)).await;
                }
            }

            self.connected.store(true, Ordering::Relaxed);
        }

        // I'm a server
        if self.is_sever() {
            if self.listen_port.is_some() {
                let mut port = self.listen_port.as_ref().unwrap().to_string();
                // port is 000.000.000.000:0000
                if port.len() <= 4 { //only port
                    let addr = local_ipaddress::get().unwrap();
                    port = format!("{}:{}", addr, port);
                }
                info!("{:?} Listening for connections on {:?}", self.name, port);
                let send_chan = self.sender.clone();
                // listen for connections loop
                let p = port.clone();
                task::spawn(async move {
                    match Hive::accept_loop(send_chan, p).await {
                        Err(e) => error!("Failed accept loop: {:?}", e),
                        _ => (),
                    }
                });
            }

            #[cfg(feature = "bluetooth")]
            if self.bt_listen.is_some() {
                info!("!! this is bluetooth");

                let clone = self.bt_listen.as_ref().unwrap().clone();

                spawn_bluetooth_listener(
                    true,
                    self.sender.clone(),
                    clone,
                ).expect("Failed to spawn bluetooth");
            }

            self.connected.store(true, Ordering::Relaxed);
        }

        if self.connected.load(Ordering::Relaxed) {

            //This is where we sit for a long time and just receive events
            self.receive_events().await?;
        }
        debug!("<<<<<<  Hive DONE");
        Ok(())
    }


    async fn receive_events(&mut self) -> Result<()> {
        while !self.sender.is_closed() {
            match self.receiver.next().await {
                Some(SocketEvent::NewPeer {
                         name,
                         stream,
                         peripheral,
                         central,
                         address,
                     }) => {
                    let has_peer = self.peers.iter().find(|p| p.address == address);

                    let is_tcp_server = self.listen_port.is_some();
                    let is_tcp_client = self.connect_to.is_some();
                    debug!("<<<< has peer:: {:?}", has_peer);
                    if has_peer.is_none() {
                        debug!("<<<<< building peer");
                        let p = Peer::new(
                            name,
                            stream,
                            peripheral,
                            central,
                            self.sender.clone(),
                            address,
                            is_tcp_server);
                        info!(".... PEER: {:?}, {}", p, is_tcp_server);

                        // if were a bluetooth peripheral, then we want to keep track of all our
                        // connected remote peers to verify that they're still there
                        // if p.peripheral.is_some() {
                        //     p.wave().await;
                        // }

                        // is_server isn't quite right, bluetooth does an independent properties request
                        // we auto send all the servers via TCP because it prevents un additional request
                        // if is_tcp_server && p.web_sock.is_none() {
                        if is_tcp_server {
                            debug!("......SEND HEADERS");
                            self.send_headers(&p).await?;
                            self.send_properties(&p).await?;
                        }

                        let mut p_stream = p.stream.clone();
                        info!("<<<< NEW PEER: {:?}", p.get_name());
                        self.peers.push(p);

                        if is_tcp_client {
                            // Send client handshake
                            let name_cline = self.name.clone();
                            task::spawn(async move {
                                match p_stream {
                                    None => {}
                                    Some(ref mut st) => {
                                        info!("<<<< SEND HIVE {:?}", HEADER_NAME);
                                        // send hive header to server
                                        let mut bm = BytesMut::new();
                                        bm.put_slice(format!("{}\n", HIVE_PROTOCOL).as_bytes());
                                        bm.put_u8(HEADER_NAME);
                                        bm.put_slice(format!("{}\n", name_cline).as_bytes());
                                        info!(".... HEADER: {:?}", bm);
                                        // st.write(bm.bytes()).await.expect("write failed");
                                        st.write(bm.as_ref()).await.expect("write failed");

                                        st.flush().await.expect("flush failed");
                                        info!("<<<< SENT HIVE");
                                    }
                                }
                            });
                        }
                    } else {
                        debug!("<<< Existing peer reconnected: {:?}", has_peer.unwrap().to_string())
                    }

                    /*
                     //self.emit_peers().await;
                    It makes sense to call this here, but it's better if we
                    wait for the peer response with it's header info so we
                    have the actual peer name that it's given us to response with
                    so instead its called from the "set_headers_from_peer" method
                     */
                }
                Some(SocketEvent::Message { from, msg }) => {
                    debug!("GOT MESSAGE {}, {:?}", from, msg);
                    self.got_message(from.as_str(), msg).await?;
                }
                Some(SocketEvent::Hangup { from }) => {
                    info!("received hangup from {:?}", from);
                    for x in 0..self.peers.len() {
                        if self.peers[x].address() == from {
                            info!("removing peer at index {}", x);
                            self.peers.remove(x);
                            self.notify_peers_change().await;
                            break;
                        }
                    }
                }
                #[allow(unused_variables)]
                Some(SocketEvent::SendBtProps { sender }) => {
                    #[cfg(target_os = "linux")]
                        {
                            debug!("<<<........... HAHAHAHAHAHA {:?}", sender);
                            let str = properties_to_sock_str(&self.properties);
                            let resp = bluster::gatt::event::Response::Success(str.into());
                            sender.send(resp).unwrap();
                        }
                }
                None => {
                    println!("Received Nothing...");
                }
            }
        };
        Ok(())
    }

    fn peer_string(&self) -> String {
        let mut peers_string = String::new();
        debug!("name {:?}", &self.name);

        // Add self to peers list
        let no_port = String::from("No port");
        let myadr = self.listen_port.as_ref().or_else(|| {
            Some(&no_port)
        }).expect("No port").clone();
        let myname = String::from(&self.name);
        peers_string.push_str(&format!("{}|{}", myname, myadr));

        for x in 0..self.peers.len() {
            peers_string.push_str(",");
            let p = &self.peers[x];
            let adr = p.address();
            let name = p.get_name();
            // let name =
            peers_string.push_str(&format!("{}|{}", name, adr))
        };
        return peers_string;
    }

    async fn notify_peers_change(&mut self) {
        debug!("..... notify peers changed");
        use futures::future::join_all;
        let peer_string = self.peer_string();
        let peer_str = peer_string.as_bytes();
        let mut bytes = BytesMut::with_capacity(peer_str.len()+1);
        bytes.put_u8(PEER_RESPONSE);
        bytes.put_slice(peer_str);
        let bf = bytes.freeze();

        let mut futures = vec![];
        self.peers.iter().filter(|p| { p.update_peers }).for_each(|p| {
            futures.push(p.send(bf.clone()));
        });
        join_all(futures).await;
    }

    async fn send_to_peer(&self, msg: &str, peer_name: &str) -> Result<()> {
        let p = self.get_peer_by_name(peer_name);
        match p {
            Some(peer) => {
                let mut bytes = BytesMut::with_capacity(msg.len() + 1);
                bytes.put_u8(PEER_MESSAGE);
                bytes.put_slice(msg.as_bytes());

                peer.send(bytes.freeze()).await?;
            }
            _ => {
                error!("No peer {} ... {}", peer_name, self.name)
            }
        };
        Ok(())
    }

    fn get_peer_by_name(&self, name: &str) -> Option<&Peer> {
        for peer in &self.peers {
            info!("PEER pname: {:?} search_name: {:?} my_name: {:?}", peer.get_name(), name, self.name);
            if peer.get_name() == String::from(name) {
                info!("<< return this peer");
                return Some(peer);
            }
        }
        return None;
    }

    async fn send_properties(&self, peer: &Peer) -> Result<()> {
        let bts = properties_to_bytes(&self.properties);
        peer.send(bts).await?;
        Ok(())
    }

    /*
    Currently this only responds to new connected peers with the peer name
    TODO we could send other init options here such as a property filter etc
     */
    async fn send_headers(&self, peer: &Peer) -> Result<()> {
        let mut bytes = BytesMut::with_capacity(self.name.len() + 3);
        bytes.put_u8(HEADER);
        //bytes.put_u8(PEER_REQUESTS);
        bytes.put_u8(HEADER_NAME);
        bytes.put_slice(self.name.as_bytes());
        trace!(".... send headers: {:?}", bytes);
        peer.send(bytes.freeze()).await?;
        Ok(())
    }

    async fn set_headers_from_peer(&mut self, mut head: Bytes, from: &str) {
        info!(".... Set headers from peer: {:?} = {:?}, self={:?}", from, head, self.name);
        for p in self.peers.as_mut_slice() {
            if p.address() == from {
                while head.remaining() > 0 {
                    match head.get_u8() {
                        HEADER_NAME => {
                            let name = String::from_utf8(head.to_vec()).unwrap();
                            head.advance(name.len());
                            p.set_name(&name).await;
                        }
                        PEER_REQUESTS => {
                            p.update_peers = true;
                        }
                        _ => {
                            info!("unrecognized header: {:?}", head)
                        }
                    }
                }
            }
        }

        if self.is_sever() {
            self.notify_peers_change().await;
        }
    }

    async fn broadcast(&self, msg_type: u8, msg: Option<Bytes>, except: &str) -> Result<()> {
        /*
        Dont broadcast to the same Peer that the original change came from
        to prevent an infinite re-broadcast loop
         */
        match msg {
            Some(m) => {
                // bluetooth clients dont receive independent updates,
                // bluetooth really is a broadcast and only needs to go out once
                let mut sent_bt_broadcast = false;
                for p in &self.peers {
                    debug!("<<<<<<<<<<<< broadcasting: {:?}, {:?}, {:?}", p.get_name(), p.is_bt_client(), m);
                    if p.address() != except {
                        if p.is_bt_client() {
                            if sent_bt_broadcast {
                                // I'm a bluetooth client and the broadcast has already been sent next
                                continue;
                            }
                            sent_bt_broadcast = true;
                        }
                        let mut msg_out = BytesMut::with_capacity(m.len() + 1);
                        msg_out.put_u8(msg_type);
                        msg_out.put_slice(&m);

                        p.send(msg_out.freeze()).await?;
                    }
                }
            }
            _ => {}
        };
        Ok(())
    }

    async fn got_message(&mut self, from: &str, mut msg: Bytes) -> Result<()> {
        info!("MESSAGE: {:?} received: {:?}, from {:?}", self.name, msg, from);
        if msg.len() < 1 {
            debug!("<<< Empty message");
        } else {
            //let (msg_type, message) = msg.split_at(3);
            let msg_type = msg.get_u8();

            match msg_type {
                PING => {
                    debug!("<<< <<< PING from {:?}", from);
                    match self.peers.iter_mut().find(|p| p.address() == from) {
                        None => {}
                        Some(p) => { p.receive_pong(); }
                    }
                }
                PONG => {
                    debug!("<<< PONG FROM {:?}", from);
                    match self.peers.iter_mut().find(|p| p.address() == from) {
                        None => {}
                        Some(p) => { p.send_pong().await?; }
                    }
                }
                HANGUP => {
                    debug!("<<<< hangup from {:?}", from);
                    let pos = self.peers.iter().position(|p| p.address() == from).unwrap();
                    self.peers.remove(pos);
                    self.notify_peers_change().await;
                }
                PROPERTIES => {
                    let message = String::from_utf8(msg.to_vec()).unwrap();
                    match toml::from_str(&message) {
                        Ok(v) => {
                            self.parse_properties(&v);
                        }
                        Err(_) => {
                            panic!("Failed to parse TOML: {:?}", message)
                        }
                    }
                }
                PROPERTY => {
                    info!(".... test 1");
                    let message = String::from_utf8(msg.to_vec()).unwrap();
                    let p_toml: toml::Value = toml::from_str(&message).unwrap();
                    let property = Property::from_table(p_toml.as_table().unwrap());
                    self.set_property(property.unwrap());
                    self.broadcast(PROPERTY, Some(msg), from).await?;
                }
                DELETE => {
                    let message = String::from_utf8(msg.to_vec()).unwrap();
                    let p = self.properties.remove(&message);
                    // this is unnecessary, but fun
                    drop(p);
                    self.broadcast(DELETE, Some(msg), from).await?;
                }
                HEADER => {
                    self.set_headers_from_peer(msg, from).await;
                }
                PEER_MESSAGE => {
                    let message = String::from_utf8(msg.to_vec()).unwrap();
                    let c = message.split(PEER_MESSAGE_DIV);
                    let vec: Vec<&str> = c.collect();
                    if vec.len() == 1 {
                        // the PEER_MESSAGE_DIV separates the peer name from the message
                        // without the DIV, there is only a message and its for me
                        debug!("the message was forwarded for me: {:?}", message);
                        self.message_received.emit(String::from(message)).await;
                        return Ok(());
                    }
                    let pear_name = vec[0];
                    if self.name.to_string() == pear_name.to_string() {
                        debug!("Message is for me: {:?}", vec[1]);
                        self.message_received.emit(String::from(vec[1])).await;
                    } else {
                        self.send_to_peer(vec[1], pear_name).await?;
                    }
                }
                // REQUEST_PEERS => {
                //     // let peer_str = format!("{}{}", REQUEST_PEERS, self.peer_string());
                //     let str = self.peer_string();
                //     let mut peer_bytes = BytesMut::with_capacity(str.len()+1);
                //     peer_bytes.put_u8(REQUEST_PEERS);
                //     peer_bytes.put_slice(str.as_bytes());
                //
                //     for p in self.peers.as_mut_slice() {
                //         if p.address() == from {
                //             p.send(peer_bytes.freeze()).await?;
                //             p.update_peers = true;
                //             break;
                //         }
                //     }
                // }
                _ => unimplemented!("Unknown message {:?}", msg_type)
            }
            // do ACK for peer
            match self.peers.iter_mut().find(|p| p.address() == from) {
                None => {}
                Some(p) => { p.ack().await; }
            }
        }
        Ok(())
    }
    // fn get_peer_by_address(&self, addr:&str)->Option<&Peer>{
    //     for peer in &self.peers {
    //         if peer.address() == String::from(addr) {
    //             return Some(peer)
    //         }
    //     }
    //     return None
    // }
}

