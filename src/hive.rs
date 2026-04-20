// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
// #![feature(async_closure)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{collections::HashMap, fs};

use tokio::net::{TcpListener, TcpStream};
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use crate::futures::channel::mpsc;
use crate::futures::{SinkExt, StreamExt};
use tracing::{info, debug, warn, error};

use toml;

#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::file_transfer::{FileTransferManager, ReceivedFile,
    FILE_HEADER, FILE_CHUNK, FILE_COMPLETE, FILE_CANCEL};
use crate::handler::Handler;
#[cfg(feature = "mqtt")]
use crate::mqtt::mqtt_peer::{MqttConfig, MqttPeerHandle, spawn_mqtt_peer};
use crate::peer::{Peer, PeerType, PropertyFilter, SocketEvent};
use crate::property::{bytes_to_property, properties_to_bytes, property_to_bytes, Property};
use crate::signal::Signal;
use ctrlc::Error;
use std::fmt::{Debug, Formatter};
use std::sync::{Mutex, Weak};
use tokio_util::sync::CancellationToken;
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
    pub file_received: Signal<ReceivedFile>,
    file_transfers: FileTransferManager,
    pub connected: Arc<AtomicBool>,
    advertising: Arc<AtomicBool>,
    /// This node's property filter, sent to peers during handshake so
    /// they know which properties to forward.
    property_filter: PropertyFilter,
    ready: Option<Arc<tokio::sync::Notify>>,
    #[cfg(feature = "mqtt")]
    mqtt_config: Option<MqttConfig>,
    #[cfg(feature = "mqtt")]
    mqtt_handle: Option<MqttPeerHandle>,
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
pub(crate) const ADDRESS_UPDATE: u8 = 0x67;
pub(crate) const HEADER_NAME: u8 = 0x78;
pub(crate) const HEADER_PROP_INCLUDE: u8 = 0x79;
pub(crate) const HEADER_PROP_EXCLUDE: u8 = 0x7A;
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
        let mut rt = tokio::runtime::Builder::new_multi_thread().build().unwrap();
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
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut central = Central::new(&ble_name, sender_clone);
            while !central.found_device {
                match central.connect().await {
                    Err(err) => error!("Failed to connect bt Central {:?}", err),
                    Ok(_) => {}
                }
                if !central.found_device {
                    debug!("No device found try again later");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        });
        println!("Bluetooth no longer listening");
        sender.close_channel();
    });
    Ok(())
}

pub(crate) const REQUEST_PEERS: &str = "<p|";

impl Hive {
    pub fn is_server(&self) -> bool {
        self.listen_port.is_some() || self.bt_listen.is_some()
    }

    pub fn is_connected(&self) -> bool {
        return self.connected.load(Ordering::Relaxed);
    }

    /// Check whether an IP address string falls within a private/internal
    /// range (RFC 1918).  Returns true for 10.x.x.x, 172.16-31.x.x,
    /// 192.168.x.x, and 127.x.x.x (loopback).
    fn is_private_ip(ip: &str) -> bool {
        if ip.starts_with("10.") || ip.starts_with("192.168.") || ip.starts_with("127.") {
            return true;
        }
        // 172.16.0.0 - 172.31.255.255
        if let Some(rest) = ip.strip_prefix("172.") {
            if let Some(second_octet_str) = rest.split('.').next() {
                if let Ok(second) = second_octet_str.parse::<u8>() {
                    return (16..=31).contains(&second);
                }
            }
        }
        false
    }

    pub async fn go(mut self, wait: bool, cancellation_token: CancellationToken) -> Handler {
        //This consumes the Hive and returns a handler and spawns it's run thread in a task
        let notify = if wait {
            let n = Arc::new(tokio::sync::Notify::new());
            self.ready = Some(n.clone());
            Some(n)
        } else {
            None
        };

        let handler = self.get_handler();
        tokio::spawn(async move {
            self.run(cancellation_token).await.expect("Failed to run Hive");
        });

        if let Some(n) = notify {
            info!("Waiting for Hive connection");
            n.notified().await;
            info!("Hive connection ready");
        }
        handler
    }
    fn broadcast_ready(&self) {
        if let Some(notify) = &self.ready {
            notify.notify_one();
        }
    }

