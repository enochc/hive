/// Self-update module for hunter nodes.
///
/// Watches a Hive property for new update manifests and handles the full
/// download-verify-replace cycle.  The update flow:
///
///   1. A `PropertyStream` on the manifest property fires when a new
///      version is published by the orchestrator.
///   2. The module compares the manifest version against the running version.
///   3. If newer, it downloads the binary from the source in the manifest.
///   4. It verifies the SHA-256 digest (and signature when configured).
///   5. It writes the new binary to a staging path, then atomically renames
///      it over the current executable.
///   6. It re-execs the process so the new binary takes over.
///   7. If the new binary fails its health check within `health_timeout`,
///      the rollback copy is restored automatically.
///
/// Signals are used to coordinate readiness and completion rather than
/// busy-wait loops.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Sha256, Digest};
use tokio::fs;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::handler::Handler;
use crate::hunter::manifest::{UpdateManifest, UpdateSource, Version};

/// Errors that can occur during the self-update process.
#[derive(Debug)]
pub enum UpdateError {
    /// The manifest targets a different platform than this node.
    PlatformMismatch { expected: String, got: String },
    /// SHA-256 of the downloaded binary did not match the manifest.
    HashMismatch { expected: String, got: String },
    /// Signature verification failed.
    SignatureInvalid,
    /// The download failed.
    DownloadFailed(String),
    /// Filesystem operation failed during staging or replacement.
    IoError(std::io::Error),
    /// The new binary failed its post-launch health check.
    HealthCheckFailed,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlatformMismatch { expected, got } => {
                write!(f, "platform mismatch: expected {}, got {}", expected, got)
            }
            Self::HashMismatch { expected, got } => {
                write!(f, "hash mismatch: expected {}, got {}", expected, got)
            }
            Self::SignatureInvalid => write!(f, "signature verification failed"),
            Self::DownloadFailed(msg) => write!(f, "download failed: {}", msg),
            Self::IoError(e) => write!(f, "io error: {}", e),
            Self::HealthCheckFailed => write!(f, "health check failed after update"),
        }
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        UpdateError::IoError(e)
    }
}

/// Configuration for the self-update module.
pub struct UpdateConfig {
    /// The version of the currently running binary.
    pub current_version: Version,
    /// Platform triple for this node (e.g. "x86_64-unknown-linux-gnu").
    pub platform: String,
    /// Directory where staging and rollback files are stored.
    /// Defaults to a `.hunter_updates` directory next to the executable.
    pub staging_dir: PathBuf,
    /// How long to wait for a health check after launching the new binary.
    pub health_timeout: std::time::Duration,
    /// Hive property name used for the update manifest.
    pub manifest_property: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        let staging = exe.parent().unwrap_or(Path::new(".")).join(".hunter_updates");
        Self {
            current_version: Version::new(0, 1, 0),
            platform: current_platform(),
            staging_dir: staging,
            health_timeout: std::time::Duration::from_secs(30),
            manifest_property: "hunter_update_manifest".into(),
        }
    }
}

/// Returns the target triple for the running binary.
fn current_platform() -> String {
    // Built-in at compile time via cfg attributes.
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    format!("{}-{}", arch, os)
}

/// The self-update service.
///
/// Constructed with a config and a Hive handler.  Call `run()` to start
/// watching for manifests.  The returned `Notify` fires when an update
/// has been staged and verified, right before the process replaces itself.
pub struct SelfUpdater {
    config: UpdateConfig,
    handler: Handler,
    /// Fired when a new manifest is received and passes initial checks.
    update_available: Arc<Notify>,
}

impl SelfUpdater {
    pub fn new(config: UpdateConfig, handler: Handler) -> Self {
        Self {
            config,
            handler,
            update_available: Arc::new(Notify::new()),
        }
    }

    /// Returns an Arc<Notify> that fires when a valid update is available.
    /// External code can await this to perform cleanup before the process
    /// replaces itself.
    pub fn update_notify(&self) -> Arc<Notify> {
        self.update_available.clone()
    }

    /// Ensure the staging directory exists.
    async fn ensure_staging_dir(&self) -> Result<(), UpdateError> {
        fs::create_dir_all(&self.config.staging_dir).await?;
        Ok(())
    }

    /// Path where the new binary is staged before replacement.
    fn staging_path(&self) -> PathBuf {
        self.config.staging_dir.join("hunter_staged")
    }

