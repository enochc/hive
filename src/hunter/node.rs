/// Hunter node orchestrator.
///
/// Wires Scanner, Quarantine, SelfUpdater, and SignatureSync together
/// around a Hive instance to form a complete hunter node.  The node:
///
/// - Starts a Hive instance with hunter-specific properties
/// - Registers signature watchers so incoming signatures flow into the scanner
/// - Subscribes to update manifests for self-replacement
/// - Subscribes to scan directives for remote-triggered scans
/// - Waits for system idle before running periodic scans
/// - Publishes scan results and quarantine records back to Hive
/// - Reports node status (version, health, last scan time)
///
/// All coordination uses channels, Notify, and PropertyStream callbacks.
/// No busy-wait loops.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn, error};

use crate::handler::Handler;
use crate::hive::Hive;
use crate::property::PropertyValue;
use crate::hunter::idle_monitor::{self, IdleConfig};
use crate::hunter::manifest::{UpdateManifest, Version};
use crate::hunter::quarantine::Quarantine;
use crate::hunter::scanner::{ScanEvent, Scanner};
use crate::hunter::self_update::{SelfUpdater, UpdateConfig};
use crate::hunter::signature_sync;

/// Configuration for a hunter node, typically parsed from the [Hunter]
/// section of the TOML config.
#[derive(Clone, Debug)]
pub struct HunterConfig {
    /// Paths to scan for threats.
    pub scan_paths: Vec<PathBuf>,
    /// How often to run a full scan (in seconds).  0 = no periodic scan.
    pub scan_interval_secs: u64,
    /// Maximum file size to scan (in bytes).
    pub max_file_size: u64,
    /// Directory for the quarantine vault.
    pub vault_dir: PathBuf,
    /// The version of the currently running hunter binary.
    pub current_version: Version,
    /// Prefix for signature property names.
    pub signature_prefix: String,
    /// Idle detection configuration for opportunistic scanning.
    pub idle_config: IdleConfig,
}

impl Default for HunterConfig {
    fn default() -> Self {
        Self {
            scan_paths: vec![],
            scan_interval_secs: 3600,
            max_file_size: 50 * 1024 * 1024,
            vault_dir: PathBuf::from("/var/lib/hunter/quarantine"),
            current_version: Version::new(0, 1, 0),
            signature_prefix: signature_sync::DEFAULT_SIG_PREFIX.into(),
            idle_config: IdleConfig::default(),
        }
    }
}

/// Node status published as a Hive property so the orchestrator and
/// other nodes can monitor health.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_name: String,
    pub version: String,
    pub platform: String,
    pub last_scan_time: Option<String>,
    pub files_scanned: u64,
    pub threats_found: u64,
    pub signatures_loaded: usize,
    pub quarantine_count: usize,
    pub status: String,
}

/// Commands that can be sent to the scan loop from sync callbacks.
#[derive(Debug)]
enum ScanCommand {
    /// Scan a specific path immediately (from scan_directive property).
    ScanPath(PathBuf),
}

/// The hunter node.
///
/// Call `HunterNode::start()` to build and launch the node.  It returns
/// a handle that can be used to trigger scans or shut down the node.
pub struct HunterNode {
    config: HunterConfig,
    handler: Handler,
    scanner: Arc<Scanner>,
    quarantine: Arc<Quarantine>,
    scan_event_rx: mpsc::Receiver<ScanEvent>,
    cancellation_token: CancellationToken,
    node_name: String,
}

/// Handle returned to the caller after starting a hunter node.
/// Provides methods to interact with the running node.
pub struct HunterHandle {
    handler: Handler,
    node_name: String,
    cancellation_token: CancellationToken,
}

impl HunterHandle {
    /// Trigger an immediate scan by setting the scan_directive property.
    pub async fn trigger_scan(&mut self, path: &str) {
        let directive = serde_json::json!({
            "action": "scan",
            "path": path,
            "requested_at": Utc::now().to_rfc3339(),
        });
        let val = PropertyValue {
            val: toml::Value::String(directive.to_string()),
        };
        self.handler.set_property("scan_directive", Some(&val)).await;
    }

    /// Get the node name.
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Signal the node to shut down gracefully.
    pub fn shutdown(&self) {
        info!("shutdown requested for node '{}'", self.node_name);
        self.cancellation_token.cancel();
    }
}

