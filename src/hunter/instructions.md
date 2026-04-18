# Hive Hunter - Architecture and Implementation Guide

## Overview

Hive Hunter is a distributed threat-hunting agent built on top of the Hive P2P
synchronization library.  Each hunter node is an authorized endpoint agent that
uses Hive's property sync to coordinate with peers, share threat intelligence
in real time, and receive self-updating binary replacements without external
orchestration tooling.

**This is not self-propagating software.**  Every node is intentionally
installed by a system administrator.  Hive handles coordination, not
deployment.  The "virus hunter" concept leverages swarm coordination, not
autonomous replication.


## Project Structure

```
src/hunter/
    mod.rs            - Module declarations
    manifest.rs       - UpdateManifest, Version, UpdateSource types
    self_update.rs    - SelfUpdater service (download, verify, replace, rollback)
    scanner.rs        - Scanner engine (signature loading, file scanning)
    quarantine.rs     - Quarantine vault (isolate, restore, delete)
    provisioner.rs    - Admin-initiated push deployment to network hosts
```

Future additions:
```
src/hunter/
    node.rs           - HunterNode orchestrator (wires everything together)
    signature_sync.rs - Hive property watcher for signature distribution
    report.rs         - Reporting and telemetry aggregation
    peer_transfer.rs  - Binary transfer between peers (for updates)
```


## Core Components

### 1. Update Manifest (`manifest.rs`)

Describes a new binary release.  Serialized to JSON and stored as a Hive
property so every connected node receives it through normal sync.

Key types:
- `Version` - semantic version triple with ordering
- `UpdateSource` - where to fetch the binary (URL or peer)
- `UpdateManifest` - version, SHA-256 hash, source, platform, optional signature

The manifest property name defaults to `hunter_update_manifest`.


### 2. Self-Update Module (`self_update.rs`)

Watches the manifest property and handles the full update lifecycle:

1. **Version check** - skip if manifest version is not newer
2. **Platform check** - skip if manifest targets a different architecture
3. **Download** - fetch binary from URL or peer
4. **Verify** - SHA-256 hash comparison (signature verification planned)
5. **Stage** - write to temp location, set executable permission
6. **Backup** - copy current binary for rollback
7. **Signal** - notify waiters so they can perform cleanup
8. **Replace** - atomic rename over current executable
9. **Re-exec** - Unix: `exec()` replaces process in place; Windows: spawn + exit

Rollback: if the new binary fails a health check within `health_timeout`
(default 30s), the rollback copy is restored.

The module uses `tokio::sync::Notify` to signal update availability rather
than polling or busy-wait loops.


### 3. Scanner Engine (`scanner.rs`)

Pattern-matching engine that loads threat signatures and scans local files.

Signatures are JSON objects distributed as Hive properties:
```json
{
  "id": "EICAR-TEST",
  "pattern_hex": "58354f2150254041...",
  "description": "EICAR standard test file",
  "severity": "low"
}
```

The scanner:
- Converts hex patterns to bytes and does windowed byte matching
- Computes SHA-256 of each scanned file for identification
- Skips files over a configurable size limit (default 50 MB)
- Sends `ScanEvent` messages through a `tokio::sync::mpsc` channel
- Supports single-file and recursive directory scanning

Planned improvements:
- YARA rule support for more expressive pattern matching
- Heuristic analysis (entropy scoring, PE header anomalies)
- Incremental scanning (skip unchanged files using modification time)


### 4. Quarantine Service (`quarantine.rs`)

Isolates suspect files and manages the quarantine vault.

When a threat is detected:
1. Read the file and compute its SHA-256 hash
2. Move it to the vault directory with a timestamped filename
3. Strip execute permissions (Unix: 0o400 read-only)
4. Return a `QuarantineRecord` for publishing to Hive

The record is serialized to JSON and synced as a Hive property so other
nodes learn about the finding.

Operations:
- `quarantine_file()` - isolate a suspect file
- `restore_file()` - return a false positive to its original location
- `delete_file()` - permanently remove a confirmed threat
- `list_quarantined()` - list all files in the vault


### 5. Provisioner (`provisioner.rs`)

Admin-initiated deployment tool for pushing hunter nodes to machines on the
local network.  This is NOT self-propagation; it requires the admin to have
SSH credentials for every target machine.

The provisioner operates in stages:

**Discovery phase:**
- Scans a configurable subnet (e.g. `192.168.1.0/24`) for reachable hosts
- Probes SSH (port 22) and the hunter port to classify each host
- Concurrent TCP probes with configurable timeout
- Results reported through a `tokio::sync::mpsc` channel
- Admin can also provide explicit target IPs, bypassing the scan

**Deployment phase (per host):**
1. SSH connect and authenticate (key file, password, or agent)
2. Detect remote OS and architecture via `uname -sm`
3. Select the correct cross-compiled binary for that platform
4. SCP the binary and a generated bootstrap TOML config
5. Run remote install commands (create dirs, set permissions, register service)
6. Wait for the new node to appear in the Hive peer list (Notify, no polling)

**Service registration:**
- Generates systemd unit files for Linux targets
- Generates launchd plists for macOS targets
- Both include security hardening (NoNewPrivileges, ProtectSystem, etc.)

