/// Quarantine service for hunter nodes.
///
/// When the scanner detects a threat, the quarantine service isolates
/// the suspect file by moving it to a secured directory, stripping its
/// execute permissions, and logging the event.  The quarantine record
/// is published back to Hive as a property so other nodes learn about
/// the finding.
///
/// Files in quarantine can be restored if a false positive is confirmed,
/// or permanently deleted after review.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use tokio::fs;
use tracing::{debug, info, warn, error};

/// A record of a quarantined file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuarantineRecord {
    /// Original path of the file before quarantine.
    pub original_path: String,
    /// SHA-256 hash of the file contents.
    pub file_hash: String,
    /// Which signature IDs triggered the quarantine.
    pub matched_signatures: Vec<String>,
    /// ISO-8601 timestamp of when the file was quarantined.
    pub quarantined_at: String,
    /// Name of the node that performed the quarantine.
    pub node_name: String,
    /// Filename inside the quarantine vault (UUID-based to avoid collisions).
    pub vault_filename: String,
}

/// Errors from quarantine operations.
#[derive(Debug)]
pub enum QuarantineError {
    /// The quarantine directory could not be created or accessed.
    VaultError(String),
    /// The file could not be moved (permission denied, in use, etc.)
    MoveError(String),
    /// The file was not found in the quarantine vault.
    NotFound(String),
    /// Generic IO error.
    IoError(std::io::Error),
}

impl std::fmt::Display for QuarantineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VaultError(msg) => write!(f, "vault error: {}", msg),
            Self::MoveError(msg) => write!(f, "move error: {}", msg),
            Self::NotFound(msg) => write!(f, "not found: {}", msg),
            Self::IoError(e) => write!(f, "io error: {}", e),
        }
    }
}

impl From<std::io::Error> for QuarantineError {
    fn from(e: std::io::Error) -> Self {
        QuarantineError::IoError(e)
    }
}

/// The quarantine vault.
///
/// Manages the isolated storage directory and provides methods to
/// quarantine, restore, and delete files.
pub struct Quarantine {
    /// Path to the quarantine vault directory.
    vault_dir: PathBuf,
    /// Name of the local node, used in quarantine records.
    node_name: String,
}

impl Quarantine {
    pub fn new(vault_dir: PathBuf, node_name: String) -> Self {
        Self { vault_dir, node_name }
    }

    /// Ensure the vault directory exists and has restricted permissions.
    pub async fn initialize(&self) -> Result<(), QuarantineError> {
        fs::create_dir_all(&self.vault_dir).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            fs::set_permissions(&self.vault_dir, perms).await?;
        }

