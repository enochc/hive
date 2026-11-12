//! Property names between hives. Values travel with ids only; names are
//! exchanged once, and only with hives that ask (`Handler::request_names`).

use hive::handler::Handler;
use hive::hive::{Hive, HiveEvent};
use hive::property::Property;
use hive::CancellationToken;
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

const LIMIT: Duration = Duration::from_secs(5);
const NAME_REQUESTS: u8 = 0x67;
const NAMES: u8 = 0x68;

/// A running hive and the names it reports learning.
async fn hive(config: &str, wait: bool) -> (Handler, UnboundedReceiver<String>) {
    let mut hive = Hive::new_from_str_unknown(config);
    let (tx, named) = unbounded_channel();
    hive.on_event(move |event| {
        if let HiveEvent::PropertyNamed { id, name } = event {
            assert_eq!(id, Property::hash_id(&name));
            let _ = tx.send(name);
        }
    });
    (hive.go(wait, CancellationToken::new()).await, named)
}

/// Wait until `named` has reported every one of `names`, in any order.
async fn learns(named: &mut UnboundedReceiver<String>, names: &[&str], who: &str) {
    let mut missing: BTreeSet<&str> = names.iter().copied().collect();
    while !missing.is_empty() {
        match tokio::time::timeout(LIMIT, named.recv()).await {
            Ok(Some(name)) => assert!(missing.remove(name.as_str()), "{} was told {:?} twice or unasked", who, name),
            _ => panic!("{} never learned {:?}", who, missing),
        }
    }
}

async fn learns_nothing(named: &mut UnboundedReceiver<String>, who: &str) {
    let more = tokio::time::timeout(Duration::from_millis(300), named.recv()).await;
    assert!(more.is_err(), "{} was told {:?}", who, more);
}

/// A bare socket that has done the client handshake as `name`.
async fn raw_client(port: u16, name: &str) -> TcpStream {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(format!("HVEP\n\u{78}{}\n", name).as_bytes()).await.unwrap();
    socket
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

/// The type byte of every frame the socket delivers within `window`, or
/// `None` in place of the rest once it is closed.
async fn frame_types(socket: &mut TcpStream, window: Duration) -> Vec<Option<u8>> {
    let mut types = Vec::new();
    let _ = tokio::time::timeout(window, async {
        loop {
            let mut len = [0u8; 4];
            if socket.read_exact(&mut len).await.is_err() {
                types.push(None);
                return;
            }
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            if socket.read_exact(&mut body).await.is_err() {
                types.push(None);
                return;
            }
            types.push(body.first().copied());
        }
    })
    .await;
    types
}

#[tokio::test(flavor = "multi_thread")]
async fn names_reach_every_hive_that_asks() {
    let (mut server, mut server_named) =
        hive("listen = \"127.0.0.1:35130\"\nname = \"Serv\"\n[Properties]\nlight = 1\nlabel = \"hi\"", true).await;

    // Asking before the connection exists is remembered.
    let (mut a, mut a_named) = hive("connect = \"127.0.0.1:35130\"\nname = \"A\"", false).await;
    a.request_names().await;
    learns(&mut a_named, &["light", "label"], "A").await;

    // A property a client creates: the client tells its server the name.
    a.set_property("volume", Some(&7.into())).await;
    learns(&mut a_named, &["volume"], "A").await;
    learns(&mut server_named, &["volume"], "the server").await;

    // A later client gets everything, including what A named.
    let (mut b, mut b_named) = hive("connect = \"127.0.0.1:35130\"\nname = \"B\"", false).await;
    b.request_names().await;
    learns(&mut b_named, &["light", "label", "volume"], "B").await;

    // New names keep flowing: from the server, and from one client to the
    // other through it. Nobody is told a name twice.
    server.set_property("fresh", Some(&1.into())).await;
    learns(&mut a_named, &["fresh"], "A").await;
    learns(&mut b_named, &["fresh"], "B").await;
    b.register_name("later").await;
    learns(&mut b_named, &["later"], "B").await;
    learns(&mut a_named, &["later"], "A").await;
    learns(&mut server_named, &["fresh", "later"], "the server").await;
    b.set_property("later", Some(&2.into())).await;
    server.set_property("volume", Some(&8.into())).await;
    for (named, who) in [(&mut a_named, "A"), (&mut b_named, "B"), (&mut server_named, "the server")] {
        learns_nothing(named, who).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_can_ask_its_clients() {
    let (mut server, mut server_named) = hive("listen = \"127.0.0.1:35131\"\nname = \"Serv\"", true).await;
    server.request_names().await;
    let (_client, mut client_named) =
        hive("connect = \"127.0.0.1:35131\"\nname = \"C\"\n[Properties]\nspeed = 3", false).await;
    learns(&mut server_named, &["speed"], "the server").await;
    // Answering made the client part of the exchange too.
    server.set_property("mode", Some(&"auto".into())).await;
    learns(&mut client_named, &["mode"], "the client").await;
}

/// Hive before 0.3.0 stops on a message type it doesn't know, so a peer
/// that hasn't asked must never be sent names.
#[tokio::test(flavor = "multi_thread")]
async fn a_hive_that_did_not_ask_is_sent_no_names() {
    let (mut server, _) = hive("listen = \"127.0.0.1:35132\"\nname = \"Serv\"\n[Properties]\nlight = 1", true).await;
    let mut old = raw_client(35132, "Old").await;
    let (mut asker, mut asker_named) = hive("connect = \"127.0.0.1:35132\"\nname = \"New\"", false).await;
    asker.request_names().await;
    learns(&mut asker_named, &["light"], "the asker").await;

    server.set_property("fresh", Some(&1.into())).await;
    asker.set_property("volume", Some(&7.into())).await;
    learns(&mut asker_named, &["fresh", "volume"], "the asker").await;

    let seen = frame_types(&mut old, Duration::from_millis(500)).await;
    assert!(seen.len() >= 3, "expected the header, the snapshot and two updates, got {:x?}", seen);
    assert!(
        !seen.iter().any(|t| matches!(t, Some(NAMES) | Some(NAME_REQUESTS) | None)),
        "a peer that never asked got {:x?}",
        seen
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_does_not_match_its_id_is_refused() {
    let (server, mut server_named) = hive("listen = \"127.0.0.1:35133\"\nname = \"Serv\"", true).await;
    let mut liar = raw_client(35133, "Liar").await;

    // A true pair is accepted from anyone.
    let mut names = vec![NAMES];
    names.extend(Property::hash_id("honest").to_be_bytes());
    names.extend(6u16.to_be_bytes());
    names.extend(b"honest");
    liar.write_all(&frame(&names)).await.unwrap();
    learns(&mut server_named, &["honest"], "the server").await;

    // "light" under the id of something else is not, and ends the connection.
    let mut names = vec![NAMES];
    names.extend(Property::hash_id("door_lock").to_be_bytes());
    names.extend(5u16.to_be_bytes());
    names.extend(b"light");
    liar.write_all(&frame(&names)).await.unwrap();
    let seen = frame_types(&mut liar, LIMIT).await;
    assert_eq!(seen.last(), Some(&None), "the connection stayed open: {:x?}", seen);
    learns_nothing(&mut server_named, "the server").await;
    assert_eq!(server.peer_count(), 0);
}

#[test]
fn a_property_asked_for_by_name_has_one() {
    let mut hive = Hive::new_from_str_unknown("connect = \"127.0.0.1:35134\"");
    assert_eq!(hive.get_mut_property_by_name("turn").unwrap().get_name(), "turn");
    assert_eq!(hive.get_property(&Property::hash_id("turn")).unwrap().get_name(), "turn");
}
