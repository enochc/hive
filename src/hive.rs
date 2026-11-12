// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
// #![feature(async_closure)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use std::{collections::HashMap, fs};

use tokio::net::{TcpListener, TcpStream};
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use crate::futures::channel::mpsc;
use crate::futures::{FutureExt, SinkExt, StreamExt};
use crate::logging::{info, debug, warn, error};

use toml;

#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::handler::Handler;
#[cfg(feature = "mqtt")]
use crate::mqtt::mqtt_peer::{MqttConfig, MqttPeerHandle, spawn_mqtt_peer};
use crate::peer::{handshake, Peer, PeerType, SocketEvent, DEFAULT_HANDSHAKE_TIMEOUT, DEFAULT_MAX_MESSAGE_SIZE};
use crate::property::{decode_property, properties_to_bytes, DecodeError, Property, PropertyMap};
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
    /// Most peers a server accepts at once (`max_peers`), None = no limit.
    max_peers: Option<usize>,
    /// Most connections allowed to be mid-handshake at once
    /// (`max_pending_handshakes`). Each one holds a task and a socket for
    /// up to `handshake_timeout`, so this bounds what a connection flood
    /// can tie up.
    max_pending_handshakes: usize,
    /// TLS / shared-secret settings applied to every TCP connection.
    security: Arc<crate::security::Security>,
    /// Shared with every `Handler` cloned from this `Hive`, so debounce
    /// state for a given property name is consistent no matter which
    /// clone sends it. See `Handler::set_property_debounced`.
    debounce: crate::handler::DebounceMap,
    /// Inbound budget shared by this hive's `Handler`s.
    local_inbound: Arc<crate::peer::Inbound>,
    /// Bytes of `Local` events handled in the current batch, released to
    /// `local_inbound` at its end.
    local_done: usize,
    peers: Vec<Peer>,
    /// Index of the peer the last message came from. Messages arrive in
    /// runs from one peer, so `peer_index` checks here before scanning.
    last_from: usize,
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
    /// Told about what an application embedding this hive would show, see
    /// `on_event`.
    observer: Option<Arc<dyn Fn(HiveEvent) + Send + Sync + 'static>>,
    /// A local `Handler::request_peers` asked for the peer list. Kept so a
    /// server we connect to later is asked as well.
    want_peers: bool,
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

/// What a `Hive` reports to the observer set with `Hive::on_event`: the
/// changes an application embedding it (through FFI, say) needs to mirror
/// its state, including properties it never declared by name.
#[derive(Debug, Clone, PartialEq)]
pub enum HiveEvent {
    /// A property got a new value, from a peer or a local `Handler`.
    /// Properties are identified by id on the wire, `Property::hash_id` of
    /// the name; the name itself is only known to hives that declared it.
    PropertyChanged { id: u64, value: Value },
    PropertyDeleted { id: u64 },
    /// The peer list of a server this hive is connected to, sent after
    /// `Handler::request_peers` and again whenever it changes. The first
    /// entry is the server itself.
    Peers(Vec<PeerInfo>),
    /// A peer attached or went away. `peers` is how many are attached now.
    Connection { connected: bool, peers: usize },
}

/// One entry of a server's peer list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub name: String,
    /// The peer's socket address as the server sees it. For the server's
    /// own entry, what it listens on.
    pub address: String,
}

/// Parse the body of a PEER_RESPONSE, `name|address,name|address`.
fn parse_peers(list: &str) -> Vec<PeerInfo> {
    list.split(',')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (name, address) = entry.split_once('|').unwrap_or((entry, ""));
            PeerInfo { name: name.to_string(), address: address.to_string() }
        })
        .collect()
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

const DEFAULT_MAX_PENDING_HANDSHAKES: usize = 64;

