# Hive

A cross-platform Rust library for real-time property synchronization between devices and services.

Hive lets you define observable properties on one device and automatically keep them in sync across every connected peer, whether they're linked over TCP sockets, Bluetooth, MQTT, or WebSockets. When a property changes on any node, the update propagates to all others.

## Core Concepts

**Properties** are the unit of state in Hive. Each property has a name, a typed value (string, integer, float, bool), and a reactive change stream. You can observe changes with callbacks or async streams:

```rust
let mut hive = Hive::new_from_str_unknown(r#"
    name = "Thermostat"
    listen = "3000"
    [Properties]
    temperature = 72
    target_temp = 68.5
    fan_active = false
"#);

// React to changes
hive.get_mut_property_by_name("temperature")
    .unwrap()
    .on_next(|value| {
        println!("Temperature changed: {:?}", value);
    });

// Or use async streams
let mut stream = hive.get_mut_property_by_name("temperature")
    .unwrap()
    .stream
    .clone();

tokio::spawn(async move {
    while let Some(value) = stream.next().await {
        println!("Stream: {:?}", value);
    }
});
```

**Peers** connect Hive instances together. A server listens for connections; clients connect to servers. Once linked, all properties synchronize automatically:

```rust
// Server
let server = Hive::new_from_str_unknown(r#"
    name = "Server"
    listen = "3000"
    [Properties]
    lightValue = 0
"#);

// Client,  connects and receives all server properties
let client = Hive::new_from_str_unknown(r#"
    name = "Client"
    connect = "127.0.0.1:3000"
"#);
```

**Handlers** let you modify properties and send messages from outside the Hive event loop:

```rust
let cancellation_token = CancellationToken::new();
let mut handler = hive.go(true, cancellation_token).await;

// Set a property, propagates to all peers
handler.set_property("lightValue", Some(&42.into())).await;

// Send a direct message to a specific peer
handler.send_to_peer("Client", "hello there").await;
```

`go(true, ..)` waits until the hive is listening, connected or attached to its MQTT broker, and panics if it fails before that (the listen port is taken, say). `try_go` returns that failure as an `Err` instead.

## Transport Protocols

| Protocol | Feature Flag | Use Case |
|----------|-------------|----------|
| **TCP** | *(always available)* | LAN communication between services |
| **WebSocket** | `websock` | Browser and web client integration |
| **Bluetooth** | `bluetooth` | Direct device-to-device without network infrastructure |
| **MQTT** | `mqtt` | IoT broker-based pub/sub across networks |

### MQTT

The MQTT transport connects Hive to any standard MQTT broker (Mosquitto, EMQX, HiveMQ, etc.). Properties map to MQTT topics under a configurable prefix, with plain-text payloads by default:

```toml
name = "SensorNode"

[MQTT]
host = "192.168.1.100"
port = 1883
client_id = "hive-sensor-01"
topic_prefix = "hive/home"
qos = 1
keep_alive = 30
subscribe = ["temperature", "humidity"]

[Properties]
temperature = 0
humidity = 0
```

With this config, property changes publish to `hive/home/temperature`, `hive/home/humidity`, etc. Any other Hive instance subscribed to the same topics will receive updates.

**Payload format.** Payloads are plain text by default (`payload = "text"`), so ordinary MQTT clients, dashboards and home-automation tools can read and publish them:

```bash
mosquitto_pub -t hive/home/temperature -m 21.5   # Hive receives 21.5
mosquitto_sub -t 'hive/home/#' -v                # prints e.g. "hive/home/heater true"
```

Values are published as plain text: `72`, `21.5` (floats always keep a decimal point), `true`/`false`, or the string itself. Incoming text is read as the property's current type, so a string property stays a string when someone publishes `123`, a float property reads `21` as 21.0, and a boolean property also accepts `on`/`off`, `yes`/`no` and `1`/`0` (case-insensitive). Properties without a value yet are inferred: `true`/`false`, then integer, then float, then string. An empty payload (how MQTT clears a retained message) is ignored unless the property is a string, where it sets `""`, so it never turns a number or boolean into a string. Every node sharing a topic prefix should use the same `payload` setting.

If every client on the topics is another hive, `payload = "binary"` uses Hive's compact typed encoding instead. Nothing else can read it: `mosquitto_sub` sees `72` arrive as the bytes `0x21 0x48`, and plain text published by `mosquitto_pub` is dropped as malformed.

Publishing never holds up the rest of the hive. Outgoing updates are queued, with only the latest value kept per topic, and sent by a background task. While the broker is slow or unreachable, TCP peers keep syncing and the broker gets the current values once it is back.

