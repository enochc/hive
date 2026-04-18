use std::sync::Arc;
use tokio::sync::{RwLock, Mutex as TokioMutex};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::io::BufReader;
#[cfg(feature = "bluetooth")]
use bluster::gatt::event::Response;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::fmt::Debug;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, SystemTime};
#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::hive::{Result, Sender, HEADER, HEADER_NAME, HIVE_PROTOCOL, PING, PONG};
#[cfg(feature = "websock")]
use crate::websocket::WebSock;
use futures::channel::mpsc::UnboundedSender;
use crate::futures::SinkExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
#[allow(unused_imports)]
use log::{debug, info, trace, warn};
use tracing::error;

const ACK_DURATION: u64 = 30;

#[cfg(not(feature = "bluetooth"))]
#[derive(Debug)]
pub struct Response {}

#[cfg(not(feature = "websock"))]
#[derive(Debug)]
pub struct WebSock {}

#[cfg(not(feature = "bluetooth"))]
#[derive(Debug)]
pub struct Central {}

#[derive(Debug)]
pub enum SocketEvent {
    NewPeer {
        name: String,
        stream: Option<TcpStream>,
        peripheral: Option<Sender<Bytes>>,
        central: Option<Central>,
        address: String,
        ptype: PeerType,
    },
    Message {
        from: String,
        msg: Bytes,
    },
    SendBtProps {
        sender: futures::channel::oneshot::Sender<Response>,
    },
    Hangup {
        from: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerType {
    TcpServer = 0,
    TcpClient = 1,
    WebSockClient = 2,
    BluetoothCentral = 3,
    BluetoothPeripheral = 4,
}

/// Controls which properties are sent to a specific peer.
///
/// Specified during connection setup (via TOML config or header exchange)
/// to reduce traffic for peers that only care about a subset of properties.
#[derive(Debug, Clone)]
pub enum PropertyFilter {
    /// Receive all properties (default, backward compatible).
    All,
    /// Only receive properties whose names start with one of these prefixes.
    Include(Vec<String>),
    /// Receive all properties except those whose names start with these prefixes.
    Exclude(Vec<String>),
}

impl Default for PropertyFilter {
    fn default() -> Self {
        PropertyFilter::All
    }
}

impl PropertyFilter {
    /// Check whether a property with the given name should be sent to
    /// a peer using this filter.
    pub fn should_send(&self, property_name: &str) -> bool {
        match self {
            PropertyFilter::All => true,
            PropertyFilter::Include(prefixes) => {
                prefixes.iter().any(|prefix| property_name.starts_with(prefix))
            }
            PropertyFilter::Exclude(prefixes) => {
                !prefixes.iter().any(|prefix| property_name.starts_with(prefix))
            }
        }
    }
}

pub struct Peer {
    name: Arc<RwLock<String>>,
    write_stream: Option<Arc<TokioMutex<OwnedWriteHalf>>>,
    pub update_peers: bool,
    pub peripheral: Option<Sender<Bytes>>,
    central: Option<Central>,
    pub address: String,
    last_received: Arc<RwLock<SystemTime>>,
    ack_check: Arc<AtomicBool>,
    web_sock: Option<WebSock>,
    event_sender: UnboundedSender<SocketEvent>,
    hive_name: String,
    peer_type: PeerType,
    /// Property filter for this peer.  Controls which properties
    /// are sent during initial sync and ongoing broadcasts.
    pub property_filter: PropertyFilter,
}

impl Debug for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peer")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("peer_type", &self.peer_type)
            .finish()
    }
}

fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) + ((array[1] as u32) << 16) + ((array[2] as u32) << 8) + (array[3] as u32)
}

impl Peer {
    pub fn is_bt_client(&self) -> bool {
        self.peripheral.is_some()
    }
    
    pub fn to_string(&self) -> String {
        format!("{:?},{:?}", self.get_name(), self.address)
    }
    
    pub fn is_web_socket(&self) -> bool {
        self.web_sock.is_some()
    }

