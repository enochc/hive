# Changelog

## 0.3.0 (unreleased, `security` branch)

Several of the changes below break code that builds against 0.2.x, hence
the minor version bump from 0.2.201.

### Breaking changes

Build and features:

- `gui` is no longer a default feature; add `features = ["gui"]` to keep it.
- `tracing` is the default logging backend. Enable `log` to route Hive's
  logging through the standard `log` crate instead (it wins when both are
  on); turn off default features as well to drop `tracing` entirely.
- `hive::LevelFilter` is Hive's own enum, not a re-export of
  `log::LevelFilter`. `SimpleLogger` and `LOGGER` are gone.
- The `bluetooth` feature uses `uuid` 0.8, the version `bluster` 0.2 is
  built on (it was 1.x, which didn't compile).

Types:

- `Hive::properties` is a `PropertyMap` (a `HashMap` with an identity
  hasher for the already-hashed ids), not `HashMap<u64, Property>`.
- `SocketEvent`:
  - `NewPeer.stream` is `Option<Box<Connection>>`, a connection that has
    already finished the handshake, instead of `Option<TcpStream>`.
  - New variants: `Frames` (a run of frames from one TCP peer, carrying a
    `Credit`), `Local` (an update from a `Handler`) and `MqttText`.
- `MqttConfig` has private fields, so it can't be built with a struct
  literal. Use `MqttConfig::from_toml` or `try_from_toml`.
- `property::text_to_value` returns `Result<Option<Value>, _>`. `None`
  means "ignore this payload" (empty text for a non-string property).

Behaviour:

- `Handler::set_property(name, None)` deletes the property instead of
  panicking.
- `Handler::set_property`, `send_property` and `delete_property` can now
  wait. They return at once unless the hive's event loop has fallen 256 KiB
  behind this hive's handlers (see "Backpressure" below).
- `Hive::go(true, ..)` panics if the hive fails before it is ready (the
  listen port is taken, say). Before, it waited forever. `Hive::try_go`
  returns the error instead.
- Servers close connections that don't open with the `HVEP` handshake line.
- Config is validated when the hive is built. A value of the wrong type or
  out of range panics with a message naming the key, where before it was
  ignored or cast:
  - `port = "1883"` was treated as unset;
  - a negative timeout became centuries;
  - `max_message_size_bytes = 0` rejected every message.

  `[TLS]`, `[MQTT]` and `[MQTT.TLS]` also refuse unknown keys. Configs that
  used to start may now fail fast; the README lists the accepted ranges.
- MQTT payloads are plain text by default (`payload = "text"`). 0.2.x
  always used Hive's binary encoding; set `payload = "binary"` on every
  node of a hive-to-hive MQTT setup to keep talking to older versions.
- MQTT `payload = "text"`: an empty payload is ignored unless the property
  is a string. It used to turn numbers and booleans into `""`.
- A `Property`'s own `stream` field subscribes when first polled, so it
  yields changes from then on, not from when the property was created.
  Clones of it behave as before.

### Added

- Optional TLS (`tls` feature: rustls on ring, optional mutual TLS) and
  shared-secret authentication (`auth` feature: HMAC-SHA256
  challenge-response). Both run once per connection. Misconfiguration fails
  closed. See the README's Security section.
- MQTT broker login (`username`, `password` / `password_env`) and TLS
  (`[MQTT.TLS]`).
- MQTT `payload = "text"` (the default) for interoperating with ordinary
  MQTT clients; `payload = "binary"` keeps Hive's own encoding.
- `max_peers` and `max_pending_handshakes` connection limits.
- Backpressure:
  - a TCP peer that sends faster than the hive can handle is slowed by TCP
    flow control instead of queuing without limit;
  - `Handler`s wait the same way.

  MQTT ingest is not budgeted yet.
- `Hive::try_go`.
- `MqttPeerHandle::publish_latest`: queue an update without waiting,
  keeping only the latest value per topic.
- `tests/bench.rs` (throughput and latency, ignored by default) and
  PERF_NOTES.md.
- Embedding:
  - `Hive::on_event` reports every property change and delete (including
    properties the hive never declared by name), peer lists and connection
    changes as a `HiveEvent`.
  - `Handler::request_peers` asks the server for its peer list and updates
    to it; `Handler::set_property_by_id` sets a property known only by id.
  - `hive::embed::EmbeddedHive`: a blocking facade with its own runtime,
    for FFI hosts and threaded Rust.
  - JNI bindings over it (`src/jni_api.rs`, built for Android targets and
    with the `jni` feature elsewhere) and the Kotlin class that uses them,
    `bindings/kotlin/com/enochc/hive/NativeHive.kt`. These replace the
    `android.rs` stub, which did not compile.
- A client's `Handler::send_to_peer` to a hive it isn't connected to goes
  to its server, which routes it to the client of that name. Before, it
  failed with "No peer".

### Fixed

- An MQTT-only hive (no `listen`/`connect`) never became ready, so
  `go(true, ..)` hung.
- Waiting for the MQTT broker couldn't be cancelled.
- A broker that was down or slow stalled the event loop, and with it every
  TCP peer. Publishing now happens off the event loop.
- A long burst of updates could starve a peer's writer on a shared worker
  thread until the peer was dropped as "not reading".
- A malformed message, a slow client or a dead peer no longer affects the
  other peers.
- The `websock` and `bluetooth` features compile again. `websock` is still
  never used for a connection; see todo.md.

### Performance

On loopback, Apple M2 Pro:

| Updates/s | 0.2.201 (bd5f83b) | Now |
|---|---|---|
| 1 receiver | 17k-35k | 3.5M-3.7M |
| 8 receivers | 7k-8k | 2.1M-2.3M |

Latency is unchanged at about 80 µs p50 with 1 receiver. Details are in
PERF_NOTES.md.
