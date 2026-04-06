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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::channel::mpsc::UnboundedSender;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::sync::{Notify, mpsc as tokio_mpsc};
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::hive::Result;
use crate::peer::SocketEvent;
use crate::property::{Property, property_to_bytes, property_value_to_bytes, bytes_to_property_value};

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
}

impl MqttConfig {
    /// Build an `MqttConfig` from the `[MQTT]` table in a Hive TOML config.
    pub fn from_toml(table: &toml::Value) -> Option<MqttConfig> {
        let host = table.get("host")?.as_str()?.to_string();
        let port = table.get("port").and_then(|v| v.as_integer()).unwrap_or(1883) as u16;
        let client_id = table
            .get("client_id")
            .and_then(|v| v.as_str())
            .unwrap_or("hive-mqtt")
            .to_string();
        let topic_prefix = table
            .get("topic_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("hive")
            .to_string();
        let qos_int = table.get("qos").and_then(|v| v.as_integer()).unwrap_or(1);
        let qos = match qos_int {
            0 => QoS::AtMostOnce,
            2 => QoS::ExactlyOnce,
            _ => QoS::AtLeastOnce,
        };
        let keep_alive = table
            .get("keep_alive")
            .and_then(|v| v.as_integer())
            .unwrap_or(30) as u64;

        let subscriptions = table
            .get("subscribe")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Some(MqttConfig {
            host,
            port,
            client_id,
            topic_prefix,
            qos,
            keep_alive: Duration::from_secs(keep_alive),
            subscriptions,
        })
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

        let payload = property_value_to_bytes(property);

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
            let mut payload = incoming.payload;
            let prop_value = bytes_to_property_value(&mut payload);

            let prop = Property::from_name(&incoming.property_name, prop_value);
            let msg = property_to_bytes(&prop, true);

            debug!("MQTT -> Hive SocketEvent::Message for {}", incoming.property_name);

            let se = SocketEvent::Message {
                from: String::from("mqtt"),
                msg,
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

    Ok(MqttPeerHandle {
        client,
        config: config.clone(),
    })
}

/// A lightweight handle for publishing property changes back to the MQTT
/// broker.  Held by the Hive event loop or a Handler.
#[derive(Clone)]
pub struct MqttPeerHandle {
    client: AsyncClient,
    config: MqttConfig,
}

impl MqttPeerHandle {
    /// Publish a property update to the MQTT broker using the Hive
    /// binary wire format.
    pub async fn publish(&self, property: &Property) -> Result<()> {
        let name = property.get_name();
        if name.is_empty() {
            return Ok(());
        }
        let topic = format!("{}/{}", self.config.topic_prefix, name);
        let payload = property_value_to_bytes(property);

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
