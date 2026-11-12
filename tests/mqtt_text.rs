#![cfg(feature = "mqtt")]

//! MQTT `payload = "text"`: interop with MQTT clients that know nothing
//! about Hive's binary format. Requires a broker on 127.0.0.1:1883
//! (`mosquitto -c mosquitto.conf`); skip with SKIP_MQTT_TESTS=1.

use hive::hive::Hive;
use hive::CancellationToken;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn should_skip() -> bool {
    std::env::var("SKIP_MQTT_TESTS").unwrap_or_default() == "1"
}

async fn wait_for<F: Fn() -> bool>(f: F, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_text_mqtt_clients_interoperate() {
    if should_skip() {
        eprintln!("Skipping MQTT test (SKIP_MQTT_TESTS=1)");
        return;
    }
    // A hive bridging TCP peers and a text-speaking MQTT world.
    let server = Hive::new_from_str_unknown(
        r#"
        name = "TextBridge"
        listen = "127.0.0.1:34950"
        [MQTT]
        host = "127.0.0.1"
        client_id = "hive-text-bridge"
        topic_prefix = "hive/text_test"
        payload = "text"
        subscribe = ["count", "temp", "power", "label"]
        [Properties]
        count = 0
        temp = 0.0
        power = false
        label = "x"
        "#,
    );
    let mut srv = tokio::time::timeout(Duration::from_secs(5), server.go(true, CancellationToken::new()))
        .await
        .expect("bridge hive never became ready");

    // A TCP peer of that hive, recording the typed values it receives.
    let mut tcp = Hive::new_from_str_unknown("connect = \"127.0.0.1:34950\"\nname = \"tcp\"");
    let seen: Arc<Mutex<HashMap<String, String>>> = Default::default();
    let history: Arc<Mutex<Vec<String>>> = Default::default();
    for name in ["count", "temp", "power", "label"] {
        let s = seen.clone();
        let h = history.clone();
        tcp.get_mut_property_by_name(name)
            .unwrap()
            .on_next(move |v| {
                let v = format!("{:?}", v.val);
                h.lock().unwrap().push(format!("{name}={v}"));
                s.lock().unwrap().insert(name.to_string(), v);
            });
    }
    let _tcp = tcp.go(false, CancellationToken::new()).await;
    assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await);

    // A plain MQTT client: publishes text, records what it receives as text.
    let mut opts = rumqttc::MqttOptions::new("plain-text-device", "127.0.0.1", 1883);
    opts.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = rumqttc::AsyncClient::new(opts, 32);
    let received: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let r = received.clone();
    tokio::spawn(async move {
        while let Ok(ev) = eventloop.poll().await {
            if let rumqttc::Event::Incoming(rumqttc::Packet::Publish(p)) = ev {
                r.lock()
                    .unwrap()
                    .push((p.topic.clone(), String::from_utf8_lossy(&p.payload).to_string()));
            }
        }
    });
    client
        .subscribe("hive/text_test/#", rumqttc::QoS::AtLeastOnce)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Text in: parsed using each property's existing type.
    for (topic, text) in [("count", "72"), ("temp", "21"), ("power", "ON"), ("label", "123")] {
        client
            .publish(format!("hive/text_test/{topic}"), rumqttc::QoS::AtLeastOnce, false, text)
            .await
            .unwrap();
    }
    let expect = [
        ("count", "Integer(72)"),
        ("temp", "Float(21.0)"),   // float property: "21" stays a float
        ("power", "Boolean(true)"), // "ON" accepted for a bool property
        ("label", "String(\"123\")"), // string property: digits stay a string
    ];
    let ok = wait_for(
        || {
            let s = seen.lock().unwrap();
            expect.iter().all(|(k, v)| s.get(*k).map(|x| x == v).unwrap_or(false))
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(ok, "TCP peer saw {:?}", seen.lock().unwrap());

    // An empty payload (how MQTT clears a retained message) is ignored
    // for a non-string property instead of turning it into "".
    for (topic, text) in [("count", ""), ("power", ""), ("count", "73")] {
        client
            .publish(format!("hive/text_test/{topic}"), rumqttc::QoS::AtLeastOnce, false, text)
            .await
            .unwrap();
    }
    let ok = wait_for(
        || seen.lock().unwrap().get("count").map(|v| v == "Integer(73)").unwrap_or(false),
        Duration::from_secs(3),
    )
    .await;
    assert!(ok, "TCP peer saw {:?}", seen.lock().unwrap());
    let h = history.lock().unwrap().clone();
    assert!(
        !h.iter().any(|e| e.contains("String") && !e.starts_with("label=")),
        "a non-string property became a string: {h:?}"
    );
    assert_eq!(seen.lock().unwrap().get("power").map(String::as_str), Some("Boolean(true)"));

    // Text out: a change from the Hive side reaches MQTT as readable text.
    srv.set_property("label", Some(&"hello world".into())).await;
    srv.set_property("temp", Some(&1.5.into())).await;
    srv.set_property("power", Some(&false.into())).await;
    let ok = wait_for(
        || {
            let r = received.lock().unwrap();
            let has = |t: &str, p: &str| r.iter().any(|(tt, pp)| tt == t && pp == p);
            has("hive/text_test/label", "hello world")
                && has("hive/text_test/temp", "1.5")
                && has("hive/text_test/power", "false")
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(ok, "plain client received {:?}", received.lock().unwrap());
}

#[test]
#[should_panic(expected = "expected \"binary\" or \"text\"")]
fn unknown_payload_format_is_rejected() {
    Hive::new_from_str_unknown("[MQTT]\nhost = \"127.0.0.1\"\npayload = \"json\"");
}

/// An MQTT peer usually talks to ordinary MQTT clients, so text is the
/// default; binary has to be asked for.
#[test]
fn text_is_the_default_payload_format() {
    use hive::mqtt::mqtt_peer::{MqttConfig, PayloadFormat};
    let t = |s: &str| toml::from_str::<toml::Value>(s).unwrap();
    let default = MqttConfig::from_toml(&t("host = \"127.0.0.1\"")).unwrap();
    assert_eq!(default.payload, PayloadFormat::Text);
    let binary = MqttConfig::from_toml(&t("host = \"127.0.0.1\"\npayload = \"binary\"")).unwrap();
    assert_eq!(binary.payload, PayloadFormat::Binary);
}
