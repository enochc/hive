# Performance notes

Hive is meant to be a fast, efficient cross-service data communication
layer, that's the top priority whenever touching this crate. This file
tracks concrete inefficiencies spotted in passing while working on other
things. Not a design doc, just a punch list with enough detail to act on,
resolved items get removed rather than kept as a changelog, git history is
the record of what happened.

## Open

- Latency with many receivers: each peer's writer task (todo.md 3.1) adds a
  task hand-off per message, about +25 us p50 at 8 receivers on loopback.
  If it matters, a fast path that writes directly when the peer's queue is
  empty and the socket is writable would remove it.
- Numeric peer ids (todo.md 3.5) deferred: one small `String` per received
  message plus a short linear peer scan, small next to the numbers below.

## Benchmarks

`tests/bench.rs`, run with
`cargo test --release --test bench -- --ignored --nocapture --test-threads=1`.
Loopback, Apple M2 Pro, 4 tokio workers. Each throughput row is 50,000
updates from one client through the server to N receiving clients; latency
is one update at a time, sender to server to receiver. Ranges are three runs.

| Date | Change | 1 rx updates/s | 8 rx updates/s | p50/p99 1 rx | p50/p99 8 rx |
|---|---|---|---|---|---|
| 2026-11-11 | baseline (bd5f83b) | 17k-35k | 7.0k-7.8k | 83/120 us | 135/200 us |
| 2026-11-11 | phase 3: writer task + batching, forward raw bytes, direct encode | 1.36M-1.52M | 670k-735k | 84-94/135-153 us | 160-165/248-264 us |
| 2026-11-11 | phase 4: boxed transport (plain TCP) | 1.46M-1.62M | 563k-649k | 86-92/151-202 us | 147-153/228-357 us |
| 2026-11-11 | phase 4: TLS 1.3, ring, AES-GCM | 1.39M-1.80M | 642k-653k | 92-100/162-219 us | 151-165/251-318 us |