    pub fn new_from_str_unknown(properties: &str) -> Hive {
        Self::new_from_str("Unknown", properties)
    }

    pub fn new_from_str(name: &str, properties: &str) -> Hive {
        let config: Value = toml::from_str(properties).unwrap();
        let prop = |p: &str| {
            return match config.get(p) {
                Some(v) => Some(String::from(v.as_str().unwrap())),
                _ => None,
            };
        };
        let name = prop("name").or_else(|| Some(name.to_string())).unwrap();

        let connect_to = prop("connect");
        let listen_port = prop("listen");
        let bt_listen = prop("bt_listen");
        let bt_connect_to = prop("bt_connect");

        #[cfg(feature = "mqtt")]
        let mqtt_config = config.get("MQTT").and_then(MqttConfig::from_toml);

        // Parse property filter from [Filters] section
        let property_filter = match config.get("Filters") {
            Some(filters) => {
                if let Some(include) = filters.get("include") {
                    if let Some(arr) = include.as_array() {
                        let prefixes: Vec<String> = arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !prefixes.is_empty() {
                            info!("property filter: include {:?}", prefixes);
                            PropertyFilter::Include(prefixes)
                        } else {
                            PropertyFilter::All
                        }
                    } else {
                        PropertyFilter::All
                    }
                } else if let Some(exclude) = filters.get("exclude") {
                    if let Some(arr) = exclude.as_array() {
                        let prefixes: Vec<String> = arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !prefixes.is_empty() {
                            info!("property filter: exclude {:?}", prefixes);
                            PropertyFilter::Exclude(prefixes)
                        } else {
                            PropertyFilter::All
                        }
                    } else {
                        PropertyFilter::All
                    }
                } else {
                    PropertyFilter::All
                }
            }
            None => PropertyFilter::All,
        };

        let props: HashMap<u64, Property> = HashMap::new();

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
            file_received: Signal::new(),
            file_transfers: FileTransferManager::new(),
            connected: Arc::new(AtomicBool::new(false)),
            advertising: Arc::new(AtomicBool::new(false)),
            property_filter,
            ready: None,
            #[cfg(feature = "mqtt")]
            mqtt_config,
            #[cfg(feature = "mqtt")]
            mqtt_handle: None,
        };

        let properties = config.get("Properties");

        if !properties.is_none() {
            match properties {
                Some(p) => hive.parse_properties(p),
                _ => (),
            }
        }
        // match properties {
        //     Some(p) => hive.parse_properties(p),
        //     _ => ()
        // };

