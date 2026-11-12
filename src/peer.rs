use std::sync::{Arc, Mutex, RwLock};
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
use tokio::sync::Notify;
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
        /// Already handshaken TCP connection, None for bluetooth. Boxed
        /// so this rare event doesn't set the size of every queued one.
        stream: Option<Box<Connection>>,
        peripheral: Option<Sender<Bytes>>,
        central: Option<Central>,
        address: String,
        ptype: PeerType,
    },
    Message {
        from: String,
        msg: Bytes,
    },
    /// A run of complete length-prefixed frames from one TCP peer, exactly
    /// as they came off the wire. `read_loop` hands over everything a read
    /// returned as one event, so a burst of small updates costs one channel
    /// hop and no per-message allocation.
    Frames {
        from: Arc<str>,
        data: Bytes,
        /// Returns `data.len()` to the peer's inbound budget when dropped,
        /// i.e. once the hive has handled (or discarded) these frames.
        credit: Credit,
    },
    /// An encoded message from a local `Handler`, budgeted like `Frames`
    /// but given back by the event loop once per batch rather than per
    /// message (see `Charged`).
    Local {
        msg: Bytes,
        charged: Charged,
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

/// How many bytes one sender (a TCP peer's `read_loop`, or all of a hive's
/// `Handler`s together) may have queued for the hive's event loop and not
/// yet handled. Past this the sender waits: a peer's `read_loop` stops
/// reading, so TCP flow control pushes back on the far end, and
/// `Handler::set_property` waits before queuing more. Without it a sender
/// that outruns the event loop grows the queue without limit.
pub(crate) const INBOUND_LIMIT: usize = 256 * 1024;

/// One sender's share of the event loop's queue, see `INBOUND_LIMIT`.
#[derive(Debug)]
pub(crate) struct Inbound {
    pub(crate) queued: std::sync::atomic::AtomicUsize,
    room: Notify,
    limit: usize,
    /// Set once nobody will release anything any more (the hive stopped):
    /// waiting would then be forever.
    closed: AtomicBool,
}

impl Inbound {
    pub(crate) fn new(limit: usize) -> Arc<Inbound> {
        Arc::new(Inbound { queued: Default::default(), room: Notify::new(), limit, closed: AtomicBool::new(false) })
    }

    /// Stop making anyone wait, for good.
    pub(crate) fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::Release);
        self.room.notify_waiters();
    }

    /// Give `n` charged bytes back, waking waiting senders once the queue
    /// has drained to half the limit.
    pub(crate) fn release(&self, n: usize) {
        let before = self.queued.fetch_sub(n, std::sync::atomic::Ordering::AcqRel);
        let low = self.resume_below();
        if before >= low && before - n < low {
            self.room.notify_waiters();
        }
    }

    /// Bytes queued and not yet handled.
    pub(crate) fn queued(&self) -> usize {
        self.queued.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Below this a waiting sender resumes. Waiting for the queue to drain
    /// by half, not just to dip under the limit, means one wake-up per
    /// half a budget instead of one per message handled: resuming at the
    /// limit itself halved benchmark throughput.
    fn resume_below(&self) -> usize {
        self.limit / 2
    }

    /// Charge this sender `n` bytes, first waiting for the queue to drain
    /// to half its limit if it is at or over the limit. Checked before
    /// charging, so a single message bigger than the limit still goes
    /// through once the queue has drained. Several `Handler` clones can
    /// overshoot by one message each, which is fine.
    pub(crate) async fn reserve(self: &Arc<Self>, n: usize) -> Credit {
        self.charge(n).await;
        Credit(Some((self.clone(), n)))
    }

    /// `reserve` without the guard: the caller owes a `release(n)`.
    pub(crate) async fn charge(&self, n: usize) {
        if self.queued() >= self.limit {
            loop {
                let room = self.room.notified();
                tokio::pin!(room);
                // Registered before the re-check, so a release in between
                // can't be missed.
                room.as_mut().enable();
                if self.queued() < self.resume_below() || self.closed.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                room.await;
            }
        }
        self.queued.fetch_add(n, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Marks a `SocketEvent::Local` as charged to its hive's `Handler` budget,
/// which the event loop gives back in bulk after each batch: a guard per
/// message (an `Arc` clone and a separate atomic subtraction each, on cache
/// lines shared with the producer's thread) cost about 20% of local update
/// throughput. Only Hive can make one, so the budget always balances.
#[derive(Debug)]
pub struct Charged(pub(crate) ());

/// Bytes charged to an `Inbound` budget, given back when dropped. Travels
/// inside the `SocketEvent` it pays for, so they come back however the
/// event ends: handled, or dropped with the queue when the hive stops.
#[derive(Debug, Default)]
pub struct Credit(Option<(Arc<Inbound>, usize)>);

impl Credit {
    /// A credit that pays for nothing, for events built outside a budget.
    pub fn none() -> Credit {
        Credit(None)
    }
}

impl Drop for Credit {
    fn drop(&mut self) {
        if let Some((inbound, n)) = self.0.take() {
            inbound.release(n);
        }
    }
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
    /// Bytes waiting for this peer's writer task (TCP peers only). See
    /// `Outbox` and `write_loop`.
    out: Option<Arc<Outbox>>,
    /// Something was queued with `try_send` and the writer hasn't been
    /// woken for it yet, see `flush`.
    unflushed: AtomicBool,
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

impl Peer {
    pub fn is_bt_client(&self) -> bool {
        self.peripheral.is_some()
    }
    
    pub fn to_string(&self) -> String {
        format!("{:?},{:?}", self.get_name(), self.address)
    }
    
    /// True for a server this hive dialed, as opposed to a client that
    /// connected to it.
    pub(crate) fn is_upstream(&self) -> bool {
        self.peer_type == PeerType::TcpServer
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
                unflushed: AtomicBool::new(false),
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

            let out = Outbox::new();
            peer.out = Some(out.clone());
            tokio::spawn(write_loop(writer, out, sender.clone(), address.clone(), peer.shutdown.clone()));

            let name_id = peer.get_id_name();
            let shutdown = peer.shutdown.clone();
            debug!("start tcp socket read loop for({})", name_id);
            tokio::spawn(read_loop(sender, name_id, address, reader, max_message_size, shutdown));

            Ok(peer)
        } else {
            Ok(Peer {
                name: Arc::new(RwLock::new(name)),
                out: None,
                unflushed: AtomicBool::new(false),
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
                        // A full outbox means plenty is already headed
                        // out; the PONG (or that traffic's replies) will
                        // still settle ack_check.
                        match out.push(&bytes) {
                            Queued::Closed => Err("writer closed".into()),
                            _ => {
                                out.ready.notify_one();
                                Ok(())
                            }
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

    /// Queue `msg` without waiting or waking anything. `Ok(true)`: it is in
    /// the outbox, and `flush` (once per batch of sends) gets it written.
    /// `Ok(false)`: the outbox is full, or this transport has none; call
    /// `send`.
    pub(crate) fn try_send(&self, msg: &Bytes) -> Result<bool> {
        match &self.out {
            Some(out) => match out.push(msg) {
                Queued::Yes => {
                    self.unflushed.store(true, Relaxed);
                    Ok(true)
                }
                Queued::Full => Ok(false),
                Queued::Closed => Err(format!("{} writer closed", self.address).into()),
            },
            None => Ok(false),
        }
    }

    /// Wake the writer task if `try_send` queued anything since last time.
    pub(crate) fn flush(&self) {
        if self.unflushed.load(Relaxed) {
            self.unflushed.store(false, Relaxed);
            if let Some(out) = &self.out {
                out.ready.notify_one();
            }
        }
    }

    /// Queue `msg` for this peer and wake its writer. For TCP this never
    /// waits on the socket: the writer task does the I/O. If the outbox is
    /// full the call waits for the writer to take it (backpressure for a
    /// healthy peer that's behind). A peer that accepts nothing at all for
    /// `SLOW_PEER_TIMEOUT` while we wait has stopped reading: the call
    /// gives up with an error so the hive can drop it.
    pub async fn send(&self, msg: Bytes) -> Result<()> {
        if let Some(out) = &self.out {
            let mut written = out.written.load(Relaxed);
            loop {
                match out.push(&msg) {
                    Queued::Yes => break,
                    Queued::Closed => return Err(format!("{} writer closed", self.address).into()),
                    Queued::Full => {}
                }
                out.ready.notify_one();
                if tokio::time::timeout(SLOW_PEER_TIMEOUT, out.drained.notified()).await.is_err() {
                    // Slow is fine, stuck is not: keep waiting as long as
                    // the socket took something since we last looked.
                    let now = out.written.load(Relaxed);
                    if now == written {
                        return Err(format!(
                            "{} send queue full for {:?}, peer not reading",
                            self.address, SLOW_PEER_TIMEOUT
                        )
                        .into());
                    }
                    written = now;
                }
            }
            self.unflushed.store(false, Relaxed);
            out.ready.notify_one();
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

/// Most bytes that may wait in a peer's outbox before `Peer::send` starts
/// waiting for the writer. One frame may take it past this, so a message
/// larger than the limit still goes through.
const OUTBOX_LIMIT: usize = 256 * 1024;
/// How long `Peer::send` waits on a full outbox, with the socket accepting
/// nothing, before declaring the peer stuck. Bounds how long one
/// non-reading peer can stall the event loop.
const SLOW_PEER_TIMEOUT: Duration = Duration::from_secs(2);
/// The writer hands the socket this much at a time and counts each piece
/// as progress, so "accepting nothing" means under this much in
/// `SLOW_PEER_TIMEOUT` however large the backlog is.
const WRITE_CHUNK: usize = 64 * 1024;
/// An idle writer gives back any buffer that grew past this.
const WRITE_BUF_KEEP: usize = 64 * 1024;

/// Framed bytes waiting for one peer's writer task. The hive appends under
/// a mutex that is never held across an await or a syscall; the writer
/// swaps the whole buffer for an empty one and writes it with a single
/// `write_all`. So a burst of small updates costs one copy each into a
/// reused buffer and one syscall for the lot, with no per-message
/// allocation, channel hop or task wake-up.
pub(crate) struct Outbox {
    buf: Mutex<BytesMut>,
    /// Wakes the writer: there is something to write.
    ready: Notify,
    /// Wakes a sender waiting on a full outbox: the writer took the buffer.
    drained: Notify,
    /// Set when the writer exits; nothing queued after that goes anywhere.
    closed: AtomicBool,
    /// Pieces of `WRITE_CHUNK` the socket has accepted so far. A waiting
    /// sender uses it to tell a slow peer from one that stopped reading.
    written: AtomicU64,
}

enum Queued {
    Yes,
    Full,
    Closed,
}

impl Outbox {
    fn new() -> Arc<Outbox> {
        Arc::new(Outbox {
            buf: Mutex::new(BytesMut::new()),
            ready: Notify::new(),
            drained: Notify::new(),
            closed: AtomicBool::new(false),
            written: AtomicU64::new(0),
        })
    }

    /// Append `msg` as one length-prefixed frame, unless `OUTBOX_LIMIT`
    /// bytes are already waiting.
    fn push(&self, msg: &[u8]) -> Queued {
        if self.closed.load(Relaxed) {
            return Queued::Closed;
        }
        let mut buf = self.buf.lock().unwrap();
        if buf.len() >= OUTBOX_LIMIT {
            return Queued::Full;
        }
        buf.reserve(4 + msg.len());
        buf.put_u32(msg.len() as u32);
        buf.put_slice(msg);
        Queued::Yes
    }
}

/// Owns a peer's write half and drains its `Outbox`. Generic over the
/// writer so TLS can reuse it.
///
/// Ends when the peer is dropped (`shutdown`, also aborting a write stuck
/// on a dead link) or on a write error, which it reports as a Hangup.
/// Dropping the write half on exit sends FIN.
pub(crate) async fn write_loop<W>(
    mut writer: W,
    out: Arc<Outbox>,
    events: UnboundedSender<SocketEvent>,
    address: String,
    shutdown: CancellationToken,
) where
    W: AsyncWrite + Unpin,
{
    let mut buf = BytesMut::with_capacity(8 * 1024);
    let cancelled = shutdown.cancelled();
    tokio::pin!(cancelled);
    'run: loop {
        // Trade our empty buffer for whatever has been queued.
        loop {
            {
                let mut pending = out.buf.lock().unwrap();
                if !pending.is_empty() {
                    std::mem::swap(&mut *pending, &mut buf);
                    break;
                }
            }
            // Nothing to do: don't sit on a buffer one large burst grew.
            if buf.capacity() > WRITE_BUF_KEEP {
                buf = BytesMut::with_capacity(8 * 1024);
            }
            tokio::select! {
                biased;
                _ = &mut cancelled => break 'run,
                _ = out.ready.notified() => {}
            }
        }
        out.drained.notify_one();
        let res = tokio::select! {
            biased;
            _ = &mut cancelled => break,
            r = async {
                for piece in buf.chunks(WRITE_CHUNK) {
                    writer.write_all(piece).await?;
                    out.written.fetch_add(1, Relaxed);
                }
                // A no-op on a plain TCP half; for TLS it makes sure the
                // encrypted records are actually on the socket.
                writer.flush().await
            } => r,
        };
        if let Err(e) = res {
            debug!("write to {} failed: {:?}", address, e);
            let _ = events.unbounded_send(SocketEvent::Hangup { from: address.clone() });
            break;
        }
        buf.clear();
    }
    // Anyone still queueing, or waiting for room, finds out now.
    out.closed.store(true, Relaxed);
    out.drained.notify_one();
    trace!("writer for {} stopped", address);
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

/// How much `read_loop` asks for when it needs a fresh read buffer. Reads
/// go straight into it, and the frames handed to the hive are slices of it.
const READ_CHUNK: usize = 64 * 1024;
/// Get a new chunk once the current one has less room than this.
const MIN_READ_SPACE: usize = 8 * 1024;

/// Reads whatever the socket has into one buffer and hands every complete
/// frame in it to the hive as a single `SocketEvent::Frames`, zero-copy.
/// A frame split across reads (even inside its 4-byte length prefix) just
/// waits in the buffer for the rest.
async fn read_loop(
    sender: UnboundedSender<SocketEvent>,
    peer_id_string: String,
    from: String,
    reader: BufReader<BoxRead>,
    max_message_size: u32,
    shutdown: CancellationToken,
) {
    let from_shared: Arc<str> = Arc::from(from.as_str());
    let inbound = Inbound::new(INBOUND_LIMIT);
    let mut buf = BytesMut::with_capacity(READ_CHUNK);
    // The handshake read through a BufReader; keep anything it read ahead.
    buf.extend_from_slice(reader.buffer());
    let mut reader = reader.into_inner();
    let cancelled = shutdown.cancelled();
    tokio::pin!(cancelled);

    let ended: io::Error = loop {
        let (complete, rest) = complete_frames(&buf, max_message_size);
        if complete > 0 {
            let data = buf.split_to(complete).freeze();
            trace!("{:?} read {} bytes of frames", peer_id_string, data.len());
            // Over budget: stop reading until the hive catches up. The
            // socket's receive buffer fills and TCP slows the sender down.
            let credit = tokio::select! {
                biased;
                _ = &mut cancelled => {
                    debug!("{:?} read loop shutting down, peer dropped", peer_id_string);
                    return;
                }
                c = inbound.reserve(data.len()) => c,
            };
            // unbounded_send only fails once the hive has stopped
            // receiving, at which point there is nobody left to tell.
            if sender
                .unbounded_send(SocketEvent::Frames { from: from_shared.clone(), data, credit })
                .is_err()
            {
                return;
            }
        }
        // Whatever came before a bad frame has been handed over by now.
        let next_frame = match rest {
            Ok(size) => size,
            Err(e) => break e,
        };
        // Room for the rest of a frame we've seen the start of, and never
        // so little that a read turns into a string of tiny syscalls.
        let want = next_frame.saturating_sub(buf.len()).max(MIN_READ_SPACE);
        if buf.capacity() - buf.len() < want {
            buf.reserve(want.max(READ_CHUNK));
        }
        let read = tokio::select! {
            biased;
            _ = &mut cancelled => {
                debug!("{:?} read loop shutting down, peer dropped", peer_id_string);
                return;
            }
            r = reader.read_buf(&mut buf) => r,
        };
        match read {
            Ok(0) => break io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"),
            Ok(_) => {}
            Err(e) => break e,
        }
    };
    if ended.kind() == io::ErrorKind::UnexpectedEof {
        debug!("{:?} closed the connection", peer_id_string);
    } else {
        error!("{:?} read failed, dropping connection: {:?}", peer_id_string, ended);
    }
    let _ = sender.unbounded_send(SocketEvent::Hangup { from });
    debug!("<< Peer run done");
}

/// How many bytes at the front of `buf` are whole frames, and what follows
/// them: the full size (prefix included) of a partial frame, 0 if its
/// length prefix hasn't all arrived, or an error. The 4-byte prefix comes
/// straight off the wire, so a claimed size over `max_message_size` is
/// rejected here, before any buffer is grown to hold it.
pub(crate) fn complete_frames(buf: &[u8], max_message_size: u32) -> (usize, io::Result<usize>) {
    let mut at = 0;
    while let Some(prefix) = buf.get(at..at + 4) {
        let size = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
        if size > max_message_size {
            let e = format!("claimed message size {} exceeds {} byte cap", size, max_message_size);
            return (at, Err(io::Error::new(io::ErrorKind::InvalidData, e)));
        }
        let end = at + 4 + size as usize;
        if end > buf.len() {
            return (at, Ok(end - at));
        }
        at = end;
    }
    (at, Ok(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc::{unbounded, UnboundedReceiver};
    use futures::StreamExt;
    use tokio::io::DuplexStream;

    /// A `Peer` whose socket is one end of an in-memory pipe holding at
    /// most `pipe` bytes; the test drives the other end.
    async fn piped_peer(pipe: usize) -> (Peer, DuplexStream, UnboundedReceiver<SocketEvent>) {
        let (near, far) = tokio::io::duplex(pipe);
        let (r, w) = tokio::io::split(near);
        let conn = Connection {
            reader: BufReader::new(Box::new(r) as BoxRead),
            writer: Box::new(w),
            address: "pipe:1".to_string(),
            remote_name: None,
        };
        let (tx, rx) = unbounded();
        let peer = Peer::new(
            "remote".to_string(),
            Some(conn),
            None,
            None,
            tx,
            "pipe:1".to_string(),
            "local".to_string(),
            PeerType::TcpClient,
            DEFAULT_MAX_MESSAGE_SIZE,
        )
        .await
        .unwrap();
        (peer, far, rx)
    }

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    /// (bytes of whole frames, size of the partial frame after them).
    fn scan(buf: &[u8]) -> (usize, usize) {
        let (complete, rest) = complete_frames(buf, 1024);
        (complete, rest.unwrap())
    }

    #[test]
    fn complete_frames_finds_whole_frames_and_sizes_the_partial_one() {
        assert_eq!(scan(&[]), (0, 0));
        // Part of a length prefix: nothing known yet.
        assert_eq!(scan(&[0, 0]), (0, 0));
        // Prefix complete, body not: the frame will be 4 + 3 bytes.
        assert_eq!(scan(&[0, 0, 0, 3, b'a']), (0, 7));

        let mut buf = framed(b"abc");
        buf.extend(framed(b""));
        buf.extend(framed(b"de"));
        let whole = buf.len();
        assert_eq!(scan(&buf), (whole, 0));
        // Two bytes of the next prefix.
        buf.extend([0, 0]);
        assert_eq!(scan(&buf), (whole, 0));
        // The rest of that prefix (a 0x0100 = 256 byte frame) and one byte.
        buf.extend([1, 0, b'x']);
        assert_eq!(scan(&buf), (whole, 4 + 256));
    }

    #[test]
    fn complete_frames_rejects_an_oversized_claim_before_its_body_arrives() {
        assert_eq!(scan(&1024u32.to_be_bytes()), (0, 4 + 1024));
        let (complete, rest) = complete_frames(&1025u32.to_be_bytes(), 1024);
        assert_eq!(complete, 0);
        assert_eq!(rest.unwrap_err().kind(), io::ErrorKind::InvalidData);
        // Good frames ahead of it in the same read still count.
        let mut buf = framed(b"ok");
        buf.extend(u32::MAX.to_be_bytes());
        let (complete, rest) = complete_frames(&buf, 1024);
        assert_eq!(complete, 6);
        assert!(rest.is_err());
    }

    /// Every frame the read loop hands over, until it reports a hangup.
    async fn frames_until_hangup(events: &mut UnboundedReceiver<SocketEvent>) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        while let Some(event) = events.next().await {
            match event {
                SocketEvent::Frames { from, mut data, .. } => {
                    assert_eq!(&*from, "pipe:1");
                    while !data.is_empty() {
                        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                        let _ = data.split_to(4);
                        got.push(data.split_to(len).to_vec());
                    }
                }
                SocketEvent::Hangup { from } => {
                    assert_eq!(from, "pipe:1");
                    break;
                }
                other => panic!("unexpected event {:?}", other),
            }
        }
        got
    }

    #[tokio::test]
    async fn read_loop_reassembles_frames_however_the_bytes_arrive() {
        let (_peer, mut far, mut events) = piped_peer(32 * 1024).await;
        // Larger than a read chunk, so its buffer has to be grown for it.
        let big: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let expected: Vec<Vec<u8>> =
            vec![b"first".to_vec(), b"a".to_vec(), Vec::new(), b"ccc".to_vec(), big.clone(), b"last".to_vec()];

        let writer = tokio::spawn(async move {
            // One byte at a time: the length prefix itself is split.
            for b in framed(b"first") {
                far.write_all(&[b]).await.unwrap();
                tokio::task::yield_now().await;
            }
            // Several frames and the front of a large one in one write.
            let mut burst = framed(b"a");
            burst.extend(framed(b""));
            burst.extend(framed(b"ccc"));
            let big_frame = framed(&big);
            burst.extend(&big_frame[..70_000]);
            far.write_all(&burst).await.unwrap();
            // The rest of it with another frame straight behind.
            let mut rest = big_frame[70_000..].to_vec();
            rest.extend(framed(b"last"));
            far.write_all(&rest).await.unwrap();
            // A claimed size over the cap ends the connection.
            far.write_all(&(DEFAULT_MAX_MESSAGE_SIZE + 1).to_be_bytes()).await.unwrap();
            far
        });

        let got = frames_until_hangup(&mut events).await;
        assert_eq!(got.len(), expected.len());
        for (g, e) in got.iter().zip(&expected) {
            assert!(g == e, "frame of {} bytes differs from the {} sent", g.len(), e.len());
        }
        let _far = writer.await.unwrap();
    }

    #[tokio::test]
    async fn sends_reach_the_socket_framed_and_in_order() {
        let (peer, mut far, _events) = piped_peer(64 * 1024).await;
        // try_send only queues; nothing moves until the flush.
        assert!(peer.try_send(&Bytes::from_static(b"one")).unwrap());
        assert!(peer.try_send(&Bytes::from_static(b"")).unwrap());
        peer.flush();
        peer.send(Bytes::from_static(b"three")).await.unwrap();

        let mut expected = framed(b"one");
        expected.extend(framed(b""));
        expected.extend(framed(b"three"));
        let mut got = vec![0u8; expected.len()];
        far.read_exact(&mut got).await.unwrap();
        assert_eq!(got, expected);
    }

    /// Every queued update is one `SocketEvent`, so a rare variant must
    /// not make all of them big.
    #[cfg(not(feature = "bluetooth"))]
    #[test]
    fn socket_event_stays_small() {
        let size = std::mem::size_of::<SocketEvent>();
        eprintln!("size_of SocketEvent = {}", size);
        assert!(size <= 96, "SocketEvent grew to {} bytes", size);
    }

    const BIG: usize = 32 * 1024;
    const COUNT: usize = 40;

    /// A peer reading far too slowly to empty a full outbox within
    /// SLOW_PEER_TIMEOUT, but never idle that long, is backpressure, not a
    /// dead link: every send must wait it out.
    #[tokio::test(start_paused = true)]
    async fn a_slow_reader_is_waited_for_not_dropped() {
        let (peer, mut far, _events) = piped_peer(16 * 1024).await;
        let total = COUNT * (BIG + 4);
        // 16 KiB every 250 ms: a quarter of a second per step, about 20 s
        // (virtual) for the lot, and over 4 s for one full outbox.
        let reader = tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            let mut seen = 0;
            while seen < total {
                tokio::time::sleep(Duration::from_millis(250)).await;
                seen += far.read(&mut buf).await.unwrap();
            }
            far
        });

        let started = tokio::time::Instant::now();
        let msg = Bytes::from(vec![7u8; BIG]);
        for i in 0..COUNT {
            peer.send(msg.clone()).await.unwrap_or_else(|e| panic!("send {} failed: {}", i, e));
        }
        assert!(
            started.elapsed() > SLOW_PEER_TIMEOUT,
            "the sends never had to wait, so this didn't test backpressure"
        );
        tokio::time::timeout(Duration::from_secs(60), reader)
            .await
            .expect("the reader never got everything")
            .unwrap();
    }

    /// A peer that takes nothing at all is reported after SLOW_PEER_TIMEOUT.
    #[tokio::test(start_paused = true)]
    async fn a_reader_that_stops_is_reported_stuck() {
        let (peer, _far, _events) = piped_peer(16 * 1024).await;
        let started = tokio::time::Instant::now();
        let msg = Bytes::from(vec![7u8; BIG]);
        let mut failed = None;
        for _ in 0..COUNT {
            if let Err(e) = peer.send(msg.clone()).await {
                failed = Some(e.to_string());
                break;
            }
        }
        let failed = failed.expect("sends to a peer that never reads kept succeeding");
        assert!(failed.contains("peer not reading"), "{}", failed);
        let waited = started.elapsed();
        assert!(
            waited >= SLOW_PEER_TIMEOUT && waited < SLOW_PEER_TIMEOUT * 2,
            "gave up after {:?}",
            waited
        );
    }
}