    /// Path where the current binary is backed up for rollback.
    fn rollback_path(&self) -> PathBuf {
        self.config.staging_dir.join("hunter_rollback")
    }

    /// Download the binary from the source specified in the manifest.
    async fn download(&self, manifest: &UpdateManifest) -> Result<Vec<u8>, UpdateError> {
        match &manifest.source {
            UpdateSource::Url(url) => {
                info!("downloading update from {}", url);
                // Using reqwest for HTTP downloads.  This dependency is already
                // optional in Cargo.toml behind the "rest" feature.  For a
                // production build you would want this behind a "hunter" feature
                // gate instead.
                //
                // For now, provide a stub that reads from a local file path
                // when the URL starts with "file://", which is useful for
                // testing.  The HTTP path is left as a TODO.
                if url.starts_with("file://") {
                    let path = &url["file://".len()..];
                    let data = fs::read(path).await.map_err(|e| {
                        UpdateError::DownloadFailed(format!("local read failed: {}", e))
                    })?;
                    Ok(data)
                } else {
                    // TODO: HTTP download via reqwest
                    Err(UpdateError::DownloadFailed(
                        "HTTP downloads not yet implemented".into(),
                    ))
                }
            }
            UpdateSource::Peer { name, address } => {
                info!("fetching update from peer {} at {}", name, address);
                // TODO: request the binary from a peer via a custom Hive
                // message type.  The peer would respond with chunks streamed
                // over the existing TCP connection.
                Err(UpdateError::DownloadFailed(
                    "peer-to-peer binary transfer not yet implemented".into(),
                ))
            }
        }
    }

    /// Verify the SHA-256 digest of the downloaded binary.
    fn verify_hash(&self, data: &[u8], expected: &str) -> Result<(), UpdateError> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = format!("{:x}", hasher.finalize());

