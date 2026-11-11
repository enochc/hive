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
  > The channel was replaced by `peer::Outbox` in 3.7.

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
  > `HIVE_BENCH_UPDATES` sets the update count (50,000 is only 15-35 ms now), `HIVE_BENCH_LOG=1` turns on warnings.

- [x] **3.7 Batch the hot path end to end**
  Profiling after phase 4 showed per-message overhead, not Hive's logic:
  ~17% malloc/free, ~16% popping the event channel, ~10% `sendto` on small
  writes. Also a bug: the event loop never yielded while it had events
  (`futures` channels and `try_send` are outside tokio's cooperative
  budget), so a writer task parked on the same worker thread could starve
  and a healthy peer was dropped as "not reading". Reproduced 5 of 5 with
  12M queued updates and 30 ms SIGSTOP/SIGCONT bursts; adding only a yield
  fixed it 4 of 4.
  - `read_loop` reads a chunk and sends every complete frame in it as one
    `SocketEvent::Frames` (zero-copy slices of the read buffer).
  - `Peer` queues into an `Outbox` (mutex + buffer) instead of an mpsc
    channel; the writer swaps the buffer out and writes it.
  - `receive_events` handles up to `EVENT_BATCH` (1024) messages, wakes each
    peer's writer once (`flush_peers`), then yields.
  > Done 2026-11-11: about 2x throughput at 1 receiver and 2.4x at 8, latency unchanged (PERF_NOTES.md). The outbox is bounded in bytes (256 KiB plus one message), and a peer is only "not reading" if the socket takes under 64 KiB in 2 s while a sender waits, so a slow link is waited for rather than dropped. `SocketEvent::NewPeer.stream` is now `Option<Box<Connection>>` (the event is 80 bytes, was 200). Tests: unit tests in `src/peer.rs` (paused time, in-memory pipe), two more in `tests/hardening.rs`. No test for the starvation bug itself, it needs the process stopped from outside.

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
  > Follow-up 2026-11-11: `[TLS]` refuses unknown or wrongly typed keys (a typo in `require_client_cert` used to leave it off), and `auth_secret` / `auth_secret_env` inside any table is an error. `[MQTT.TLS]` is not checked this way yet.

## Phase 5 — Findings from the 2026-11-11 branch review

Review of `security` against `master`. What was fixed during the review is
recorded above: the event-loop starvation that dropped healthy peers, the
send queue bounded in frames rather than bytes, and slow peers being
dropped instead of waited for are all under 3.7; unknown `[TLS]` keys and
a misplaced `auth_secret` are the follow-up under 4.6. These are what's
left, most important first. "Read, not reproduced" means the finding comes
from reading the code and has no failing test or run behind it yet, so
start by writing one.

- [ ] **5.1 An empty MQTT text payload changes a property's type**
  With `payload = "text"`, `property::text_to_value` falls through to
  "string" for an empty payload whatever the property holds, so an empty
  message on an integer, float or boolean property turns it into `""`, and
  that then syncs to every TCP peer. Empty payloads are normal in MQTT
  (it's how a retained message is cleared). The other direction doesn't
  round-trip either: `value_to_text(None)` publishes a cleared property as
  an empty payload, which the receiving hive reads back as `""`, and an
  empty string publishes the same bytes.
  Decide what an empty payload means: ignore it, or treat it as a clear
  when the property isn't a string. Either way a non-string property must
  not become a string.
  Done when: a unit test for `text_to_value(b"", Some(&Integer))` and an
  `mqtt_text` test publishing an empty payload to an integer property show
  the type unchanged.
  (Read, not reproduced; `text_inference_edge_cases` only covers the
  no-hint case.)

- [ ] **5.2 Keep MQTT publishing off the event loop**
  The `PROPERTY` branch of `Hive::got_message` awaits
  `MqttPeerHandle::publish`, which awaits rumqttc's request channel (64
  slots, `AsyncClient::new(opts, 64)`). While the broker link is down
  rumqttc's task sleeps 2 s per error and that channel fills, so the hive's
  event loop, and with it every TCP peer, waits on the broker. Same on
  master. 3.7 flushes TCP output before the publish, which keeps what was
  already queued moving but not the loop itself.
  Options: `try_publish` and drop (or keep only the latest value per
  topic) when full, or hand publishes to a small task through a
  latest-value map.
  Done when: a test that stops the broker mid-run still sees TCP peers
  syncing within the usual latency.
  (Read, not reproduced.)

- [ ] **5.3 Backpressure on the way in**
  `read_loop` and `Handler` push into an unbounded channel
  (`mpsc::unbounded` in `Hive::new_from_str`). A peer or local producer
  that outruns the event loop grows memory without limit; nothing ever
  makes a fast sender slow down. The long benchmark runs do exactly this:
  the sender queues all 12M local updates ahead of the event loop.
  For peers: count bytes handed to the hive and not yet handled (an atomic
  per peer, added in `read_loop`, subtracted when `handle_event` finishes
  a `Frames`), and have `read_loop` stop reading above a limit so TCP
  pushes back on the sender. For `Handler`: `set_property` is already
  `async`, so it can wait for room, or at least document the limit.
  Done when: a test with a slow `on_next` on the server and a raw client
  writing far more than the limit shows the client's writes blocking
  rather than the queue growing.

