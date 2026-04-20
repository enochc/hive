# Hive Hunter - Project Status

**Last updated:** 2026-04-18

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
  - Internal `std::sync::RwLock` for thread-safe signature storage
  - Byte-pattern matching via signature snapshot (no lock held during scan)
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
- [x] Add `hunter` feature gate to `Cargo.toml`
- [x] Add dependencies: `sha2`, `serde`, `serde_json`, `hex`, `tempfile`
- [x] Wire `pub mod hunter` into `src/lib.rs` behind `#[cfg(feature = "hunter")]`
- [x] Compile and tests pass with `cargo test --features hunter`
- [x] **instructions.md** - architecture and implementation guide
- [x] **status.md** - this file

---

## Phase 2: Hive Integration

### Done

- [x] **signature_sync.rs** - Property watcher for signature distribution
  - `register_signature_watchers()` walks existing properties and sets up on_next
  - `register_single_watcher()` for dynamic registration of new signatures
  - `load_signature_from_value()` deserializes JSON and loads/removes from Scanner
  - Uses `Arc<Scanner>` directly (no external Mutex needed thanks to internal RwLock)
  - Unit tests: load valid sig, remove on empty value, invalid JSON resilience
- [x] **node.rs** - HunterNode orchestrator
  - `HunterConfig` with scan paths, interval, max file size, vault dir, version
  - `NodeStatus` struct (serializable to JSON) for health reporting
  - `HunterNode::start()` wires Scanner, Quarantine, SelfUpdater, SignatureSync
  - Registers signature watchers before Hive launch
  - Registers update manifest watcher via on_next
  - Registers scan directive watcher via on_next
  - `process_scan_events()` loop: quarantines threats and publishes records to Hive
  - `periodic_scan_loop()` using `tokio::time::interval()` (no busy wait)
  - Publishes initial `node_status_<name>` property on startup
  - `HunterHandle` for external interaction (trigger_scan, shutdown)
  - Graceful shutdown via CancellationToken in all spawned tasks
  - Unit tests: NodeStatus serialization, default config
- [x] **Scanner refactored** to use internal `std::sync::RwLock`
  - `load_signature()` and `remove_signature()` take `&self` (write lock internally)
  - `scan_file()` and `scan_directory()` snapshot signatures (read lock, then release)
  - Eliminates need for external Mutex wrapping
  - Signature mutations from sync on_next callbacks work cleanly
  - Async scans never hold any lock across await points
- [x] **idle_monitor.rs** - System idle detection for opportunistic scanning
  - `IdleConfig` with threshold, sample interval, consecutive count, max wait
  - Linux: reads /proc/stat for CPU idle/total counters
  - macOS: uses host_processor_info() via FFI for CPU ticks
  - `wait_for_idle()` samples CPU usage, signals when sustainably idle
  - Falls back to fixed interval on unsupported platforms
  - Max-wait deadline prevents indefinite deferral on busy systems
  - Unit tests: usage calculation, fully idle, fully busy, disabled fallback, cancellation
- [x] **Scan directive dispatch** wired via `mpsc` channel
  - Sync on_next callback parses directive JSON and sends `ScanCommand`
  - Async scan loop receives commands and runs on-demand scans
  - No busy wait, no blocking in the sync callback (uses try_send)
- [x] **Update manifest dispatch** wired via `mpsc` channel
  - Sync on_next callback parses manifest and sends to async processor
  - Async processor calls `SelfUpdater::process_manifest()` directly
  - Full download-verify-replace-reexec flow is now connected end to end

### Todo

(Phase 2 complete)

---

## Phase 3: Network and Security

### Done

- [x] **HTTP download** in `self_update.rs`
  - URL-based downloads via `reqwest` (behind `rest` feature)
  - `file://` support retained for local testing
  - Stub when `rest` feature is not enabled (returns descriptive error)
  - Protocol scheme validation (file://, http://, https://)
- [x] **Peer-to-peer binary transfer** path wired
  - `UpdateSource::Peer` documented with file_received signal approach
  - `process_received_binary()` method for processing binary delivered via file transfer
  - Leverages base Hive file transfer protocol (FILE_HEADER/CHUNK/COMPLETE)
- [x] **Ed25519 signature verification** structure
  - `UpdateConfig` gains `require_signature` and `signing_public_key` fields
  - `verify_signature()` validates signature hex, key presence, digest decoding
  - Rejects unsigned manifests when `require_signature` is true
  - Rejects missing public key when signatures are required
  - Crypto call stubbed pending `ed25519-dalek` dependency (logged warning)
  - Unit tests: reject missing sig when required, reject no key when required,
    accept no sig when not required
- [x] **Health check** framework
  - `wait_for_health_check()` standalone function with timeout
  - Designed for orchestrator-side monitoring of updated node status
  - Node's `node_status_*` property publication serves as health signal

### Todo

- [ ] **SSH integration for provisioner**
  - Add `thrussh` or `ssh2` dependency behind `hunter` feature
  - Implement SSH connect + authenticate in `provision_host()`
  - Remote `uname -sm` for platform detection
  - SCP file transfer with progress reporting
  - Remote command execution for install steps
  - Verify new node in Hive peer list via Notify (no polling)
- [ ] **ed25519-dalek integration**
  - Add dependency behind `hunter` feature
  - Replace the stubbed crypto call in `verify_signature()` with real verification
  - Add unit test with a real keypair: sign, verify, reject bad signature

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

## Architectural Decisions

- **Scanner uses internal RwLock** instead of external Mutex wrapping.
  This was driven by the need to call `load_signature()` from sync `on_next`
  callbacks while also calling `scan_directory()` from async tasks.  The
  internal RwLock lets signature mutations take a brief write lock while
  scans snapshot the signatures via a read lock, avoiding any lock held
  across await points.

- **All synchronization** uses channels, `Notify`, and `CancellationToken`.
  No busy-wait loops.  No broadcasts except where structurally required
  by the Hive peer notification system.

---

## Notes

- The scanner uses a simple windowed byte search; production deployments
  should plan for YARA integration (Phase 4) for real-world effectiveness.
- The provisioner scaffolding is complete but SSH operations are stubbed
  out pending an SSH library dependency.
- The Ed25519 signature verification is structurally complete (validation
  logic, config fields, error paths, tests) but the actual crypto call
  requires adding the `ed25519-dalek` crate.
- HTTP downloads require the `rest` feature flag (which pulls in `reqwest`).
  Without it, URL-based updates return a descriptive error.
- Peer-to-peer binary transfer uses the base Hive file transfer protocol.
  The requesting node receives the binary via `file_received` signal and
  passes it to `SelfUpdater::process_received_binary()`.
