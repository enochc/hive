# Hive Core - Enhancement Status

**Last updated:** 2026-04-18

Tracks planned improvements to the base Hive architecture that benefit
all consumers (Hunter, IoT sync, general P2P applications).

---

## 1. File Transfer Protocol

**Status:** Todo

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

**Status:** Todo

When a node's IP address changes (DHCP renewal, network switch, VPN
connect/disconnect), its connected peers hold a stale address and can't
reconnect.  The node should detect the change and notify its direct peers.

### Design

- On a configurable interval (default: 60s), compare the current local IP
  (`local_ipaddress::get()`) against the last known address.
- When a change is detected:
  1. Send an `ADDRESS_UPDATE` message to all connected peers with the new address
  2. If the node is a server (listening), rebind the listener on the new address
  3. If the node is a client, attempt to reconnect to its upstream peer

New message type:

| Constant          | Value  | Purpose                                       |
|-------------------|--------|-----------------------------------------------|
| `ADDRESS_UPDATE`  | `0x67` | Notify peers of an IP address change          |

**ADDRESS_UPDATE layout:**
```
[ADDRESS_UPDATE: u8]
[addr_len: u16]          -- length of the new address string
[new_address: [u8]]      -- UTF-8 "ip:port" string
```

When a peer receives an ADDRESS_UPDATE, it updates the stored address for
that peer in its peer list.  If the connection was dropped, it can attempt
to reconnect using the new address.

### Important
- Only run address detection logic if the bound address is not an internal address: ie: 192.168

### Tasks

- [ ] Add `ADDRESS_UPDATE` constant (0x67) to `hive.rs`
- [ ] Add IP monitoring task in `Hive::run()` using `tokio::time::interval`
- [ ] Detect IP change by comparing `local_ipaddress::get()` against stored value
- [ ] Send ADDRESS_UPDATE to all connected peers on change
- [ ] Handle ADDRESS_UPDATE in `got_message()`: update peer address
- [ ] For server nodes: rebind listener on new address
- [ ] For client nodes: reconnect to upstream if connection was lost
- [ ] Store last known IP in Hive struct for comparison
- [ ] Unit test: simulate IP change, verify peers receive update
- [ ] Unit test: verify peer address is updated after receiving ADDRESS_UPDATE

---

## 3. Property Filters (Include/Exclude Lists)

**Status:** Todo

Not every peer needs every property.  A hunter node might only care about
`threat_sig_*` and `hunter_update_manifest`, while an IoT thermostat only
needs `temperature` and `target_temp`.  Property filters reduce traffic by
letting peers declare which properties they want.

### Design

Filters are exchanged during the handshake or via a new header type.  Each
peer can specify either:

- **Include list**: only send properties whose names match these prefixes
- **Exclude list**: send all properties except those matching these prefixes

Include and exclude are mutually exclusive.  If neither is set, the peer
receives everything (current default behavior).

New header byte for the handshake/header exchange:

| Constant              | Value  | Purpose                                  |
|-----------------------|--------|------------------------------------------|
| `HEADER_PROP_INCLUDE` | `0x79` | Property include filter (prefix list)    |
| `HEADER_PROP_EXCLUDE` | `0x7A` | Property exclude filter (prefix list)    |

**Filter layout (in header):**
```
[HEADER_PROP_INCLUDE or HEADER_PROP_EXCLUDE: u8]
[count: u16]             -- number of filter entries
For each entry:
  [prefix_len: u16]      -- length of prefix string
  [prefix: [u8]]         -- UTF-8 property name prefix
```

### Peer struct changes

```rust
pub struct Peer {
    // ... existing fields ...

    /// Property filter mode for this peer.
    property_filter: PropertyFilter,
}

pub enum PropertyFilter {
    /// Send all properties (default, current behavior).
    All,
    /// Only send properties whose names start with one of these prefixes.
    Include(Vec<String>),
    /// Send all properties except those whose names start with these prefixes.
    Exclude(Vec<String>),
}
```

### Filtering points

1. **`send_properties()`** -- when sending the initial property dump to a
   newly connected peer, skip properties that don't match the filter.

2. **`broadcast()`** -- when broadcasting a property change to all peers,
   check each peer's filter before sending.

3. **Property name lookup** -- the filter operates on property names, but
   properties arrive on the wire with only their u64 hash ID.  The filter
   must be checked in `send_properties()` and `broadcast()` where the
   Property struct (with its name) is available.  For properties that arrive
   by ID only (no name), the filter cannot be applied until the name is
   resolved.

### TOML config

```toml
name = "hunter-alpha"
connect = "192.168.1.100:9100"

[Filters]
include = ["threat_sig_", "hunter_update_manifest", "scan_directive"]
```

Or for exclusion:

```toml
[Filters]
exclude = ["debug_", "internal_"]
```

### Tasks

- [ ] Add `PropertyFilter` enum to `peer.rs`
- [ ] Add `property_filter` field to `Peer` struct (default: `All`)
- [ ] Add `HEADER_PROP_INCLUDE` (0x79) and `HEADER_PROP_EXCLUDE` (0x7A) constants
- [ ] Parse filter headers during handshake in `Peer::handshake()`
- [ ] Send filter headers during handshake (client side)
- [ ] Parse `[Filters]` section from TOML config in `Hive::new_from_str()`
- [ ] Add `should_send_to_peer(property, peer)` helper method
- [ ] Apply filter in `send_properties()` during initial sync
- [ ] Apply filter in `broadcast()` for individual property changes
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

- All new message types should be backward compatible: older Hive versions
  that don't recognize the new byte will hit the `unimplemented!()` arm in
  `got_message()`.  This should be changed to a `warn!()` + skip for
  graceful degradation.
- The `unimplemented!()` in the message dispatch match is a crash risk in
  production.  As part of this work, it should be replaced with a logged
  warning and message skip.
- The `local_ipaddress` crate is already a dependency, so IP detection
  adds no new crate.
- Property filters interact with the MQTT peer: the MQTT topic mapping
  should respect the same filter rules if a filter is configured.