        info!("quarantine vault initialized at {:?}", self.vault_dir);
        Ok(())
    }

    /// Move a suspect file into the quarantine vault.
    ///
    /// The file is renamed to a UUID-based filename inside the vault
    /// and its execute permissions are stripped.  Returns a record
    /// that can be serialized and published as a Hive property.
    pub async fn quarantine_file(
        &self,
        file_path: &Path,
        matched_signatures: Vec<String>,
    ) -> Result<QuarantineRecord, QuarantineError> {
        // Generate a unique vault filename
        let vault_name = format!(
            "{}-{}",
            Utc::now().format("%Y%m%d%H%M%S"),
            file_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        );
        let vault_path = self.vault_dir.join(&vault_name);

        // Read the file to compute its hash before moving
        let data = fs::read(file_path).await.map_err(|e| {
            QuarantineError::MoveError(format!(
                "cannot read {:?}: {}", file_path, e
            ))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&data);
        let file_hash = format!("{:x}", hasher.finalize());

        // Move the file into the vault
        fs::rename(file_path, &vault_path).await.or_else(|_| {
            // rename() fails across filesystems, fall back to copy+delete
            debug!("rename failed, falling back to copy+delete");
            // We need to do this synchronously since we are in an or_else closure.
            // Returning an error here and handling copy+delete in a separate branch.
            Err(QuarantineError::MoveError("cross-device".into()))
        }).or_else(|_| -> Result<(), QuarantineError> {
            // This will be handled async below
            Ok(())
        })?;

        // If the file still exists at the original path (rename failed),
        // do the copy+delete fallback.
        if file_path.exists() {
            fs::copy(file_path, &vault_path).await.map_err(|e| {
                QuarantineError::MoveError(format!("copy failed: {}", e))
            })?;
            fs::remove_file(file_path).await.map_err(|e| {
                QuarantineError::MoveError(format!(
                    "delete original failed: {}", e
                ))
            })?;
        }

        // Strip execute permissions on the quarantined file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o400);
            fs::set_permissions(&vault_path, perms).await?;
        }

        let record = QuarantineRecord {
            original_path: file_path.to_string_lossy().into(),
            file_hash,
            matched_signatures,
            quarantined_at: Utc::now().to_rfc3339(),
            node_name: self.node_name.clone(),
            vault_filename: vault_name,
        };

        info!(
            "quarantined {:?} as {} (sigs: {:?})",
            file_path, record.vault_filename, record.matched_signatures
        );

        Ok(record)
    }

    /// Restore a quarantined file to its original location.
    pub async fn restore_file(
        &self,
        record: &QuarantineRecord,
    ) -> Result<(), QuarantineError> {
        let vault_path = self.vault_dir.join(&record.vault_filename);
        if !vault_path.exists() {
            return Err(QuarantineError::NotFound(
                record.vault_filename.clone(),
            ));
        }

        let original = PathBuf::from(&record.original_path);

        // Ensure the parent directory exists
        if let Some(parent) = original.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::rename(&vault_path, &original).await.or_else(|_| {
            Err(QuarantineError::MoveError(
                "restore rename failed".into(),
            ))
        })?;

        info!("restored {:?} from quarantine", original);
        Ok(())
    }

    /// Permanently delete a quarantined file.
    pub async fn delete_file(
        &self,
        record: &QuarantineRecord,
    ) -> Result<(), QuarantineError> {
        let vault_path = self.vault_dir.join(&record.vault_filename);
        if !vault_path.exists() {
            return Err(QuarantineError::NotFound(
                record.vault_filename.clone(),
            ));
        }

        fs::remove_file(&vault_path).await?;
        info!("permanently deleted quarantined file: {}", record.vault_filename);
        Ok(())
    }

    /// List all files currently in the quarantine vault.
    pub async fn list_quarantined(&self) -> Result<Vec<String>, QuarantineError> {
        let mut entries = fs::read_dir(&self.vault_dir).await?;
        let mut names = Vec::new();

        loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    if let Some(name) = entry.file_name().to_str() {
                        names.push(name.to_string());
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!("error listing vault: {}", e);
                    continue;
                }
            }
        }

        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quarantine_and_restore() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_dir = tmp.path().join("vault");
        let q = Quarantine::new(vault_dir, "test-node".into());
        q.initialize().await.unwrap();

        // Create a test file
        let suspect = tmp.path().join("bad_file.exe");
        fs::write(&suspect, b"definitely malicious content").await.unwrap();

        // Quarantine it
        let record = q.quarantine_file(
            &suspect,
            vec!["SIG-001".into()],
        ).await.unwrap();

        // Original should be gone
        assert!(!suspect.exists());

        // Vault should have the file
        let listed = q.list_quarantined().await.unwrap();
        assert_eq!(listed.len(), 1);

        // Restore it
        q.restore_file(&record).await.unwrap();
        assert!(suspect.exists());

        let content = fs::read(&suspect).await.unwrap();
        assert_eq!(content, b"definitely malicious content");
    }

    #[tokio::test]
    async fn quarantine_and_delete() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_dir = tmp.path().join("vault");
        let q = Quarantine::new(vault_dir, "test-node".into());
        q.initialize().await.unwrap();

        let suspect = tmp.path().join("bad_file.bin");
        fs::write(&suspect, b"payload").await.unwrap();

        let record = q.quarantine_file(
            &suspect,
            vec!["SIG-002".into()],
        ).await.unwrap();

        q.delete_file(&record).await.unwrap();

        let listed = q.list_quarantined().await.unwrap();
        assert!(listed.is_empty());
    }
}
