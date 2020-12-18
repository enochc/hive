// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
use std::{collections::HashMap, fs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::Arc,
    task,
};
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
use log::{debug, error, info};
use toml;

#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::handler::Handler;
use crate::peer::{Peer, SocketEvent};
use crate::property::{properties_to_sock_str, Property, PropertyType};
use crate::signal::Signal;

// use usbd_serial::{SerialPort, USB_CLASS_CDC, UsbError};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type Sender<T> = mpsc::UnboundedSender<T>;
pub type Receiver<T> = mpsc::UnboundedReceiver<T>;

// #[derive(Debug)]
pub struct Hive {
    pub properties: HashMap<String, Property>,
    sender: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    connect_to: Option<String>,
    bt_connect_to: Option<String>,
    bt_listen: Option<String>,
    listen_port: Option<String>,
    pub name: Box<str>,
    peers: Vec<Peer>,
    pub message_received: Signal<String>,
    pub connected: Arc<AtomicBool>,
    advertising: Arc<AtomicBool>,
}


pub(crate) const PROPERTIES: &str = "|P|";
pub(crate) const PROPERTY: &str = "|p|";
pub(crate) const DELETE: &str = "|d|";
pub(crate) const PEER_MESSAGE: &str = "|s|";
pub(crate) const HEADER: &str = "|H|";
pub(crate) const PEER_MESSAGE_DIV: &str = "|=|";
pub(crate) const REQUEST_PEERS: &str = "<p|";
// pub (crate) const ACK:&str = "<<|";

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

    pub fn new_from_str(name: &str, properties: &str) -> Hive {
        let config: toml::Value = toml::from_str(properties).unwrap();
        let prop = |p: &str| {
            return match config.get(p) {
                // Some(v) => Some(v.to_string()),
                Some(v) => Some(String::from(v.as_str().unwrap())),
                _ => None
            };
        };
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
            name: String::from(name).into_boxed_str(),
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
        };
    }

    pub fn new(name: &str, toml_path: &str) -> Hive {
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        Hive::new_from_str(name, foo.as_str())
    }

    pub fn get_mut_property(&mut self, key: &str, def_val: Option<PropertyType>) -> Option<&mut Property> {
        // property value must be initialized to something if it doesn't currenlty exist
        // otherwise Hive will treat the property as a delete
        let val = match def_val {
            Some(v) => Some(v),
            None => Some(true.into())
        };
        if !self.properties.contains_key(key) {
            let p = Property::new(key, val);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
        return op;
    }
    fn has_property(&self, key: &str) -> bool {
        return self.properties.contains_key(key);
    }

    fn set_property(&mut self, property: Property) {
        let name = property.get_name().clone();
        if self.has_property(name) {
            /*
            This calls emit on the existing property
             */
            self.get_mut_property(name, None).unwrap().set_from_prop(property);
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
                        name: addr.to_string(),
                        stream: Some(stream),
                        peripheral: None,
                        central: None,
                        address: None,
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


    pub async fn run(&mut self) {//} -> Result<()> {
        self.setup_stop_listener();
        println!("<< RUN {:?} :: {:?}", self.listen_port, self.bt_connect_to);
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
                                        name: String::from(""),
                                        stream: Some(s),
                                        peripheral: None,
                                        central: None,
                                        address: None,
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
        // if self.listen_port.is_some() || self.bt_listen.is_some() {
        if self.is_sever() {
            if self.listen_port.is_some() {
                let port = self.listen_port.as_ref().unwrap().to_string().clone();
                info!("{:?} Listening for connections on {:?}", self.name, self.listen_port);
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
            self.receive_events(true).await;
        }
        debug!("<<<<<<  Hive DONE");
    }


    async fn receive_events(&mut self, is_server: bool) {
        while !self.sender.is_closed() {
            match self.receiver.next().await {
                Some(SocketEvent::NewPeer {
                         name,
                         stream,
                         address,
                         central,
                         peripheral,
                     }) => {
                    let p = Peer::new(
                        name,
                        stream,
                        peripheral,
                        central,
                        self.sender.clone(),
                        address, );
                    debug!("<<<< NEW PEER: {:?}", p.name);
                    if is_server {
                        self.send_properties(&p).await;
                    }
                    // Send the header with my "peer name"
                    self.send_header(&p).await;
                    self.peers.push(p);

                    /*
                     //self.emit_peers().await;
                    It makes sense to call this here, but it's better if we
                    wait for the peer response with it's header info so we
                    have the actual peer name that it's given us to response with
                    so instead its called from the "set_headers_from_peer" method
                     */
                }
                Some(SocketEvent::Message { from, msg }) => {
                    self.got_message(from.as_str(), msg).await;
                }
                Some(SocketEvent::Hangup { from }) => {
                    for x in 0..self.peers.len() {
                        if self.peers[x].address() == from {
                            self.peers.remove(x);
                            self.notify_peers_change().await;
                            break;
                        }
                    }
                }
                None => {
                    println!("Received Nothing...");
                }
            }
        }
    }

    fn peer_string(&self) -> String {
        let mut peers_string = String::new();
        debug!("name {:?}", &self.name);

        // Add self to peers list
        let myadr = self.listen_port.as_ref().expect("No port").clone();
        let myname = String::from(self.name.as_ref());
        peers_string.push_str(&format!("{}|{}", myname, myadr));

        for x in 0..self.peers.len() {
            peers_string.push_str(",");
            let p = &self.peers[x];
            let adr = p.address();
            let name = &p.name;
            peers_string.push_str(&format!("{}|{}", name, adr))
        };
        return peers_string;
    }

    async fn notify_peers_change(&mut self) {
        let peer_str = format!("{}{}", REQUEST_PEERS, self.peer_string());
        for p in &self.peers {
            if p.update_peers {
                let stream = p.stream.clone().unwrap();
                let msg = peer_str.clone();
                task::spawn(async move {
                    Peer::send_on_stream(&stream, &msg).await.unwrap();
                });
            }
        }
    }


    async fn send_to_peer(&self, msg: &str, peer_name: &str) {
        let p = self.get_peer_by_name(peer_name);
        match p {
            Some(peer) => {
                let msg = format!("{}{}", PEER_MESSAGE, msg);
                peer.send(msg.as_str()).await;
            }
            _ => error!("No peer {}", peer_name)
        }
    }

    fn get_peer_by_name(&self, name: &str) -> Option<&Peer> {
        for peer in &self.peers {
            if peer.name == String::from(name) {
                return Some(peer);
            }
        }
        return None;
    }

    async fn send_properties(&self, peer: &Peer) {
        let str = properties_to_sock_str(&self.properties);
        peer.send(str.as_str()).await;
    }

    /*
    Currently this only responds to new connected peers with the peer name
    TODO we could send other init options here such as a property filter etc
     */
    async fn send_header(&self, peer: &Peer) {
        let mut str = HEADER.to_string();
        str.push_str(format!("NAME={}", self.name).as_ref());
        peer.send(str.as_ref()).await;
    }

    async fn set_headers_from_peer(&mut self, head: &str, from: &str) {
        for p in self.peers.as_mut_slice() {
            if p.address() == from {
                let vec: Vec<&str> = head.split("NAME=").collect();
                let name = vec[1];
                p.set_name(name);
            }
        }

        if self.is_sever() {
            self.notify_peers_change().await;
        }
    }

    async fn broadcast(&self, msg: Option<String>, except: &str) {
        /*
        Dont broadcast to the same Peer that the original change came from
        to prevent an infinite re-broadcast loop
         */
        match msg {
            Some(m) => {
                for p in &self.peers {
                    if p.address() != except {
                        p.send(m.as_str()).await;
                    }
                }
            }
            _ => {}
        }
    }

    async fn got_message(&mut self, from: &str, msg: String) {
        debug!("GOT MESSAGE: {}: {:?}", self.name, msg);
        if msg.len() < 3 {
            debug!("<<< Whats this? {}", msg);
        } else {
            let (msg_type, message) = msg.split_at(3);
            match msg_type {
                PROPERTIES => {
                    let map: toml::Value = toml::from_str(message).unwrap();
                    self.parse_properties(&map);
                }
                PROPERTY => {
                    let p_toml: toml::Value = toml::from_str(message).unwrap();
                    let property = Property::from_table(p_toml.as_table().unwrap());
                    self.set_property(property.unwrap());
                    self.broadcast(Some(msg), from).await;
                }
                DELETE => {
                    let p = self.properties.remove(&message.to_owned().clone());
                    // this is unnecessary, but fun
                    drop(p);
                    self.broadcast(Some(msg), from).await;
                }
                HEADER => {
                    self.set_headers_from_peer(msg.as_ref(), from).await;
                }
                PEER_MESSAGE => {
                    let c = message.split(PEER_MESSAGE_DIV);
                    let vec: Vec<&str> = c.collect();
                    if vec.len() == 1 {
                        // the PEER_MESSAGE_DIV separates the peer name from the message
                        // without the DIV, there is only a message and its for me
                        debug!("the message was forwarded for me: {:?}", message);
                        self.message_received.emit(String::from(message)).await;
                        return;
                    }
                    let pear_name = vec[0];
                    if self.name.to_string() == pear_name.to_string() {
                        debug!("Message is for me: {:?}", vec[1]);
                        self.message_received.emit(String::from(vec[1])).await;
                    } else {
                        self.send_to_peer(vec[1], pear_name).await;
                    }
                }
                REQUEST_PEERS => {
                    let peer_str = format!("{}{}", REQUEST_PEERS, self.peer_string());
                    for p in self.peers.as_mut_slice() {
                        if p.address() == from {
                            p.send(&peer_str).await;
                            p.update_peers = true;
                            break;
                        }
                    }
                }
                _ => debug!("got unknown message {:?},{:?}", msg_type, msg)
            }
        }
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