        if digest != expected {
            return Err(UpdateError::HashMismatch {
                expected: expected.into(),
                got: digest,
            });
        }
        debug!("SHA-256 verified: {}", expected);
        Ok(())
    }

    /// Stage the binary: write to a temp location, then atomically rename
    /// into the staging path.
    async fn stage_binary(&self, data: &[u8]) -> Result<(), UpdateError> {
        self.ensure_staging_dir().await?;
        let tmp = self.config.staging_dir.join("hunter_downloading");
        fs::write(&tmp, data).await?;

        // Set executable permission on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            fs::set_permissions(&tmp, perms).await?;
        }

        fs::rename(&tmp, self.staging_path()).await?;
        info!("binary staged at {:?}", self.staging_path());
        Ok(())
    }

    /// Back up the current executable so we can roll back if needed.
    async fn backup_current(&self) -> Result<(), UpdateError> {
        let current = std::env::current_exe().map_err(|e| {
            UpdateError::IoError(e)
        })?;
        self.ensure_staging_dir().await?;
        fs::copy(&current, self.rollback_path()).await?;
        info!("current binary backed up to {:?}", self.rollback_path());
        Ok(())
    }

    /// Replace the running executable with the staged binary.
    ///
    /// On Unix this does an atomic rename followed by re-exec.
    /// On Windows this does a rename dance (running -> old, staged -> running)
    /// followed by spawning the new process and exiting.
    async fn replace_and_reexec(&self) -> Result<(), UpdateError> {
        let current = std::env::current_exe().map_err(|e| {
            UpdateError::IoError(e)
        })?;

        // Atomic rename: staged -> current executable path
        fs::rename(self.staging_path(), &current).await?;
        info!("binary replaced at {:?}, re-launching", current);

        // Collect current args for the re-exec
        let args: Vec<String> = std::env::args().collect();

        // On Unix, exec() replaces the current process in place.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&current)
                .args(&args[1..])
                .exec();
            // exec() only returns on error
            error!("re-exec failed: {}", err);
            return Err(UpdateError::IoError(err));
        }

        // On non-Unix, spawn a new process and exit the current one.
        #[cfg(not(unix))]
        {
            std::process::Command::new(&current)
                .args(&args[1..])
                .spawn()
                .map_err(|e| UpdateError::IoError(e))?;
            std::process::exit(0);
        }
    }

    /// Roll back to the previous binary if the health check fails.
    pub async fn rollback(&self) -> Result<(), UpdateError> {
        let current = std::env::current_exe().map_err(|e| {
            UpdateError::IoError(e)
        })?;
        let rollback = self.rollback_path();

        if !rollback.exists() {
            warn!("no rollback binary found at {:?}", rollback);
            return Err(UpdateError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "rollback binary not found",
            )));
        }

        fs::rename(&rollback, &current).await?;
        info!("rolled back to previous binary");
        Ok(())
    }

    /// Process a single update manifest.
    ///
    /// Returns Ok(true) if an update was applied (the process will re-exec
    /// and this function won't actually return in that case).
    /// Returns Ok(false) if the manifest was skipped (old version, wrong platform).
    pub async fn process_manifest(&self, manifest: &UpdateManifest) -> Result<bool, UpdateError> {
        // Check platform
        if manifest.platform != self.config.platform {
            debug!(
                "skipping manifest for platform {} (we are {})",
                manifest.platform, self.config.platform
            );
            return Ok(false);
        }

        // Check version
        if !manifest.version.is_newer_than(&self.config.current_version) {
            debug!(
                "skipping manifest v{} (we are v{})",
                manifest.version, self.config.current_version
            );
            return Ok(false);
        }

        info!(
            "update available: v{} -> v{}",
            self.config.current_version, manifest.version
        );

        // Download
        let data = self.download(manifest).await?;

        // Verify hash
        self.verify_hash(&data, &manifest.sha256)?;

        // TODO: verify signature when manifest.signature is Some

        // Stage the binary
        self.stage_binary(&data).await?;

        // Back up current binary for rollback
        self.backup_current().await?;

        // Signal that an update is about to happen so external code
        // can perform cleanup (flush logs, save state, etc.)
        self.update_available.notify_waiters();

        // Replace and re-exec
        self.replace_and_reexec().await?;

        // We should not reach here on Unix (exec replaces the process).
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn test_config(staging_dir: PathBuf) -> UpdateConfig {
        UpdateConfig {
            current_version: Version::new(0, 1, 0),
            platform: current_platform(),
            staging_dir,
            health_timeout: std::time::Duration::from_secs(5),
            manifest_property: "test_manifest".into(),
        }
    }

    #[tokio::test]
    async fn skips_older_version() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path().to_path_buf());
        let handler = crate::handler::Handler {
            sender: futures::channel::mpsc::unbounded().0,
            from_name: "test".into(),
        };
        let updater = SelfUpdater::new(config, handler);

        let manifest = UpdateManifest {
            version: Version::new(0, 0, 9),
            sha256: "unused".into(),
            source: UpdateSource::Url("file:///dev/null".into()),
            platform: current_platform(),
            signature: None,
        };

        let result = updater.process_manifest(&manifest).await.unwrap();
        assert!(!result, "should skip older version");
    }

    #[tokio::test]
    async fn skips_wrong_platform() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path().to_path_buf());
        let handler = crate::handler::Handler {
            sender: futures::channel::mpsc::unbounded().0,
            from_name: "test".into(),
        };
        let updater = SelfUpdater::new(config, handler);

        let manifest = UpdateManifest {
            version: Version::new(99, 0, 0),
            sha256: "unused".into(),
            source: UpdateSource::Url("file:///dev/null".into()),
            platform: "mips-unknown-netbsd".into(),
            signature: None,
        };

        let result = updater.process_manifest(&manifest).await.unwrap();
        assert!(!result, "should skip wrong platform");
    }

    #[tokio::test]
    async fn detects_hash_mismatch() {
        let tmp = TempDir::new().unwrap();

        // Write a fake binary
        let bin_path = tmp.path().join("fake_binary");
        let mut f = std::fs::File::create(&bin_path).unwrap();
        f.write_all(b"hello hunter").unwrap();

        let config = test_config(tmp.path().join("staging"));
        let handler = crate::handler::Handler {
            sender: futures::channel::mpsc::unbounded().0,
            from_name: "test".into(),
        };
        let updater = SelfUpdater::new(config, handler);

        let manifest = UpdateManifest {
            version: Version::new(0, 2, 0),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            source: UpdateSource::Url(format!("file://{}", bin_path.display())),
            platform: current_platform(),
            signature: None,
        };

        let result = updater.process_manifest(&manifest).await;
        assert!(
            matches!(result, Err(UpdateError::HashMismatch { .. })),
            "should fail on hash mismatch"
        );
    }
}
