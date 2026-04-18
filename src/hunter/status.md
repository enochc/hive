# Hive Hunter - Project Status

**Last updated:** 2026-04-17

---

## Phase 1: Core Module Scaffolding

### Done

- [x] Create `src/hunter/` module directory and `mod.rs`
- [x] **manifest.rs** - `Version`, `UpdateSource`, `UpdateManifest` types
  - Version ordering with `is_newer_than()`
  - JSON serialization round-trip
  - Unit tests for version comparison and JSON round-trip
- [x] **self_update.rs** - `SelfUpdater` service
  - `UpdateConfig` with sensible defaults
  - Platform detection via `std::env::consts`
  - `file://` download support for local testing
  - SHA-256 hash verification
  - Binary staging with temp file + atomic rename
  - Backup of current binary for rollback
  - `replace_and_reexec()` with Unix exec() and Windows spawn+exit
  - `rollback()` to restore previous binary
  - `process_manifest()` orchestrating the full flow
  - `tokio::sync::Notify` for pre-update signaling
  - Unit tests: skip older version, skip wrong platform, hash mismatch
- [x] **scanner.rs** - `Scanner` engine
  - `ThreatSignature` type with hex pattern decoding
  - Byte-pattern matching (windowed search)
  - SHA-256 hashing of scanned files
  - Configurable max file size
  - `scan_file()` single file scanning
  - `scan_directory()` recursive tree walk
  - `ScanEvent` channel for reporting results
  - Unit tests: pattern matching, hex decode, file scan with detection
- [x] **quarantine.rs** - `Quarantine` vault
  - Vault directory initialization with restricted permissions
  - `quarantine_file()` with cross-device fallback (copy+delete)
  - Execute permission stripping on quarantined files
  - `QuarantineRecord` with timestamps, hashes, signature matches
  - `restore_file()` to undo false positives
  - `delete_file()` for confirmed threats
  - `list_quarantined()` vault inventory
  - Unit tests: quarantine+restore, quarantine+delete
- [x] **provisioner.rs** - Admin-initiated push deployment
  - `TargetHost` type with OS/arch/reachability tracking
  - `ProvisionConfig` with subnet, credentials, binary map
  - `SshCredential` enum (key file, password, agent)
  - `scan_subnet()` with concurrent TCP probes via spawned tasks
  - `ProvisionEvent` channel for progress reporting
  - `generate_bootstrap_config()` TOML generation with connect-back address
  - `generate_systemd_unit()` with security hardening
  - `generate_launchd_plist()` for macOS targets
  - `binary_for_target()` platform-to-binary selection
  - `provision_host()` orchestrating the full deploy flow (SSH stubs for Phase 3)
  - `parse_subnet()` helper for CIDR notation
  - Unit tests: subnet parsing, config generation, systemd/launchd generation
- [x] **instructions.md** - architecture and implementation guide
- [x] **status.md** - this file

### Todo

- [ ] Add `hunter` feature gate to `Cargo.toml`
- [ ] Add new dependencies: `sha2`, `serde`, `serde_json`, `hex`, `tempfile`
- [ ] Wire `pub mod hunter` into `src/lib.rs` behind `#[cfg(feature = "hunter")]`
- [ ] Verify all modules compile with `cargo check --features hunter`
- [ ] Run unit tests with `cargo test --features hunter`

---

## Phase 2: Hive Integration

### Done

(nothing yet)

### Todo

- [ ] **signature_sync.rs** - Property watcher for signature distribution
  - Watch `threat_sig_*` properties via PropertyStream
  - Deserialize incoming signatures and load into Scanner
  - Handle signature removal (property set to None)
- [ ] **node.rs** - HunterNode orchestrator
  - Wire Scanner, Quarantine, and SelfUpdater together
  - Start Hive instance with hunter-specific properties
  - Subscribe to `scan_directive` property for remote commands
  - Publish `node_status_<n>` property with health info
  - Publish quarantine records as `quarantine_<hash>` properties
- [ ] **PropertyStream subscription** for `hunter_update_manifest`
  - On change, deserialize manifest and call `SelfUpdater::process_manifest()`
- [ ] **Scan scheduling** - periodic scan based on config interval
  - Use `tokio::time::interval()`, not a busy loop
  - Respect `scan_directive` overrides from orchestrator

---

## Phase 3: Network and Security

### Done

