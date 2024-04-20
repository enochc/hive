// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
// #![feature(async_closure)]
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
use futures::channel::mpsc;
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use toml;

#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::handler::Handler;
use crate::peer::{Peer, SocketEvent, PeerType};
use crate::property::{properties_to_bytes, Property, bytes_to_property, property_to_bytes};
use crate::signal::Signal;
use std::sync::{Condvar, Mutex, Weak};
use std::fmt::{Debug, Formatter};
use toml::Value;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type Sender<T> = mpsc::UnboundedSender<T>;
pub type Receiver<T> = mpsc::UnboundedReceiver<T>;

// #[derive(Debug)]
pub struct Hive {
    pub properties: HashMap<u64, Property>,
    sender: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    connect_to: Option<String>,
    #[allow(dead_code)]
    bt_connect_to: Option<String>,
    bt_listen: Option<String>,
    listen_port: Option<String>,
    pub name: String,
    peers: Vec<Peer>,
    pub message_received: Signal<String>,
    pub connected: Arc<AtomicBool>,
    advertising: Arc<AtomicBool>,
    ready: Option<Arc<(Mutex<bool>, Condvar)>>
}

impl Debug for Hive {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Hive").field("name", &self.name).finish()
    }
}

pub(crate) const PROPERTIES: u8 = 0x10;
pub(crate) const PROPERTY: u8 = 0x11;
pub(crate) const DELETE: u8 = 0x12;
pub(crate) const PEER_MESSAGE: u8 = 0x13;
pub(crate) const HEADER: u8 = 0x72;
pub(crate) const PEER_MESSAGE_DIV: &str = "\n";
pub(crate) const PONG: u8 = 0x61;
pub(crate) const HANGUP: u8 = 0x62;
pub(crate) const PING: u8 = 0x63;

// pub(crate) const NEW_PEER:u8 = 0x64;
pub(crate) const PEER_REQUESTS: u8 = 0x65;
pub(crate) const PEER_RESPONSE: u8 = 0x66;
pub(crate) const HEADER_NAME: u8 = 0x78;
pub(crate) const HIVE_PROTOCOL: &str = "HVEP";



