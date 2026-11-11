# Performance notes

Hive is meant to be a fast, efficient cross-service data communication
layer, that's the top priority whenever touching this crate. This file
tracks concrete inefficiencies spotted in passing while working on other
things. Not a design doc, just a punch list with enough detail to act on,
resolved items get removed rather than kept as a changelog, git history is
the record of what happened.

## Open

- `Property::set_value` is now the largest single cost per message: the
  `tokio::sync::watch` send takes a write lock and then eight `Notify`
  mutexes whether or not anyone subscribed to the stream. About 13% of
  worker CPU at 1 receiver and 22% at 8 (profile of todo.md 3.7). Fixing it
  means `PropertyStream` keeping its value somewhere cheaper and only
  touching the watch channel once a stream has been handed out.
- Latency with many receivers: each peer's writer task (todo.md 3.1) adds a
  task hand-off per flush, about +25 us p50 at 8 receivers on loopback.
  If it matters, a fast path that writes directly when the peer's outbox is
  empty and the socket is writable would remove it.
- No backpressure on the way in: `read_loop` and `Handler` push into an
  unbounded event channel, so a producer that outruns the event loop grows
  memory without limit. Each queued local update is one `SocketEvent` (80
  bytes, down from 200 now `NewPeer`'s connection is boxed) plus its
  payload.
- A lagging TCP peer holds its own copy of what it hasn't been sent, up to
  256 KiB plus one message (`OUTBOX_LIMIT`). The old queue shared one
  `Bytes` between peers but was bounded in frames, not bytes.
- Numeric peer ids (todo.md 3.5) deferred: peers are still found by
  address string, but the last sender is checked first so the scan only
  runs when the sender changes, and `from` is one `Arc` clone per read.

## Benchmarks

`tests/bench.rs`, run with
`cargo test --release --test bench -- --ignored --nocapture --test-threads=1`.
Loopback, Apple M2 Pro, 4 tokio workers. Each throughput row is 50,000
updates from one client through the server to N receiving clients; latency
is one update at a time, sender to server to receiver. Ranges are three runs
(four for the last two rows).

50,000 updates now take 15-35 ms, short enough that warm-up shows: the
plain 1 rx run is the first thing the process does and reads low.
`HIVE_BENCH_UPDATES=500000` gives steadier numbers: 2.9M-3.5M (1 rx) and
1.73M-1.77M (8 rx) for the last plain row, against 1.42M and 0.69M for
phase 4 plain at 1,000,000.

| Date | Change | 1 rx updates/s | 8 rx updates/s | p50/p99 1 rx | p50/p99 8 rx |
|---|---|---|---|---|---|
| 2026-11-11 | baseline (bd5f83b) | 17k-35k | 7.0k-7.8k | 83/120 us | 135/200 us |
| 2026-11-11 | phase 3: writer task + batching, forward raw bytes, direct encode | 1.36M-1.52M | 670k-735k | 84-94/135-153 us | 160-165/248-264 us |
| 2026-11-11 | phase 4: boxed transport (plain TCP) | 1.46M-1.62M | 563k-649k | 86-92/151-202 us | 147-153/228-357 us |
| 2026-11-11 | phase 4: TLS 1.3, ring, AES-GCM | 1.39M-1.80M | 642k-653k | 92-100/162-219 us | 151-165/251-318 us |
| 2026-11-11 | 3.7: one event per read, per-peer outbox, event loop flushes per batch (plain TCP) | 1.87M-2.53M | 1.34M-1.70M | 79-86/110-128 us | 138-146/188-225 us |
| 2026-11-11 | 3.7: same, TLS 1.3 | 2.85M-3.66M | 1.56M-1.65M | 81-89/112-132 us | 148-158/199-220 us |