/// Most messages the event loop handles before it wakes the peers' writers
/// and lets other tasks on its worker thread run. Batching is what makes a
/// burst cheap (one write per peer for the lot); the cap keeps a sustained
/// flood from holding output back, or the thread to itself, for long.
const EVENT_BATCH: usize = 1024;

struct AcceptLimits {
    max_peers: Option<usize>,
    max_pending_handshakes: usize,
    peer_count: Arc<AtomicUsize>,
    security: Arc<crate::security::Security>,
}

impl Drop for Hive {
    fn drop(&mut self) {
        // Nothing will handle (and give back) what Handlers still have
        // queued, so stop them waiting for room that never comes.
        self.local_inbound.close();
    }
}


impl Hive {
    pub fn is_server(&self) -> bool {
        self.listen_port.is_some() || self.bt_listen.is_some()
    }

    pub fn is_connected(&self) -> bool {
        return self.connected.load(Ordering::Relaxed);
    }

    /// Consume the Hive, run it in its own task and return a `Handler`.
    /// With `wait`, first wait until it is listening, connected or
    /// attached to its MQTT broker.
    ///
    /// Panics if the hive fails before it is ready (the listen address is
    /// already in use, say) while `wait` is set; use `try_go` to get that
    /// as an error instead. Without `wait` such a failure is only logged.
    pub async fn go(self, wait: bool, cancellation_token: CancellationToken) -> Handler {
        let name = self.name.clone();
        self.try_go(wait, cancellation_token)
            .await
            .unwrap_or_else(|e| panic!("Hive {:?} failed to start: {}", name, e))
    }

