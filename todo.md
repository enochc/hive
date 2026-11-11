# Hive TODO

Tracks the work from the 2026-11-11 review: reliability fixes, hot-path
speed, and optional security. Each task is written to be picked up cold in
its own session. Tick the box and delete nothing; add a short note under a
task if the outcome differed from the plan.

**Rules for every task**

- Hot path = property update: `Handler::send_property` → event channel →
  `Hive::receive_events` → `got_message` → `broadcast` → `Peer::send` →
  `write_framed`, and the reverse via `read_loop`.
- Do not change the wire format unless the task says so. Property ids are
  FNV-1a hashes and must stay identical across architectures.
- Run `cargo test` before and after. Add a test for each behaviour change
  (see `tests/reconnect.rs` and `tests/streams.rs` for the style).
- One task per branch/commit. Tasks in the same phase that touch the same
  file should not run in parallel sessions.
- Line numbers are from commit `a5b9799` and will drift; search for the
  named function.

---

## Phase 1 — Reliability bugs (do first, all small)

- [x] **1.1 Read the length prefix with `read_exact`**
  `read_loop` in `src/peer.rs` (~line 497) uses `reader.read(&mut size_buff)`,
  which can return 1–3 bytes when a header straddles the `BufReader`
  boundary; the size is then garbage and the stream desyncs.
  Use `read_exact` for the 4 bytes (keep the `shutdown` select) and
  `read_exact` into the body buffer instead of the `read_buf` loop, which
  relies on `BytesMut` capacity being exactly the message size.
  Done when: a test that writes a frame one byte at a time, and a burst of
  several thousand small frames, both arrive intact.
  > Done 2026-11-11: new `read_frame` in `src/peer.rs`; body read bounded with `take`. Tests: `tests/hardening.rs`.

- [x] **1.2 A failed write to one peer must not stop the hive**
  `Hive::broadcast` in `src/hive.rs` (~line 837) does `p.send(..).await?`.
  The error propagates through `got_message` and ends `receive_events`.
  Log the error, remove that peer (same handling as `SocketEvent::Hangup`),
  and keep going. Check the other `?` uses in `got_message` and
  `receive_events` for the same problem.
  Done when: a test with one server and two clients, where one client's
  socket is killed, shows the other client still receiving updates.
  > Done 2026-11-11: `Hive::remove_peer` shared by Hangup, HANGUP and failed broadcast writes; `got_message` errors are logged, not propagated. Peer ack moved before dispatch so a removal mid-message can't leave a stale index.

- [x] **1.3 Strings longer than 255 bytes corrupt the frame**
  `property_value_to_bytes` in `src/property.rs` (~line 557) writes
  `str_val.len() as u8` but then the whole string. Smallest fix that keeps
  the wire format: add a new type tag (e.g. `IS_LONG_STR`) with a `u32`
  length for strings over 255 bytes, keep `IS_STR` for short ones, and
  decode both in `bytes_to_property_value`.
  Done when: a 300-byte and a 70 000-byte string round-trip in a unit test.
  > Done 2026-11-11: tag `IS_LONG_STR = 0x22`. Strings <=255 bytes encode exactly as before.

- [x] **1.4 Remove remotely triggerable panics**
  Any peer can crash the hive task. Replace with "log and drop that
  connection":
  - `unimplemented!("Unknown message ...")` in `got_message` (`src/hive.rs` ~980)
  - `unimplemented!("Why are you here?")` in the `PROPERTY` branch
  - `unimplemented!` on unknown type tag in `bytes_to_property_value`
  - `bytes` getters (`get_u64`, `get_u8`, `get_i8`, …) panic on short
    input: check `remaining()` first in `bytes_to_property`,
    `bytes_to_property_value`, the `DELETE` branch and `set_headers_from_peer`
  - `String::from_utf8(..).unwrap()` in `PEER_MESSAGE`, `set_headers_from_peer`
    and `IS_STR` decoding
  - `.expect(..)` calls in `read_loop` and `Peer::handshake`
  Make the decode functions return `Result`/`Option` rather than panic.
  Done when: a test sends an unknown type byte, a truncated property frame
  and invalid UTF-8, and the server keeps serving a second client.
  > Done 2026-11-11: decode errors are `property::DecodeError` and drop the sending peer. Deviation: unknown message *types* are logged and ignored (not dropped) for forward compatibility. Added `try_bytes_to_property_value`; old `bytes_to_property_value` kept as a panicking wrapper for API compatibility. MQTT ingest uses the fallible version. Config-time `unwrap`s (bad TOML etc.) left as is.