#[cfg(feature = "bluetooth")]
fn spawn_bluetooth_listener(do_advertise: bool, sender: Sender<SocketEvent>, ble_name: String) -> Result<()> {
    // let str = ble_name.clone();
    std::thread::spawn(move || {
        let sender_clone = sender.clone();
        // let mut rt = tokio_0_2::runtime::Builder::new()
        //     // .basic_scheduler()
        //     .threaded_scheduler()
        //     .enable_all()
        //     .build()
        //     .unwrap();
        let mut rt = tokio::runtime::Builder::new_multi_thread()
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

    // wait will wait until the Hive service is running before returning
    pub fn go(mut self, wait:bool)->Handler {
        //This consumes the Hive and returns a handler and spawns it's run thread in a task
        if wait {
            self.ready = Some(
                Arc::new((Mutex::new(false), Condvar::new()))
            );
        }

        let handler = self.get_handler();
        let ready_clone = self.ready.clone();
        task::spawn(async move {
            self.run().await.expect("Failed to run Hive");
        });

        return match ready_clone {
            None => {
                handler
            }
            Some(r) => {
                let (lock, cvar) = &*r;
                let lock = lock.lock().unwrap();
                let guard = cvar.wait(lock).expect("Wait on lock failed");
                handler
            }
        }
    }
    fn broadcast_ready(&self){
        match self.ready.clone() {
            None => {}
            Some(ready) => {
                let (_lock, cvar) = &*ready;
                cvar.notify_all();
            }
        }
    }

    pub fn new_from_str(properties: &str) -> Hive {

        let config: toml::Value = toml::from_str(properties).unwrap();
        let prop = |p: &str| {
            return match config.get(p) {
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


        let props: HashMap::<u64, Property> = HashMap::new();

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
            ready: None
        };

        let properties = config.get("Properties");

        match properties {
            Some(p) => hive.parse_properties(p),
            _ => ()
        };

        // fetch initial values from the rest api
        #[cfg(feature = "rest")]
        match config.get("REST"){
            Some(t) => {
                match t.get("get"){
                    Some(rest) => {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_io()
                            .enable_time()
                            .build()
                            .unwrap();
                        rt.block_on(async move {
                            let url = rest.to_string();
                            debug!("<<<<<< rest: {:?}", url);
                            let resp = reqwest::get("http://127.0.0.1:8000/hive/get").await.unwrap()
                                .json::<HashMap<String, Value>>()
                                .await.unwrap();

                            println!("{:#?}", resp);
                            // TODO finish this!!
                        });
                    }
                    None => {}
                }
            }
            None => {}
        }


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

    pub fn get_mut_property(&mut self, key: &u64) -> Option<&mut Property> {
        if !self.properties.contains_key(key) {
            let p = Property::from_id(key, None);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
        return op;
    }
    pub fn get_mut_property_by_name(&mut self, name: &str) -> Option<&mut Property> {
        let key = &Property::hash_id(name);
        if !self.properties.contains_key(key) {
            let p = Property::from_id(key, None);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
        return op;
    }

    pub fn get_property(&self, key: &u64) -> Option<&Property> {
        let op = self.properties.get(key);
        return op;
    }

    fn has_property(&self, key: &u64) -> bool {
        return self.properties.contains_key(key);
    }

    fn set_property(&mut self, property: Property) {
        debug!("{:?} SET PROPERTY: {:?}", property.get_name(), property);
        let id = &property.id;
        if self.has_property(id) {
            /*
            This calls emit on the existing property
             */
            self.get_mut_property(id).unwrap().set_from_prop(property);
        } else {
            // TODO when added for first time, no change event is emitted, EMIT CHANGE!!
            //  De couple add_property and get_mut_property
            self.properties.insert(*id, property);
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
    async fn accept_loop(mut sender: Sender<SocketEvent>, addr: impl ToSocketAddrs, hive_name:String) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            debug!("{:?} ------------  Accepting from: {}", hive_name, stream.peer_addr()?);
            match stream.peer_addr() {
                Ok(addr) => {
                    let se = SocketEvent::NewPeer {
                        name: "unnamed client".to_string(),
                        stream: Some(stream),
                        peripheral: None,
                        central: None,
                        address: addr.to_string(),
                        ptype: PeerType::TcpClient,
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


    pub async fn run(& mut self) -> Result<()> {
        self.setup_stop_listener();
        self.advertising.store(true, Ordering::Relaxed);

        // I'm a bluetooth client
        #[cfg(feature = "bluetooth")]
        if self.bt_connect_to.is_some() {
            debug!("Connect bluetooth to :{:?}", self.bt_connect_to);
            let name = self.bt_connect_to.as_ref().unwrap().clone();
            spawn_bluetooth_central(name, self.sender.clone()).await
                .expect("Failed to spawn bluetooth central");
            self.connected.store(true, Ordering::Relaxed);
        }

        // I'm a TCP client
        if self.connect_to.is_some() {
            debug!("{} Connect To: {:?}", self.name, self.connect_to);
            let mut address = self.connect_to.as_ref().unwrap().to_string().clone();
            if address.len() <= 4 { //only port
                let addr = local_ipaddress::get().unwrap();
                address = format!("{}:{}", addr, address);
            }

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
                                        ptype: PeerType::TcpServer,
                                    };
                                    send_chan_clone.clone().send(se).await.expect("failed to send peer");


                                    tx_clone.send(true).await.expect("Failed to send connected signal");
                                },
                                Err(e) => {
                                    let msg = format!("No peer address: {:?}", e);
                                    warn!("{:?}", msg);

                                }
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
                    warn!("failed to connect, retry in a moment");
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
                debug!("{:?} Listening for connections on {:?}", self.name, port);
                let send_chan = self.sender.clone();
                // listen for connections loop
                let p = port.clone();
                let name_clone = self.name.clone();
                task::spawn(async move {
                    match Hive::accept_loop(send_chan, p, name_clone).await {
                        Err(e) => {
                            panic!("Failed accept loop: {:?}", e)
                        },
                        _ => (),
                    }
                });
                // Ima TCP listener and I'm listening
                self.broadcast_ready();
            }

            #[cfg(feature = "bluetooth")]
            if self.bt_listen.is_some() {
                debug!("!! this is bluetooth");

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
        debug!("Hive DONE");
        Ok(())
    }


    async fn receive_events(& mut self) -> Result<()> {
        while !self.sender.is_closed() {
            debug!("{:?} waiting for event",self.name);
            let nn = self.receiver.next().await;
            debug!("{:?} Received event: {:?}",self.name, nn);
            match nn {
                Some(SocketEvent::NewPeer {
                         name,
                         stream,
                         peripheral,
                         central,
                         address,
                         ptype,
                     }) => {
                    let has_peer = self.peers.iter().find(|p| p.address == address);

                    let is_tcp_server = self.listen_port.is_some();
                    let is_tcp_client = self.connect_to.is_some();
                    if has_peer.is_none() {
                        debug!("----------------------- {:?} building peer: {:?}", self.name, name);
                        let ptype_clne = ptype.clone();
                        let p = Peer::new(
                            name,
                            stream,
                            peripheral,
                            central,
                            self.sender.clone(),
                            address,
                        self.name.clone(),
                            ptype_clne,
                        ).await;
                        debug!(".... PEER: {:?}, {}", p, is_tcp_server);


                        // we auto send all the peers via TCP because it prevents un additional request
                        // TODO should this be moved into the Peer handshake
                        if ptype == PeerType::TcpClient {
                            self.send_properties(&p).await?;
                        }


                        debug!("<<<< NEW PEER for {:?}: {:?}",self.name, p.get_name());
                        let is_web_sock_peer = p.is_web_socket();

                        self.peers.push(p);

                        if !is_tcp_client {
                            // I'm a server and another tcp hive client connected
                            // a websock sends a separate Header package with it's name, other
                            // tcp sockets send the name in the handshake so we can update peers
                            // right away, instead of waiting for the header update
                            if !is_web_sock_peer {
                                self.notify_peers_change().await;
                            }
                        }
                    } else {
                        debug!("<<< Existing peer reconnected: {:?}", has_peer.unwrap().to_string());
                    }

                    // We've connected or been connected to, so were ready by most definitions.
                    self.broadcast_ready();

                }
                Some(SocketEvent::Message { from, msg }) => {
                    debug!("GOT MESSAGE {}, {:?}", from, msg);
                    self.got_message(from.as_str(), msg).await?;
                }
                Some(SocketEvent::Hangup { from }) => {
                    debug!("received hangup from {:?}", from);
                    for x in 0..self.peers.len() {
                        if self.peers[x].address() == from {
                            debug!("removing peer at index {}", x);
                            self.peers.remove(x);
                            self.notify_peers_change().await;
                            break;
                        }
                    }
                }
                #[allow(unused_variables)]
                Some(SocketEvent::SendBtProps { sender }) => {
                    #[cfg(all(target_os = "linux", feature = "bluetooth"))]
                        {
                            debug!("<<<........... HAHAHAHAHAHA {:?}", sender);
                            let bytes = properties_to_bytes(&self.properties);
                            let resp = bluster::gatt::event::Response::Success(bytes.to_vec());
                            sender.send(resp).unwrap();
                        }
                }
                None => {
                    println!("Received Nothing...");
                }
            }
        };
        info!("Stopped receiving events!");
        Ok(())
    }

    fn peer_string(&self) -> String {
        let mut peers_string = String::new();

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
        // debug!(".. TEST 3: {:?}", bf);

        let mut futures = vec![];
        self.peers.iter().filter(|p| { p.update_peers }).for_each(|p| {
            futures.push(p.send(bf.clone()));
        });
        debug!("send peers update to {:?} peers", futures.len());
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
                let err_msg = format!("No peer {} ... {}", peer_name, self.name);
                panic!("{}", err_msg);
            }
        };
        Ok(())
    }

    fn get_peer_by_name(&self, name: &str) -> Option<&Peer> {
        for peer in &self.peers {
            debug!("PEER pname: {:?} search_name: {:?} my_name: {:?}", peer.get_name(), name, self.name);
            if peer.get_name() == String::from(name) {
                debug!("<< return this peer");
                return Some(peer);
            }
        }
        return None;
    }

    // TODO send properties with handshake
    async fn send_properties(&self, peer: &Peer) -> Result<()> {
        let bts = properties_to_bytes(&self.properties);
        peer.send(bts).await?;
        Ok(())
    }

    pub fn get_weak_property(&mut self, prop_name:&str) -> Weak<Mutex<Property>> {

        // let prop = self.get_mut_property(&Property::hash_id(prop_name)).expect("nopers");
        let mp = Arc::new(Mutex::new(self.get_mut_property(&Property::hash_id(prop_name)).expect("nopers").clone()));
        Arc::downgrade(&mp).clone()
    }


    async fn set_headers_from_peer(&mut self, mut head: Bytes, from: &str) {
        debug!(".... Set headers from peer: {:?} = {:?}, self={:?}", from, head, self.name);
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
                            debug!("unrecognized header: {:?}", head);
                        }
                    }
                }
            }
        }

        if self.is_sever() {
            // Web sockets send a separate header update with its name
            // other Hive sockets send the name in the initial connection heder.
            self.notify_peers_change().await;
        }
    }

    async fn broadcast(&self, msg_type: u8, property: &Property, except: &str) -> Result<()> {
        /*
        Dont broadcast to the same Peer that the original change came from
        to prevent an infinite re-broadcast loop
         */

        // bluetooth clients dont receive independent updates,
        // bluetooth really is a broadcast and only needs to go out once
        let mut sent_bt_broadcast = false;
        let msg = property_to_bytes(property, false);
        for p in &self.peers {
            debug!("{} broadcasting: {:?}, {:?}",self.name, p.get_name(), p.is_bt_client(),);
            if p.address() != except {
                debug!("... TEST 1");
                if p.is_bt_client() {
                    if sent_bt_broadcast {
                        // I'm a bluetooth client and the broadcast has already been sent next
                        continue;
                    }
                    sent_bt_broadcast = true;
                }
                let mut msg_out = BytesMut::with_capacity(msg.len() + 1);
                msg_out.put_u8(msg_type);
                msg_out.put_slice(&msg.clone());
                debug!("<<{} sending:{:?} {}", self.name, msg_out, msg_out.len());
                p.send(msg_out.freeze()).await?;
            }
        }


        Ok(())
    }

    async fn got_message(&mut self, from: &str, mut msg: Bytes) -> Result<()> {
        debug!("MESSAGE: {:?} received: {:?}, from {:?}", self.name, msg, from);
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
                    debug!("properties:: {:?}", msg);
                    while msg.has_remaining().clone(){
                        let p = bytes_to_property(&mut msg);
                        debug!("thing: {:?}", p);
                        match p{
                            None => {}
                            Some(p) => {
                                self.set_property(p)
                            }
                        }
                    }

                    debug!("done");
                }

                PROPERTY => {
                    let p = bytes_to_property(&mut msg);
                    debug!("property: {:?}", p);
                    match p{
                        None => {
                            unimplemented!("Why are you here?")
                        }
                        Some(p) => {
                            if p.is_none() {
                                debug!("DELETING {:?}", p.get_name());
                                self.broadcast(DELETE, &p, from).await?;
                                self.properties.remove(&p.id);
                            } else {
                                self.broadcast(PROPERTY, &p, from).await?;
                                self.set_property(p);
                            }
                        }
                    }
                }
                // todo Delete is redundant, setting the value to None does the same thing
                DELETE => {
                    // let message = String::from_utf8(msg.to_vec()).unwrap();
                    let p = self.properties.remove(&msg.get_u64());
                    match p{
                        None => {}
                        Some(p) => {
                            self.broadcast(DELETE, &p, from).await?;
                            // this is unnecessary, but fun
                            drop(p);
                        }
                    }
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
                        info!("the message was forwarded for me: {:?}", message);
                        self.message_received.emit(String::from(message)).await;
                        return Ok(());
                    }
                    let pear_name = vec[0];
                    if self.name.to_string() == pear_name.to_string() {
                        info!("Message is for me: {:?}", vec[1]);
                        self.message_received.emit(String::from(vec[1])).await;
                    } else {
                        self.send_to_peer(vec[1], pear_name).await?;
                    }
                }
                PEER_REQUESTS => {
                    let str = self.peer_string();
                    let mut peer_bytes = BytesMut::with_capacity(str.len()+1);
                    peer_bytes.put_u8(PEER_RESPONSE);
                    peer_bytes.put_slice(str.as_bytes());

                    for p in self.peers.as_mut_slice() {
                        if p.address() == from {
                            p.send(peer_bytes.freeze()).await?;
                            p.update_peers = true;
                            break;
                        }
                    }
                }
                _ => unimplemented!("Unknown message {:#02x}", msg_type)//error!("Unknown message {:?}", msg_type)
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

