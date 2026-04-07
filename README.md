# Hive

A cross-platform Rust library for real-time property synchronization between devices and services.

Hive lets you define observable properties on one device and automatically keep them in sync across every connected peer — whether they're linked over TCP sockets, Bluetooth, MQTT, or WebSockets. When a property changes on any node, the update propagates to all others.

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

// Client — connects and receives all server properties
let client = Hive::new_from_str_unknown(r#"
    name = "Client"
    connect = "127.0.0.1:3000"
"#);
```

**Handlers** let you modify properties and send messages from outside the Hive event loop:

```rust
let cancellation_token = CancellationToken::new();
let mut handler = hive.go(true, cancellation_token).await;

// Set a property — propagates to all peers
handler.set_property("lightValue", Some(&42.into())).await;

// Send a direct message to a specific peer
handler.send_to_peer("Client", "hello there").await;
```

## Transport Protocols

| Protocol | Feature Flag | Use Case |
|----------|-------------|----------|
| **TCP** | *(always available)* | LAN communication between services |
| **WebSocket** | `websock` | Browser and web client integration |
| **Bluetooth** | `bluetooth` | Direct device-to-device without network infrastructure |
| **MQTT** | `mqtt` | IoT broker-based pub/sub across networks |

### MQTT

The MQTT transport connects Hive to any standard MQTT broker (Mosquitto, EMQX, HiveMQ, etc.). Properties map to MQTT topics under a configurable prefix, using a compact binary wire format for payloads:

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

With this config, property changes publish to `hive/home/temperature`, `hive/home/humidity`, etc. Any other Hive instance (or MQTT client speaking the binary format) subscribed to the same topics will receive updates.

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

```toml
# Identity
name = "MyHive"

# TCP — pick one: listen (server) or connect (client)
listen = "3000"                    # or "192.168.1.10:3000"
connect = "192.168.1.10:3000"      # server address

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

# Properties with optional arguments
[Properties]
simple_int = 42
simple_str = "hello"
simple_bool = true
simple_float = 3.14
with_args = { val = 0, rest_set = "http://localhost:8000/save/{val}" }
```

## Property Features

**Debouncing** — for high-frequency updates like motor control, properties support a backoff window. The first change fires immediately, subsequent changes within the window are absorbed, and the latest value is delivered when the timer expires:

```rust
let prop = hive.get_mut_property_by_name("motor_speed").unwrap();
prop.set_backoff(&50); // 50ms debounce window
prop.on_next(|v| {
    // Fires at most once per 50ms
    send_to_motor(v);
});
```

**Typed values** — properties support bool, string, integers (i8 through i64, auto-sized to the smallest fit), and f64 floats. The binary wire format encodes values with type tags for compact transmission.

**Reactive streams** — every property exposes a `tokio::sync::watch`-backed stream. Multiple consumers can independently observe changes:

```rust
let mut stream = prop.stream.clone();
tokio::spawn(async move {
    while let Some(value) = stream.next().await {
        // Each clone gets independent cursor
    }
});
```

## Building

```bash
# Default — TCP only
cargo build

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
```

## Examples

```bash
# Basic property usage
cargo run --example properties

# Run a Hive server from a TOML file
cargo run --example just_run -- examples/listen_3000.toml

# Hive with MQTT
cargo run --example mqtt_example --features mqtt
```

## Cross-Platform

Hive builds as a static library, cdylib, and standard Rust lib — designed to embed into iOS, Android, and Linux applications:

```toml
[lib]
crate-type = ["staticlib", "cdylib", "lib"]
```

Android JNI bindings and a C FFI entry point (`newHive`) are included for native mobile integration.

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

Hive instances can simultaneously use multiple transports. A node can listen for TCP clients, connect to an MQTT broker, and advertise over Bluetooth — all properties stay synchronized across every transport.

## License

MIT — see [LICENSE.txt](LICENSE.txt)