    /// Like `go`, but a failure before the hive is ready comes back as an
    /// `Err` when `wait` is set, rather than a panic. Errors after that
    /// point, or with `wait` unset, can only be logged.
    pub async fn try_go(mut self, wait: bool, cancellation_token: CancellationToken) -> Result<Handler> {
        let notify = if wait {
            let n = Arc::new(tokio::sync::Notify::new());
            self.ready = Some(n.clone());
            Some(n)
        } else {
            None
        };

        let handler = self.get_handler();
        let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let name = self.name.clone();
            if let Err(e) = self.run(cancellation_token).await {
                error!("Hive {:?} stopped: {}", name, e);
                let _ = failed_tx.send(e);
            }
        });

        if let Some(n) = notify {
            info!("Waiting for Hive connection");
            tokio::select! {
                _ = n.notified() => info!("Hive connection ready"),
                failed = failed_rx => match failed {
                    Ok(e) => return Err(e),
                    // `run` ended without an error before it was ready:
                    // cancelled. Nothing to wait for any more.
                    Err(_) => info!("Hive stopped before it was ready"),
                },
            }
        }
        Ok(handler)
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
        // A wrong type or out-of-range value is fatal, naming the key: a
        // negative timeout or a quoted number must not quietly turn into
        // something else (see config.rs).
        let given_name = name;
        let fail = |e: String| -> ! { panic!("Hive {:?}: {}", given_name, e) };
        let prop = |p: &str| crate::config::get_str(&config, "", p).unwrap_or_else(|e| fail(e)).map(String::from);
        let int = |k: &str, min: i64, max: i64| -> Option<i64> {
            crate::config::get_int(&config, "", k, min, max).unwrap_or_else(|e| fail(e))
        };
        // Timeouts: 1 ms to one day.
        const DAY_MS: i64 = 24 * 60 * 60 * 1000;
        let name = prop("name").unwrap_or_else(|| name.to_string());

        let connect_to = prop("connect");
        let listen_port = prop("listen");
        let bt_listen = prop("bt_listen");
        let bt_connect_to = prop("bt_connect");
        // Presence of this key both enables the liveness check and sets its
        // timeout, e.g. `wave_timeout_ms = 800`. Absent means off.
        let wave_timeout = int("wave_timeout_ms", 1, DAY_MS).map(|ms| Duration::from_millis(ms as u64));

        // At least room for a property update; at most 1 GiB, since a frame
        // that size is buffered whole.
        let max_message_size = int("max_message_size_bytes", 64, 1 << 30)
            .map(|b| b as u32)
            .unwrap_or(DEFAULT_MAX_MESSAGE_SIZE);

        let handshake_timeout = int("handshake_timeout_ms", 1, DAY_MS)
            .map(|ms| Duration::from_millis(ms as u64))
            .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT);

        let max_peers = int("max_peers", 0, 1_000_000).map(|n| n as usize);
        let max_pending_handshakes = int("max_pending_handshakes", 1, 1_000_000)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_PENDING_HANDSHAKES);

        // Bad security config is fatal: carrying on would mean silently
        // running in plaintext or unauthenticated.
        let security = crate::security::Security::from_config(
            &config,
            listen_port.is_some(),
            connect_to.is_some(),
        )
        .unwrap_or_else(|e| panic!("Hive {:?}: {}", name, e));

        #[cfg(feature = "mqtt")]
        let mqtt_config = config.get("MQTT").and_then(|t| {
            MqttConfig::try_from_toml(t).unwrap_or_else(|e| panic!("Hive {:?} [MQTT]: {}", name, e))
        });

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
            max_peers,
            max_pending_handshakes,
            security: Arc::new(security),
            debounce: Arc::new(Mutex::new(HashMap::new())),
            local_inbound: crate::peer::Inbound::new(crate::peer::INBOUND_LIMIT),
            local_done: 0,
            peers: Vec::new(),
            last_from: 0,
            message_received: Default::default(),
            connected: Arc::new(AtomicBool::new(false)),
            peer_count: Arc::new(AtomicUsize::new(0)),
            advertising: Arc::new(AtomicBool::new(false)),
            ready: None,
            observer: None,
            want_peers: false,
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

    /// Have `f` called for every `HiveEvent`. Set it before `go`. It runs on
    /// the hive's event loop, so it should hand the event off (to a channel,
    /// say) rather than do slow work. One observer per hive; a second call
    /// replaces the first.
    pub fn on_event(&mut self, f: impl Fn(HiveEvent) + Send + Sync + 'static) {
        self.observer = Some(Arc::new(f));
    }

    fn emit_connection(&self) {
        if let Some(observer) = &self.observer {
            observer(HiveEvent::Connection {
                connected: self.connected.load(Ordering::Relaxed),
                peers: self.peers.len(),
            });
        }
    }

    /// Drop a property, telling the observer if there was one to drop.
    fn remove_property(&mut self, id: u64) -> bool {
        let removed = self.properties.remove(&id).is_some();
        if removed {
            if let Some(observer) = &self.observer {
                observer(HiveEvent::PropertyDeleted { id });
            }
        }
        removed
    }

    pub fn get_handler(&self) -> Handler {
        return Handler {
            sender: self.sender.clone(),
            from_name: self.name.clone(),
            connected: self.connected.clone(),
            peer_count: self.peer_count.clone(),
            debounce: self.debounce.clone(),
            inbound: self.local_inbound.clone(),
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
            // Only an observed hive pays for a copy of the value.
            let changed = match self.properties.get_mut(&id) {
                Some(p) => {
                    if p.set_value(v) && self.observer.is_some() {
                        p.get_value()
                    } else {
                        None
                    }
                }
                None => {
                    let seen = self.observer.as_ref().map(|_| v.val.clone());
                    self.properties.insert(id, Property::from_id(&id, Some(v)));
                    seen
                }
            };
            if let (Some(observer), Some(value)) = (&self.observer, changed) {
                observer(HiveEvent::PropertyChanged { id, value });
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
        limits: AcceptLimits,
    ) {
        let pending = Arc::new(tokio::sync::Semaphore::new(limits.max_pending_handshakes));
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
            // Both checks happen before any handshake (or TLS) work, so a
            // refused connection costs one accept and one close.
            if let Some(max) = limits.max_peers {
                if limits.peer_count.load(Ordering::Relaxed) >= max {
                    warn!("{:?} refusing {}: already at max_peers ({})", hive_name, peer_addr, max);
                    continue;
                }
            }
            let permit = match pending.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    warn!("{:?} refusing {}: too many handshakes in progress", hive_name, peer_addr);
                    continue;
                }
            };
            debug!("{:?} ------------  Accepting from: {}", hive_name, peer_addr);
            let sender = sender.clone();
            let name = hive_name.clone();
            let security = limits.security.clone();
            // Each handshake runs in its own task so one slow or silent
            // client can't hold up the next accept or the event loop.
            tokio::spawn(async move {
                let _permit = permit;
                match handshake(stream, &PeerType::TcpClient, &name, handshake_timeout, &security, "").await {
                    Ok(conn) => {
                        let _ = sender.unbounded_send(SocketEvent::NewPeer {
                            name: "unnamed client".to_string(),
                            address: conn.address.clone(),
                            stream: Some(Box::new(conn)),
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
                let security = self.security.clone();
                tokio::spawn(async move {
                    let attempt = async {
                        let s = TcpStream::connect(&addr).await?;
                        handshake(s, &PeerType::TcpServer, &my_name, timeout, &security, &addr).await
                    };
                    match attempt.await {
                        Ok(conn) => {
                            let se = SocketEvent::NewPeer {
                                name: String::from("unnamed server"),
                                address: conn.address.clone(),
                                stream: Some(Box::new(conn)),
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
                let limits = AcceptLimits {
                    max_peers: self.max_peers,
                    max_pending_handshakes: self.max_pending_handshakes,
                    peer_count: self.peer_count.clone(),
                    security: self.security.clone(),
                };
                tokio::spawn(Hive::accept_loop(send_chan, listener, name_clone, self.handshake_timeout, limits));
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
            // spawn_mqtt_peer waits for the broker's ConnAck, which never
            // comes while the broker is down, so the wait has to honour
            // cancellation like the TCP connect loop does.
            let spawned = tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("{} cancelled while waiting for the MQTT broker", self.name);
                    return Ok(());
                }
                r = spawn_mqtt_peer(mqtt_cfg, self.sender.clone()) => r,
            };
            match spawned {
                Ok(handle) => {
                    self.mqtt_handle = Some(handle);
                    info!("MQTT peer connected and running");
                    self.connected.store(true, Ordering::Relaxed);
                    // An MQTT-only hive has no TCP listener or peer to
                    // signal readiness, without this go(true, ..) never
                    // returned for it.
                    self.broadcast_ready();
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
            let mut next = self.receiver.next().await;
            // Handle everything that's already waiting before waking any
            // writer, so each peer gets a burst as one write.
            let mut handled = 0;
            loop {
                debug!("{:?} Received event: {:?}", self.name, next);
                match next {
                    Some(event) => handled += self.handle_event(event).await,
                    None => {
                        println!("Received Nothing...");
                        break;
                    }
                }
                if handled >= EVENT_BATCH {
                    break;
                }
                // Take another only if one is already there.
                next = match self.receiver.next().now_or_never() {
                    Some(event) => event,
                    None => break,
                };
            }
            self.flush_peers();
            if self.local_done > 0 {
                self.local_inbound.release(std::mem::take(&mut self.local_done));
            }
            if handled >= EVENT_BATCH {
                // More is probably queued. Nothing above waits while there
                // is work, so step aside for the writers just woken and
                // whatever else shares this worker thread.
                tokio::task::yield_now().await;
            }
        }
        info!("Stopped receiving events!");
        Ok(())
    }

    /// Wake the writer of every peer that had something queued.
    fn flush_peers(&self) {
        for p in &self.peers {
            p.flush();
        }
    }

    /// Queue `msg` for `peer`. The writer is woken by `flush_peers` at the
    /// end of the current batch of events.
    async fn send_peer(&self, peer: &Peer, msg: &Bytes) -> Result<()> {
        if peer.try_send(msg)? {
            return Ok(());
        }
        // About to wait on a full outbox (or use a transport that has
        // none): don't leave what is already queued for the other peers
        // sitting behind that wait.
        self.flush_peers();
        peer.send(msg.clone()).await
    }

    /// Position in `self.peers` of the peer at address `from`.
    fn peer_index(&mut self, from: &str) -> Option<usize> {
        if self.peers.get(self.last_from).is_some_and(|p| p.address == from) {
            return Some(self.last_from);
        }
        let idx = self.peers.iter().position(|p| p.address == from)?;
        self.last_from = idx;
        Some(idx)
    }

    /// Any traffic counts as "heard from this peer" for the liveness check.
    /// Only `wave` reads the timestamp, so skip the clock read without it.
    fn ack_peer(&mut self, from: &str) {
        if self.wave_timeout.is_some() {
            if let Some(idx) = self.peer_index(from) {
                self.peers[idx].ack();
            }
        }
    }

    /// Handle one message. False if the sender had to be dropped for it.
    async fn on_message(&mut self, from: &str, msg: Bytes) -> bool {
        // A bad message or a failed write to one peer must never end the
        // event loop for every other peer.
        if let Err(e) = self.got_message(from, msg).await {
            if e.downcast_ref::<DecodeError>().is_some() {
                // The peer sent something we can't parse; its stream can't
                // be trusted past this point.
                warn!("{} malformed message from {:?}, dropping peer: {}", self.name, from, e);
                self.remove_peer(from).await;
                return false;
            }
            warn!("{} error handling message from {:?}: {:?}", self.name, from, e);
        }
        true
    }

    /// Handle one event. Returns how many messages it carried, for the
    /// batch accounting in `receive_events`.
    async fn handle_event(&mut self, event: SocketEvent) -> usize {
        match event {
            SocketEvent::NewPeer {
                name,
                stream,
                peripheral,
                central,
                address,
                ptype,
            } => {
                let has_peer = self.peers.iter().find(|p| p.address == address);

                let is_tcp_server = self.listen_port.is_some();
                let is_tcp_client = self.connect_to.is_some();

                let at_limit = ptype == PeerType::TcpClient
                    && self.max_peers.is_some_and(|max| self.peers.len() >= max);
                if at_limit {
                    // Several handshakes can finish together after the
                    // accept-time check passed; this is the exact one.
                    warn!("{} dropping {}: at max_peers", self.name, address);
                } else if has_peer.is_none() {
                    debug!("----------------------- {:?} building peer: {:?}", self.name, name);
                    let ptype_clne = ptype.clone();
                    let p = Peer::new(
                        name,
                        stream.map(|conn| *conn),
                        peripheral,
                        central,
                        self.sender.clone(),
                        address,
                        self.name.clone(),
                        ptype_clne,
                        self.max_message_size,
                    )
                    .await;
                    if let Ok(peer) = p {
                        debug!(".... PEER: {:?}, {}", peer, is_tcp_server);
                        // we auto send all the peers via TCP because it prevents un additional request
                        // TODO should this be moved into the Peer handshake
                        if ptype == PeerType::TcpClient {
                            if let Err(e) = self.send_properties(&peer).await {
                                warn!("{} failed to send properties to new peer {:?}: {:?}", self.name, peer.address, e);
                                return 1;
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
                        self.emit_connection();

                        if self.want_peers && ptype == PeerType::TcpServer {
                            let ask = Bytes::from_static(&[PEER_REQUESTS]);
                            if let Some(server) = self.peers.last() {
                                if let Err(e) = self.send_peer(server, &ask).await {
                                    debug!("peer list request to {:?} failed: {:?}", server.address, e);
                                }
                            }
                        }

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
            SocketEvent::Message { from, msg } => {
                debug!("GOT MESSAGE {}, {:?}", from, msg);
                self.ack_peer(&from);
                self.on_message(&from, msg).await;
            }
            SocketEvent::Local { msg, charged: _ } => {
                // Given back to the Handlers' budget after this batch.
                self.local_done += msg.len();
                self.on_message("", msg).await;
            }
            SocketEvent::Frames { from, mut data, credit: _credit } => {
                self.ack_peer(&from);
                let mut count = 0;
                // `read_loop` only sends whole frames, each within the
                // size cap, so the lengths here need no second check.
                while data.remaining() >= 4 {
                    let len = data.get_u32() as usize;
                    let msg = data.split_to(len.min(data.remaining()));
                    count += 1;
                    if !self.on_message(&from, msg).await {
                        break;
                    }
                }
                return count;
            }
            SocketEvent::Hangup { from } => {
                debug!("received hangup from {:?}", from);
                self.remove_peer(&from).await;
            }
            SocketEvent::MqttText { id, text } => {
                let hint = self.properties.get(&id).and_then(|p| p.get_value());
                match crate::property::text_to_value(&text, hint.as_ref()) {
                    Ok(None) => debug!("{} ignoring empty MQTT text payload for {}", self.name, id),
                    Ok(Some(v)) => {
                        // From here on it's an ordinary property update:
                        // applied locally, broadcast to TCP peers, and
                        // not echoed back to MQTT.
                        let frame = crate::property::encode_property(id, Some(&v), true);
                        if let Err(e) = self.got_message("mqtt", frame).await {
                            warn!("{} MQTT text update failed: {:?}", self.name, e);
                        }
                    }
                    Err(e) => warn!("{} dropping MQTT text payload: {}", self.name, e),
                }
            }
            #[allow(unused_variables)]
            SocketEvent::SendBtProps { sender } => {
                #[cfg(all(target_os = "linux", feature = "bluetooth"))]
                {
                    debug!("<<<........... HAHAHAHAHAHA {:?}", sender);
                    let bytes = properties_to_bytes(&self.properties);
                    let resp = bluster::gatt::event::Response::Success(bytes.to_vec());
                    sender.send(resp).unwrap();
                }
            }
        }
        1
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
        let peer_string = self.peer_string();
        let peer_str = peer_string.as_bytes();
        let mut bytes = BytesMut::with_capacity(peer_str.len() + 1);
        bytes.put_u8(PEER_RESPONSE);
        bytes.put_slice(peer_str);
        let bf = bytes.freeze();
        // debug!(".. TEST 3: {:?}", bf);

        for p in self.peers.iter().filter(|p| p.update_peers) {
            if let Err(e) = self.send_peer(p, &bf).await {
                debug!("peers update to {:?} failed: {:?}", p.address, e);
            }
        }
    }

    async fn send_to_peer(&self, msg: &str, peer_name: &str) -> Result<()> {
        let p = self.get_peer_by_name(peer_name);
        match p {
            Some(peer) => {
                let mut bytes = BytesMut::with_capacity(msg.len() + 1);
                bytes.put_u8(PEER_MESSAGE);
                bytes.put_slice(msg.as_bytes());

                self.send_peer(peer, &bytes.freeze()).await?;
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
        self.send_peer(peer, &bts).await
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
            self.emit_connection();
            self.notify_peers_change().await;
        }
    }

    /// Send an already encoded message to every peer except the one at
    /// index `except` (the peer it came from, so it isn't echoed back in a
    /// loop). Nothing is re-encoded: each TCP peer's outbox gets the bytes
    /// copied in behind a length prefix, and its writer is woken once per
    /// batch of events rather than per message.
    async fn broadcast(&mut self, msg: &Bytes, except: Option<usize>) {
        // bluetooth clients dont receive independent updates,
        // bluetooth really is a broadcast and only needs to go out once
        let mut sent_bt_broadcast = false;
        let mut failed: Vec<String> = Vec::new();
        for (i, p) in self.peers.iter().enumerate() {
            if Some(i) == except {
                continue;
            }
            if p.is_bt_client() {
                if sent_bt_broadcast {
                    continue;
                }
                sent_bt_broadcast = true;
            }
            if let Err(e) = self.send_peer(p, msg).await {
                warn!("{} write to {:?} failed, dropping peer: {:?}", self.name, p.address, e);
                failed.push(p.address.clone());
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
            // The whole message as received, type byte included, so
            // PROPERTY/DELETE can be forwarded without re-encoding.
            let raw = msg.clone();
            let msg_type = msg.get_u8();

            // Most branches below act on the peer that sent this message,
            // so look it up once by index instead of re-scanning self.peers
            // per branch. (The liveness ack happens before this, per event,
            // in `handle_event`.)
            let peer_idx = self.peer_index(from);

            match msg_type {
                PING => {
                    // Someone is asking if we're alive, answer with a PONG.
                    debug!("<<< <<< PING from {:?}", from);
                    if let Some(idx) = peer_idx {
                        debug!("SEND PONG {:?}", self.peers[idx].to_string());
                        self.send_peer(&self.peers[idx], &Bytes::from_static(&[PONG])).await?;
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
                    let (prop_id, value) = decode_property(&mut msg)?;
                    // Exactly the bytes that made up this property, minus
                    // anything trailing.
                    let frame = raw.slice(..raw.len() - msg.remaining());
                    match value {
                        None => {
                            debug!("DELETING {:?}", prop_id);
                            // Same id and payload, retagged as DELETE.
                            let mut del = BytesMut::with_capacity(frame.len());
                            del.put_u8(DELETE);
                            del.put_slice(&frame[1..]);
                            self.broadcast(&del.freeze(), peer_idx).await;
                            self.remove_property(prop_id);
                        }
                        Some(v) => {
                            self.broadcast(&frame, peer_idx).await;
                            self.apply_value(prop_id, Some(v));
                            // Publish to MQTT if the change did NOT originate from MQTT.
                            // We look up the property from self.properties by ID because
                            // the wire format carries only the numeric hash, not the
                            // name, and we need the name to build the MQTT topic.
                            #[cfg(feature = "mqtt")]
                            if from != "mqtt" {
                                if let Some(ref handle) = self.mqtt_handle {
                                    // Queued, never awaited: a slow or
                                    // missing broker must not stall the
                                    // TCP peers (see `PublishQueue`).
                                    if let Some(named_prop) = self.properties.get(&prop_id) {
                                        handle.publish_latest(named_prop);
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
                    if self.remove_property(msg.get_u64()) {
                        self.broadcast(&raw.slice(..9), peer_idx).await;
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
                        // Listeners run before this returns; don't hold
                        // queued output behind them.
                        self.flush_peers();
                        self.message_received.emit(String::from(message)).await;
                        return Ok(());
                    }
                    let pear_name = vec[0];
                    if self.name.to_string() == pear_name.to_string() {
                        info!("Message is for me: {:?}", vec[1]);
                        self.flush_peers();
                        self.message_received.emit(String::from(vec[1])).await;
                    } else if from.is_empty()
                        && self.get_peer_by_name(pear_name).is_none()
                        && self.peers.iter().any(|p| p.is_upstream())
                    {
                        // One of our own handlers is writing to a hive we
                        // aren't connected to directly. The server we dialed
                        // knows every client, so it gets the message whole,
                        // name included, and routes it. Only local messages
                        // go this way: relaying a peer's would let hives
                        // connected in a ring pass one around forever.
                        for p in self.peers.iter().filter(|p| p.is_upstream()) {
                            self.send_peer(p, &raw).await?;
                        }
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
                        self.send_peer(&self.peers[idx], &peer_bytes.freeze()).await?;
                        self.peers[idx].update_peers = true;
                    } else if from.is_empty() {
                        // `Handler::request_peers`: ask the servers we are
                        // connected to, and any we connect to later.
                        self.want_peers = true;
                        for p in self.peers.iter().filter(|p| p.is_upstream()) {
                            self.send_peer(p, &raw).await?;
                        }
                    }
                }
                PEER_RESPONSE => {
                    if let Some(observer) = &self.observer {
                        let list = std::str::from_utf8(&msg)
                            .map_err(|e| DecodeError(format!("peer list not UTF-8: {}", e)))?;
                        observer(HiveEvent::Peers(parse_peers(list)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::Charged;
    use std::sync::atomic::AtomicI64;
    use std::time::Instant;

    /// todo.md 5.8: the event loop must yield between batches. It never
    /// waits while events are queued (`futures` channels and the outbox are
    /// outside tokio's cooperative budget), so without the `yield_now` in
    /// `receive_events` a long backlog kept every other task on its worker
    /// thread from running; in 3.7 that starved a peer's writer until the
    /// peer was dropped as "not reading". The inbound budgets (5.3) now
    /// stop real senders from building such a backlog, so it is queued
    /// here directly, and a timer on the same single thread stands in for
    /// the writer.
    #[tokio::test(flavor = "current_thread")]
    async fn event_loop_yields_between_batches() {
        let mut hive = Hive::new_from_str_unknown("listen = \"127.0.0.1:0\"\nname = \"Backlog\"\n[Properties]\nm = 0");
        let last = Arc::new(AtomicI64::new(-1));
        let l = last.clone();
        hive.get_mut_property_by_name("m").unwrap().on_next(move |v| {
            l.store(v.val.as_integer().unwrap(), Ordering::Relaxed);
        });
        let events = 300_000i64;
        let id = Property::hash_id("m");
        for v in 1..=events {
            // Charged like a Handler would, without waiting.
            let msg = crate::property::encode_property(id, Some(&Value::Integer(v)), true);
            hive.local_inbound.queued.fetch_add(msg.len(), Ordering::Relaxed);
            hive.sender.unbounded_send(SocketEvent::Local { msg, charged: Charged(()) }).unwrap();
        }

        // Fired while the backlog is being worked through, and the
        // longest it waited. A starved timer doesn't fire at all.
        let (ticks, worst) = (Arc::new(AtomicI64::new(0)), Arc::new(AtomicI64::new(0)));
        let (t, w, l) = (ticks.clone(), worst.clone(), last.clone());
        let ticker = tokio::spawn(async move {
            loop {
                let at = Instant::now();
                tokio::time::sleep(Duration::from_millis(1)).await;
                if l.load(Ordering::Relaxed) > 0 && l.load(Ordering::Relaxed) < events {
                    t.fetch_add(1, Ordering::Relaxed);
                    w.fetch_max(at.elapsed().as_millis() as i64, Ordering::Relaxed);
                }
            }
        });
        tokio::task::yield_now().await; // let the ticker arm its first sleep

        let started = Instant::now();
        let cancel = CancellationToken::new();
        let _handler = hive.go(true, cancel.clone()).await;
        while last.load(Ordering::Relaxed) != events && started.elapsed() < Duration::from_secs(60) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let took = started.elapsed();
        ticker.abort();
        assert_eq!(last.load(Ordering::Relaxed), events);
        let (ticks, worst) = (ticks.load(Ordering::Relaxed), worst.load(Ordering::Relaxed));
        // One batch is ~1k events, well under a millisecond in release and
        // a few in debug, against tens of ms (release) for the backlog.
        assert!(
            ticks >= 5 && worst < 50,
            "a 1 ms timer fired {ticks} times (worst wait {worst} ms) during a {took:?} backlog: the event loop didn't yield"
        );
        cancel.cancel();
    }
}