    pub async fn set_name(&self, name: &str) {
        debug!("{:?} set name = {:?}", self.get_id_name(), name);
        *self.name.write().await = String::from(name);
        debug!("set: {:?}", self.get_id_name());
    }
    
    pub fn address(&self) -> String {
        self.address.clone()
    }

    pub async fn new(
        name: String,
        stream: Option<TcpStream>,
        peripheral: Option<Sender<Bytes>>,
        central: Option<Central>,
        sender: UnboundedSender<SocketEvent>,
        address: String,
        hive_name: String,
        peer_type: PeerType,
    ) -> Result<Peer> {
        if let Some(tcp_stream) = stream {
            let addr = tcp_stream.peer_addr()?.to_string();
            let from = addr.clone();
            
            let mut peer = Peer {
                name: Arc::new(RwLock::new(name)),
                write_stream: None,
                update_peers: false,
                peripheral,
                central,
                address: addr,
                last_received: Arc::new(RwLock::new(std::time::SystemTime::now())),
                ack_check: Arc::new(AtomicBool::new(false)),
                event_sender: sender.clone(),
                web_sock: None,
                hive_name,
                peer_type,
                property_filter: PropertyFilter::default(),
            };

            let send_clone = sender.clone();
            
            // Perform handshake, then split the stream
            let (reader, write_half) = peer.handshake(tcp_stream, &sender).await?;
            peer.write_stream = Some(Arc::new(TokioMutex::new(write_half)));
            
            let is_websocket = peer.is_web_socket();

            // WebSock runs its own read loop
            if !is_websocket {
                let name_id = peer.get_id_name();
                debug!("start tcp socket read loop for({})", name_id);

                tokio::spawn(async move {
                    read_loop(send_clone, name_id, from, reader)
                        .await
                        .expect("Read Loop Failed");
                });
            }

            Ok(peer)
        } else {
            Ok(Peer {
                name: Arc::new(RwLock::new(name)),
                write_stream: None,
                update_peers: false,
                peripheral,
                central,
                address,
                last_received: Arc::new(RwLock::new(std::time::SystemTime::now())),
                ack_check: Arc::new(AtomicBool::new(false)),
                event_sender: sender,
                web_sock: None,
                hive_name,
                peer_type,
                property_filter: PropertyFilter::default(),
            })
        }
    }

