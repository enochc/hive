use std::sync::{Arc, RwLock};
use tokio::sync::Mutex as TokioMutex;
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
use tokio_util::sync::CancellationToken;
use tracing::error;

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
    /// Cancelled when this `Peer` is dropped, which is what ends
    /// `read_loop`. Dropping the peer drops the `OwnedWriteHalf`, and tokio
    /// shuts the write side down for us, so on a healthy link the remote
    /// sees EOF and replies in kind, ending the read loop on its own. On a
    /// link that's already dead (the case `wave` exists to catch) that FIN
    /// never lands, nothing comes back, and the read task would sit parked
    /// on `read()` holding the socket forever. A `Hive` that tears down and
    /// reconnects on a timer would strand one per dropped session.
    shutdown: CancellationToken,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
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

    pub fn set_name(&self, name: &str) {
        debug!("{:?} set name = {:?}", self.get_id_name(), name);
        *self.name.write().unwrap() = String::from(name);
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
        max_message_size: u32,
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
                shutdown: CancellationToken::new(),
            };

            let send_clone = sender.clone();

            // Perform handshake, then split the stream
            let (reader, write_half) = peer.handshake(tcp_stream, &sender).await?;
            peer.write_stream = Some(Arc::new(TokioMutex::new(write_half)));

            let is_websocket = peer.is_web_socket();

            // WebSock runs its own read loop
            if !is_websocket {
                let name_id = peer.get_id_name();
                let shutdown = peer.shutdown.clone();
                debug!("start tcp socket read loop for({})", name_id);

                tokio::spawn(async move {
                    read_loop(send_clone, name_id, from, reader, max_message_size, shutdown)
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
                shutdown: CancellationToken::new(),
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
                                self.set_name(&name);
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
        self.name.read().unwrap().clone()
    }
    
    pub fn get_id_name(&self) -> String {
        format!("{}/{} @ {}", self.hive_name, self.get_name(), self.address)
    }
    
    pub fn receive_pong(&self) {
        debug!("RECEIVED PING {:?}", self.to_string());
        self.ack_check.store(false, Relaxed);
    }
    
    pub fn ack(&self) {
        debug!("ACK {:?}", self.to_string());
        *self.last_received.write().unwrap() = SystemTime::now();
    }
    
    pub async fn send_pong(&self) -> Result<()> {
        debug!("SEND PONG {:?}", self.to_string());
        let byte = Bytes::from_static(&[PONG]);
        self.send(byte).await
    }

    /// Start a background liveness check for this peer. If nothing at all
    /// has been heard from it (any message, not just a pong, counts, see
    /// `ack()`) for `timeout`, send it a PING. If neither a PONG nor any
    /// other traffic shows up within a further `timeout`, declare it dead
    /// and emit a Hangup, same as a real TCP disconnect would.
    ///
    /// Only clones the two handles actually needed to send a byte on the
    /// wire (a TCP write half or a bluetooth peripheral sender, both just
    /// cheap Arc/channel clones), not the whole `Peer`. That keeps `Peer`
    /// itself from having to be `Clone` (which would also force `WebSock`
    /// and `Central` to be, dragging in things like the websocket header
    /// map for no reason). Websocket and bluetooth-central peers don't get
    /// a liveness check from this yet, both have their own read loops that
    /// already emit Hangup on a real disconnect, this only adds detection
    /// of a silently dead link for the transports we actually exercise.
    pub async fn wave(&self, timeout: Duration) {
        let write_stream = self.write_stream.clone();
        let peripheral = self.peripheral.clone();
        let name_clone = self.name.clone();
        let addr_clone = self.address.clone();
        let ack_check_clone = self.ack_check.clone();
        let mut sender_clone = self.event_sender.clone();
        let last_received_clone = self.last_received.clone();
        let tick = (timeout / 2).max(Duration::from_millis(20));

        if write_stream.is_none() && peripheral.is_none() {
            debug!("wave: {:?} is not TCP or bluetooth-peripheral, skipping", addr_clone);
            return;
        }

        tokio::spawn(async move {
            'wave_loop: loop {
                tokio::time::sleep(tick).await;
                let since_last_comm = SystemTime::now()
                    .duration_since(*last_received_clone.read().unwrap())
                    .unwrap_or_default();
                if since_last_comm >= timeout {
                    let nc = name_clone.read().unwrap().clone();
                    debug!("wave: no traffic from {:?} for {:?}, pinging", nc, since_last_comm);
                    let mut bytes = BytesMut::with_capacity(nc.len() + 1);
                    bytes.put_u8(PING);
                    bytes.put_slice(nc.as_bytes());
                    let bytes = bytes.freeze();

                    ack_check_clone.store(true, Relaxed);
                    let sent = if let Some(stream) = &write_stream {
                        write_framed(stream, &bytes).await
                    } else {
                        peripheral
                            .as_ref()
                            .unwrap()
                            .clone()
                            .send(bytes)
                            .await
                            .map_err(Into::into)
                    };
                    if let Err(e) = sent {
                        debug!("wave: failed to ping {:?}: {:?}", addr_clone, e);
                        break 'wave_loop;
                    }

                    tokio::time::sleep(timeout).await;
                    if ack_check_clone.load(Relaxed) {
                        debug!("wave: {:?} unresponsive, declaring dead", addr_clone);
                        let _ = sender_clone
                            .send(SocketEvent::Hangup {
                                from: addr_clone.clone(),
                            })
                            .await;
                        break 'wave_loop;
                    }
                }
            }
            trace!("wave stopped for {:?}", addr_clone);
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
                    self.name.read().unwrap(),
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
        write_framed(self.write_stream.as_ref().unwrap(), &message).await?;
        Ok(true)
    }
}

/// Length-prefix and write a message directly to a TCP write half. Shared by
/// `Peer::send_on_stream` and `Peer::wave`, the latter only holds a cloned
/// `Arc<TokioMutex<OwnedWriteHalf>>` rather than a whole `Peer`, since that's
/// the only piece it actually needs and it's a cheap refcount clone.
async fn write_framed(stream: &Arc<TokioMutex<OwnedWriteHalf>>, message: &Bytes) -> Result<()> {
    let mut bytes = Vec::with_capacity(4 + message.len());
    bytes.extend_from_slice(&(message.len() as u32).to_be_bytes());
    bytes.extend_from_slice(message);

    let mut stream = stream.lock().await;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

// Messages on the wire here are property syncs and small control frames, a
// float, a short string, at most a modest peer list, never anything close
// to this. The 4-byte length prefix comes straight off the wire with no
// other validation, so a corrupted stream or a bug on the sending side
// could otherwise claim a message is up to ~4GB and we'd try to allocate
// that in one shot. Reject anything absurd before allocating rather than
// trusting the prefix. Generous on purpose, this is a safety ceiling, not a
// tuned limit. Overridable per-Hive via the `max_message_size_bytes` config
// key (see Hive::new_from_str), this is only the fallback when that's unset.
pub(crate) const DEFAULT_MAX_MESSAGE_SIZE: u32 = 1024 * 1024;

async fn read_loop(
    sender: UnboundedSender<SocketEvent>,
    peer_id_string: String,
    from: String,
    mut reader: BufReader<OwnedReadHalf>,
    max_message_size: u32,
    shutdown: CancellationToken,
) -> Result<()> {
    let mut is_running = true;
    while is_running {
        let mut sender = sender.clone();
        let mut size_buff = [0; 4];
        debug!("{:?} READING!!!!", peer_id_string);
        let r = tokio::select! {
            _ = shutdown.cancelled() => {
                debug!("{:?} read loop shutting down, peer dropped", peer_id_string);
                break;
            }
            r = reader.read(&mut size_buff) => r,
        };
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
                    if message_size > max_message_size {
                        error!(
                            "{:?} claimed message size {} exceeds {} byte cap, dropping connection",
                            peer_id_string, message_size, max_message_size
                        );
                        sender
                            .send(SocketEvent::Hangup { from })
                            .await
                            .expect("Failed to send Hangup");
                        is_running = false;
                        continue;
                    }

                    // BytesMut::with_capacity reserves the exact size but
                    // leaves it uninitialized rather than zero-filling it
                    // like vec![0u8; n] would, read_buf writes directly
                    // into that spare capacity. We're about to overwrite
                    // every byte with the real read anyway, so there's no
                    // reason to pay for zeroing memory first.
                    let mut buf = BytesMut::with_capacity(message_size as usize);
                    let mut read_err = None;
                    while buf.len() < message_size as usize {
                        match reader.read_buf(&mut buf).await {
                            Ok(0) => {
                                read_err = Some(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "connection closed mid-message",
                                ));
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                read_err = Some(e);
                                break;
                            }
                        }
                    }

                    match read_err {
                        None => {
                            let msg = buf.freeze();
                            debug!("<<<<<<<<<<<<<<<<< {:?} Read message: {:?}", peer_id_string, &msg);
                            let debug_msg = msg.clone();
                            let se = SocketEvent::Message { from, msg };
                            if !sender.is_closed() {
                                sender.send(se).await.expect("Failed to send message");
                                debug!("{:?} send message {:?}", peer_id_string, debug_msg);
                            }
                        }
                        Some(e) => {
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