## Configuration

Hive instances are configured via TOML, either inline strings or files:

```rust
// From a string
let hive = Hive::new_from_str_unknown(r#"
    name = "MyHive"
    listen = "3000"
    [Properties]
    sensor = 0
"#);

// From a file
let hive = Hive::new("config.toml");
```

### Config Reference

Hive checks its config when it is built and panics with a message naming the key if a value has the wrong type or is out of range (`port = "1883"`, a negative timeout, `max_message_size_bytes = 0`). `[TLS]`, `[MQTT]` and `[MQTT.TLS]` also refuse keys they don't know, so a typo such as `password_evn` can't quietly leave a setting off. Ranges: timeouts 1 ms to one day, `max_message_size_bytes` 64 B to 1 GiB, `max_peers` 0 to 1,000,000, `max_pending_handshakes` 1 to 1,000,000, MQTT `port` 1-65535, `keep_alive` 0-65535 s (0 turns it off), `qos` 0-2.

```toml
# Identity
name = "MyHive"

# TCP, pick one: listen (server) or connect (client)
listen = "3000"                    # or "192.168.1.10:3000"
connect = "192.168.1.10:3000"      # server address

# Optional reliability settings, see "Connection Reliability" below
wave_timeout_ms = 800              # actively ping if a peer goes idle this long
max_message_size_bytes = 1048576   # cap on a single message's claimed size, this is the default
handshake_timeout_ms = 5000        # time a new connection gets to finish the handshake, default
max_peers = 32                     # server: refuse clients beyond this, default unlimited
max_pending_handshakes = 64        # server: connections allowed mid-handshake at once, default

# Optional security, see "Security" below
auth_secret = "change-me"          # requires "auth" feature; or auth_secret_env = "HIVE_SECRET"
[TLS]                              # requires "tls" feature
cert = "certs/node.pem"
key = "certs/node.key"
ca = "certs/ca.pem"

# Bluetooth (requires "bluetooth" feature)
bt_listen = "Hive_Peripheral"      # advertise as BLE peripheral
bt_connect = "Hive_Central"        # connect as BLE central

# MQTT (requires "mqtt" feature)
[MQTT]
host = "127.0.0.1"
port = 1883
client_id = "hive-node"
topic_prefix = "hive"
qos = 1                            # 0 = AtMostOnce, 1 = AtLeastOnce, 2 = ExactlyOnce
keep_alive = 30
subscribe = ["prop1", "prop2"]
payload = "text"                   # default; "binary" if only other hives use the topics
username = "hive"                  # optional broker login
password_env = "MQTT_PASSWORD"     # or password = "..."
[MQTT.TLS]                         # optional, requires "tls"; port then defaults to 8883
ca = "certs/broker-ca.pem"

# Properties with optional arguments
[Properties]
simple_int = 42
simple_str = "hello"
simple_bool = true
simple_float = 3.14
with_args = { val = 0, rest_set = "http://localhost:8000/save/{val}" }
```

## Property Features

**Debouncing (receive side)**  For high-frequency updates like motor control, properties support a backoff window on their local `on_next` callback. The first change fires immediately, subsequent changes within the window are absorbed, and the latest value is delivered when the timer expires:

```rust
let prop = hive.get_mut_property_by_name("motor_speed").unwrap();
prop.set_backoff(&50); // 50ms debounce window
prop.on_next(|v| {
    // Fires at most once per 50ms
    send_to_motor(v);
});
```

This only rate-limits the local callback. It does not reduce outgoing network traffic, `Handler::set_property` encodes straight to the wire and never looks at `backoff`.

**Debouncing (send side)**  To rate-limit how often you actually send updates, e.g. a UI slider firing on every pixel of drag, use `Handler::set_property_debounced` instead. Same leading+trailing behavior as `backoff`, applied to the network send itself, keyed per property name:

```rust
// First call in a burst sends immediately, further calls within 50ms
// coalesce down to the latest value, sent once the window closes.
handler.set_property_debounced("motor_speed", 72.0.into(), 50).await;
```

**Property names** A value travels with its property's id, a hash of the name, never the name itself, so an update costs the same however long the name is. A hive knows the names of the properties in its own config and of those set through its `Handler`s. To learn the rest, ask:

```rust
handler.request_names().await;
```

Every hive it is connected to, now or later, answers with the names it knows and keeps it up to date, and gets this hive's names in return. So a client that asks receives its server's names, and a property one client creates by name reaches the other clients that asked with its name. Each name is sent once per connection, and a name that doesn't hash to its id is refused. Names learned this way show up as `HiveEvent::PropertyNamed` (see `Hive::on_event`) and on the `Property` itself.

