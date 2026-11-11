use std::sync::{Arc, RwLock};
use tokio::net::TcpStream;
use tokio::io::BufReader;
#[cfg(feature = "bluetooth")]
use bluster::gatt::event::Response;
use bytes::{BufMut, Bytes, BytesMut};
use std::fmt::Debug;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
#[cfg(feature = "bluetooth")]
use crate::bluetooth::central::Central;
use crate::hive::{Result, Sender, HEADER, HEADER_NAME, HIVE_PROTOCOL, PING, PONG};
use crate::security::{BoxRead, BoxWrite, Security};
#[cfg(feature = "websock")]
use crate::websocket::WebSock;
use futures::channel::mpsc::UnboundedSender;
use crate::futures::SinkExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc as tokio_mpsc;
#[allow(unused_imports)]
use crate::logging::{debug, error, info, trace, warn};
use tokio_util::sync::CancellationToken;

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
        /// Already handshaken TCP connection, None for bluetooth.
        stream: Option<Connection>,
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
    /// A plain-text MQTT payload (`payload = "text"`) for property `id`.
    /// Parsed in the event loop, which knows the property's current type.
    MqttText {
        id: u64,
        text: Bytes,
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
    /// Queue to this peer's writer task (TCP peers only). See `write_loop`.
    out: Option<tokio_mpsc::Sender<Bytes>>,
    pub update_peers: bool,
    pub peripheral: Option<Sender<Bytes>>,
    central: Option<Central>,
    pub address: String,
    /// Milliseconds since process start (`now_ms`) when anything was last
    /// heard from this peer. An atomic so `ack`, which runs on every
    /// message, never takes a lock, and monotonic so a wall-clock change
    /// can't trigger a false hangup.
    last_received: Arc<AtomicU64>,
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

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
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
        conn: Option<Connection>,
        peripheral: Option<Sender<Bytes>>,
        central: Option<Central>,
        sender: UnboundedSender<SocketEvent>,
        address: String,
        hive_name: String,
        peer_type: PeerType,
        max_message_size: u32,
    ) -> Result<Peer> {
        if let Some(conn) = conn {
            let Connection { reader, writer, address, remote_name } = conn;
            let mut peer = Peer {
                name: Arc::new(RwLock::new(remote_name.unwrap_or(name))),
                out: None,
                update_peers: false,
                peripheral,
                central,
                address: address.clone(),
                last_received: Arc::new(AtomicU64::new(now_ms())),
                ack_check: Arc::new(AtomicBool::new(false)),
                event_sender: sender.clone(),
                web_sock: None,
                hive_name,
                peer_type,
                shutdown: CancellationToken::new(),
            };

            let (tx, rx) = tokio_mpsc::channel(SEND_QUEUE_FRAMES);
            peer.out = Some(tx);
            tokio::spawn(write_loop(writer, rx, sender.clone(), address.clone(), peer.shutdown.clone()));

            let name_id = peer.get_id_name();
            let shutdown = peer.shutdown.clone();
            debug!("start tcp socket read loop for({})", name_id);
            tokio::spawn(async move {
                let _ = read_loop(sender, name_id, address, reader, max_message_size, shutdown).await;
            });

            Ok(peer)
        } else {
            Ok(Peer {
                name: Arc::new(RwLock::new(name)),
                out: None,
                update_peers: false,
                peripheral,
                central,
                address,
                last_received: Arc::new(AtomicU64::new(now_ms())),
                ack_check: Arc::new(AtomicBool::new(false)),
                event_sender: sender,
                web_sock: None,
                hive_name,
                peer_type,
                shutdown: CancellationToken::new(),
            })
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
        self.last_received.store(now_ms(), Relaxed);
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
        let out = self.out.clone();
        let peripheral = self.peripheral.clone();
        let name_clone = self.name.clone();
        let addr_clone = self.address.clone();
        let ack_check_clone = self.ack_check.clone();
        let mut sender_clone = self.event_sender.clone();
        let last_received_clone = self.last_received.clone();
        let tick = (timeout / 2).max(Duration::from_millis(20));

        if out.is_none() && peripheral.is_none() {
            debug!("wave: {:?} is not TCP or bluetooth-peripheral, skipping", addr_clone);
            return;
        }

        tokio::spawn(async move {
            'wave_loop: loop {
                tokio::time::sleep(tick).await;
                let since_last_comm =
                    Duration::from_millis(now_ms().saturating_sub(last_received_clone.load(Relaxed)));
                if since_last_comm >= timeout {
                    let nc = name_clone.read().unwrap().clone();
                    debug!("wave: no traffic from {:?} for {:?}, pinging", nc, since_last_comm);
                    let mut bytes = BytesMut::with_capacity(nc.len() + 1);
                    bytes.put_u8(PING);
                    bytes.put_slice(nc.as_bytes());
                    let bytes = bytes.freeze();

                    ack_check_clone.store(true, Relaxed);
                    let sent: Result<()> = if let Some(out) = &out {
                        // A full queue means plenty is already headed
                        // out; the PONG (or that traffic's replies) will
                        // still settle ack_check.
                        match out.try_send(bytes) {
                            Err(tokio_mpsc::error::TrySendError::Closed(_)) => Err("writer closed".into()),
                            _ => Ok(()),
                        }
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

    /// Queue `msg` for this peer. For TCP this never waits on the socket:
    /// the writer task does the I/O. If the queue is full the call waits up
    /// to `SLOW_PEER_TIMEOUT` for room (backpressure for a healthy peer
    /// that's briefly behind), then gives up with an error so the hive can
    /// drop a peer that has stopped reading.
    pub async fn send(&self, msg: Bytes) -> Result<()> {
        if let Some(out) = &self.out {
            match out.try_send(msg) {
                Ok(()) => {}
                Err(tokio_mpsc::error::TrySendError::Full(msg)) => {
                    match tokio::time::timeout(SLOW_PEER_TIMEOUT, out.send(msg)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => return Err(format!("{} writer closed", self.address).into()),
                        Err(_) => {
                            return Err(format!(
                                "{} send queue full for {:?}, peer not reading",
                                self.address, SLOW_PEER_TIMEOUT
                            )
                            .into())
                        }
                    }
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                    return Err(format!("{} writer closed", self.address).into())
                }
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
        } else if let Some(peripheral) = &self.peripheral {
            peripheral.clone().send(msg).await?;
        } else {
            return Err(format!("peer {:?} has no transport to send on", self.address).into());
        }
        Ok(())
    }
}

/// Frames a single peer's writer may queue before `Peer::send` starts
/// waiting. Frames are mostly property updates of a few dozen bytes.
const SEND_QUEUE_FRAMES: usize = 1024;
/// How long `Peer::send` waits on a full queue before declaring the peer
/// stuck. Bounds how long one non-reading peer can stall the event loop.
const SLOW_PEER_TIMEOUT: Duration = Duration::from_secs(2);
/// Stop batching once a write buffer reaches this size.
const MAX_WRITE_BATCH: usize = 64 * 1024;

/// Owns a peer's write half. Takes everything queued, length-prefixes it
/// into one reused buffer and writes it with a single `write_all`, so a
/// burst of small updates costs one syscall rather than one each, and no
/// per-message allocation. Generic over the writer so TLS can reuse it.
///
/// Ends when the peer is dropped (`shutdown`, also aborting a write stuck
/// on a dead link) or on a write error, which it reports as a Hangup.
/// Dropping the write half on exit sends FIN.
pub(crate) async fn write_loop<W>(
    mut writer: W,
    mut rx: tokio_mpsc::Receiver<Bytes>,
    events: UnboundedSender<SocketEvent>,
    address: String,
    shutdown: CancellationToken,
) where
    W: AsyncWrite + Unpin,
{
    let mut buf = BytesMut::with_capacity(8 * 1024);
    loop {
        let first = tokio::select! {
            _ = shutdown.cancelled() => break,
            m = rx.recv() => match m {
                Some(m) => m,
                None => break,
            },
        };
        buf.clear();
        put_frame(&mut buf, &first);
        while buf.len() < MAX_WRITE_BATCH {
            match rx.try_recv() {
                Ok(m) => put_frame(&mut buf, &m),
                Err(_) => break,
            }
        }
        let res = tokio::select! {
            _ = shutdown.cancelled() => break,
            // flush is a no-op on a plain TCP half; for TLS it makes sure
            // the encrypted records are actually on the socket.
            r = async { writer.write_all(&buf).await?; writer.flush().await } => r,
        };
        if let Err(e) = res {
            debug!("write to {} failed: {:?}", address, e);
            let _ = events.unbounded_send(SocketEvent::Hangup { from: address.clone() });
            break;
        }
        // Don't keep a one-off huge buffer around for the peer's lifetime.
        if buf.capacity() > 4 * MAX_WRITE_BATCH {
            buf = BytesMut::with_capacity(8 * 1024);
        }
    }
    trace!("writer for {} stopped", address);
}

fn put_frame(buf: &mut BytesMut, msg: &Bytes) {
    buf.put_u32(msg.len() as u32);
    buf.put_slice(msg);
}

/// A TCP connection that has completed the Hive handshake and is ready to
/// become a `Peer`. Produced off the event loop (in the accept or connect
/// task) so a slow or silent remote can never stall the hive itself.
pub struct Connection {
    /// Plain TCP halves or TLS halves, see `Security::wrap`.
    pub(crate) reader: BufReader<BoxRead>,
    pub(crate) writer: BoxWrite,
    pub(crate) address: String,
    /// The name the remote announced in its handshake, when it sends one
    /// there (clients do; servers send theirs later as a HEADER message).
    pub(crate) remote_name: Option<String>,
}

impl Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("address", &self.address)
            .field("remote_name", &self.remote_name)
            .finish()
    }
}

/// Longest handshake line we'll buffer. Hive's are a protocol tag or a
/// name, a few dozen bytes. Without a cap a remote could stream bytes with
/// no newline and grow the line buffer without bound.
const MAX_HANDSHAKE_LINE: u64 = 1024;
pub(crate) const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

fn protocol_err(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

async fn read_handshake_line(reader: &mut BufReader<BoxRead>) -> io::Result<String> {
    let mut line = String::new();
    let n = (&mut *reader).take(MAX_HANDSHAKE_LINE).read_line(&mut line).await?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed during handshake"));
    }
    if !line.ends_with('\n') {
        return Err(protocol_err("handshake line too long or truncated"));
    }
    Ok(line)
}

/// Marker the dialing side appends to the protocol line when it has an
/// `auth_secret`, so a configuration mismatch fails with a clear error
/// instead of hanging until the handshake timeout.
const AUTH_MARKER: &str = "+auth";

/// Run the Hive handshake on a freshly connected or accepted stream, with
/// `timeout` covering the whole exchange: TLS (if configured), the Hive
/// protocol line and name, and the shared-secret check (if configured).
///
/// `peer_type` is what the *remote* is to us: `TcpServer` when we dialed
/// out (we announce ourselves), `TcpClient` when we accepted (we read the
/// remote's announcement, then send our name back as a HEADER frame).
/// `dial_addr` is the address we dialed, used as the TLS server name when
/// none is configured.
pub(crate) async fn handshake(
    stream: TcpStream,
    peer_type: &PeerType,
    hive_name: &str,
    timeout: Duration,
    security: &Security,
    dial_addr: &str,
) -> io::Result<Connection> {
    // Small frames are the norm here; don't let Nagle hold them back.
    stream.set_nodelay(true)?;
    let address = stream.peer_addr()?.to_string();

    let exchange = async {
        let accepting = matches!(peer_type, PeerType::TcpClient);
        let (read_half, mut writer) = security.wrap(stream, accepting, dial_addr).await?;
        let mut reader = BufReader::new(read_half);

        let remote_name = match peer_type {
            PeerType::TcpServer => {
                let marker = if security.auth_enabled() { AUTH_MARKER } else { "" };
                let mut bm = BytesMut::with_capacity(HIVE_PROTOCOL.len() + marker.len() + hive_name.len() + 3);
                bm.put_slice(HIVE_PROTOCOL.as_bytes());
                bm.put_slice(marker.as_bytes());
                bm.put_u8(b'\n');
                bm.put_u8(HEADER_NAME);
                bm.put_slice(hive_name.as_bytes());
                bm.put_u8(b'\n');
                writer.write_all(&bm).await?;
                writer.flush().await?;
                security.auth_dial(&mut reader, &mut writer).await?;
                None
            }
            PeerType::TcpClient => {
                let mut line = read_handshake_line(&mut reader).await?;
                if line == "\u{0}\u{0}\u{0}\n" {
                    line = read_handshake_line(&mut reader).await?;
                }
                if !line.starts_with(HIVE_PROTOCOL) {
                    return Err(protocol_err("not a Hive client"));
                }
                let offers_auth = line[HIVE_PROTOCOL.len()..].trim_end() == AUTH_MARKER;
                match (security.auth_enabled(), offers_auth) {
                    (true, false) => {
                        return Err(crate::security::denied("peer has no auth_secret, this hive requires one"))
                    }
                    (false, true) => {
                        return Err(crate::security::denied("peer wants auth_secret, this hive has none"))
                    }
                    _ => {}
                }
                let line = read_handshake_line(&mut reader).await?;
                let bytes = line.as_bytes();
                let remote_name = if bytes.first() == Some(&HEADER_NAME) {
                    Some(line[1..line.len() - 1].to_string())
                } else {
                    warn!("handshake from {} had no name header", address);
                    None
                };
                security.auth_accept(&mut reader, &mut writer).await?;

                let name_bytes = hive_name.as_bytes();
                let mut out = BytesMut::with_capacity(4 + 2 + name_bytes.len());
                out.put_u32((name_bytes.len() + 2) as u32);
                out.put_u8(HEADER);
                out.put_u8(HEADER_NAME);
                out.put_slice(name_bytes);
                writer.write_all(&out).await?;
                writer.flush().await?;
                remote_name
            }
            other => return Err(protocol_err(format!("no TCP handshake for {:?}", other))),
        };
        Ok((reader, writer, remote_name))
    };

    let (reader, writer, remote_name) = tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))??;

    Ok(Connection { reader, writer, address, remote_name })
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
    mut reader: BufReader<BoxRead>,
    max_message_size: u32,
    shutdown: CancellationToken,
) -> Result<()> {
    loop {
        let r = tokio::select! {
            _ = shutdown.cancelled() => {
                debug!("{:?} read loop shutting down, peer dropped", peer_id_string);
                break;
            }
            r = read_frame(&mut reader, max_message_size) => r,
        };
        match r {
            Ok(msg) => {
                trace!("{:?} read message: {:?}", peer_id_string, &msg);
                // unbounded_send only fails once the hive has stopped
                // receiving, at which point there is nobody left to tell.
                if sender
                    .unbounded_send(SocketEvent::Message { from: from.clone(), msg })
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    debug!("{:?} closed the connection", peer_id_string);
                } else {
                    error!("{:?} read failed, dropping connection: {:?}", peer_id_string, e);
                }
                let _ = sender.unbounded_send(SocketEvent::Hangup { from });
                break;
            }
        }
    }
    debug!("<< Peer run done");
    Ok(())
}

/// Read one length-prefixed frame. `read_exact` for the prefix matters: a
/// plain `read` can legally return 1-3 bytes when the 4-byte header
/// straddles a buffer boundary, which used to desync the whole stream.
/// The body read is bounded with `take` so it can never consume bytes
/// belonging to the next frame, and fills `BytesMut`'s spare capacity
/// directly instead of zeroing a buffer first.
pub(crate) async fn read_frame<R>(reader: &mut R, max_message_size: u32) -> io::Result<Bytes>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut size_buff = [0u8; 4];
    reader.read_exact(&mut size_buff).await?;
    let message_size = as_u32_be(&size_buff);
    if message_size > max_message_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("claimed message size {} exceeds {} byte cap", message_size, max_message_size),
        ));
    }
    let n = message_size as usize;
    let mut buf = BytesMut::with_capacity(n);
    let mut limited = reader.take(n as u64);
    while buf.len() < n {
        if limited.read_buf(&mut buf).await? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed mid-message"));
        }
    }
    Ok(buf.freeze())
}
