//! todo.md 5.4 / 5.5: config values of the wrong type, out of range, or
//! under a misspelled key refuse to start, naming the key, instead of
//! being cast or quietly ignored.

use hive::hive::Hive;

/// Building a hive from `config` panics with a message containing `expect`.
fn refuses(config: &str, expect: &str) {
    let r = std::panic::catch_unwind(|| Hive::new_from_str_unknown(config));
    let err = match r {
        Ok(_) => panic!("accepted {:?}, expected an error mentioning {:?}", config, expect),
        Err(e) => e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default(),
    };
    assert!(err.contains(expect), "{:?}: panic {:?} doesn't mention {:?}", config, err, expect);
}

#[test]
fn hive_integers_are_range_checked() {
    std::panic::set_hook(Box::new(|_| {}));
    for key in ["wave_timeout_ms", "handshake_timeout_ms", "max_message_size_bytes", "max_pending_handshakes"] {
        refuses(&format!("{key} = -1"), &format!("`{key}` = -1 is out of range"));
        refuses(&format!("{key} = 99999999999"), &format!("`{key}` = 99999999999 is out of range"));
        refuses(&format!("{key} = \"500\""), &format!("`{key}` must be an integer"));
    }
    refuses("max_peers = -1", "`max_peers` = -1 is out of range");
    refuses("max_peers = \"5\"", "`max_peers` must be an integer");
    // 0 used to be accepted and then rejected every non-empty message.
    refuses("max_message_size_bytes = 0", "`max_message_size_bytes` = 0 is out of range");
    refuses("listen = 34000", "`listen` must be a string");
    let _ = std::panic::take_hook();
}

#[test]
fn sane_values_are_accepted() {
    Hive::new_from_str_unknown(
        "wave_timeout_ms = 800\nhandshake_timeout_ms = 2000\nmax_message_size_bytes = 4096\nmax_peers = 0\nmax_pending_handshakes = 8",
    );
}

#[cfg(feature = "mqtt")]
#[test]
fn mqtt_integers_are_range_checked() {
    std::panic::set_hook(Box::new(|_| {}));
    let m = |rest: &str| format!("[MQTT]\nhost = \"127.0.0.1\"\n{rest}");
    refuses(&m("port = -1"), "[MQTT] `port` = -1 is out of range");
    refuses(&m("port = 70000"), "[MQTT] `port` = 70000 is out of range");
    refuses(&m("port = \"1883\""), "[MQTT] `port` must be an integer");
    refuses(&m("keep_alive = -5"), "[MQTT] `keep_alive` = -5 is out of range");
    refuses(&m("keep_alive = 100000"), "[MQTT] `keep_alive` = 100000 is out of range");
    refuses(&m("keep_alive = \"30\""), "[MQTT] `keep_alive` must be an integer");
    refuses(&m("qos = 3"), "[MQTT] `qos` = 3 is out of range");
    refuses(&m("subscribe = \"x\""), "[MQTT] `subscribe` must be an array of strings");
    let _ = std::panic::take_hook();
}

#[cfg(feature = "mqtt")]
#[test]
fn mqtt_unknown_keys_are_refused() {
    std::panic::set_hook(Box::new(|_| {}));
    for typo in ["paylaod = \"text\"", "usrename = \"hive\"", "password_evn = \"X\"", "hots = \"x\""] {
        let key = typo.split(' ').next().unwrap();
        refuses(
            &format!("[MQTT]\nhost = \"127.0.0.1\"\n{typo}"),
            &format!("[MQTT] has an unknown key `{key}`"),
        );
    }
    let _ = std::panic::take_hook();
}

#[cfg(all(feature = "mqtt", feature = "tls"))]
#[test]
fn mqtt_tls_unknown_keys_are_refused() {
    std::panic::set_hook(Box::new(|_| {}));
    refuses(
        "[MQTT]\nhost = \"127.0.0.1\"\n[MQTT.TLS]\nca = \"ca.pem\"\nclient_cert = \"c.pem\"",
        "[MQTT.TLS] has an unknown key `client_cert`",
    );
    refuses(
        "[MQTT]\nhost = \"127.0.0.1\"\n[MQTT.TLS]\nca = \"ca.pem\"\nprefer_chacha20 = \"yes\"",
        "[MQTT.TLS] `prefer_chacha20` must be true or false",
    );
    let _ = std::panic::take_hook();
}