        // fetch initial values from the rest api
        #[cfg(feature = "rest")]
        match config.get("REST") {
            Some(t) => {
                match t.get("get") {
                    Some(rest) => {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_io()
                            .enable_time()
                            .build()
                            .unwrap();
                        rt.block_on(async move {
                            let url = rest.to_string();
                            debug!("<<<<<< rest: {:?}", url);
                            let resp = reqwest::get("http://127.0.0.1:8000/hive/get")
                                .await
                                .unwrap()
                                .json::<HashMap<String, Value>>()
                                .await
                                .unwrap();

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
        Hive::new_from_str_unknown(foo.as_str())
    }

    pub fn get_mut_property(&mut self, key: &u64) -> Option<&mut Property> {
        if !self.properties.contains_key(key) {
            let p = Property::from_id(key, None);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
        return op;
    }
    // pub fn get_mut_property_by_name(&mut self, name: &str) -> Option<&mut Property> {
    //     let key = &Property::hash_id(name);
    //     if !self.properties.contains_key(key) {
    //         let p = Property::from_id(key, None);
    //         self.set_property(p);
    //     }
    //
    //     let op = self.properties.get_mut(key);
    //     return op;
    // }
    pub fn get_mut_property_by_name(&mut self, name: &str) -> Option<&mut Property> {
        let key = &Property::hash_id(name);
        if !self.properties.contains_key(key) {
            let p = Property::from_id(key, None);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
         op
    }

    pub fn get_property(&self, key: &u64) -> Option<&Property> {
        let op = self.properties.get(key);
         op
    }

    fn has_property(&self, key: &u64) -> bool {
         self.properties.contains_key(key)
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
    async fn accept_loop(mut sender: Sender<SocketEvent>, listener: TcpListener, hive_name: String) -> Result<()> {
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            debug!("{:?} ------------  Accepting from: {}", hive_name, peer_addr);
            let se = SocketEvent::NewPeer {
                name: "unnamed client".to_string(),
                stream: Some(stream),
                peripheral: None,
                central: None,
                address: peer_addr.to_string(),
                ptype: PeerType::TcpClient,
            };
            sender.send(se).await.expect("failed to send message");
        }
    }

    pub fn stop(&self) {
        debug!("Stopped called.");
        self.sender.close_channel();
    }

    pub fn get_advertising(&self) -> Arc<AtomicBool> {
        self.advertising.clone()
    }

    pub async fn run(&mut self, cancellation_token: CancellationToken) -> Result<()> {
        self.advertising.store(true, Ordering::Relaxed);

        // I'm a bluetooth client
        #[cfg(feature = "bluetooth")]
        if self.bt_connect_to.is_some() {
            debug!("Connect bluetooth to :{:?}", self.bt_connect_to);
            let name = self.bt_connect_to.as_ref().unwrap().clone();
            spawn_bluetooth_central(name, self.sender.clone())
                .await
                .expect("Failed to spawn bluetooth central");
            self.connected.store(true, Ordering::Relaxed);
        }

        // I'm a TCP client
        if self.connect_to.is_some() {
            debug!("{} Connect To: {:?}", self.name, self.connect_to);
            let mut address = self.connect_to.as_ref().unwrap().to_string().clone();
            if address.len() <= 4 {
                //only port
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
                tokio::spawn(async move {
                    let stream = TcpStream::connect(&addr).await;
                    match stream {
                        Ok(s) => match s.peer_addr() {
                            Ok(_addr) => {
                                let se = SocketEvent::NewPeer {
                                    name: String::from("unnamed server"),
                                    stream: Some(s),
                                    peripheral: None,
                                    central: None,
                                    address: addr,
                                    ptype: PeerType::TcpServer,
                                };
                                send_chan_clone.clone().send(se).await.expect("failed to send peer");

                                tx_clone.send(true).await.expect("Failed to send connected signal");
                            }
                            Err(e) => {
                                let msg = format!("No peer address: {:?}", e);
                                warn!("{:?}", msg);
                            }
                        },
                        Err(e) => {
                            error!("Nope:: {:?}", e);
                            tx_clone
                                .send(false)
                                .await
                                .expect("Failed to send connect failed signal");
                        }
                    };
                });

                connected = rx.next().await.unwrap();
                if !connected {
                    warn!("failed to connect, retry in a moment");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }

            self.connected.store(true, Ordering::Relaxed);
        }

        // I'm a server
        if self.is_server() {
            if self.listen_port.is_some() {
                let mut port = self.listen_port.as_ref().unwrap().to_string();
                // port is 000.000.000.000:0000
                if port.len() <= 4 {
                    //only port
                    let addr = local_ipaddress::get().unwrap();
                    port = format!("{}:{}", addr, port);
                }
                debug!("{:?} Listening for connections on {:?}", self.name, port);
                // Bind the listener BEFORE spawning the accept loop
                // so the port is guaranteed to be ready when broadcast_ready fires
                let listener = TcpListener::bind(&port).await?;
                let send_chan = self.sender.clone();
                let name_clone = self.name.clone();
                tokio::spawn(async move {
                    match Hive::accept_loop(send_chan, listener, name_clone).await {
                        Err(e) => {
                            panic!("Failed accept loop: {:?}", e)
                        }
                        _ => (),
                    }
                });
                // Port is bound and accepting — safe to signal ready
                self.broadcast_ready();
            }

            #[cfg(feature = "bluetooth")]
            if self.bt_listen.is_some() {
                debug!("!! this is bluetooth");

                let clone = self.bt_listen.as_ref().unwrap().clone();

                spawn_bluetooth_listener(true, self.sender.clone(), clone).expect("Failed to spawn bluetooth");
            }

            self.connected.store(true, Ordering::Relaxed);
        }

        // I'm an MQTT client
        #[cfg(feature = "mqtt")]
        if let Some(mqtt_cfg) = self.mqtt_config.take() {
            debug!("{} connecting MQTT to {}:{}", self.name, mqtt_cfg.host, mqtt_cfg.port);
            match spawn_mqtt_peer(mqtt_cfg, self.sender.clone()).await {
                Ok(handle) => {
                    self.mqtt_handle = Some(handle);
                    info!("MQTT peer connected and running");
                    self.connected.store(true, Ordering::Relaxed);
                }
                Err(e) => {
                    error!("Failed to spawn MQTT peer: {:?}", e);
                }
            }
        }

        if self.connected.load(Ordering::Relaxed) {
            // Spawn IP address monitor to detect network changes.
            // When the local IP changes, notify all connected peers
            // so they can update their records.
            let ip_sender = self.sender.clone();
            let ip_token = cancellation_token.clone();
            let ip_name = self.name.clone();
            tokio::spawn(async move {
                Self::ip_monitor_loop(ip_sender, ip_token, &ip_name).await;
            });

            // This is where we sit for a long time and just receive events
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    warn!("Hive Canceled! *******");
                },
                _ = self.receive_events() => {
                    debug!("DONE receive_events");
                }
            }
            // self.receive_events().await?;
        }
        debug!("Hive DONE");
        Ok(())
    }

    async fn receive_events(&mut self) -> Result<()> {
        while !self.sender.is_closed() {
            debug!("{:?} waiting for event", self.name);
            let nn = self.receiver.next().await;
            debug!("{:?} Received event: {:?}", self.name, nn);
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
                        )
                        .await;
                        if p.is_ok() {
                            debug!(".... PEER: {:?}, {}", p, is_tcp_server);
                            let peer = p?;
                            // we auto send all the peers via TCP because it prevents un additional request
                            // TODO should this be moved into the Peer handshake
                            if ptype == PeerType::TcpClient {
                                self.send_properties(&peer).await?;
                            }

                            debug!("<<<< NEW PEER for {:?}: {:?}", self.name, peer.get_name());
                            let is_web_sock_peer = peer.is_web_socket();

                            self.peers.push(peer);

                            if !is_tcp_client {
                                // I'm a server and another tcp hive client connected
                                // a websock sends a separate Header package with it's name, other
                                // tcp sockets send the name in the handshake so we can update peers
                                // right away, instead of waiting for the header update
                                if !is_web_sock_peer {
                                    self.notify_peers_change().await;
                                }
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
        }
        info!("Stopped receiving events!");
        Ok(())
    }

    fn peer_string(&self) -> String {
        let mut peers_string = String::new();

        // Add self to peers list
        let no_port = String::from("No port");
        let myadr = self
            .listen_port
            .as_ref()
            .or_else(|| Some(&no_port))
            .expect("No port")
            .clone();
        let myname = String::from(&self.name);
        peers_string.push_str(&format!("{}|{}", myname, myadr));

        for x in 0..self.peers.len() {
            peers_string.push_str(",");
            let p = &self.peers[x];
            let adr = p.address();
            let name = p.get_name();
            // let name =
            peers_string.push_str(&format!("{}|{}", name, adr))
        }
        peers_string
    }

    async fn notify_peers_change(&mut self) {
        debug!("..... notify peers changed");
        use futures::future::join_all;
        let peer_string = self.peer_string();
        let peer_str = peer_string.as_bytes();
        let mut bytes = BytesMut::with_capacity(peer_str.len() + 1);
        bytes.put_u8(PEER_RESPONSE);
        bytes.put_slice(peer_str);
        let bf = bytes.freeze();
        // debug!(".. TEST 3: {:?}", bf);

        let mut futures = vec![];
        self.peers.iter().filter(|p| p.update_peers).for_each(|p| {
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
                error!("Send Error: {}", err_msg);
                return Err(err_msg.into());
            }
        };
        Ok(())
    }

    fn get_peer_by_name(&self, name: &str) -> Option<&Peer> {
        for peer in &self.peers {
            debug!(
                "PEER pname: {:?} search_name: {:?} my_name: {:?}",
                peer.get_name(),
                name,
                self.name
            );
            if peer.get_name() == String::from(name) {
                debug!("<< return this peer");
                return Some(peer);
            }
        }
        return None;
    }

    // TODO send properties with handshake
    async fn send_properties(&self, peer: &Peer) -> Result<()> {
        // Respect the peer's property filter during initial sync.
        // Only send properties whose names pass the filter.
        let mut bytes = BytesMut::new();
        bytes.put_u8(PROPERTIES);
        for (_id, prop) in &self.properties {
            if !prop.is_none() {
                let name = prop.get_name();
                if peer.property_filter.should_send(name) {
                    bytes.put_slice(&property_to_bytes(prop, false));
                }
            }
        }
        peer.send(bytes.freeze()).await?;
        Ok(())
    }

    /// Periodically check whether the local IP address has changed.
    /// When a change is detected, send an ADDRESS_UPDATE message to
    /// all connected peers so they can update their records.
    ///
    /// Skips monitoring if the current IP is in a private range
    /// (192.168.x.x, 10.x.x.x, 172.16-31.x.x) since those addresses
    /// are stable within a LAN.
    ///
    /// Uses `tokio::time::interval` (not a busy-wait loop).
    async fn ip_monitor_loop(
        mut sender: Sender<SocketEvent>,
        token: CancellationToken,
        hive_name: &str,
    ) {
        let mut last_ip = local_ipaddress::get().unwrap_or_default();
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        // Skip the first immediate tick
        interval.tick().await;

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    debug!("[{}] IP monitor shutting down", hive_name);
                    break;
                }
                _ = interval.tick() => {
                    match local_ipaddress::get() {
                        Some(current_ip) => {
                            if current_ip != last_ip {
                                // Only notify peers about non-private addresses.
                                // Private IPs (192.168, 10.x, 172.16-31) are stable
                                // within a LAN and changes are typically noise
                                // (e.g. interface reordering).
                                if Self::is_private_ip(&current_ip) {
                                    debug!(
                                        "[{}] IP changed to private {}, skipping notification",
                                        hive_name, current_ip
                                    );
                                } else {
                                    info!(
                                        "[{}] IP address changed: {} -> {}",
                                        hive_name, last_ip, current_ip
                                    );

                                    let addr_bytes = current_ip.as_bytes();
                                    let mut buf = BytesMut::with_capacity(
                                        1 + 2 + addr_bytes.len(),
                                    );
                                    buf.put_u8(ADDRESS_UPDATE);
                                    buf.put_u16(addr_bytes.len() as u16);
                                    buf.put_slice(addr_bytes);

                                    let event = SocketEvent::Message {
                                        from: String::new(),
                                        msg: buf.freeze(),
                                    };
                                    if let Err(e) = sender.send(event).await {
                                        warn!("failed to send ADDRESS_UPDATE: {}", e);
                                        break;
                                    }
                                }
                                last_ip = current_ip;
                            }
                        }
                        None => {
                            debug!("[{}] could not determine local IP", hive_name);
                        }
                    }
                }
            }
        }
    }

    pub fn get_weak_property(&mut self, prop_name: &str) -> Weak<Mutex<Property>> {
        // let prop = self.get_mut_property(&Property::hash_id(prop_name)).expect("nopers");
        let mp = Arc::new(Mutex::new(
            self.get_mut_property(&Property::hash_id(prop_name))
                .expect("nopers")
                .clone(),
        ));
        Arc::downgrade(&mp).clone()
    }

    async fn set_headers_from_peer(&mut self, mut head: Bytes, from: &str) {
        debug!(
            ".... Set headers from peer: {:?} = {:?}, self={:?}",
            from, head, self.name
        );
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
                        HEADER_PROP_INCLUDE => {
                            if let Some(filter) = Self::parse_filter_prefixes(&mut head) {
                                info!(
                                    "peer {} requests include filter: {:?}",
                                    from, filter
                                );
                                p.property_filter = PropertyFilter::Include(filter);
                            }
                        }
                        HEADER_PROP_EXCLUDE => {
                            if let Some(filter) = Self::parse_filter_prefixes(&mut head) {
                                info!(
                                    "peer {} requests exclude filter: {:?}",
                                    from, filter
                                );
                                p.property_filter = PropertyFilter::Exclude(filter);
                            }
                        }
                        _ => {
                            debug!("unrecognized header: {:?}", head);
                        }
                    }
                }
            }
        }

        if self.is_server() {
            self.notify_peers_change().await;
        }
    }

