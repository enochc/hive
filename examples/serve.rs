//! A hive to try clients against (the Android test app, for one). Runs a
//! config, prints everything that happens, and takes commands on stdin:
//!
//!   cargo run --example serve -- examples/listen_3000.toml
//!
//!   lightValue = 42          set a property (true/false, integer, float, else text)
//!   -lightValue              delete it
//!   @Android hello there     send a message to the peer called Android
use hive::hive::{Hive, HiveEvent};
use hive::property::{text_to_value, PropertyValue};
use hive::CancellationToken;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() {
    let path = std::env::args().nth(1).expect("usage: serve <config.toml>");
    let mut hive = Hive::new(&path);

    // Values arrive with ids only. Names come from the config and, since
    // we ask for them below, from the clients.
    let names: HashMap<u64, String> =
        hive.properties.values().map(|p| (p.id, p.get_name().to_string())).collect();
    let names = Arc::new(Mutex::new(names));
    let known = names.clone();
    let label = {
        let names = names.clone();
        move |id: u64| names.lock().unwrap().get(&id).cloned().unwrap_or_else(|| format!("#{:x}", id))
    };
    hive.on_event(move |event| match event {
        HiveEvent::PropertyChanged { id, value } => println!("{} = {}", label(id), value),
        HiveEvent::PropertyDeleted { id } => println!("{} deleted", label(id)),
        HiveEvent::PropertyNamed { id, name } => {
            println!("#{:x} is called {}", id, name);
            known.lock().unwrap().insert(id, name);
        }
        HiveEvent::Connection { peers, .. } => println!("{} peer(s) attached", peers),
        HiveEvent::Peers(peers) => println!("peers: {:?}", peers),
    });
    hive.message_received.connect(|message| println!("message: {}", message));

    let mut handler = hive.go(true, CancellationToken::new()).await;
    handler.request_names().await;
    println!("{} is up", handler.from_name);

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('@') {
            let (peer, message) = rest.split_once(' ').unwrap_or((rest, ""));
            if handler.send_to_peer(peer, message).await.is_err() {
                break;
            }
        } else if let Some(name) = line.strip_prefix('-') {
            handler.delete_property(name.trim()).await;
        } else if let Some((name, text)) = line.split_once('=') {
            let name = name.trim();
            match text_to_value(text.trim().as_bytes(), None) {
                Ok(Some(value)) => handler.set_property(name, Some(&PropertyValue::from(value))).await,
                _ => println!("nothing to set {} to", name),
            }
        } else if !line.is_empty() {
            println!("commands: name = value, -name, @peer message");
        }
    }
    // Out of input (stdin closed or not a terminal): keep serving.
    std::future::pending::<()>().await;
}
