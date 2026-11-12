//! `hive::embed::EmbeddedHive`, the blocking facade FFI hosts sit on,
//! against ordinary hives.

use hive::embed::{property_id, EmbeddedHive, Event, Value};
use hive::handler::Handler;
use hive::hive::{Hive, HiveEvent};
use hive::CancellationToken;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(5);

/// The next thing on `rx` that `pick` accepts, skipping everything else.
fn expect<T: std::fmt::Debug, R>(rx: &Receiver<T>, what: &str, pick: impl Fn(&T) -> Option<R>) -> R {
    let deadline = Instant::now() + LIMIT;
    let mut skipped = Vec::new();
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(item) => match pick(&item) {
                Some(found) => return found,
                None => skipped.push(item),
            },
            Err(RecvTimeoutError::Timeout) => panic!("never saw {}; got {:?}", what, skipped),
            Err(RecvTimeoutError::Disconnected) => panic!("channel closed before {}; got {:?}", what, skipped),
        }
    }
}

fn property(event: &Event, name: Option<&str>, id: u64, value: Option<Value>) -> Option<()> {
    let wanted = Event::Property { id, name: name.map(String::from), value };
    (*event == wanted).then_some(())
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !f() {
        assert!(Instant::now() < deadline, "{}", what);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A plain hive whose events and peer messages land on channels.
fn plain_hive(rt: &tokio::runtime::Runtime, config: &str, wait: bool) -> (Handler, Receiver<HiveEvent>, Receiver<String>) {
    let mut hive = Hive::new_from_str_unknown(config);
    let (event_tx, events) = channel();
    hive.on_event(move |e| {
        let _ = event_tx.send(e);
    });
    let (message_tx, messages) = channel();
    hive.message_received.connect(move |m| {
        let _ = message_tx.send(m);
    });
    let handler = rt.block_on(hive.go(wait, CancellationToken::new()));
    (handler, events, messages)
}

#[test]
fn embedded_client_mirrors_its_server() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (mut server, server_events, server_messages) = plain_hive(
        &rt,
        "listen = \"127.0.0.1:35101\"\nname = \"Serv\"\n[Properties]\nlight = 1\nlabel = \"hi\"",
        true,
    );

    let (tx, events) = channel();
    let phone = EmbeddedHive::start(
        "name = \"Phone\"\nconnect = \"127.0.0.1:35101\"\n[Properties]\nlight = 0",
        move |e| {
            let _ = tx.send(e);
        },
    )
    .unwrap();
    let (light, label, volume) = (property_id("light"), property_id("label"), property_id("volume"));

    // What the config declared, before anything from the network.
    expect(&events, "the declared value", |e| property(e, Some("light"), light, Some(Value::Integer(0))));
    expect(&events, "the connection", |e| (*e == Event::Connection { connected: true, peers: 1 }).then_some(()));
    assert!(phone.is_connected());
    assert_eq!(phone.peer_count(), 1);

    // The server's snapshot, in no particular order: one property the
    // phone can name, one it can't.
    let mut snapshot = vec![
        Event::Property { id: light, name: Some("light".into()), value: Some(Value::Integer(1)) },
        Event::Property { id: label, name: None, value: Some(Value::String("hi".into())) },
    ];
    while !snapshot.is_empty() {
        let found = expect(&events, "the server's snapshot", |e| snapshot.iter().position(|s| s == e));
        snapshot.remove(found);
    }

    // Once told the name, later events carry it.
    assert_eq!(phone.register_name("label"), label);
    rt.block_on(server.set_property("label", Some(&"there".into())));
    expect(&events, "the renamed label", |e| property(e, Some("label"), label, Some(Value::String("there".into()))));

    // Phone to server, by name and by id, every value type.
    phone.set_property("volume", 7);
    expect(&events, "its own volume", |e| property(e, Some("volume"), volume, Some(Value::Integer(7))));
    expect(&server_events, "volume on the server", |e| {
        (*e == HiveEvent::PropertyChanged { id: volume, value: Value::Integer(7) }).then_some(())
    });
    phone.set_property_by_id(label, "by id");
    phone.set_property("ratio", 1.5);
    phone.set_property("on", true);
    for (name, value) in [
        ("label", Value::String("by id".into())),
        ("ratio", Value::Float(1.5)),
        ("on", Value::Boolean(true)),
    ] {
        let id = property_id(name);
        expect(&server_events, name, |e| (*e == HiveEvent::PropertyChanged { id, value: value.clone() }).then_some(()));
    }

    // Deletes, both directions.
    phone.delete_property("light");
    expect(&events, "its own delete", |e| property(e, Some("light"), light, None));
    expect(&server_events, "the delete on the server", |e| (*e == HiveEvent::PropertyDeleted { id: light }).then_some(()));
    rt.block_on(server.delete_property("volume"));
    expect(&events, "the server's delete", |e| property(e, Some("volume"), volume, None));

    // The peer list, and its updates when someone else joins.
    phone.request_peers();
    let names = |e: &Event| match e {
        Event::Peers(peers) => Some(peers.iter().map(|p| p.name.clone()).collect::<Vec<_>>()),
        _ => None,
    };
    assert_eq!(expect(&events, "the peer list", names), ["Serv", "Phone"]);
    let (_other, _other_events, other_messages) =
        plain_hive(&rt, "name = \"Other\"\nconnect = \"127.0.0.1:35101\"", false);
    expect(&events, "the peer list with Other", |e| names(e).filter(|n| n.contains(&"Other".to_string())));

    // Messages: from the server, to the server, and through it to another
    // client the phone has no connection to.
    rt.block_on(server.send_to_peer("Phone", "hello phone")).unwrap();
    expect(&events, "the server's message", |e| (*e == Event::PeerMessage("hello phone".into())).then_some(()));
    phone.send_to_peer("Serv", "hello server").unwrap();
    expect(&server_messages, "the phone's message", |m| (m == "hello server").then_some(()));
    phone.send_to_peer("Other", "via the server").unwrap();
    expect(&other_messages, "the routed message", |m| (m == "via the server").then_some(()));

    // Stopping disconnects, and leaves the handle harmless.
    assert_eq!(server.peer_count(), 2);
    phone.stop();
    wait_for("the server still counts the stopped phone", || server.peer_count() == 1);
    phone.set_property("volume", 8);
    phone.request_peers();
    assert!(phone.send_to_peer("Serv", "anyone?").is_err());
    phone.stop();
}

#[test]
fn a_bad_config_is_an_error_not_a_panic() {
    let started = EmbeddedHive::start("connect = 3000", |_| {});
    let error = started.err().expect("a number for `connect` was accepted");
    assert!(error.contains("`connect` must be a string"), "{}", error);

    let error = EmbeddedHive::start("this is not toml", |_| {}).err().expect("junk was accepted");
    assert!(!error.is_empty());
}

#[test]
fn stop_abandons_a_connection_attempt() {
    // Nothing listens here, so the hive is retrying in the background.
    let (tx, events) = channel();
    let hive = EmbeddedHive::start("name = \"Lonely\"\nconnect = \"127.0.0.1:35102\"", move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    hive.request_peers();
    hive.set_property("queued", 1);
    assert!(!hive.is_connected());

    let started = Instant::now();
    hive.stop();
    assert!(started.elapsed() < Duration::from_secs(4), "stop took {:?}", started.elapsed());
    // The callback, and with it the sender, is gone once stop returns.
    assert!(events.recv_timeout(LIMIT).is_err(), "an event from a hive that never connected");
}