    /// Parse a list of prefix strings from a filter header payload.
    /// Format: [count: u16] then for each: [len: u16][prefix bytes]
    fn parse_filter_prefixes(head: &mut Bytes) -> Option<Vec<String>> {
        if head.remaining() < 2 {
            return None;
        }
        let count = head.get_u16() as usize;
        let mut prefixes = Vec::with_capacity(count);
        for _ in 0..count {
            if head.remaining() < 2 {
                return None;
            }
            let len = head.get_u16() as usize;
            if head.remaining() < len {
                return None;
            }
            let prefix = String::from_utf8(head.slice(..len).to_vec())
                .unwrap_or_default();
            head.advance(len);
            prefixes.push(prefix);
        }
        Some(prefixes)
    }

    /// Encode a property filter as header bytes to send to a peer.
    fn encode_filter_header(filter: &PropertyFilter) -> Option<Bytes> {
        match filter {
            PropertyFilter::All => None,
            PropertyFilter::Include(prefixes) => {
                Some(Self::encode_filter_bytes(HEADER_PROP_INCLUDE, prefixes))
            }
            PropertyFilter::Exclude(prefixes) => {
                Some(Self::encode_filter_bytes(HEADER_PROP_EXCLUDE, prefixes))
            }
        }
    }

    /// Encode the wire bytes for a filter header.
    fn encode_filter_bytes(header_type: u8, prefixes: &[String]) -> Bytes {
        let mut buf = BytesMut::new();
        buf.put_u8(HEADER);
        buf.put_u8(header_type);
        buf.put_u16(prefixes.len() as u16);
        for prefix in prefixes {
            let pb = prefix.as_bytes();
            buf.put_u16(pb.len() as u16);
            buf.put_slice(pb);
        }
        buf.freeze()
    }