**Bootstrap config:**
The provisioner generates a TOML config for each new node containing:
- A unique node name
- The `connect` address pointing back to an existing Hive peer
- Default hunter settings (vault dir, scan paths, scan interval)

**Security boundaries:**
- Admin must already have SSH access to every target
- Credentials are held in memory only, never persisted
- The provisioner is an interactive tool, not an autonomous service
- Each target machine must be under the admin's administrative control


## Hive Integration Points

### Properties Used

| Property Name             | Purpose                                       |
|---------------------------|-----------------------------------------------|
| `hunter_update_manifest`  | Binary update manifest (JSON)                 |
| `threat_sig_<ID>`         | Individual threat signatures (JSON)           |
| `node_status_<n>`      | Node health, version, last scan time (JSON)   |
| `quarantine_<HASH>`       | Quarantine records (JSON)                     |
| `scan_directive`          | Orchestrator commands (scan path, schedule)    |

### Using the Handler

The `Handler` struct drives all outbound communication:

```rust
// Push a new signature to all peers
handler.set_property("threat_sig_EICAR", Some(&sig_json.into())).await;

// Push an update manifest
handler.set_property("hunter_update_manifest", Some(&manifest_json.into())).await;

// Report a quarantine event
handler.set_property("quarantine_abc123", Some(&record_json.into())).await;
```

### Watching Properties with PropertyStream

Each hunter node subscribes to property changes using the existing
`PropertyStream` mechanism:

```rust
let prop = hive.get_mut_property_by_name("hunter_update_manifest").unwrap();
let mut stream = prop.stream.clone();

tokio::spawn(async move {
    while let Some(val) = stream.next().await {
        let json_str = val.val.as_str().unwrap();
        let manifest = UpdateManifest::from_json(json_str).unwrap();
        updater.process_manifest(&manifest).await;
    }
});
```


## Cargo Dependencies

New dependencies needed (add behind a `hunter` feature gate):

```toml
[features]
hunter = ["sha2", "serde", "serde_json", "hex", "chrono"]

[dependencies]
sha2 = { version = "0.10", optional = true }
serde = { version = "1", features = ["derive"], optional = true }
serde_json = { version = "1", optional = true }
hex = { version = "0.4", optional = true }
chrono = { version = "0.4", features = ["serde"], optional = true }

[dev-dependencies]
tempfile = "3"
```

Note: `chrono` is already a non-optional dependency.  When adding the feature
gate, keep the existing usage and add `serde` support for the hunter module.

For the provisioner's SSH integration (Phase 3), add:
```toml
thrussh = { version = "0.35", optional = true }
thrussh-keys = { version = "0.23", optional = true }
```
Or alternatively `ssh2` (libssh2 bindings) if you prefer a C-backed SSH
implementation over a pure-Rust one.


## Security Model

1. **No self-propagation** - nodes are installed by administrators only
2. **Admin-initiated provisioning** - the provisioner requires SSH credentials
   and interactive approval for each target machine
3. **Signed binaries** - update manifests include an optional Ed25519 signature
   over the SHA-256 digest; nodes verify against a built-in public key
4. **Hash verification** - every downloaded binary is SHA-256 checked before
   staging
5. **Rollback safety** - the previous binary is preserved and automatically
   restored if the new one fails health checks
6. **Vault permissions** - quarantine directory is restricted to owner-only
   access (0o700)
7. **Platform filtering** - nodes ignore manifests targeting other architectures
8. **Credential hygiene** - provisioner holds SSH credentials in memory only,
   never written to disk


## Testing

Each module has unit tests covering the core logic.  Run with:

```sh
cargo test --features hunter
```

Integration tests that exercise the full Hive sync path (property change
triggers scan, scan triggers quarantine, quarantine record syncs to peers)
will be added in `tests/hunter_integration.rs`.


## Build and Run

```sh
# Build with hunter support
cargo build --features hunter

# Run a hunter node with a config file
cargo run --features hunter -- examples/hunter_node.toml
```

Example TOML config for a hunter node:

```toml
name = "hunter-alpha"
listen = "9100"

[Properties]
hunter_update_manifest = ""
scan_directive = ""

[Hunter]
vault_dir = "/var/lib/hunter/quarantine"
scan_paths = ["/home", "/tmp", "/var/www"]
scan_interval_secs = 3600
max_file_size_mb = 100
```

## Provisioner Usage

The provisioner is a CLI tool the admin runs from a trusted workstation:

```sh
# Scan local network and show discovered hosts
cargo run --features hunter --bin hive-provision -- scan 192.168.1.0/24

# Deploy to a specific host
cargo run --features hunter --bin hive-provision -- deploy \
    --target 192.168.1.50 \
    --name hunter-bravo \
    --key ~/.ssh/id_ed25519 \
    --hub 192.168.1.100:9100
```

The provisioner will:
1. SSH into the target machine
2. Detect the platform (OS + arch)
3. Transfer the correct hunter binary
4. Write a bootstrap TOML config pointing back to the hub
5. Register and start the systemd/launchd service
6. Confirm the new node appears in the Hive peer list