- [x] **1.5 Handshake must not block the event loop**
  `receive_events` awaits `Peer::new` (`src/hive.rs` ~597), which awaits
  `handshake`, which waits on an unbounded `read_line` (`src/peer.rs` ~238).
  A connection that sends nothing freezes every peer.
  Move the handshake into the accept task (and the connect task), wrap it
  in a timeout (default 5 s, config key `handshake_timeout_ms`), cap the
  handshake line length (e.g. 1 KiB), and only send `SocketEvent::NewPeer`
  once the handshake has succeeded. This is also the place TLS will hook in
  (task 4.2), so keep "raw stream in → handshaken peer out" as one function.
  Done when: a test opens a silent TCP connection to a server and a second,
  real client still connects and exchanges properties.
  > Done 2026-11-11: `peer::handshake` returns a `Connection`; `SocketEvent::NewPeer.stream` is now `Option<Connection>`. Behaviour change: the server now rejects connections that don't open with `HVEP` (the websocket path never worked anyway). A failed client handshake goes through the normal 10 s connect retry. `accept` errors are logged and retried instead of ending the listener.

## Phase 2 — Quick speed wins (independent of each other)

- [x] **2.1 Set `TCP_NODELAY`**
  Call `set_nodelay(true)` on the stream in `Hive::accept_loop` and after
  `TcpStream::connect` in `Hive::run` (`src/hive.rs`). Not set anywhere
  today; small frames can be delayed up to ~40 ms.
  Optional config key `tcp_nodelay = false` to turn it off.
  > Done 2026-11-11: set in `peer::handshake` for both directions. Skipped the `tcp_nodelay = false` key.

- [x] **2.2 Sensible default log level**
  `init_logging` in `src/lib.rs` defaults to `Trace`, and `SimpleLogger`
  does a `println!` with a `chrono` timestamp per line; each message emits
  ten or more lines. Default to `Info`, and make `SimpleLogger::enabled`
  respect `log::max_level()` instead of the hardcoded `Debug`.

- [x] **2.3 Cheaper liveness bookkeeping**
  `Peer::ack` (`src/peer.rs`) takes an `RwLock` write lock and calls
  `SystemTime::now()` per message. Store milliseconds since a process-start
  `Instant` in an `AtomicU64`; update `wave` to match. This also removes
  the wall-clock dependency (a clock change can currently cause a false
  hangup).
  > Done 2026-11-11: `AtomicU64` of ms since a process-start `Instant` (`peer::now_ms`).

- [x] **2.4 Property map lookups**
  `Hive::set_property` does three lookups (`has_property`, then
  `get_mut_property` which does `contains_key` + `get_mut`) using SipHash on
  keys that are already 64-bit hashes. Use the `entry` API and an identity
  hasher for `HashMap<u64, Property>` (hand-written `Hasher`, no new crate).
  > Done 2026-11-11: `property::PropertyMap` (identity hasher). `Hive::properties` changed type from `HashMap<u64, Property>`: same methods, but code that names the type needs updating.

- [x] **2.5 Make `gui` a non-default feature**
  `Cargo.toml` has `default = ["gui"]`, so headless builds link Slint.
  Check `build.rs`, `src/main.rs` and the examples still build with and
  without the feature. Confirm with the owner first: this changes what
  downstream crates get by default.
  > Done 2026-11-11 at the owner's request. `slint-build` is now optional too, and `hive_window` requires `gui`. Same change added the `tracing`/`log` logging features (src/logging.rs).