(nothing yet)

### Todo

- [ ] **SSH integration for provisioner**
  - Add `thrussh` or `ssh2` dependency behind `hunter` feature
  - Implement SSH connect + authenticate in `provision_host()`
  - Remote `uname -sm` for platform detection
  - SCP file transfer with progress reporting
  - Remote command execution for install steps
  - Verify new node in Hive peer list via Notify (no polling)
- [ ] **peer_transfer.rs** - Binary transfer between peers
  - Define a new message type for binary chunk streaming
  - Sender: read binary, chunk into sized pieces, send via peer
  - Receiver: reassemble chunks, verify hash, stage binary
  - Update `UpdateSource::Peer` handling in `self_update.rs`
- [ ] **Signature verification** - Ed25519 signing for update manifests
  - Add `ed25519-dalek` dependency behind `hunter` feature
  - Embed a public key at compile time (or load from config)
  - Verify `manifest.signature` in `process_manifest()` before staging
  - Reject unsigned manifests when `require_signature` config is true
- [ ] **HTTP download** in `self_update.rs`
  - Use `reqwest` (already an optional dep) for URL-based downloads
  - Support HTTPS with certificate validation
  - Progress reporting via a channel or callback
- [ ] **Health check** after binary replacement
  - New binary sends a "healthy" property update within `health_timeout`
  - If timeout expires without the property, trigger rollback
  - Use `tokio::sync::Notify` or `tokio::time::timeout` (no busy wait)

---

## Phase 4: Advanced Scanning

### Done

(nothing yet)

### Todo

- [ ] **YARA rule support** - more expressive pattern matching
  - Integrate `yara-rust` crate
  - Load YARA rules from Hive properties or local files
  - Use YARA scanner alongside byte-pattern scanner
- [ ] **Heuristic analysis**
  - Entropy scoring (high entropy suggests packed/encrypted payloads)
  - PE header anomaly detection (Windows executables)
  - ELF header validation (Linux binaries)
- [ ] **Incremental scanning**
  - Track file modification times in a local database
  - Skip files unchanged since last scan
  - Use inotify/FSEvents/ReadDirectoryChanges for real-time monitoring
- [ ] **report.rs** - Reporting and telemetry
  - Aggregate scan results across nodes via Hive properties
  - Generate summary reports (threats found, files scanned, node health)
  - Configurable reporting interval

---

## Phase 5: Production Hardening

### Done

(nothing yet)

### Todo

- [ ] **Integration tests** - `tests/hunter_integration.rs`
  - Two Hive nodes, one pushes a signature, other detects and quarantines
  - Update manifest triggers self-update on a test node
  - Rollback test: staged binary with bad hash triggers rollback
  - Provisioner subnet scan integration test (loopback network)
- [ ] **Provisioner CLI binary** - `src/bin/hive_provision.rs`
  - Argument parsing (clap) for scan/deploy subcommands
  - Interactive host selection after subnet scan
  - Progress display during deployment
- [ ] **Example config** - `examples/hunter_node.toml`
  - Documented TOML with all hunter-specific options
- [ ] **Logging and observability**
  - Structured logging for all scanner and quarantine events
  - Metrics: scan duration, files/sec, threats/scan, update latency
- [ ] **Cross-platform testing**
  - Verify quarantine permissions on macOS, Linux, Windows
  - Verify self-update re-exec on each platform
  - Verify scanner handles symlinks and hard links correctly
  - Verify provisioner generates correct service files per platform
- [ ] **Documentation**
  - README section for Hive Hunter
  - Deployment guide for setting up a hunter network
  - Provisioner usage guide with examples
  - API docs for all public types and methods

---

## Notes

- The self-update module currently supports `file://` URLs only; HTTP
  downloads need the `reqwest` integration (Phase 3).
- Signature verification (Ed25519) is defined in the manifest struct but
  not yet enforced; the verification logic is a Phase 3 task.
- The scanner uses a simple windowed byte search; production deployments
  should plan for YARA integration (Phase 4) for real-world effectiveness.
- The provisioner scaffolding is complete but SSH operations are stubbed
  out pending an SSH library dependency (Phase 3).  Config generation,
  service file generation, subnet scanning, and the overall orchestration
  flow are all functional.
- All synchronization uses channels and `Notify` primitives, consistent
  with the project's preference against busy-wait loops and broadcasts.