    async fn handshake(
        &mut self,
        stream: TcpStream,
        _sender: &UnboundedSender<SocketEvent>,
    ) -> Result<(BufReader<OwnedReadHalf>, OwnedWriteHalf)> {
        match self.peer_type {
            PeerType::TcpServer => {
                // Split stream into read/write halves
                let (read_half, mut write_half) = stream.into_split();
                
                // send name of client connecting to server
                debug!("<<<< SEND CLIENT HANDSHAKE ⭐️🔻 ");
                let mut bm = BytesMut::new();
                bm.put_slice(format!("{}\n", HIVE_PROTOCOL).as_bytes());
                bm.put_u8(HEADER_NAME);
                bm.put_slice(format!("{}\n", self.hive_name).as_bytes());
                debug!("<<<<< SENDING:: {:?} >>>>>>", bm);
                write_half.write_all(bm.as_ref()).await.expect("write failed");
                write_half.flush().await.expect("flush failed");

                Ok((BufReader::new(read_half), write_half))
            }
            PeerType::TcpClient => {
                // Split stream into read/write halves
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                
                debug!("<<<< READ CLIENT HANDSHAKE ⭐️⭐️🔺 ");
                let mut str = String::new();
                let bytes = reader.read_line(&mut str).await?;
                debug!("READ🔥🔥🔥🔥: {}, {:?}", bytes, str);
                if bytes == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Client disconnected").into());
                }
                info!("BYTES READ ***** : {:?}", str);
                if str == "\u{0}\u{0}\u{0}\n" {
                    str = "".to_string();
                    reader.read_line(&mut str).await?;
                    debug!("again: {:?}", str);
                }

                if str.starts_with(HIVE_PROTOCOL) {
                    loop {
                        let mut bm = BytesMut::new();
                        str = "".to_string();
                        reader.read_line(&mut str)
                            .await
                            .expect("failed to read line post header");
                        debug!("next line:: {:?}", str);
                        bm.put_slice(str.as_bytes());
                        let my8 = bm.get_u8();
                        match my8 {
                            HEADER_NAME => {
                                debug!("got name: {:?}", bm);
                                bm.truncate(bm.len() - 1);
                                let name = String::from_utf8(bm.to_vec()).expect("Can't parse name");
                                self.set_name(&name).await;
                                break;
                            }
                            _ => {
                                warn!("something else: {:?}", my8);
                                break;
                            }
                        }
                    }
                }
                
                // send headers using the write half
                let name_bytes = self.hive_name.as_bytes();
                let mut header_bytes = BytesMut::with_capacity(name_bytes.len() + 2);
                header_bytes.put_u8(HEADER);
                header_bytes.put_u8(HEADER_NAME);
                header_bytes.put_slice(name_bytes);
                info!(".... send headers: {:?}", header_bytes);
                
                // Build the message with length prefix
                let msg = header_bytes.freeze();
                let mut wire_bytes = Vec::new();
                let msg_length: u32 = msg.len() as u32;
                wire_bytes.extend_from_slice(&msg_length.to_be_bytes());
                wire_bytes.extend_from_slice(&msg);
                write_half.write_all(&wire_bytes).await?;
                write_half.flush().await?;
                
                Ok((reader, write_half))
            }
            _ => {
                unimplemented!("finish this!!");
            }
        }
    }

    pub fn get_name(&self) -> String {
        match self.name.try_read() {
            Ok(guard) => String::from(&*guard),
            Err(_) => {
                let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
                let name = self.name.clone();
                rt.block_on(async { String::from(&*name.read().await) })
            }
        }
    }
    
    pub fn get_id_name(&self) -> String {
        format!("{}/{} @ {}", self.hive_name, self.get_name(), self.address)
    }
    
    pub fn receive_pong(&self) {
        debug!("RECEIVED PING {:?}", self.to_string());
        self.ack_check.store(false, Relaxed);
    }
    
    pub async fn ack(&self) {
        debug!("ACK {:?}", self.to_string());
        *self.last_received.write().await = SystemTime::now();
    }
    
    pub async fn send_pong(&self) -> Result<()> {
        debug!("SEND PONG {:?}", self.to_string());
        let byte = Bytes::from_static(&[PONG]);
        self.send(byte).await
    }

    #[allow(dead_code)]
    pub async fn _wave(&self) {
        let name_clone = self.name.clone();
        let addr_clone = self.address.clone();
        let mut perf_clone = self.peripheral.as_ref().unwrap().clone();
        let ack_check_clone = self.ack_check.clone();
        let mut sender_clone = self.event_sender.clone();
        let adr_clone = self.address.clone();
        let last_received_clone = self.last_received.clone();
        tokio::spawn(async move {
            'wave_loop: loop {
                debug!("send Hi {:?}, {}", name_clone.read().await, addr_clone);
                tokio::time::sleep(Duration::from_secs(ACK_DURATION)).await;
                let since_last_comm = SystemTime::now().duration_since(*last_received_clone.read().await);
                if since_last_comm.unwrap() > Duration::from_secs(ACK_DURATION) {
                    let nc = &*name_clone.read().await;
                    let mut bytes = BytesMut::with_capacity(nc.len() + 1);
                    bytes.put_u8(PING);
                    bytes.put_slice(nc.as_bytes());

                    match perf_clone.send(bytes.into()).await {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("Error sending:: {:?}", e);
                            break 'wave_loop;
                        }
                    }
                    ack_check_clone.store(true, Relaxed);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if ack_check_clone.load(Relaxed) {
                        debug!("KILL THIS PEER IS DEAD:: {:?}", name_clone.read().await);
                        sender_clone
                            .send(SocketEvent::Hangup {
                                from: adr_clone.clone(),
                            })
                            .await
                            .expect("failed to send hangup");
                        break 'wave_loop;
                    }
                }
            }
            trace!("Done Waving <<< {:?}", name_clone.read().await);
        });
    }

    pub async fn send(&self, msg: Bytes) -> Result<()> {
        debug!("SEND starts here {:?}", msg);
        if self.write_stream.is_some() {
            if self.web_sock.is_some() {
                warn!(
                    "<<< fail webclient is broken >>>> Sending message to web client at {:?} = {:?}",
                    self.address, msg
                );
            } else {
                debug!(
                    "{:?} ⭐️ Send to peer {}: {:?}",
                    self.get_id_name(),
                    self.name.read().await,
                    msg
                );
                self.send_on_stream(msg).await?;
            }
        } else if self.central.is_some() {
            #[cfg(feature = "bluetooth")]
            {
                debug!("SEND to bluetooth {:?} from {:?}", msg, self.name);
                let mut buff = BytesMut::new();
                buff.put_slice(msg.as_ref());
                let sender = self.central.as_ref().unwrap();
                sender.send(buff).await;
            }
        } else if self.peripheral.is_some() {
            debug!("Send via bt peripheral");
            let mut buff = BytesMut::with_capacity(msg.len());
            buff.put_slice(msg.as_ref());
            let b = buff.freeze();
            self.peripheral
                .as_ref()
                .unwrap()
                .send(b.clone())
                .await
                .expect("failed to send something somewhere");
            debug!("sent...");
        } else {
            unimplemented!("cant send: {:?}", msg);
        }

        Ok(())
    }

    async fn send_on_stream(&self, message: Bytes) -> Result<bool> {
        let mut bytes = Vec::new();
        let msg_length: u32 = message.len() as u32;
        bytes.extend_from_slice(&msg_length.to_be_bytes());
        bytes.extend_from_slice(&message);
        
        let mut stream = self.write_stream.as_ref().unwrap().lock().await;
        stream.write_all(&bytes).await.expect("Failed to write to stream");
        stream.flush().await.expect("flush failed");
        Ok(true)
    }
}