impl HunterNode {
    /// Build and start a hunter node.
    ///
    /// This:
    /// 1. Creates the Scanner, Quarantine, and SelfUpdater
    /// 2. Registers signature watchers on the Hive instance
    /// 3. Registers the update manifest watcher (dispatches via channel)
    /// 4. Registers the scan directive watcher (dispatches via channel)
    /// 5. Launches the Hive instance
    /// 6. Starts the scan event processor
    /// 7. Starts the idle-aware periodic scan loop
    /// 8. Starts the update manifest processor
    /// 9. Publishes initial node status
    ///
    /// Returns a HunterHandle for external interaction.
    pub async fn start(
        mut hive: Hive,
        config: HunterConfig,
        cancellation_token: CancellationToken,
    ) -> HunterHandle {
        let node_name = hive.name.clone();
        info!("starting hunter node '{}'", node_name);

        // Create the scan event channel
        let (scan_tx, scan_rx) = mpsc::channel::<ScanEvent>(256);

        // Channel for scan directives (sync on_next -> async scan loop)
        let (cmd_tx, cmd_rx) = mpsc::channel::<ScanCommand>(32);

        // Channel for update manifests (sync on_next -> async updater)
        let (manifest_tx, manifest_rx) = mpsc::channel::<UpdateManifest>(8);

        // Build the scanner
        let mut scanner = Scanner::new(scan_tx);
        scanner.set_max_file_size(config.max_file_size);
        let scanner = Arc::new(scanner);

        // Build the quarantine vault
        let quarantine = Arc::new(Quarantine::new(
            config.vault_dir.clone(),
            node_name.clone(),
        ));
        if let Err(e) = quarantine.initialize().await {
            error!("failed to initialize quarantine vault: {}", e);
        }

        // Register signature watchers on existing properties
        signature_sync::register_signature_watchers(
            &mut hive,
            scanner.clone(),
            &config.signature_prefix,
        );

        // Register update manifest watcher -- dispatches via channel
        Self::register_update_watcher(&mut hive, &config, &node_name, manifest_tx);

        // Register scan directive watcher -- dispatches via channel
        Self::register_directive_watcher(&mut hive, cmd_tx);

        // Get the handler before consuming hive with go()
        let handler = hive.get_handler();

        // Launch the Hive instance
        let hive_handler = hive.go(true, cancellation_token.clone()).await;

        // Capture values before node takes ownership
        let sig_count = scanner.signature_count();

        let node = HunterNode {
            config: config.clone(),
            handler: hive_handler,
            scanner,
            quarantine,
            scan_event_rx: scan_rx,
            cancellation_token: cancellation_token.clone(),
            node_name: node_name.clone(),
        };

        // Start the scan event processor (handles ThreatFound events)
        let event_handler = node.handler.clone();
        let event_quarantine = node.quarantine.clone();
        let event_token = cancellation_token.clone();
        let event_rx = node.scan_event_rx;
        let event_node_name = node_name.clone();
        tokio::spawn(async move {
            Self::process_scan_events(
                event_rx,
                event_handler,
                event_quarantine,
                event_token,
                &event_node_name,
            ).await;
        });

        // Start idle-aware periodic scan loop
        if config.scan_interval_secs > 0 && !config.scan_paths.is_empty() {
            let scan_scanner = node.scanner.clone();
            let scan_interval = Duration::from_secs(config.scan_interval_secs);
            let scan_paths = config.scan_paths.clone();
            let scan_token = cancellation_token.clone();
            let scan_node_name = node_name.clone();
            let idle_config = config.idle_config.clone();

            tokio::spawn(async move {
                Self::idle_scan_loop(
                    scan_scanner,
                    scan_paths,
                    scan_interval,
                    idle_config,
                    cmd_rx,
                    scan_token,
                    &scan_node_name,
                ).await;
            });
        }

        // Start the update manifest processor
        let update_config = UpdateConfig {
            current_version: config.current_version.clone(),
            staging_dir: config.vault_dir.parent()
                .unwrap_or(std::path::Path::new("."))
                .join(".hunter_updates"),
            ..UpdateConfig::default()
        };
        let update_handler = node.handler.clone();
        let update_token = cancellation_token.clone();
        let update_node_name = node_name.clone();
        tokio::spawn(async move {
            Self::process_update_manifests(
                manifest_rx,
                update_config,
                update_handler,
                update_token,
                &update_node_name,
            ).await;
        });

        // Publish initial node status
        let mut status_handler = node.handler.clone();
        let status_node_name = node_name.clone();
        let status_version = config.current_version.to_string();
        tokio::spawn(async move {
            let status = NodeStatus {
                node_name: status_node_name.clone(),
                version: status_version,
                platform: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
                last_scan_time: None,
                files_scanned: 0,
                threats_found: 0,
                signatures_loaded: sig_count,
                quarantine_count: 0,
                status: "running".into(),
            };
            let prop_name = format!("node_status_{}", status_node_name);
            match serde_json::to_string(&status) {
                Ok(json) => {
                    let val = PropertyValue { val: toml::Value::String(json) };
                    status_handler.set_property(&prop_name, Some(&val)).await;
                }
                Err(e) => error!("failed to serialize node status: {}", e),
            }
        });

        HunterHandle {
            handler,
            node_name,
            cancellation_token,
        }
    }

