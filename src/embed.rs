//! A `Hive` for applications that aren't written in async Rust: a host
//! reached through FFI (see `jni_api` for the JVM and Android), or plain
//! threaded Rust.
//!
//! `EmbeddedHive` owns its own tokio runtime, so the caller needs none. Its
//! methods are ordinary blocking calls that can be made from any thread, and
//! everything the hive has to report arrives as an `Event` on one dedicated
//! thread, in order.
//!
//! ```no_run
//! use hive::embed::{EmbeddedHive, Event};
//!
//! let hive = EmbeddedHive::start(
//!     "name = \"Phone\"\nconnect = \"192.168.1.10:3000\"",
//!     |event| match event {
//!         Event::Property { name, value, .. } => println!("{:?} = {:?}", name, value),
//!         other => println!("{:?}", other),
//!     },
//! )
//! .unwrap();
//! hive.request_peers();
//! hive.set_property("lightValue", 42);
//! hive.stop();
//! ```

use crate::handler::Handler;
use crate::hive::{Hive, HiveEvent};
use crate::property::{Property, PropertyValue};
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{JoinHandle, ThreadId};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub use crate::hive::PeerInfo;
pub use toml::Value;

/// How long `stop` gives the hive's tasks to wind down.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Something the hive has to tell the application.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A property has a new value, or with `value: None` was deleted.
    ///
    /// Only ids travel between hives (`property_id` of the name), so `name`
    /// is `None` for a property this hive has never been told the name of.
    /// Names come from the `[Properties]` table of the config passed to
    /// `start` and from `set_property` / `register_name`.
    Property { id: u64, name: Option<String>, value: Option<Value> },
    /// The peer list of the server this hive is connected to. Sent after
    /// `request_peers` and again whenever it changes; the first entry is the
    /// server itself.
    Peers(Vec<PeerInfo>),
    /// A message another hive addressed to this one, see `send_to_peer`.
    PeerMessage(String),
    /// A peer attached or went away. `peers` is how many are attached now.
    Connection { connected: bool, peers: usize },
}

/// The id a property name has on the wire.
pub fn property_id(name: &str) -> u64 {
    Property::hash_id(name)
}

/// A running hive, see the module docs. Stops when dropped.
pub struct EmbeddedHive {
    handler: Mutex<Handler>,
    names: Arc<Mutex<HashMap<u64, String>>>,
    cancel: CancellationToken,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    events: Mutex<Option<JoinHandle<()>>>,
    events_thread: ThreadId,
}

impl EmbeddedHive {
    /// Build a hive from `config` (the same TOML `Hive::new_from_str` takes)
    /// and start it. A client keeps trying to reach its server in the
    /// background, so this returns before any connection exists; watch for
    /// `Event::Connection`.
    ///
    /// `on_event` is called on a thread of its own, one event at a time and
    /// in the order things happened, starting with one `Event::Property`
    /// for each property the config declares. It may call back into this
    /// hive.
    ///
    /// A config that `Hive::new_from_str` would panic over is returned as
    /// an `Err` with the same message.
    pub fn start<F>(config: &str, mut on_event: F) -> Result<EmbeddedHive, String>
    where
        F: FnMut(Event) + Send + 'static,
    {
        let mut hive = std::panic::catch_unwind(|| Hive::new_from_str_unknown(config)).map_err(panic_message)?;

        let (tx, rx) = mpsc::channel::<Event>();
        let mut names = HashMap::new();
        for property in hive.properties.values() {
            let name = property.get_name();
            if !name.is_empty() {
                names.insert(property.id, name.to_string());
            }
            if let Some(value) = property.get_value() {
                let _ = tx.send(Event::Property { id: property.id, name: None, value: Some(value) });
            }
        }
        let names = Arc::new(Mutex::new(names));

        // Sends only fail once the events thread is gone, when nobody is
        // left to tell.
        let events_tx = tx.clone();
        hive.on_event(move |event| {
            let _ = events_tx.send(match event {
                HiveEvent::PropertyChanged { id, value } => Event::Property { id, name: None, value: Some(value) },
                HiveEvent::PropertyDeleted { id } => Event::Property { id, name: None, value: None },
                HiveEvent::Peers(peers) => Event::Peers(peers),
                HiveEvent::Connection { connected, peers } => Event::Connection { connected, peers },
            });
        });
        hive.message_received.connect(move |message| {
            let _ = tx.send(Event::PeerMessage(message));
        });

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("hive")
            .enable_all()
            .build()
            .map_err(|e| format!("starting the hive runtime: {}", e))?;
        let cancel = CancellationToken::new();
        let handler = runtime
            .block_on(hive.try_go(false, cancel.clone()))
            .map_err(|e| e.to_string())?;

        // The hive holds every sender, so this thread ends once the hive
        // has been dropped.
        let known = names.clone();
        let events = std::thread::Builder::new()
            .name("hive-events".into())
            .spawn(move || {
                for mut event in rx {
                    if let Event::Property { id, name, .. } = &mut event {
                        *name = known.lock().unwrap().get(id).cloned();
                    }
                    on_event(event);
                }
            })
            .map_err(|e| format!("starting the hive events thread: {}", e))?;

        Ok(EmbeddedHive {
            handler: Mutex::new(handler),
            names,
            cancel,
            runtime: Mutex::new(Some(runtime)),
            events_thread: events.thread().id(),
            events: Mutex::new(Some(events)),
        })
    }