async fn read_loop(
    sender: UnboundedSender<SocketEvent>,
    peer_id_string: String,
    from: String,
    mut reader: BufReader<OwnedReadHalf>,
) -> Result<()> {
    let mut is_running = true;
    while is_running {
        let mut sender = sender.clone();
        let mut size_buff = [0; 4];
        debug!("{:?} READING!!!!", peer_id_string);
        let r = reader.read(&mut size_buff).await;
        let from = String::from(&from);
        match r {
            Ok(read) => {
                if read == 0 {
                    debug!("Read zero bytes");
                    sender
                        .send(SocketEvent::Hangup { from })
                        .await
                        .expect("Failed to send Hangup");
                    is_running = false;
                } else {
                    let message_size = as_u32_be(&size_buff);
                    let mut size_buff = vec![0u8; message_size as usize];
                    let red = reader.read_exact(&mut size_buff).await;
                    match red {
                        Ok(_t) => {
                            let msg = Bytes::from(size_buff);
                            debug!("<<<<<<<<<<<<<<<<< {:?} Read message: {:?}", peer_id_string, &msg);
                            let debug_msg = msg.clone();
                            let se = SocketEvent::Message { from, msg };
                            if !sender.is_closed() {
                                sender.send(se).await.expect("Failed to send message");
                                debug!("{:?} send message {:?}", peer_id_string, debug_msg);
                            }
                        }
                        Err(e) => {
                            error!("Error reading from socket: {:?}", e);
                            sender
                                .send(SocketEvent::Hangup { from })
                                .await
                                .expect("Failed to send Hangup");
                        }
                    }
                }
            }
            Err(e) => {
                error!("ERROR: {:?}", e);
                sender
                    .send(SocketEvent::Hangup { from })
                    .await
                    .expect("Failed to send hangup");
                is_running = false;
            }
        }
    }
    debug!("<< Peer run done");
    Ok(())
}