    /// Register the on_next callback for the update manifest property.
    /// Dispatches parsed manifests through a channel to the async updater.
    fn register_update_watcher(
        hive: &mut Hive,
        config: &HunterConfig,
        node_name: &str,
        tx: mpsc::Sender<UpdateManifest>,
    ) {
        let version = config.current_version.clone();
        let name = node_name.to_string();

        let prop = hive.get_mut_property_by_name("hunter_update_manifest");
        if let Some(p) = prop {
            p.on_next(move |value| {
                if let Some(json_str) = value.val.as_str() {
                    if json_str.is_empty() {
                        return;
                    }
                    match UpdateManifest::from_json(json_str) {
                        Ok(manifest) => {
                            if manifest.version.is_newer_than(&version) {
                                info!(
                                    "[{}] update available: v{} -> v{}",
                                    name, version, manifest.version
                                );
                                // Send to the async processor via channel.
                                // try_send avoids blocking in the sync callback.
                                if let Err(e) = tx.try_send(manifest) {
                                    warn!("failed to dispatch update manifest: {}", e);
                                }
                            } else {
                                debug!(
                                    "[{}] ignoring manifest v{} (current: v{})",
                                    name, manifest.version, version
                                );
                            }
                        }
                        Err(e) => {
                            warn!("failed to parse update manifest: {}", e);
                        }
                    }
                }
            });
            debug!("registered update manifest watcher");
        }
    }

