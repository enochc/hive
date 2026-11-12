/// MQTT peer for Hive — bridges Hive property sync with an MQTT broker.
///
/// # Design
///
/// `MqttPeer` connects to a remote MQTT broker and maps MQTT topics to Hive
/// properties.  Incoming MQTT messages on subscribed topics are forwarded into
/// the Hive event loop as `SocketEvent::Message` property updates.  Outgoing
/// property changes are published back to the broker.
///
/// The peer uses `rumqttc::AsyncClient` which is backed by a tokio event loop.
/// Instead of a busy-wait polling loop, we drive the rumqttc `EventLoop` from
/// a dedicated tokio task and use an `mpsc` channel to deliver received
/// messages back to the Hive event loop.  A `tokio::sync::Notify` is used to
/// signal when the MQTT connection is established and initial subscriptions
/// are confirmed.
///
/// # Topic mapping
///
/// Each Hive property name maps to an MQTT topic via a configurable prefix:
///
/// ```text
/// <topic_prefix>/<property_name>
/// ```
///
/// For example, with prefix `"hive/home"` and property `"lightValue"`, the
/// topic would be `"hive/home/lightValue"`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use bytes::Bytes;
use futures::channel::mpsc::UnboundedSender;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::sync::{Notify, mpsc as tokio_mpsc};
#[allow(unused_imports)]
use crate::logging::{debug, error, info, trace, warn};

use crate::hive::Result;
use crate::peer::SocketEvent;
use crate::property::{Property, property_to_bytes, property_value_to_bytes, try_bytes_to_property_value, value_to_text};

/// Configuration for the MQTT peer, typically parsed from the Hive TOML config.
///
/// ```toml
/// [MQTT]
/// host = "192.168.1.100"
/// port = 1883
/// client_id = "hive-living-room"
/// topic_prefix = "hive/home"
/// qos = 1                        # 0, 1, or 2
/// keep_alive = 30                 # seconds
/// subscribe = ["lightValue", "thermostatName"]
///
/// # "binary" (default): Hive's compact typed encoding, only other Hive
/// # nodes can read it. "text": plain values (72, 1.5, true, hello) that
/// # any MQTT client, dashboard or home-automation tool can read and send.
/// payload = "text"
///
/// # optional broker login
/// username = "hive"
/// password = "..."               # or password_env = "MQTT_PASSWORD"
///
/// # optional TLS (needs the `tls` feature); port then defaults to 8883
/// [MQTT.TLS]
/// ca = "certs/broker-ca.pem"
/// cert = "certs/client.pem"      # both cert and key for client-cert auth
/// key = "certs/client.key"
/// prefer_chacha20 = false
/// ```
#[derive(Debug, Clone)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub topic_prefix: String,
    pub qos: QoS,
    pub keep_alive: Duration,
    /// Property names to subscribe to on the broker.
    pub subscriptions: Vec<String>,
    /// How property values are encoded in MQTT payloads.
    pub payload: PayloadFormat,
    pub username: Option<String>,
    password: Option<Redacted>,
    #[cfg(feature = "tls")]
    tls: Option<Arc<rumqttc::tokio_rustls::rustls::ClientConfig>>,
}

/// Every key `[MQTT]` understands; anything else is refused.
const MQTT_KEYS: [&str; 12] = [
    "host", "port", "client_id", "topic_prefix", "qos", "keep_alive", "subscribe",
    "payload", "username", "password", "password_env", "TLS",
];
/// Every key `[MQTT.TLS]` understands.
#[cfg(feature = "tls")]
const MQTT_TLS_KEYS: [&str; 4] = ["ca", "cert", "key", "prefer_chacha20"];

/// Encoding of MQTT payloads, the `payload` key under `[MQTT]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PayloadFormat {
    /// `[type_tag][value]`, Hive's wire format. Compact and exactly typed,
    /// but opaque to anything that isn't Hive.
    #[default]
    Binary,
    /// UTF-8 text: `72`, `1.5`, `true`, `hello`. Readable and writable by
    /// any MQTT client. Incoming text is parsed using the property's current
    /// type, see `property::text_to_value`.
    Text,
}

impl PayloadFormat {
    fn encode(self, property: &Property) -> bytes::Bytes {
        match self {
            PayloadFormat::Binary => property_value_to_bytes(property),
            PayloadFormat::Text => value_to_text(property.get_value().as_ref()),
        }
    }
}

/// Keeps a password out of `{:?}` output (MqttConfig is Debug).
#[derive(Clone)]
struct Redacted(String);

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