    /// Set a property here and on every connected hive, creating it if it
    /// doesn't exist. The change comes back as an `Event::Property`, like
    /// anyone else's.
    ///
    /// Returns at once, except that a caller producing updates faster than
    /// the hive can send them is made to wait (see the README on
    /// backpressure).
    pub fn set_property(&self, name: &str, value: impl Into<PropertyValue>) {
        let id = self.register_name(name);
        self.set_property_by_id(id, value);
    }

    /// `set_property` for a property known only by the id it arrived with.
    pub fn set_property_by_id(&self, id: u64, value: impl Into<PropertyValue>) {
        let value = value.into();
        let mut handler = self.handler.lock().unwrap();
        futures::executor::block_on(handler.set_property_by_id(id, Some(&value)));
    }

    /// Delete a property here and on every connected hive.
    pub fn delete_property(&self, name: &str) {
        self.delete_property_by_id(property_id(name));
    }

    pub fn delete_property_by_id(&self, id: u64) {
        let mut handler = self.handler.lock().unwrap();
        futures::executor::block_on(handler.set_property_by_id(id, None));
    }

    /// Tell this hive what a property is called, so its events carry the
    /// name. Returns the property's id.
    pub fn register_name(&self, name: &str) -> u64 {
        let id = property_id(name);
        let mut names = self.names.lock().unwrap();
        if !names.contains_key(&id) {
            names.insert(id, name.to_string());
        }
        id
    }

    /// Send `message` to the hive called `peer`: the server this hive is
    /// connected to, or any other client of it, which the server routes to.
    /// Delivery isn't confirmed; a name nobody has is dropped by whichever
    /// hive finds that out. `Err` means this hive has stopped.
    pub fn send_to_peer(&self, peer: &str, message: &str) -> Result<(), String> {
        let mut handler = self.handler.lock().unwrap();
        futures::executor::block_on(handler.send_to_peer(peer, message)).map_err(|e| e.to_string())
    }

    /// Ask for `Event::Peers`: the current list, then every change to it.
    /// Can be called before the connection is up.
    pub fn request_peers(&self) {
        let mut handler = self.handler.lock().unwrap();
        futures::executor::block_on(handler.request_peers());
    }

    /// See `Handler::is_connected`.
    pub fn is_connected(&self) -> bool {
        self.handler.lock().unwrap().is_connected()
    }

    /// See `Handler::peer_count`.
    pub fn peer_count(&self) -> usize {
        self.handler.lock().unwrap().peer_count()
    }

    /// Disconnect and shut the hive down. Events already queued are still
    /// delivered before this returns, unless it is called from the event
    /// callback itself. Calling it again does nothing, and the other
    /// methods become no-ops.
    pub fn stop(&self) {
        self.cancel.cancel();
        if let Some(runtime) = self.runtime.lock().unwrap().take() {
            runtime.shutdown_timeout(STOP_TIMEOUT);
        }
        let events = self.events.lock().unwrap().take();
        if let Some(events) = events {
            if std::thread::current().id() != self.events_thread {
                let _ = events.join();
            }
        }
    }
}

impl Drop for EmbeddedHive {
    fn drop(&mut self) {
        self.stop();
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "invalid hive config".to_string()
    }
}