    /// Register the on_next callback for the scan_directive property.
    /// Dispatches scan commands through a channel to the scan loop.
    fn register_directive_watcher(
        hive: &mut Hive,
        tx: mpsc::Sender<ScanCommand>,
    ) {
        let prop = hive.get_mut_property_by_name("scan_directive");
        if let Some(p) = prop {
            p.on_next(move |value| {
                if let Some(json_str) = value.val.as_str() {
                    if json_str.is_empty() {
                        return;
                    }
                    match serde_json::from_str::<serde_json::Value>(json_str) {
                        Ok(directive) => {
                            if directive.get("action").and_then(|a| a.as_str()) == Some("scan") {
                                let path = directive.get("path")
                                    .and_then(|p| p.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !path.is_empty() {
                                    info!("received scan directive for path: {}", path);
                                    if let Err(e) = tx.try_send(ScanCommand::ScanPath(PathBuf::from(path))) {
                                        warn!("failed to dispatch scan directive: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("invalid scan directive JSON: {}", e);
                        }
                    }
                }
            });
            debug!("registered scan directive watcher");
        }
    }

    /// Process scan events from the scanner channel.
    ///
    /// When a ThreatFound event arrives, quarantine the file and
    /// publish the record as a Hive property.
    async fn process_scan_events(
        mut rx: mpsc::Receiver<ScanEvent>,
        mut handler: Handler,
        quarantine: Arc<Quarantine>,
        token: CancellationToken,
        node_name: &str,
    ) {
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    info!("scan event processor shutting down");
                    break;
                }
                event = rx.recv() => {
                    match event {
                        Some(ScanEvent::ThreatFound(result)) => {
                            info!(
                                "[{}] threat detected in {:?}: {:?}",
                                node_name, result.path, result.matches
                            );

                            // Quarantine the file
                            match quarantine.quarantine_file(
                                &result.path,
                                result.matches.clone(),
                            ).await {
                                Ok(record) => {
                                    // Publish quarantine record to Hive
                                    let prop_name = format!(
                                        "quarantine_{}",
                                        &result.file_hash[..16]
                                    );
                                    match serde_json::to_string(&record) {
                                        Ok(json) => {
                                            let val = PropertyValue {
                                                val: toml::Value::String(json),
                                            };
                                            handler.set_property(
                                                &prop_name, Some(&val),
                                            ).await;
                                            info!(
                                                "quarantine record published: {}",
                                                prop_name,
                                            );
                                        }
                                        Err(e) => {
                                            error!(
                                                "failed to serialize quarantine record: {}",
                                                e,
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        "failed to quarantine {:?}: {}",
                                        result.path, e,
                                    );
                                }
                            }
                        }
                        Some(ScanEvent::ScanComplete { files_scanned, threats_found }) => {
                            info!(
                                "[{}] scan complete: {} files, {} threats",
                                node_name, files_scanned, threats_found,
                            );

                            let status = NodeStatus {
                                node_name: node_name.to_string(),
                                version: "".into(),
                                platform: format!(
                                    "{}-{}",
                                    std::env::consts::ARCH,
                                    std::env::consts::OS,
                                ),
                                last_scan_time: Some(Utc::now().to_rfc3339()),
                                files_scanned,
                                threats_found,
                                signatures_loaded: 0,
                                quarantine_count: 0,
                                status: "running".into(),
                            };
                            let prop_name = format!("node_status_{}", node_name);
                            if let Ok(json) = serde_json::to_string(&status) {
                                let val = PropertyValue {
                                    val: toml::Value::String(json),
                                };
                                handler.set_property(&prop_name, Some(&val)).await;
                            }
                        }
                        Some(ScanEvent::FileError { path, error }) => {
                            debug!(
                                "[{}] scan error on {:?}: {}",
                                node_name, path, error,
                            );
                        }
                        None => {
                            debug!("scan event channel closed");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Idle-aware scan loop.
    ///
    /// Waits for the system to become idle (or falls back to a fixed
    /// interval), then runs a full scan of all configured paths.
    /// Also receives on-demand scan commands from the directive channel.
    async fn idle_scan_loop(
        scanner: Arc<Scanner>,
        scan_paths: Vec<PathBuf>,
        interval: Duration,
        idle_config: IdleConfig,
        mut cmd_rx: mpsc::Receiver<ScanCommand>,
        token: CancellationToken,
        node_name: &str,
    ) {
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    info!("[{}] scan loop shutting down", node_name);
                    break;
                }
                // On-demand scan from a directive property change
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(ScanCommand::ScanPath(path)) => {
                            info!("[{}] on-demand scan: {:?}", node_name, path);
                            if path.exists() {
                                let (files, threats) = scanner.scan_directory(&path).await;
                                info!(
                                    "[{}] on-demand scan complete: {} files, {} threats",
                                    node_name, files, threats,
                                );
                            } else {
                                warn!("[{}] on-demand scan path does not exist: {:?}", node_name, path);
                            }
                        }
                        None => {
                            debug!("scan command channel closed");
                            break;
                        }
                    }
                }
                // Idle-aware periodic scan
                ready = idle_monitor::wait_for_idle(&idle_config, interval, &token) => {
                    if !ready {
                        // Cancelled
                        break;
                    }
                    info!("[{}] system idle detected, starting periodic scan", node_name);
                    for path in &scan_paths {
                        if !path.exists() {
                            warn!("[{}] scan path does not exist: {:?}", node_name, path);
                            continue;
                        }
                        info!("[{}] scanning {:?}", node_name, path);
                        let (files, threats) = scanner.scan_directory(path).await;
                        debug!(
                            "[{}] scanned {:?}: {} files, {} threats",
                            node_name, path, files, threats,
                        );
                    }
                }
            }
        }
    }

    /// Process update manifests received from the on_next channel.
    ///
    /// This runs in an async context so it can call the SelfUpdater's
    /// async methods directly.
    async fn process_update_manifests(
        mut rx: mpsc::Receiver<UpdateManifest>,
        update_config: UpdateConfig,
        handler: Handler,
        token: CancellationToken,
        node_name: &str,
    ) {
        let updater = SelfUpdater::new(update_config, handler);

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    info!("[{}] update processor shutting down", node_name);
                    break;
                }
                manifest = rx.recv() => {
                    match manifest {
                        Some(m) => {
                            info!(
                                "[{}] processing update manifest v{}",
                                node_name, m.version
                            );
                            match updater.process_manifest(&m).await {
                                Ok(true) => {
                                    // Process will re-exec; we won't reach here on Unix
                                    info!("[{}] update applied successfully", node_name);
                                }
                                Ok(false) => {
                                    debug!("[{}] update skipped", node_name);
                                }
                                Err(e) => {
                                    error!("[{}] update failed: {}", node_name, e);
                                }
                            }
                        }
                        None => {
                            debug!("update manifest channel closed");
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_status_serialization() {
        let status = NodeStatus {
            node_name: "test-node".into(),
            version: "0.1.0".into(),
            platform: "x86_64-macos".into(),
            last_scan_time: Some("2026-04-17T12:00:00Z".into()),
            files_scanned: 1500,
            threats_found: 2,
            signatures_loaded: 42,
            quarantine_count: 2,
            status: "running".into(),
        };

        let json = serde_json::to_string(&status).unwrap();
        let parsed: NodeStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.node_name, "test-node");
        assert_eq!(parsed.files_scanned, 1500);
        assert_eq!(parsed.threats_found, 2);
    }

    #[test]
    fn default_config() {
        let config = HunterConfig::default();
        assert_eq!(config.scan_interval_secs, 3600);
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert!(config.scan_paths.is_empty());
        assert!(config.idle_config.enabled);
    }
}