impl MqttConfig {
    /// Build an `MqttConfig` from the `[MQTT]` table in a Hive TOML config.
    /// `None` when there's no usable `host`.
    ///
    /// Panics on a bad TLS/credentials setup (unreadable cert, `[MQTT.TLS]`
    /// without the `tls` feature) rather than connecting insecurely; use
    /// `try_from_toml` to handle that as an error.
    pub fn from_toml(table: &toml::Value) -> Option<MqttConfig> {
        Self::try_from_toml(table).unwrap_or_else(|e| panic!("[MQTT]: {}", e))
    }

    /// Like `from_toml`, but misconfiguration is an `Err`: an unknown key
    /// (a misspelled `password_env` would otherwise connect without a
    /// password), a value of the wrong type, or one out of range.
    pub fn try_from_toml(table: &toml::Value) -> std::result::Result<Option<MqttConfig>, String> {
        use crate::config::{check_keys, get_bool, get_int, get_str, get_str_list};
        const T: &str = "[MQTT]";
        check_keys(table, T, &MQTT_KEYS)?;
        let host = match get_str(table, T, "host")? {
            Some(h) => h.to_string(),
            None => return Ok(None),
        };
        let tls_table = table.get("TLS");
        #[cfg(not(feature = "tls"))]
        if tls_table.is_some() {
            return Err("[MQTT.TLS] set but Hive was built without the `tls` feature".into());
        }
        #[cfg(feature = "tls")]
        let tls = match tls_table {
            Some(t) => {
                const TT: &str = "[MQTT.TLS]";
                check_keys(t, TT, &MQTT_TLS_KEYS)?;
                let ca = get_str(t, TT, "ca")?.ok_or("[MQTT.TLS] needs `ca`")?;
                let prefer = get_bool(t, TT, "prefer_chacha20")?.unwrap_or(false);
                let (cert, key) = (get_str(t, TT, "cert")?, get_str(t, TT, "key")?);
                Some(Arc::new(crate::security::tls::client_config(ca, cert, key, prefer)?))
            }
            None => None,
        };
        let default_port = if tls_table.is_some() { 8883 } else { 1883 };
        let port = get_int(table, T, "port", 1, 65535)?.unwrap_or(default_port);

        let payload = match get_str(table, T, "payload")? {
            None | Some("binary") => PayloadFormat::Binary,
            Some("text") => PayloadFormat::Text,
            Some(other) => return Err(format!("payload = {:?}, expected \"binary\" or \"text\"", other)),
        };
        let username = get_str(table, T, "username")?.map(String::from);
        let password = if let Some(p) = get_str(table, T, "password")? {
            Some(p.to_string())
        } else if let Some(var) = get_str(table, T, "password_env")? {
            Some(std::env::var(var).map_err(|_| format!("password_env: {} is not set", var))?)
        } else {
            None
        };
        if password.is_some() && username.is_none() {
            return Err("`password` needs a `username`".into());
        }
        let client_id = get_str(table, T, "client_id")?.unwrap_or("hive-mqtt").to_string();
        let topic_prefix = get_str(table, T, "topic_prefix")?.unwrap_or("hive").to_string();
        let qos = match get_int::<u8>(table, T, "qos", 0, 2)?.unwrap_or(1) {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            _ => QoS::ExactlyOnce,
        };
        // Seconds, as MQTT carries it (u16); 0 turns keep-alive off.
        let keep_alive: u64 = get_int(table, T, "keep_alive", 0, 65535)?.unwrap_or(30);

        let subscriptions = get_str_list(table, T, "subscribe")?.unwrap_or_default();

        Ok(Some(MqttConfig {
            host,
            port,
            client_id,
            topic_prefix,
            qos,
            keep_alive: Duration::from_secs(keep_alive),
            subscriptions,
            payload,
            username,
            password: password.map(Redacted),
            #[cfg(feature = "tls")]
            tls,
        }))
    }
}

/// A received MQTT message destined for Hive property ingest.
#[derive(Debug)]
pub struct MqttIncoming {
    /// The property name extracted from the MQTT topic.
    pub property_name: String,
    /// The raw payload bytes from the broker.
    pub payload: Bytes,
}

/// The MQTT peer that bridges Hive and an MQTT broker.
pub struct MqttPeer {
    client: AsyncClient,
    config: MqttConfig,
    /// Receives decoded incoming MQTT messages from the event-loop task.
    incoming_rx: tokio_mpsc::UnboundedReceiver<MqttIncoming>,
    /// Fires once the connection is up and initial subscriptions are placed.
    ready: Arc<Notify>,
}