- [ ] **5.4 Range-check config integers**
  `Hive::new_from_str` casts TOML integers straight to the target type:
  `wave_timeout_ms` and `handshake_timeout_ms` with `as u64` (a negative
  value becomes centuries), `max_message_size_bytes` with `as u32` (wraps
  for negatives and anything over 4 GiB; `0` rejects every non-empty
  message).
  `MqttConfig::try_from_toml` does the same for `port` (`as u16`) and
  `keep_alive` (`as u64`). Also, a key present with the wrong type (a
  quoted number) is treated as absent.
  Fail at construction with a message naming the key, the way the
  security config does.
  Done when: tests for a negative, an oversized and a quoted value each
  panic with the key's name.

- [ ] **5.5 Strict keys for `[MQTT]` and `[MQTT.TLS]`**
  The `[TLS]` table now refuses unknown or wrongly typed keys; the MQTT
  tables still ignore them. Lower risk (a missing `ca` already errors and
  a missing client certificate just gets refused by the broker), but a
  misspelled `payload`, `username` or `password_env` is silently dropped.
  Reuse the helpers in `security::tls` (`check_keys`, `get_str`,
  `get_bool`).
  Done when: a misspelled key under each table refuses to start.

- [ ] **5.6 Cheaper `Property::set_value`**
  Now the largest single cost per message (PERF_NOTES.md): the
  `tokio::sync::watch` send takes a write lock and eight `Notify` mutexes
  whether or not anyone is subscribed. About 13% of worker CPU at 1
  receiver, 22% at 8. `PropertyStream` would keep the current value
  somewhere cheap and only create or touch the watch channel once a stream
  has been handed out. `Property.stream` is a public field, so this changes
  API; keep `get_value`, `current_value` and stream semantics (see
  `tests/streams.rs`).
  Done when: `tests/streams.rs` and `tests/property_backoff.rs` pass and
  the benchmark rows improve.

- [ ] **5.7 `Hive::go(true, ..)` never returns if the hive fails to start**
  `go` spawns `run(..).await.expect("Failed to run Hive")` and then waits
  on `ready`. If `run` returns an error first (the listen address is
  already in use, say) the spawned task panics and nothing ever notifies
  the caller. Same on master. Return the error to the caller, or at least
  wake it.
  Done when: a test that binds the port first sees `go(true, ..)` come
  back (with an error or a not-connected handler) instead of hanging.
  (Read, not reproduced.)

- [ ] **5.8 A test for the event-loop starvation bug**
  3.7 fixed it but has no regression test: the reproduction stopped the
  process from outside (`kill -STOP`/`-CONT` in 30 ms bursts during a 12M
  update benchmark run). A deterministic version probably needs a
  current-thread or single-worker runtime: one hive with a long backlog
  of local updates and a peer whose writer shares the worker, asserting
  the peer is still attached and gets the last value.
  Done when: the test fails with the `yield_now` in `receive_events`
  removed and passes with it.

- [ ] **5.9 Steadier benchmark**
  50,000 updates now finish in 15-35 ms. The first run in the process
  (plain, 1 receiver) reads 1.9M-2.5M where the same code gives 2.9M-3.5M
  at 500,000 updates, so the table in PERF_NOTES.md is comparing warm-up.
  Add an untimed warm-up pass, or raise the default count, and say so in
  PERF_NOTES.md since earlier rows used 50,000.

- [ ] **5.10 `websock` and `bluetooth` don't build**
  `cargo check --features websock` fails with 6 errors and
  `--features bluetooth` with 17 on macOS, identically on `master`, so
  not caused by this branch. Both construct `SocketEvent`s, and 1.5 and
  3.7 changed that type without being able to compile them. Either fix
  them (bluetooth on a Linux machine) or remove them; see the websocket
  note below.

- [ ] **5.11 Write down the public API changes**
  This branch changes what downstream crates see, with no changelog:
  - `hive::LevelFilter` is Hive's own enum, no longer a re-export of
    `log::LevelFilter`; `SimpleLogger` and `LOGGER` are gone.
  - `gui` is off by default; `tracing` is the default logging backend.
  - `Hive::properties` is a `PropertyMap`, not `HashMap<u64, Property>`.
  - `SocketEvent::NewPeer.stream` went from `Option<TcpStream>` to
    `Option<Box<Connection>>`, and there are new `Frames` and `MqttText`
    variants.
  - `Handler::set_property(name, None)` deletes instead of panicking.
  - Servers reject connections that don't open with `HVEP`.
  Add a CHANGELOG (or a README section) and decide whether `0.2.201` is
  the right version for it.

## Not planned / notes

- Websocket peers: `Peer::handshake` never constructs a `WebSock`, so the
  `websock` feature is currently dead code. Decide separately whether to
  revive or remove it.
- Bluetooth transports are out of scope for the security work; they rely
  on BLE pairing.