## Phase 3 — Hot-path restructuring (do in order)

- [x] **3.1 Per-peer writer task**
  Replace `Arc<TokioMutex<OwnedWriteHalf>>` in `Peer` with a channel to a
  spawned writer task that owns the write half. `Peer::send` becomes a
  non-blocking channel send. The writer drains everything queued and writes
  it in one `write_all`/`write_vectored`. Use a bounded channel; when it is
  full, drop the peer rather than block the hive. `wave` sends its PING
  through the same channel. The writer task ends when the peer is dropped.
  Make the writer generic over `AsyncWrite + Unpin + Send` so TLS can reuse
  it (task 4.2).
  Done when: existing tests pass, and a test with one stalled client (never
  reads) shows a second client still getting updates promptly.
  > Done 2026-11-11: `peer::write_loop`, queue of 1024 frames. Deviation: a full queue makes `Peer::send` wait up to 2 s (`SLOW_PEER_TIMEOUT`) before the peer is dropped, so a local burst applies backpressure instead of disconnecting a healthy peer. Test: `a_client_that_stops_reading_is_dropped_and_others_keep_up`.

- [x] **3.2 Decode without building a `Property`**
  `bytes_to_property` (`src/property.rs` ~613) builds a full `Property`
  (watch channel, boxed stream, several `Arc`s) for every received message.
  Add a decode that returns `(u64, Option<PropertyValue>)` and use it in the
  `PROPERTY` and `PROPERTIES` branches of `got_message`; apply the value to
  the existing property with `set_value`. Avoid the double copy in `IS_STR`
  decoding (`to_vec` then `PropertyValue::from(&str)`).
  > Done 2026-11-11: PROPERTY/PROPERTIES go through `decode_property` + `Hive::apply_value`.

- [x] **3.3 Forward received bytes; frame once**
  In the `PROPERTY` branch the incoming `msg` is already wire format.
  `broadcast` re-encodes it, then allocates and copies per peer, and
  `write_framed` allocates and copies again. Build one `Bytes` containing
  `[u32 length][type][payload]` and hand a refcount clone to each peer's
  writer (needs 3.1).
  > Done 2026-11-11: received PROPERTY/DELETE bytes are forwarded as one `Bytes` (refcount clone per peer); each writer copies into its reused batch buffer while framing, no per-message allocation.

- [x] **3.4 Encode directly in `Handler`**
  `Handler::set_property` / `send_property_value` (`src/handler.rs`) clone
  the value twice and build a throwaway `Property`. Add an encoder that
  takes `(id, &Value)` and writes straight into one `BytesMut`. Use
  `unbounded_send` instead of `SinkExt::send(..).await`.
  > Done 2026-11-11: `property::encode_property`; `Handler` uses `unbounded_send` and no longer panics if the hive has stopped. `set_property(name, None)` now deletes instead of panicking. Removed the `num-traits` dependency.

- [ ] **3.5 Numeric peer ids**
  `SocketEvent::Message { from: String }` allocates per message, peers are
  found by linear string compare, and `Peer::address()` clones per peer per
  broadcast. Give each peer a `u32` id assigned by the hive, carry that in
  `SocketEvent`, and index peers by it. Keep the address string for logs
  and `peer_string`. Reserve ids for the local handler and for MQTT
  (currently `""` and `"mqtt"`).
  > Deferred 2026-11-11: small gain next to the 3.1 results (see PERF_NOTES.md), and it changes `SocketEvent`, which the bluetooth modules build; do it on a Linux machine that can compile them.

- [x] **3.6 Benchmark**
  Add a `benches/` or ignored test that measures property updates per
  second and round-trip latency, server + 1 client and server + 8 clients,
  on loopback. Record before/after numbers for Phase 2 and 3 in
  `PERF_NOTES.md`. Ideally do this before 3.1 to get a baseline.
  > Done 2026-11-11: `tests/bench.rs`; numbers in `PERF_NOTES.md`. Throughput up ~40-95x, latency at 8 receivers +25 us p50.

## Phase 4 — Optional security

