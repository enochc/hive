# Performance notes

Hive is meant to be a fast, efficient cross-service data communication
layer, that's the top priority whenever touching this crate. This file
tracks concrete inefficiencies spotted in passing while working on other
things. Not a design doc, just a punch list with enough detail to act on,
resolved items get removed rather than kept as a changelog, git history is
the record of what happened.

## Open

- `Property::set_value` still takes the watch channel's write lock per
  update, but since todo.md 5.6 skips its wake-ups (eight `Notify`
  mutexes) while nobody is streaming the property. A stream that is
  cloned and kept, even if never polled, brings that cost back for its
  property.
- Latency with many receivers: each peer's writer task (todo.md 3.1) adds a
  task hand-off per flush, about +25 us p50 at 8 receivers on loopback.
  If it matters, a fast path that writes directly when the peer's outbox is
  empty and the socket is writable would remove it.
- Inbound budgets (todo.md 5.3): a TCP peer's batches each carry a
  `Credit` (one `Arc` clone per read, not per message). `Handler` updates
  are charged per message, but the event loop releases them once per batch:
  a guard per message cost about 20% of 1-receiver throughput. Senders
  resume at half the budget; resuming at the limit woke them once per
  handled message and halved throughput. MQTT ingest is still unbounded.
- A lagging TCP peer holds its own copy of what it hasn't been sent, up to
  256 KiB plus one message (`OUTBOX_LIMIT`). The old queue shared one
  `Bytes` between peers but was bounded in frames, not bytes.
- Numeric peer ids (todo.md 3.5) deferred: peers are still found by
  address string, but the last sender is checked first so the scan only
  runs when the sender changes, and `from` is one `Arc` clone per read.

## Benchmarks

`tests/bench.rs`, run with
`cargo test --release --features tls --test bench -- --ignored --nocapture --test-threads=1`.
Loopback, Apple M2 Pro, 4 tokio workers. Each throughput row is N updates
from one client through the server to the receiving clients; latency is one
update at a time, sender to server to receiver. Ranges are three runs
(four for some rows).

Up to the 3.7 rows N was 50,000 with no warm-up. That finished in 15-35 ms,
short enough that the first run in the process (plain, 1 rx) read 25-40%
low. Since todo.md 5.9 each benchmark does an untimed 200,000-update pass
first and N defaults to 500,000 (`HIVE_BENCH_UPDATES` overrides it). The
"5.9" row is the 3.7 code measured that way, for comparison with what
follows.

| Date | Change | 1 rx updates/s | 8 rx updates/s | p50/p99 1 rx | p50/p99 8 rx |
|---|---|---|---|---|---|
| 2026-11-11 | baseline (bd5f83b) | 17k-35k | 7.0k-7.8k | 83/120 us | 135/200 us |
| 2026-11-11 | phase 3: writer task + batching, forward raw bytes, direct encode | 1.36M-1.52M | 670k-735k | 84-94/135-153 us | 160-165/248-264 us |
| 2026-11-11 | phase 4: boxed transport (plain TCP) | 1.46M-1.62M | 563k-649k | 86-92/151-202 us | 147-153/228-357 us |
| 2026-11-11 | phase 4: TLS 1.3, ring, AES-GCM | 1.39M-1.80M | 642k-653k | 92-100/162-219 us | 151-165/251-318 us |
| 2026-11-11 | 3.7: one event per read, per-peer outbox, event loop flushes per batch (plain TCP) | 1.87M-2.53M | 1.34M-1.70M | 79-86/110-128 us | 138-146/188-225 us |
| 2026-11-11 | 3.7: same, TLS 1.3 | 2.85M-3.66M | 1.56M-1.65M | 81-89/112-132 us | 148-158/199-220 us |
| 2026-11-11 | 5.9: 3.7 code, warm-up + 500,000 updates (plain TCP) | 2.84M-3.55M | 1.71M-1.83M | 78-97/123-288 us | 151-156/220-262 us |
| 2026-11-11 | 5.9: same, TLS 1.3 | 3.84M-4.02M | 1.72M-1.82M | 80-88/117-130 us | 151-156/221-236 us |
| 2026-11-11 | 5.3 + 5.6: inbound budgets, `set_value` skips watch wake-ups with no stream (plain TCP) | 3.46M-3.66M | 2.10M-2.26M | 80-84/121-125 us | 140-148/200-217 us |
| 2026-11-11 | 5.3 + 5.6: same, TLS 1.3 | 3.50M-3.78M | 2.01M-2.11M | 82-83/121-124 us | 148-160/221-552 us |