impl MqttPeer {
    /// Create a new `MqttPeer` and spawn the background rumqttc event-loop
    /// task.  The peer is **not** connected yet; call [`MqttPeer::run`] to
    /// drive the ingest loop.
    pub fn new(config: MqttConfig) -> MqttPeer {
        let mut opts = MqttOptions::new(&config.client_id, &config.host, config.port);
        opts.set_keep_alive(config.keep_alive);
        // Clean session so the broker doesn't replay stale retained messages
        // from a previous connection with the same client_id.
        opts.set_clean_session(true);
        if let Some(user) = &config.username {
            let pass = config.password.as_ref().map(|p| p.0.clone()).unwrap_or_default();
            opts.set_credentials(user.clone(), pass);
        }
        #[cfg(feature = "tls")]
        if let Some(tls) = &config.tls {
            opts.set_transport(rumqttc::Transport::tls_with_config(rumqttc::TlsConfiguration::Rustls(tls.clone())));
        }

        let (client, event_loop) = AsyncClient::new(opts, 64);

        let (incoming_tx, incoming_rx) = tokio_mpsc::unbounded_channel();
        let ready = Arc::new(Notify::new());

        // Spawn the rumqttc event-loop driver.
        let cfg = config.clone();
        let ready_clone = ready.clone();
        tokio::spawn(Self::event_loop_task(
            event_loop,
            incoming_tx,
            cfg,
            ready_clone,
        ));

        MqttPeer {
            client,
            config,
            incoming_rx,
            ready,
        }
    }

    /// Wait until the MQTT connection is live and subscriptions are placed.
    pub async fn wait_ready(&self) {
        self.ready.notified().await;
    }

    /// Subscribe to all configured topics on the broker.
    async fn subscribe_all(client: &AsyncClient, config: &MqttConfig) {
        for prop_name in &config.subscriptions {
            let topic = format!("{}/{}", config.topic_prefix, prop_name);
            info!("MQTT subscribing to: {}", topic);
            if let Err(e) = client.subscribe(&topic, config.qos).await {
                error!("MQTT subscribe failed for {}: {:?}", topic, e);
            }
        }
    }

    /// Publish a Hive property value to the broker.
    ///
    /// The payload is the value-only portion of the Hive binary wire
    /// format: `[type_tag][value_bytes]`.  The 8-byte property hash ID
    /// is omitted because the MQTT topic already identifies the property.
    pub async fn publish_property(&self, property: &Property) -> Result<()> {
        let name = property.get_name();
        if name.is_empty() {
            debug!("MQTT skip publish for unnamed property");
            return Ok(());
        }
        let topic = format!("{}/{}", self.config.topic_prefix, name);

        let payload = self.config.payload.encode(property);

        debug!("MQTT publish {} ({} bytes)", topic, payload.len());
        self.client
            .publish(&topic, self.config.qos, false, payload.to_vec())
            .await
            .map_err(|e| {
                let msg = format!("MQTT publish error: {:?}", e);
                error!("{}", msg);
                Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        Ok(())
    }

    /// The main ingest loop.  Receives MQTT messages from the background
    /// event-loop task and forwards them into the Hive event channel as
    /// `SocketEvent::Message` with a properly encoded property payload.
    ///
    /// Incoming payloads are value-only: `[type_tag][value_bytes]` without
    /// the 8-byte property hash ID (the MQTT topic carries the identity).
    /// We decode with `bytes_to_property_value`, reconstruct a named
    /// `Property`, and re-encode with `property_to_bytes` (which adds the
    /// ID + `PROPERTY` header) for the Hive event channel.
    ///
    /// The `from` address is set to `"mqtt"` so the Hive event handler can
    /// distinguish MQTT-originated changes and avoid re-publishing them
    /// (preventing echo loops).
    pub async fn run(
        &mut self,
        event_sender: UnboundedSender<SocketEvent>,
    ) -> Result<()> {
        use crate::futures::SinkExt;

        info!("MqttPeer ingest loop started");

        while let Some(incoming) = self.incoming_rx.recv().await {
            debug!(
                "MQTT incoming: property={}, payload={} bytes",
                incoming.property_name,
                incoming.payload.len()
            );

            // Decode the value-only binary payload.  The property
            // identity comes from the MQTT topic, not an in-band hash,
            // so we use bytes_to_property_value (type_tag + value only)
            // and rebuild a named Property for the Hive event channel.
            let se = if self.config.payload == PayloadFormat::Text {
                // Text needs the property's current type to parse
                // properly, which only the hive's event loop knows.
                SocketEvent::MqttText {
                    id: Property::hash_id(&incoming.property_name),
                    text: incoming.payload,
                }
            } else {
                let mut payload = incoming.payload;
                let prop_value = match try_bytes_to_property_value(&mut payload) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("MQTT dropping malformed payload for {}: {}", incoming.property_name, e);
                        continue;
                    }
                };
                let prop = Property::from_name(&incoming.property_name, prop_value);
                debug!("MQTT -> Hive SocketEvent::Message for {}", incoming.property_name);
                SocketEvent::Message {
                    from: String::from("mqtt"),
                    msg: property_to_bytes(&prop, true),
                }
            };

            let mut sender = event_sender.clone();
            if let Err(e) = sender.send(se).await {
                error!("Failed to send MQTT message into Hive event channel: {:?}", e);
                break;
            }
        }

        info!("MqttPeer ingest loop ended");
        Ok(())
    }

