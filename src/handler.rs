// use std::sync::mpmc::SendError;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc::{SendError, UnboundedSender};
use crate::futures::SinkExt;
#[allow(unused_imports)]
use crate::logging::{debug, error, info};
use crate::hive::{PEER_MESSAGE, PEER_MESSAGE_DIV, PEER_REQUESTS};
use crate::peer::{Charged, Inbound, SocketEvent};
use crate::property::{encode_property, Property, property_to_bytes, PropertyValue, PropertyType};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Per-property-name debounce state for `Handler::set_property_debounced`.
/// Mirrors `Property`'s own `backoff_active`/`backoff_pending` fields
/// (property.rs), same leading+trailing pattern, applied to the network
/// send instead of a local `on_next` callback.
pub(crate) struct DebounceEntry {
    active: AtomicBool,
    pending: Mutex<Option<PropertyValue>>,
}

pub(crate) type DebounceMap = Arc<Mutex<HashMap<String, Arc<DebounceEntry>>>>;

#[derive(Clone)]
pub struct Handler {
    pub (crate) sender: UnboundedSender<SocketEvent>,
    pub from_name: String,
    pub (crate) connected: Arc<AtomicBool>,
    pub (crate) peer_count: Arc<AtomicUsize>,
    pub (crate) debounce: DebounceMap,
    /// Shared by every clone: what they have queued for the event loop
    /// and it hasn't handled yet, see `peer::INBOUND_LIMIT`.
    pub (crate) inbound: Arc<Inbound>,
}

impl Handler {

    /// True once we've connected to (or been connected to by) at least one
    /// peer and that peer hasn't since disconnected. A `wave_timeout_ms` in
    /// the Hive config is what makes this notice a silently dead link, not
    /// just a clean TCP close, see `Peer::wave`.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// How many peers are attached right now. Unlike `is_connected`, a
    /// server that has bound its listen port but has nobody connected
    /// reports 0 here, so this is what to check for "is there actually
    /// someone on the other end". Same `wave_timeout_ms` caveat applies for
    /// noticing a silently dead link rather than a clean close.
    pub fn peer_count(&self) -> usize {
        self.peer_count.load(Ordering::Relaxed)
    }

    pub async fn send_property_value(&mut self, prop_name: &str, prop_value: Option<&PropertyType>) {
        let p = Property::from_toml(prop_name, prop_value);
        self.send_property(p).await;
    }

    pub fn set_str(&self, id:&u64, value:&str){
        let _prop = Property::from_value(id, value.into());
    }

    // This is basically a set_property method, however, Hive itself does not deal directly with property values
    // It only propagates value change notifications, hence the name
    ///
    /// `None` clears the property (same as `delete_property`).
    pub async fn set_property(&mut self, prop_name:&str, prop_value:Option<&PropertyValue>){
        self.set_property_by_id(Property::hash_id(prop_name), prop_value).await;
    }

    /// `set_property` for a property known only by its id, as one that
    /// arrived from a peer is (see `HiveEvent::PropertyChanged`).
    pub async fn set_property_by_id(&mut self, id: u64, prop_value: Option<&PropertyValue>) {
        // Encode straight from the borrowed value: no clone of it and no
        // throwaway Property (watch channel, Arcs) per call.
        let msg = encode_property(id, prop_value.map(|v| &v.val), true);
        self.send_message(msg).await;
    }

    /// Ask the servers this hive is connected to, now or later, for their
    /// peer list. Each answers at once and again whenever the list changes;
    /// the lists arrive as `HiveEvent::Peers` (see `Hive::on_event`).
    pub async fn request_peers(&mut self) {
        self.send_message(bytes::Bytes::from_static(&[PEER_REQUESTS])).await;
    }

    /// Hand an encoded message to the hive's event loop. Returns at once
    /// unless this hive's handlers already have `peer::INBOUND_LIMIT`
    /// bytes queued that the event loop hasn't got to, in which case it
    /// waits for room, so a producer can't outrun the hive without bound.
    /// Only fails once the hive has stopped, which isn't worth panicking a
    /// caller over.
    async fn send_message(&self, msg: bytes::Bytes) {
        let n = msg.len();
        self.inbound.charge(n).await;
        let ev = SocketEvent::Local { msg, charged: Charged(()) };
        if self.sender.unbounded_send(ev).is_err() {
            self.inbound.release(n);
            debug!("hive {} is stopped, dropping update", self.from_name);
        }
    }

    /// Like `set_property`, but rate-limited per property name: the first
    /// call in a burst sends immediately, further calls within
    /// `debounce_ms` are coalesced and only the latest value goes out once
    /// the window closes. `set_property` itself never consults
    /// `Property::backoff`, it just serializes a throwaway `Property` for
    /// the wire, so that field only throttles a receiver's local `on_next`
    /// callback, never the sender's outgoing traffic. This is the
    /// equivalent for the send side: for something like a UI slider firing
    /// many updates a second, only the value it settles on (plus immediate
    /// feedback on the first tick) is usually worth putting on the wire.
    pub async fn set_property_debounced(&mut self, prop_name: &str, prop_value: PropertyValue, debounce_ms: u64) {
        if debounce_ms == 0 {
            self.set_property(prop_name, Some(&prop_value)).await;
            return;
        }

        let entry = {
            let mut map = self.debounce.lock().unwrap();
            map.entry(prop_name.to_string())
                .or_insert_with(|| {
                    Arc::new(DebounceEntry {
                        active: AtomicBool::new(false),
                        pending: Mutex::new(None),
                    })
                })
                .clone()
        };

        if !entry.active.swap(true, Ordering::SeqCst) {
            self.set_property(prop_name, Some(&prop_value)).await;

            let mut handler = self.clone();
            let name = prop_name.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(debounce_ms)).await;
                entry.active.store(false, Ordering::SeqCst);
                let pending = entry.pending.lock().unwrap().take();
                if let Some(v) = pending {
                    handler.set_property(&name, Some(&v)).await;
                }
            });
        } else {
            *entry.pending.lock().unwrap() = Some(prop_value);
        }
    }

    pub async fn send_property(&mut self, property:Property){

        self.send_message(property_to_bytes(&property, true)).await;

    }

    pub async fn delete_property(&mut self, name: &str){
        self.send_message(encode_property(Property::hash_id(name), None, true)).await;
    }

    pub async fn send_to_peer(&mut self, peer_name:&str, msg:&str) -> Result<(), SendError>{
        let mut bytes = BytesMut::with_capacity(peer_name.len()+msg.len()+2);
        bytes.put_u8(PEER_MESSAGE);
        bytes.put_slice(peer_name.as_bytes());
        bytes.put_u8(PEER_MESSAGE_DIV.as_bytes()[0]);
        bytes.put_slice(msg.as_bytes());
        debug!(".... send to peer: {:?}", bytes);
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: bytes.freeze()
        };
        self.sender.send(socket_event).await //.expect("failed to send to peer");

    }

    pub async fn hangup(&mut self) {
        self.sender.send(SocketEvent::Hangup{from:String::from("")}).await
            .expect("failed to hangup");
    }

}