Only hives that take part are ever sent names, because Hive before 0.3.0 stops on a message type it doesn't know. For the same reason, only call `request_names` where every peer is 0.3.0 or newer.

**Typed values** Properties support bool, string, integers (i8 through i64, auto-sized to the smallest fit), and f64 floats. The binary wire format encodes values with type tags for compact transmission.

**Reactive streams** Every property exposes a `tokio::sync::watch`-backed stream. Multiple consumers can independently observe changes. Each clone yields the changes made after it was cloned; a property's own `stream` field starts when it is first polled. Updates to a property nobody is streaming skip the channel's wake-ups entirely:

```rust
let mut stream = prop.stream.clone();
tokio::spawn(async move {
    while let Some(value) = stream.next().await {
        // Each clone gets independent cursor
    }
});
```

## Connection Reliability

**Liveness detection (`wave_timeout_ms`)** By default a Hive instance only notices a peer is gone when the OS reports the TCP connection closed. A link that dies silently, Wi-Fi drops, a process hangs instead of exiting, never triggers that. Setting `wave_timeout_ms` makes that Hive instance actively watch its peers instead: once a peer's gone that many milliseconds without sending anything at all, it gets pinged and given the same interval again to reply. No reply, a PONG or literally any other message both count, means it's declared dead and removed, same as a real disconnect.

```toml
name = "flight_computer"
connect = "192.168.1.10:3000"
wave_timeout_ms = 800   # ping after 800ms idle, dead if no reply within another 800ms
```

Only the side that needs to detect a disconnect has to set this, replying to a PING is unconditional on the receiving end regardless of its own config. Watch for it in application code via `handler.is_connected()`. Applies to TCP and Bluetooth-peripheral peers; WebSocket and Bluetooth-central peers already emit a disconnect from their own read loops and don't need this.

**`is_connected()` vs `peer_count()`** `is_connected()` answers "is this Hive up", which for a server means "has it bound its listen port" and is true with nobody attached. `peer_count()` returns how many peers are actually attached, so that's the one to check for "is anyone on the other end", and the one to build a connection indicator from.

**Reconnecting** A Hive doesn't reconnect on its own. `go()` consumes it, and once its run loop is up, a peer that goes away stays away. To reconnect, cancel the `CancellationToken` you passed to `go()` and build a fresh Hive. Cancelling is honored while it's dialing too, so an attempt that hasn't connected yet can be abandoned without leaving a task retrying in the background, which is what lets a caller drive its own retry schedule instead of the built-in 10s one.

Note that a server sends its entire property set to a client as soon as that client connects. A reconnecting client therefore receives current values immediately, before anything has changed. That's usually what you want for state sync, but if a property drives something physical, treat the opening snapshot accordingly rather than as a fresh command.

**Message size limit (`max_message_size_bytes`)** Every TCP message is length-prefixed, and that length is trusted before the payload is read into memory. Without a cap, a corrupted stream or a bug on the sending side claiming an absurd size would attempt one huge allocation. Defaults to 1 MiB, comfortably more than typical property payloads need, override it only if you have a legitimate reason to send larger messages.

```toml
max_message_size_bytes = 4194304   # 4 MiB
```

**Slow or stuck peers** Each TCP peer has its own writer task and send buffer (256 KiB), so a peer that stops reading can't stall updates to everyone else. A slow peer is waited for; one whose buffer is full and whose socket accepts nothing for 2 seconds is dropped. A malformed message also drops only the peer that sent it, and a new connection gets `handshake_timeout_ms` to complete the handshake without holding up anyone else.

**Backpressure on the way in** Each TCP peer may have 256 KiB that the hive has received but not yet handled. Past that, Hive stops reading from that peer's socket until the hive has worked half of it off, so TCP flow control slows the sender instead of memory growing. A hive's `Handler`s share the same kind of budget: `set_property`, `send_property` and `delete_property` normally return at once, but wait while the event loop is that far behind. Updates arriving from MQTT are not budgeted yet.

**Connection limits (`max_peers`, `max_pending_handshakes`)** A server can cap how many clients it keeps (`max_peers`, unlimited by default) and how many connections may be mid-handshake at once (`max_pending_handshakes`, default 64). Connections over either limit are closed straight after `accept`, before any handshake or TLS work. To keep a server off the LAN entirely, listen on loopback: `listen = "127.0.0.1:3000"`. A bare port such as `listen = "3000"` binds the machine's LAN address.

## Security