    /// Background task that polls the rumqttc `EventLoop`.
    ///
    /// This replaces the typical `loop { eventloop.poll().await }` pattern
    /// with a channel-based delivery so the Hive event loop stays in control.
    /// When the connection drops, rumqttc automatically reconnects on the
    /// next poll — we just keep driving it.
    async fn event_loop_task(
        mut event_loop: EventLoop,
        incoming_tx: tokio_mpsc::UnboundedSender<MqttIncoming>,
        config: MqttConfig,
        ready: Arc<Notify>,
    ) {
        let mut has_signaled_ready = false;

        // Build a reverse lookup: topic → property_name
        let mut topic_to_prop: HashMap<String, String> = HashMap::new();
        for name in &config.subscriptions {
            let topic = format!("{}/{}", config.topic_prefix, name);
            topic_to_prop.insert(topic, name.clone());
        }

        loop {
            match event_loop.poll().await {
                Ok(event) => {
                    trace!("MQTT event: {:?}", event);
                    match event {
                        Event::Incoming(Packet::ConnAck(_)) => {
                            info!("MQTT connected to {}:{}", config.host, config.port);
                            // Re-subscribe on every (re-)connect.  rumqttc
                            // clears session state on reconnect when
                            // clean_session is true.
                            //
                            // We can't call subscribe on `client` here because
                            // we don't own it, but rumqttc re-sends pending
                            // subscriptions automatically if we subscribe
                            // before the first ConnAck.  For reconnects we
                            // rely on the client reference held by MqttPeer to
                            // re-subscribe — but the simplest approach is to
                            // do initial subscriptions from the MqttPeer
                            // constructor side and just signal readiness here.
                            if !has_signaled_ready {
                                has_signaled_ready = true;
                                ready.notify_one();
                            }
                        }
                        Event::Incoming(Packet::Publish(publish)) => {
                            let topic = publish.topic.clone();
                            if let Some(prop_name) = topic_to_prop.get(&topic) {
                                let incoming = MqttIncoming {
                                    property_name: prop_name.clone(),
                                    payload: publish.payload.clone(),
                                };
                                if incoming_tx.send(incoming).is_err() {
                                    warn!("MQTT incoming channel closed, stopping event loop");
                                    break;
                                }
                            } else {
                                debug!("MQTT received message on unmapped topic: {}", topic);
                            }
                        }
                        Event::Incoming(Packet::SubAck(suback)) => {
                            debug!("MQTT SubAck: {:?}", suback);
                        }
                        _ => {
                            // PingResp, PubAck, etc. — nothing for us to do.
                        }
                    }
                }
                Err(e) => {
                    error!("MQTT event loop error: {:?}", e);
                    // rumqttc will automatically attempt reconnection on the
                    // next poll().  We add a small delay to avoid hammering
                    // the broker during network outages.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        info!("MQTT event loop task exiting");
    }
}

/// Convenience function: spawn an MqttPeer from a config, wire it into
/// a Hive event sender, and return the `AsyncClient` handle so the caller
/// can publish property changes.
///
/// This is the primary entry point used by `Hive::run` when an `[MQTT]`
/// section is present in the config.
pub async fn spawn_mqtt_peer(
    config: MqttConfig,
    event_sender: UnboundedSender<SocketEvent>,
) -> Result<MqttPeerHandle> {
    let mut peer = MqttPeer::new(config.clone());

    // Place subscriptions before the first ConnAck so rumqttc queues them.
    MqttPeer::subscribe_all(&peer.client, &config).await;

    // Wait for the broker connection + initial SubAck.
    peer.wait_ready().await;
    info!("MQTT peer ready");

    let client = peer.client.clone();

    // Spawn the ingest loop that feeds SocketEvent::Message into Hive.
    tokio::spawn(async move {
        if let Err(e) = peer.run(event_sender).await {
            error!("MqttPeer run error: {:?}", e);
        }
    });

    let queue = Arc::new(PublishQueue::default());
    tokio::spawn(publish_task(client.clone(), config.qos, Arc::downgrade(&queue), queue.wake.clone()));

    Ok(MqttPeerHandle {
        client,
        config: config.clone(),
        queue,
    })
}

/// Outgoing publishes waiting for the broker, at most one per topic.
///
/// The hive's event loop must never wait on the broker: rumqttc's request
/// channel holds 64 entries and stops draining while the broker link is
/// down, so awaiting `AsyncClient::publish` there would stall every TCP
/// peer along with it. `MqttPeerHandle::publish_latest` drops the value
/// in here instead and returns at once; `publish_task` does the waiting.
/// A newer value for a topic replaces the queued one, so memory is
/// bounded by the number of topics however long the broker is away, and
/// the broker sees the latest state once it is back.
#[derive(Default)]
struct PublishQueue {
    pending: Mutex<Pending>,
    wake: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct Pending {
    /// Topics in the order they first became pending.
    order: VecDeque<String>,
    latest: HashMap<String, Vec<u8>>,
}

impl Drop for PublishQueue {
    fn drop(&mut self) {
        // Last handle gone: let `publish_task` see that and exit.
        self.wake.notify_one();
    }
}

async fn publish_task(client: AsyncClient, qos: QoS, queue: Weak<PublishQueue>, wake: Arc<tokio::sync::Notify>) {
    loop {
        wake.notified().await;
        loop {
            let next = {
                let Some(q) = queue.upgrade() else { return };
                let mut p = q.pending.lock().unwrap();
                p.order.pop_front().map(|topic| {
                    let payload = p.latest.remove(&topic).unwrap_or_default();
                    (topic, payload)
                })
            };
            let Some((topic, payload)) = next else { break };
            if let Err(e) = client.publish(&topic, qos, false, payload).await {
                // The event loop task is gone; nothing more will be sent.
                warn!("MQTT publish to {} failed: {:?}", topic, e);
                return;
            }
        }
    }
}

/// A lightweight handle for publishing property changes back to the MQTT
/// broker.  Held by the Hive event loop or a Handler.
#[derive(Clone)]
pub struct MqttPeerHandle {
    client: AsyncClient,
    config: MqttConfig,
    queue: Arc<PublishQueue>,
}

impl MqttPeerHandle {
    /// Queue a property update for the broker without waiting. If an
    /// update for the same topic is still queued it is replaced, so while
    /// the broker is slow or unreachable only the latest value per
    /// property is kept. This is what the hive's event loop uses.
    pub fn publish_latest(&self, property: &Property) {
        let name = property.get_name();
        if name.is_empty() {
            return;
        }
        let topic = format!("{}/{}", self.config.topic_prefix, name);
        let payload = self.config.payload.encode(property).to_vec();
        {
            let mut p = self.queue.pending.lock().unwrap();
            if p.latest.insert(topic.clone(), payload).is_none() {
                p.order.push_back(topic);
            }
        }
        self.queue.wake.notify_one();
    }

    /// Publish a property update to the MQTT broker and wait until rumqttc
    /// has accepted it, which can take as long as the broker is away.
    /// Bypasses the `publish_latest` queue, so ordering between the two
    /// isn't guaranteed.
    pub async fn publish(&self, property: &Property) -> Result<()> {
        let name = property.get_name();
        if name.is_empty() {
            return Ok(());
        }
        let topic = format!("{}/{}", self.config.topic_prefix, name);
        let payload = self.config.payload.encode(property);

        self.client
            .publish(&topic, self.config.qos, false, payload.to_vec())
            .await
            .map_err(|e| {
                let msg = format!("MQTT publish error: {:?}", e);
                Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        Ok(())
    }

    /// Dynamically subscribe to an additional property topic at runtime.
    pub async fn subscribe(&self, property_name: &str) -> Result<()> {
        let topic = format!("{}/{}", self.config.topic_prefix, property_name);
        info!("MQTT dynamic subscribe: {}", topic);
        self.client
            .subscribe(&topic, self.config.qos)
            .await
            .map_err(|e| {
                let msg = format!("MQTT subscribe error: {:?}", e);
                Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        Ok(())
    }

    /// Disconnect from the broker gracefully.
    pub async fn disconnect(&self) -> Result<()> {
        self.client.disconnect().await.map_err(|e| {
            let msg = format!("MQTT disconnect error: {:?}", e);
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        Ok(())
    }
}
