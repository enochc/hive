// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
// #![feature(async_closure)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use crate::handler::Handler;
#[cfg(feature = "mqtt")]
use crate::mqtt::mqtt_peer::{MqttConfig, MqttPeerHandle, spawn_mqtt_peer};
use crate::peer::{handshake, Peer, PeerType, SocketEvent, DEFAULT_HANDSHAKE_TIMEOUT, DEFAULT_MAX_MESSAGE_SIZE};
use crate::property::{decode_property, properties_to_bytes, property_to_bytes, DecodeError, Property, PropertyMap};
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
    pub properties: PropertyMap,
    sender: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    connect_to: Option<String>,
    #[allow(dead_code)]
    bt_connect_to: Option<String>,
    bt_listen: Option<String>,
    listen_port: Option<String>,
    pub name: String,
    /// Per-peer liveness check timeout, from the `wave_timeout_ms` config
    /// key. None (the default) means peers are never actively pinged, a
    /// dead link is only noticed via a real TCP-level disconnect.
    wave_timeout: Option<Duration>,
    /// Ceiling on a single incoming message's claimed size, from the
    /// `max_message_size_bytes` config key. Defaults to
    /// `DEFAULT_MAX_MESSAGE_SIZE`, see `Peer::read_loop` for why this
    /// exists at all.
    max_message_size: u32,
    /// How long a new connection gets to complete the Hive handshake, from
    /// the `handshake_timeout_ms` config key. The handshake runs off the
    /// event loop, so this only bounds how long a silent socket is held.
    handshake_timeout: Duration,
    /// Shared with every `Handler` cloned from this `Hive`, so debounce
    /// state for a given property name is consistent no matter which
    /// clone sends it. See `Handler::set_property_debounced`.
    debounce: crate::handler::DebounceMap,
    peers: Vec<Peer>,
    pub message_received: Signal<String>,
    pub connected: Arc<AtomicBool>,
    /// Number of peers currently attached, mirrored out to every `Handler`.
    /// `connected` can't answer "is anyone actually on the other end": a
    /// server sets it as soon as it binds its listen port, with no client
    /// in sight. This counts real peers, so a listener can tell an idle
    /// port from a live connection.
    peer_count: Arc<AtomicUsize>,
    advertising: Arc<AtomicBool>,
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
        // Presence of this key both enables the liveness check and sets its
        // timeout, e.g. `wave_timeout_ms = 800`. Absent means off.
        let wave_timeout = config
            .get("wave_timeout_ms")
            .and_then(|v| v.as_integer())
            .map(|ms| Duration::from_millis(ms as u64));

        let max_message_size = config
            .get("max_message_size_bytes")
            .and_then(|v| v.as_integer())
            .map(|bytes| bytes as u32)
            .unwrap_or(DEFAULT_MAX_MESSAGE_SIZE);

        let handshake_timeout = config
            .get("handshake_timeout_ms")
            .and_then(|v| v.as_integer())
            .map(|ms| Duration::from_millis(ms as u64))
            .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT);

        #[cfg(feature = "mqtt")]
        let mqtt_config = config.get("MQTT").and_then(MqttConfig::from_toml);

        let props = PropertyMap::default();

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
            wave_timeout,
            max_message_size,
            handshake_timeout,
            debounce: Arc::new(Mutex::new(HashMap::new())),
            peers: Vec::new(),
            message_received: Default::default(),
            connected: Arc::new(AtomicBool::new(false)),
            peer_count: Arc::new(AtomicUsize::new(0)),
            advertising: Arc::new(AtomicBool::new(false)),
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
            connected: self.connected.clone(),
            peer_count: self.peer_count.clone(),
            debounce: self.debounce.clone(),
        };
    }

    pub fn new(toml_path: &str) -> Hive {
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        Hive::new_from_str_unknown(foo.as_str())
    }

    pub fn get_mut_property(&mut self, key: &u64) -> Option<&mut Property> {
        Some(self.properties.entry(*key).or_insert_with(|| Property::from_id(key, None)))
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
        self.get_mut_property(&Property::hash_id(name))
    }

    pub fn get_property(&self, key: &u64) -> Option<&Property> {
        let op = self.properties.get(key);
         op
    }

    fn set_property(&mut self, property: Property) {
        debug!("{:?} SET PROPERTY: {:?}", property.get_name(), property);
        use std::collections::hash_map::Entry;
        match self.properties.entry(property.id) {
            // set_from_prop emits on_next on the existing property
            Entry::Occupied(mut e) => {
                e.get_mut().set_from_prop(property);
            }
            // TODO when added for first time, no change event is emitted, EMIT CHANGE!!
            Entry::Vacant(e) => {
                e.insert(property);
            }
        }
    }

    /// Apply a decoded `(id, value)` from the wire to the local property,
    /// creating it if needed. A `None` value is ignored here, deletes go
    /// through the PROPERTY/DELETE paths.
    fn apply_value(&mut self, id: u64, value: Option<crate::property::PropertyValue>) {
        if let Some(v) = value {
            match self.properties.get_mut(&id) {
                Some(p) => {
                    p.set_value(v);
                }
                None => {
                    self.properties.insert(id, Property::from_id(&id, Some(v)));
                }
            }
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
    async fn accept_loop(
        sender: Sender<SocketEvent>,
        listener: TcpListener,
        hive_name: String,
        handshake_timeout: Duration,
    ) {
        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    // Usually transient (out of file descriptors, a
                    // connection reset before accept). Back off briefly
                    // instead of ending the listener for good.
                    warn!("{:?} accept failed: {:?}", hive_name, e);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            debug!("{:?} ------------  Accepting from: {}", hive_name, peer_addr);
            let sender = sender.clone();
            let name = hive_name.clone();
            // Each handshake runs in its own task so one slow or silent
            // client can't hold up the next accept or the event loop.
            tokio::spawn(async move {
                match handshake(stream, &PeerType::TcpClient, &name, handshake_timeout).await {
                    Ok(conn) => {
                        let _ = sender.unbounded_send(SocketEvent::NewPeer {
                            name: "unnamed client".to_string(),
                            address: conn.address.clone(),
                            stream: Some(conn),
                            peripheral: None,
                            central: None,
                            ptype: PeerType::TcpClient,
                        });
                    }
                    Err(e) => warn!("{:?} handshake with {} failed: {}", name, peer_addr, e),
                }
            });
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
                let my_name = self.name.clone();
                let timeout = self.handshake_timeout;
                tokio::spawn(async move {
                    let attempt = async {
                        let s = TcpStream::connect(&addr).await?;
                        handshake(s, &PeerType::TcpServer, &my_name, timeout).await
                    };
                    match attempt.await {
                        Ok(conn) => {
                            let se = SocketEvent::NewPeer {
                                name: String::from("unnamed server"),
                                address: conn.address.clone(),
                                stream: Some(conn),
                                peripheral: None,
                                central: None,
                                ptype: PeerType::TcpServer,
                            };
                            let _ = send_chan_clone.unbounded_send(se);
                            let _ = tx_clone.send(true).await;
                        }
                        Err(e) => {
                            error!("{} connect to {} failed: {:?}", my_name, addr, e);
                            let _ = tx_clone.send(false).await;
                        }
                    }
                });

                // Both the wait for the attempt's result and the retry
                // sleep have to be cancellable. Without this a caller that
                // gives up on a `Hive` (say to rebuild it against a
                // different address, or to drive its own retry schedule)
                // can't actually stop this loop, it keeps dialing forever
                // in a detached task and every abandoned Hive stacks
                // another one on top.
                connected = tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("{} connect loop cancelled", self.name);
                        return Ok(());
                    }
                    r = rx.next() => r.unwrap_or(false),
                };
                if !connected {
                    warn!("failed to connect, retry in a moment");
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            debug!("{} connect loop cancelled while backing off", self.name);
                            return Ok(());
                        }
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    }
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
                tokio::spawn(Hive::accept_loop(send_chan, listener, name_clone, self.handshake_timeout));
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
            //This is where we sit for a long time and just receive events
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
                            self.max_message_size,
                        )
                        .await;
                        if p.is_ok() {
                            debug!(".... PEER: {:?}, {}", p, is_tcp_server);
                            let peer = p?;
                            // we auto send all the peers via TCP because it prevents un additional request
                            // TODO should this be moved into the Peer handshake
                            if ptype == PeerType::TcpClient {
                                if let Err(e) = self.send_properties(&peer).await {
                                    warn!("{} failed to send properties to new peer {:?}: {:?}", self.name, peer.address, e);
                                    continue;
                                }
                            }

                            debug!("<<<< NEW PEER for {:?}: {:?}", self.name, peer.get_name());
                            let is_web_sock_peer = peer.is_web_socket();

                            if let Some(timeout) = self.wave_timeout {
                                peer.wave(timeout).await;
                            }

                            self.peers.push(peer);
                            self.peer_count.store(self.peers.len(), Ordering::Relaxed);
                            self.connected.store(true, Ordering::Relaxed);

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
                    // A bad message or a failed write to one peer must
                    // never end the event loop for every other peer.
                    if let Err(e) = self.got_message(from.as_str(), msg).await {
                        if e.downcast_ref::<DecodeError>().is_some() {
                            // The peer sent something we can't parse; its
                            // stream can't be trusted past this point.
                            warn!("{} malformed message from {:?}, dropping peer: {}", self.name, from, e);
                            self.remove_peer(&from).await;
                        } else {
                            warn!("{} error handling message from {:?}: {:?}", self.name, from, e);
                        }
                    }
                }
                Some(SocketEvent::Hangup { from }) => {
                    debug!("received hangup from {:?}", from);
                    self.remove_peer(&from).await;
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
        let bts = properties_to_bytes(&self.properties);
        peer.send(bts).await?;
        Ok(())
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

    async fn set_headers_from_peer(&mut self, mut head: Bytes, from: &str) -> Result<()> {
        debug!(
            ".... Set headers from peer: {:?} = {:?}, self={:?}",
            from, head, self.name
        );
        for p in self.peers.as_mut_slice() {
            if p.address == from {
                while head.remaining() > 0 {
                    match head.get_u8() {
                        HEADER_NAME => {
                            // The name runs to the end of the header.
                            let raw = head.split_to(head.remaining());
                            let name = std::str::from_utf8(&raw)
                                .map_err(|e| DecodeError(format!("peer name not UTF-8: {}", e)))?;
                            p.set_name(name);
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

        if self.is_server() {
            // Web sockets send a separate header update with its name
            // other Hive sockets send the name in the initial connection heder.
            self.notify_peers_change().await;
        }
        Ok(())
    }

    /// Remove the peer at `address`, if present, and tell the remaining
    /// peers. Shared by a read-side hangup, a HANGUP message, and a failed
    /// write during broadcast.
    async fn remove_peer(&mut self, address: &str) {
        if let Some(idx) = self.peers.iter().position(|p| p.address == address) {
            debug!("removing peer {:?} at index {}", address, idx);
            self.peers.remove(idx);
            self.peer_count.store(self.peers.len(), Ordering::Relaxed);
            if self.peers.is_empty() {
                self.connected.store(false, Ordering::Relaxed);
            }
            self.notify_peers_change().await;
        }
    }

    async fn broadcast(&mut self, msg_type: u8, property: &Property, except: &str) {
        /*
        Dont broadcast to the same Peer that the original change came from
        to prevent an infinite re-broadcast loop
         */

        // bluetooth clients dont receive independent updates,
        // bluetooth really is a broadcast and only needs to go out once
        let mut sent_bt_broadcast = false;
        let mut failed: Vec<String> = Vec::new();
        let msg = property_to_bytes(property, false);
        for p in &self.peers {
            debug!("{} broadcasting: {:?}, {:?}", self.name, p.get_name(), p.is_bt_client(),);
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
                if let Err(e) = p.send(msg_out.freeze()).await {
                    warn!("{} write to {:?} failed, dropping peer: {:?}", self.name, p.address, e);
                    failed.push(p.address.clone());
                }
            }
        }
        for address in failed {
            self.remove_peer(&address).await;
        }
    }

    async fn got_message(&mut self, from: &str, mut msg: Bytes) -> Result<()> {
        debug!("MESSAGE: {:?} received: {:?}, from {:?}", self.name, msg, from);
        if msg.len() < 1 {
            debug!("<<< Empty message");
        } else {
            //let (msg_type, message) = msg.split_at(3);
            let msg_type = msg.get_u8();

            // Most branches below act on the peer that sent this message,
            // and the trailing ack does too, so look it up once by index
            // instead of re-scanning self.peers (and re-cloning its
            // address for the comparison) per branch. HANGUP removes this
            // peer, so the ack at the bottom skips it explicitly rather
            // than trusting a now-possibly-stale index.
            let peer_idx = self.peers.iter().position(|p| p.address == from);
            // Any message counts as "heard from this peer". Done up front
            // because broadcast or HANGUP below can remove peers, which
            // would leave peer_idx pointing at the wrong slot afterwards.
            if let Some(idx) = peer_idx {
                self.peers[idx].ack();
            }

            match msg_type {
                PING => {
                    // Someone is asking if we're alive, answer with a PONG.
                    debug!("<<< <<< PING from {:?}", from);
                    if let Some(idx) = peer_idx {
                        self.peers[idx].send_pong().await?;
                    }
                }
                PONG => {
                    // Reply to a PING we sent, clear the outstanding ack_check.
                    debug!("<<< PONG FROM {:?}", from);
                    if let Some(idx) = peer_idx {
                        self.peers[idx].receive_pong();
                    }
                }
                HANGUP => {
                    debug!("<<<< hangup from {:?}", from);
                    self.remove_peer(from).await;
                }
                PROPERTIES => {
                    debug!("properties:: {:?}", msg);
                    while msg.has_remaining() {
                        let (id, value) = decode_property(&mut msg)?;
                        self.apply_value(id, value);
                    }

                    debug!("done");
                }

                PROPERTY => {
                    let (id, value) = decode_property(&mut msg)?;
                    let p = Property::from_id(&id, value);
                    {
                        {
                            if p.is_none() {
                                debug!("DELETING {:?}", p.get_name());
                                self.broadcast(DELETE, &p, from).await;
                                self.properties.remove(&p.id);
                            } else {
                                self.broadcast(PROPERTY, &p, from).await;
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
                    if msg.remaining() < 8 {
                        return Err(DecodeError("DELETE needs an 8 byte id".into()).into());
                    }
                    let p = self.properties.remove(&msg.get_u64());
                    match p {
                        None => {}
                        Some(p) => {
                            self.broadcast(DELETE, &p, from).await;
                            // this is unnecessary, but fun
                            drop(p);
                        }
                    }
                }
                HEADER => {
                    self.set_headers_from_peer(msg, from).await?;
                }
                PEER_MESSAGE => {
                    let message = String::from_utf8(msg.to_vec())
                        .map_err(|e| DecodeError(format!("peer message not UTF-8: {}", e)))?;
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

                    if let Some(idx) = peer_idx {
                        self.peers[idx].send(peer_bytes.freeze()).await?;
                        self.peers[idx].update_peers = true;
                    }
                }
                // Ignore rather than drop: a newer peer may speak message
                // types this build doesn't know yet.
                _ => warn!("{} ignoring unknown message type {:#04x} from {:?}", self.name, msg_type, from),
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
