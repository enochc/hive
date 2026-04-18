# Hive Core - Enhancement Status

**Last updated:** 2026-04-18

Tracks planned improvements to the base Hive architecture that benefit
all consumers (Hunter, IoT sync, general P2P applications).

---

## 1. File Transfer Protocol

**Status:** Complete

Large binary payloads (firmware images, hunter binaries, signature databases)
need a chunked transfer mechanism.  The current wire protocol sends each
message as a single `[u32 length][payload]` frame, which works for properties
(typically < 1 KB) but is not suitable for multi-megabyte files.

### Tasks

- [x] Add `FILE_HEADER`, `FILE_CHUNK`, `FILE_COMPLETE`, `FILE_CANCEL` constants to `hive.rs`
- [x] Define `FileTransferManager` to track in-progress transfers
- [x] Implement sender-side chunking in `Handler` (`send_file()`, `send_raw()`)
- [x] Implement receiver-side reassembly in `Hive::got_message()`
- [x] Add `file_received` Signal on Hive struct (emitted on FILE_COMPLETE)
- [x] SHA-256 verification on completion
- [x] Timeout handling for stalled transfers (`cleanup_stale()`)
- [x] FILE_CANCEL for graceful abort
- [x] Replace `unimplemented!()` crash on unknown message types with `warn!()`
- [x] Make `sha2` a non-optional core dependency (file_transfer is not feature-gated)
- [x] Unit test: encode/decode roundtrip (header + chunks + complete)
- [x] Unit test: hash mismatch detection
- [x] Unit test: cancel mid-transfer
- [x] Unit test: timeout cleanup
- [x] Unit test: duplicate chunk ignored
- [x] Integration test: multi-chunk transfer across TCP connection between two Hive instances
  - Test in `tests/file_transfer.rs`: server with `file_received` callback, client sends 10KB payload
  - Verifies filename, data integrity, and callback firing
- [x] Fixed port collision between `middle_man.rs` and `streams.rs` (3000 -> 3100/3101)

---

## 2. IP Address Change Detection

**Status:** In Progress

When a node's IP address changes (DHCP renewal, network switch, VPN
connect/disconnect), its connected peers hold a stale address and can't
reconnect.  The node should detect the change and notify its direct peers.

### Important

Only run address detection logic if the bound address is not an internal
address (i.e. 192.168.x.x, 10.x.x.x, 172.16-31.x.x).  Internal addresses
are typically stable within a LAN and monitoring them creates noise without
value.

### Tasks

- [x] Add `ADDRESS_UPDATE` constant (0x67) to `hive.rs`
- [x] Add IP monitoring task in `Hive::run()` using `tokio::time::interval` (60s)
- [x] Detect IP change by comparing `local_ipaddress::get()` against stored value
- [x] Send ADDRESS_UPDATE to all connected peers on change (via event loop)
- [x] Handle ADDRESS_UPDATE in `got_message()`: update peer address (preserves port)
- [x] Graceful shutdown via CancellationToken
- [ ] Skip monitoring for private/internal IP ranges (192.168, 10.x, 172.16-31)
- [ ] For server nodes: rebind listener on new address
- [ ] For client nodes: reconnect to upstream if connection was lost
- [ ] Unit test: simulate IP change, verify peers receive update
- [ ] Unit test: verify peer address is updated after receiving ADDRESS_UPDATE

---

## 3. Property Filters (Include/Exclude Lists)

**Status:** In Progress

Not every peer needs every property.  A hunter node might only care about
`threat_sig_*` and `hunter_update_manifest`, while an IoT thermostat only
needs `temperature` and `target_temp`.  Property filters reduce traffic by
letting peers declare which properties they want.

### Tasks

- [x] Add `PropertyFilter` enum to `peer.rs` (All, Include, Exclude)
- [x] Add `property_filter` field to `Peer` struct (default: `All`)
- [x] Add `should_send()` method on `PropertyFilter` for prefix matching
- [x] Apply filter in `send_properties()` during initial sync
- [x] Apply filter in `broadcast()` for individual property changes
- [ ] Add `HEADER_PROP_INCLUDE` (0x79) and `HEADER_PROP_EXCLUDE` (0x7A) constants
- [ ] Parse filter headers during handshake in `Peer::handshake()`
- [ ] Send filter headers during handshake (client side)
- [ ] Parse `[Filters]` section from TOML config in `Hive::new_from_str()`
- [ ] Unit test: include filter blocks unmatched properties
- [ ] Unit test: exclude filter blocks matched properties
- [ ] Unit test: no filter sends everything (backward compatible)
- [ ] Integration test: two nodes with filters, verify only matching properties sync

---

## 4. Potential Future Enhancements (Not Yet Planned)

- **Property namespaces**: formalize the prefix convention (e.g. `hunter/`,
  `iot/`) with first-class support in the protocol
- **Compression**: optional zlib/lz4 compression for property values and
  file transfers over slow links
- **Encryption**: TLS wrapping for TCP connections, or application-level
  encryption for property values
- **Peer authentication**: challenge-response during handshake to prevent
  unauthorized peers from joining the network
- **Rate limiting**: configurable limits on property update frequency per
  peer to prevent flooding

---

## Notes

- All new message types are backward compatible: older Hive versions that
  don't recognize the new bytes now get a `warn!()` log instead of a crash
  (the `unimplemented!()` was replaced).
- The `local_ipaddress` crate is already a dependency, so IP detection
  adds no new crate.
- Property filters interact with the MQTT peer: the MQTT topic mapping
  should respect the same filter rules if a filter is configured.
- The `Signal` type now has an explicit `new()` constructor so it works
  with types that don't implement `Default` (like `ReceivedFile`).