Out of the box Hive has no authentication or encryption: anything that can reach the port receives every property and can set any of them. Three options, each optional and configured per Hive, all applied once per connection during the handshake. Nothing runs per message except TLS's own encryption.

| Option | Feature | Protects against | Doesn't protect against | Runtime cost |
|---|---|---|---|---|
| Limits and timeouts (above) | *(always)* | Connection floods, silent sockets, oversized or malformed messages | Anyone who can connect | None |
| `auth_secret` | `auth` | Clients or servers that don't know the secret | Eavesdropping, tampering on the wire | One HMAC exchange per connection, ~1 round trip |
| `[TLS]` | `tls` | Eavesdropping, tampering, impersonating the server | Unknown clients, unless `require_client_cert` | Handshake per connection; see below |
| `[TLS]` + `require_client_cert` | `tls` | All of the above plus unknown clients | Theft of a client's key | Same as TLS |

**Recommendation:** TLS with `require_client_cert = true` (mutual TLS) on anything beyond a trusted LAN. `auth_secret` alone is a cheap way to keep stray clients out of a network you already trust; it can also be combined with TLS.

Misconfiguration fails closed. A `[TLS]` table or `auth_secret` in a Hive built without the matching feature, an unreadable certificate, a client with no `ca`, a misspelled or wrongly typed key under `[TLS]`, or an `auth_secret` that ended up inside a table all panic at `Hive::new*`, never fall back to plaintext. Remember that in TOML every key below a `[table]` header belongs to that table, so `auth_secret` has to come before the first one. Both ends must use the same settings: a TLS client can't talk to a plaintext server and vice versa, and a hive with `auth_secret` refuses one without it in either direction.

### Shared secret

```toml
auth_secret_env = "HIVE_SECRET"   # read from the environment, or auth_secret = "..."
```

Both sides prove they hold the secret with HMAC-SHA256 over fresh random nonces from each side, compared in constant time, so a recorded handshake can't be replayed. Use a long random value (`openssl rand -base64 32`).

### TLS

```toml
# Server
listen = "0.0.0.0:3000"
[TLS]
cert = "certs/server.pem"
key = "certs/server.key"
ca = "certs/ca.pem"              # needed for require_client_cert
require_client_cert = true       # mutual TLS

# Client
connect = "192.168.1.10:3000"
[TLS]
ca = "certs/ca.pem"              # trust only certificates signed by your CA
cert = "certs/client.pem"        # present a client cert (mutual TLS)
key = "certs/client.key"
server_name = "hive.local"       # optional, defaults to the host in `connect`
```

A Hive that both listens and connects (a middle man) uses the same `[TLS]` table for both roles. TLS 1.3 and 1.2 via rustls on the ring backend, so cross-compiling to ARM needs no CMake or C++ toolchain. `prefer_chacha20 = true` puts ChaCha20-Poly1305 first, much faster than AES on boards without AES instructions (most Raspberry Pis); leave it off on x86 and Apple silicon.

**Cost:** on loopback (Apple M2 Pro, `tests/bench.rs`) TLS throughput matched plain TCP at about 3M updates/s to one receiver, and median latency was within a few us. Updates are batched into large writes, so encryption cost grows with bytes sent rather than with message count. Expect a larger difference on small ARM boards.

**Making certificates** with OpenSSL (any CA works, this is a minimal private one):

```bash
# Your CA, keep ca.key offline
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
  -subj "/CN=Hive CA" -keyout ca.key -out ca.pem

# A server certificate; list every name and IP clients will use in `connect`
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj "/CN=hive-server" -keyout server.key -out server.csr
printf "subjectAltName=DNS:hive.local,IP:192.168.1.10\nextendedKeyUsage=serverAuth\n" > server.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 825 \
  -extfile server.ext -out server.pem

# A client certificate, for require_client_cert
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj "/CN=hive-client" -keyout client.key -out client.csr
printf "extendedKeyUsage=clientAuth\n" > client.ext
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 825 \
  -extfile client.ext -out client.pem
```

### MQTT

Broker credentials and TLS go in the `[MQTT]` table (`username`, `password` or `password_env`, and an `[MQTT.TLS]` subtable with `ca`, and `cert`/`key` for client-certificate login). See the config reference above.

## Building

```bash
# Default, TCP only, logging through `tracing`
cargo build

# Security: TLS / mutual TLS, shared-secret auth
cargo build --features "tls,auth"

# Log through the standard `log` crate instead of `tracing`
cargo build --features log

# Desktop GUI (Slint)
cargo build --features gui

# With MQTT support
cargo build --features mqtt

# With WebSocket support
cargo build --features websock

# With Bluetooth support (Linux)
cargo build --features bluetooth

# Multiple features
cargo build --features "mqtt,websock"
```