    async fn broadcast(&self, msg_type: u8, property: &Property, except: &str) -> Result<()> {
        // Don't broadcast to the same peer that the original change came from
        // to prevent an infinite re-broadcast loop.

        // Check property name for peer filtering.
        let prop_name = property.get_name();

        // Bluetooth clients don't receive independent updates;
        // bluetooth really is a broadcast and only needs to go out once.
        let mut sent_bt_broadcast = false;
        let msg = property_to_bytes(property, false);
        for p in &self.peers {
            debug!("{} broadcasting: {:?}, {:?}", self.name, p.get_name(), p.is_bt_client(),);
            if p.address() != except {
                // Apply property filter for this peer
                if !p.property_filter.should_send(prop_name) {
                    debug!(
                        "skipping broadcast of '{}' to peer {} (filtered)",
                        prop_name, p.get_name()
                    );
                    continue;
                }
                if p.is_bt_client() {
                    if sent_bt_broadcast {
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
                        Some(p) => {
                            p.receive_pong();
                        }
                    }
                }
                PONG => {
                    debug!("<<< PONG FROM {:?}", from);
                    match self.peers.iter_mut().find(|p| p.address() == from) {
                        None => {}
                        Some(p) => {
                            p.send_pong().await?;
                        }
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
                    while msg.has_remaining().clone() {
                        let p = bytes_to_property(&mut msg);
                        debug!("thing: {:?}", p);
                        match p {
                            None => {}
                            Some(p) => self.set_property(p),
                        }
                    }

                    debug!("done");
                }

                PROPERTY => {
                    let p = bytes_to_property(&mut msg);
                    debug!("property: {:?}", p);
                    match p {
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
                                let prop_id = p.id;
                                self.set_property(p);
                                // Publish to MQTT if the change did NOT originate from MQTT.
                                // We look up the property from self.properties by ID because
                                // the property decoded from the wire format carries only the
                                // numeric hash — not the name — and we need the name to
                                // build the MQTT topic.
                                #[cfg(feature = "mqtt")]
                                if from != "mqtt" {
                                    if let Some(ref handle) = self.mqtt_handle {
                                        if let Some(named_prop) = self.properties.get(&prop_id) {
                                            if let Err(e) = handle.publish(named_prop).await {
                                                warn!("MQTT publish failed: {:?}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // todo Delete is redundant, setting the value to None does the same thing
                DELETE => {
                    // let message = String::from_utf8(msg.to_vec()).unwrap();
                    let p = self.properties.remove(&msg.get_u64());
                    match p {
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
                    let mut peer_bytes = BytesMut::with_capacity(str.len() + 1);
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
                FILE_HEADER | FILE_CHUNK | FILE_COMPLETE | FILE_CANCEL => {
                    if from.is_empty() {
                        // Message originated locally (from Handler::send_file).
                        // Forward the raw message to all peers.
                        let mut forward = BytesMut::with_capacity(1 + msg.len());
                        forward.put_u8(msg_type);
                        forward.put_slice(&msg);
                        let frozen = forward.freeze();
                        for p in &self.peers {
                            if let Err(e) = p.send(frozen.clone()).await {
                                warn!("failed to forward file message to peer: {}", e);
                            }
                        }
                    } else {
                        // Message came from a remote peer.  Process it locally.
                        match msg_type {
                            FILE_HEADER => {
                                self.file_transfers.handle_file_header(from, &mut msg);
                            }
                            FILE_CHUNK => {
                                self.file_transfers.handle_file_chunk(from, &mut msg);
                            }
                            FILE_COMPLETE => {
                                if let Some(received) = self.file_transfers.handle_file_complete(from, &mut msg) {
                                    info!(
                                        "file received: '{}' ({} bytes) from {}",
                                        received.filename, received.data.len(), received.from_peer
                                    );
                                    self.file_received.emit(received).await;
                                }
                            }
                            FILE_CANCEL => {
                                self.file_transfers.handle_file_cancel(from, &mut msg);
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                ADDRESS_UPDATE => {
                    if from.is_empty() {
                        // Local IP change detected by ip_monitor_loop.
                        // Forward to all peers so they update our address.
                        let mut forward = BytesMut::with_capacity(1 + msg.len());
                        forward.put_u8(ADDRESS_UPDATE);
                        forward.put_slice(&msg);
                        let frozen = forward.freeze();
                        for p in &self.peers {
                            if let Err(e) = p.send(frozen.clone()).await {
                                warn!("failed to send ADDRESS_UPDATE to peer: {}", e);
                            }
                        }
                    } else {
                        // A remote peer is telling us their IP changed.
                        if msg.remaining() >= 2 {
                            let addr_len = msg.get_u16() as usize;
                            if msg.remaining() >= addr_len {
                                let new_addr = String::from_utf8(
                                    msg.slice(..addr_len).to_vec()
                                ).unwrap_or_default();
                                msg.advance(addr_len);
                                info!(
                                    "peer {} reports new address: {}",
                                    from, new_addr
                                );
                                // Update the peer's stored address
                                for p in self.peers.as_mut_slice() {
                                    if p.address() == from {
                                        // The peer's TCP address includes the port,
                                        // but the reported address is just the IP.
                                        // Preserve the existing port.
                                        let old = p.address();
                                        let port = old.rsplit_once(':')
                                            .map(|(_, port)| port)
                                            .unwrap_or("");
                                        let updated = if port.is_empty() {
                                            new_addr.clone()
                                        } else {
                                            format!("{}:{}", new_addr, port)
                                        };
                                        info!(
                                            "updating peer address: {} -> {}",
                                            old, updated
                                        );
                                        p.address = updated;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    warn!("unrecognized message type {:#04x} from {}", msg_type, from);
                }
            }
            // do ACK for peer
            match self.peers.iter_mut().find(|p| p.address() == from) {
                None => {}
                Some(p) => {
                    p.ack().await;
                }
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