- [x] **4.1 Connection limits (no runtime cost)**
  Config keys: `max_peers` (reject beyond it), and document that `listen`
  with only a port binds the LAN address from `local_ipaddress`, so use
  `127.0.0.1:port` for local-only. Depends on 1.4 and 1.5 being done.
  > Done 2026-11-11: `max_peers` (checked at accept and again when the peer is added) and `max_pending_handshakes` (default 64), both refuse before any handshake/TLS work.

- [x] **4.2 TLS behind a `tls` cargo feature**
  Add `tokio-rustls` (plus `rustls-pemfile` or the current equivalent;
  check current crate docs for the recommended versions and crypto
  provider) as optional dependencies. Config:
  ```toml
  [TLS]
  cert = "certs/node.pem"      # this node's certificate chain
  key  = "certs/node.key"
  ca   = "certs/ca.pem"        # trust root for the other side
  server_name = "hive.local"   # client only; name to verify
  ```
  Server wraps the accepted stream with `TlsAcceptor`, client with
  `TlsConnector`, inside the handshake function from 1.5 and under the same
  timeout. `Peer`, `read_loop` and the writer task must work over any
  `AsyncRead`/`AsyncWrite` (use `tokio::io::split`). With no `[TLS]` table
  behaviour is unchanged. Prefer ChaCha20-Poly1305 capable suites so ARM
  boards without AES instructions stay fast (rustls defaults include it).
  Done when: a test with generated certs syncs a property over TLS, a
  client with the wrong CA is rejected, and a plaintext client to a TLS
  server is rejected without affecting other peers.
  > Done 2026-11-11: `src/security.rs`; tokio-rustls on ring only (no aws-lc-rs anywhere, rumqttc switched to `use-rustls-no-provider`). Transport is boxed `AsyncRead`/`AsyncWrite`; plain TCP keeps lock-free halves. `prefer_chacha20` option. Bad config panics at construction (fails closed). Tests: `tests/security.rs`. Benchmarked: throughput same as plain, +5-10 us p50.

- [x] **4.3 Mutual TLS**
  Config `require_client_cert = true` under `[TLS]`: server verifies the
  client certificate against `ca`; client presents `cert`/`key`.
  Done when: a client without a certificate, or with one from another CA,
  is rejected.
  > Done 2026-11-11: `require_client_cert = true` with `ca`.

- [x] **4.4 Shared-secret auth (lighter alternative)**
  Config `auth_secret = "..."`. Challenge–response at connect: server sends
  a random nonce, client replies with HMAC-SHA256(secret, nonce), server
  compares in constant time. One hash per connection, nothing per message.
  Document clearly: without TLS this only keeps out unauthorized clients,
  it does not encrypt and does not stop an on-path attacker. This extends
  the handshake, so both sides must be configured the same way.
  > Done 2026-11-11: `auth` feature (ring HMAC-SHA256), mutual: both sides send nonces and prove the secret. `auth_secret` or `auth_secret_env`. The dialing side sends `HVEP+auth` so a mismatch fails immediately with a clear error.

- [x] **4.5 MQTT TLS and credentials**
  `MqttConfig` in `src/mqtt/mqtt_peer.rs`: add `username`, `password`,
  and TLS options, using `rumqttc`'s rustls transport (check its docs for
  the feature flag and API in the version pinned in `Cargo.toml`).
  > Done 2026-11-11: `username`, `password`/`password_env`, `[MQTT.TLS]`; password is redacted in Debug output. New `MqttConfig::try_from_toml`.

- [x] **4.6 Document the security options**
  README section: threat model for each option, example configs, how to
  generate a CA and node certificates, and the runtime cost of each.
  > Done 2026-11-11: README "Security" section; openssl commands verified against rustls with a real mTLS connection.

## Not planned / notes

- Websocket peers: `Peer::handshake` never constructs a `WebSock`, so the
  `websock` feature is currently dead code. Decide separately whether to
  revive or remove it.
- Bluetooth transports are out of scope for the security work; they rely
  on BLE pairing.