## Running Tests

```bash
# Core tests (no external dependencies)
cargo test

# MQTT tests (requires broker on localhost:1883)
mosquitto -c mosquitto.conf -v  # in another terminal
cargo test --features mqtt

# Skip MQTT tests in CI
SKIP_MQTT_TESTS=1 cargo test --features mqtt

# TLS, mutual TLS and shared-secret tests
cargo test --features "tls,auth" --test security

# Throughput / latency benchmark (numbers in PERF_NOTES.md)
cargo test --release --features tls --test bench -- --ignored --nocapture --test-threads=1
```

## Examples

```bash
# Basic property usage
cargo run --example properties

# Run a Hive server from a TOML file. Prints every change, and takes
# commands on stdin: `name = value`, `-name`, `@peer message`
cargo run --example serve -- examples/listen_3000.toml

# Hive with MQTT
cargo run --example mqtt_example --features mqtt
```

## Cross-Platform

Hive builds as a static library, cdylib, and standard Rust lib, designed to embed into iOS, Android, and Linux applications:

```toml
[lib]
crate-type = ["staticlib", "cdylib", "lib"]
```

A C FFI entry point (`newHive`) is included for native mobile integration.

### Embedding (Android, JVM, threaded Rust)

`hive::embed::EmbeddedHive` is a Hive for hosts that aren't async Rust. It runs its own tokio runtime, its methods are plain blocking calls safe from any thread, and everything it has to report arrives as an `Event` on one dedicated thread:

```rust
use hive::embed::{EmbeddedHive, Event};

let hive = EmbeddedHive::start(
    "name = \"Phone\"\nconnect = \"192.168.1.10:3000\"",
    |event| match event {
        Event::Property { id, name, value } => { /* value: None = deleted */ }
        Event::PropertyNamed { id, name } => {}
        Event::Peers(peers) => {}
        Event::PeerMessage(text) => {}
        Event::Connection { connected, peers } => {}
    },
)?;
hive.request_peers();
hive.request_names();
hive.set_property("lightValue", 42);
hive.send_to_peer("Server", "hello");
hive.stop();
```

A property the hive doesn't know the name of is reported with `name: None`. After `request_names` (see "Property names" above) that only lasts until the server has answered: an `Event::PropertyNamed` follows, and the property's later events carry the name. Without it, names come from the `[Properties]` table of the config, from `set_property` and from `register_name`.

Underneath are hooks any Rust application can use: `Hive::on_event` (every property change, delete and name, peer list and connection change, as a `HiveEvent`), `Handler::request_peers` and `Handler::request_names`.

**Kotlin / Java.** `src/jni_api.rs` exposes `EmbeddedHive` over JNI and `bindings/kotlin/com/enochc/hive/NativeHive.kt` is the class that goes with it; copy that file into the app (or add `bindings/kotlin` as a source directory). The bindings are always part of an Android build:

```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 --platform 24 \
    -o path/to/app/src/main/jniLibs build --lib --release
```

Add `--features tls,auth` for those. On other hosts the `jni` feature builds the same bindings, which is how to load Hive into a desktop JVM (`cargo build --features jni`, then `-Djava.library.path=target/debug`).

```kotlin
val hive = NativeHive("name = \"Phone\"\nconnect = \"192.168.1.10:3000\"", object : HiveListener {
    override fun onProperty(id: Long, name: String?, value: Any?) { /* String, Boolean, Long, Double; null = deleted */ }
    override fun onConnection(connected: Boolean, peers: Int) {}
})
hive.set("lightValue", 42L)
hive.close()
```

Listener methods run on the hive's events thread, so hand off to the main thread before touching UI.

## Architecture

```
┌──────────┐     TCP      ┌──────────┐     TCP      ┌──────────┐
│  Hive A  │◄────────────►│  Hive B  │◄────────────►│  Hive C  │
│ (server) │              │ (middle) │              │ (client) │
└──────────┘              └──────────┘              └──────────┘
                               ▲
                               │ MQTT
                               ▼
                        ┌──────────────┐
                        │ MQTT Broker  │
                        └──────┬───────┘
                               │
                        ┌──────┴───────┐
                        │  Hive D      │
                        │ (MQTT only)  │
                        └──────────────┘
```

Hive instances can simultaneously use multiple transports. A node can listen for TCP clients, connect to an MQTT broker, and advertise over Bluetooth, all properties stay synchronized across every transport.

## License

MIT: see [LICENSE.txt](LICENSE.txt)
