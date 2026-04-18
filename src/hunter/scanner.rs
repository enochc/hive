/// Threat scanner engine for hunter nodes.
///
/// Loads signature definitions from Hive-synced properties and scans
/// local filesystem paths for matches.  Discovered threats are reported
/// back to Hive through the handler so other nodes can learn about them.
///
/// # Thread safety
///
/// The signature store is internally protected by a `std::sync::RwLock`,
/// allowing signature mutations (from sync `on_next` callbacks) and
/// concurrent async scans without external locking.  Callers never need
/// to wrap Scanner in a Mutex.
///
/// # Signature format
///
/// Signatures are distributed as Hive properties under a configurable
/// prefix (default: `threat_sig_`).  Each property value is a JSON
/// object describing one threat signature:
///
/// ```json
/// {
///   "id": "EICAR-TEST",
///   "pattern_hex": "...",
///   "description": "EICAR test file",
///   "severity": "low"
/// }
/// ```
///
/// The scanner converts the hex pattern to bytes and searches for that
/// byte sequence in each scanned file.  More sophisticated matching
/// (YARA rules, regex patterns, heuristics) is planned for later phases.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use tokio::fs;
use tokio::sync::mpsc;
use tracing::{debug, info, warn, error};

/// A single threat signature definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreatSignature {
    /// Unique identifier for this threat.
    pub id: String,
    /// Hex-encoded byte pattern to search for.
    pub pattern_hex: String,
    /// Human-readable description.
    pub description: String,
    /// Severity level: "low", "medium", "high", "critical".
    pub severity: String,
}

impl ThreatSignature {
    /// Decode the hex pattern into raw bytes.
    pub fn pattern_bytes(&self) -> Option<Vec<u8>> {
        hex::decode(&self.pattern_hex).ok()
    }
}

/// Result of scanning a single file.
#[derive(Clone, Debug)]
pub struct ScanResult {
    /// Path of the scanned file.
    pub path: PathBuf,
    /// SHA-256 digest of the file contents.
    pub file_hash: String,
    /// Which signatures matched, if any.
    pub matches: Vec<String>,
}

/// Events emitted by the scanner.
#[derive(Debug)]
pub enum ScanEvent {
    /// A file matched one or more signatures.
    ThreatFound(ScanResult),
    /// A scan pass completed.
    ScanComplete {
        files_scanned: u64,
        threats_found: u64,
    },
    /// An error occurred while scanning a file (permission denied, etc.)
    FileError {
        path: PathBuf,
        error: String,
    },
}

/// The scanner engine.
///
/// Signature storage is internally synchronized with an RwLock so that
/// `load_signature` / `remove_signature` (called from sync on_next
/// callbacks) and `scan_file` / `scan_directory` (called from async
/// tasks) can operate concurrently without external locking.
pub struct Scanner {
    signatures: RwLock<HashMap<String, ThreatSignature>>,
    /// Channel for reporting scan events back to the caller.
    event_tx: mpsc::Sender<ScanEvent>,
    /// Maximum file size to scan (skip anything larger). Default 50 MB.
    max_file_size: u64,
}

impl Scanner {
    pub fn new(event_tx: mpsc::Sender<ScanEvent>) -> Self {
        Self {
            signatures: RwLock::new(HashMap::new()),
            event_tx,
            max_file_size: 50 * 1024 * 1024,
        }
    }

    /// Set the maximum file size to scan in bytes.
    pub fn set_max_file_size(&mut self, size: u64) {
        self.max_file_size = size;
    }

    /// Load a signature into the scanner.  Replaces any existing
    /// signature with the same ID.  Safe to call from sync contexts
    /// (e.g. on_next callbacks).
    pub fn load_signature(&self, sig: ThreatSignature) {
        debug!("loaded signature: {} - {}", sig.id, sig.description);
        self.signatures.write().unwrap().insert(sig.id.clone(), sig);
    }

    /// Remove a signature by ID.  Safe to call from sync contexts.
    pub fn remove_signature(&self, id: &str) -> bool {
        self.signatures.write().unwrap().remove(id).is_some()
    }

    /// Number of currently loaded signatures.
    pub fn signature_count(&self) -> usize {
        self.signatures.read().unwrap().len()
    }

    /// Take a snapshot of the current signatures for scanning.
    /// This briefly acquires a read lock, clones the map, and releases.
    fn snapshot_signatures(&self) -> HashMap<String, ThreatSignature> {
        self.signatures.read().unwrap().clone()
    }

    /// Scan a single file against all loaded signatures.
    pub async fn scan_file(&self, path: &Path) -> Option<ScanResult> {
        let metadata = match fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => {
                let _ = self.event_tx.send(ScanEvent::FileError {
                    path: path.to_path_buf(),
                    error: e.to_string(),
                }).await;
                return None;
            }
        };

        if !metadata.is_file() {
            return None;
        }

        if metadata.len() > self.max_file_size {
            debug!("skipping large file: {:?} ({} bytes)", path, metadata.len());
            return None;
        }

        let data = match fs::read(path).await {
            Ok(d) => d,
            Err(e) => {
                let _ = self.event_tx.send(ScanEvent::FileError {
                    path: path.to_path_buf(),
                    error: e.to_string(),
                }).await;
                return None;
            }
        };

        // Compute file hash
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let file_hash = format!("{:x}", hasher.finalize());

        // Snapshot signatures so we don't hold the lock during matching
        let sigs = self.snapshot_signatures();

        // Check each signature
        let mut matches = Vec::new();
        for (id, sig) in &sigs {
            if let Some(pattern) = sig.pattern_bytes() {
                if contains_pattern(&data, &pattern) {
                    info!("threat match: {} in {:?}", id, path);
                    matches.push(id.clone());
                }
            }
        }

        if matches.is_empty() {
            return None;
        }

        let result = ScanResult {
            path: path.to_path_buf(),
            file_hash,
            matches,
        };

        let _ = self.event_tx.send(ScanEvent::ThreatFound(result.clone())).await;
        Some(result)
    }

    /// Recursively scan a directory tree.
    ///
    /// Sends `ScanEvent` messages through the channel as threats are found.
    /// Returns the total count of files scanned and threats found.
    pub async fn scan_directory(&self, root: &Path) -> (u64, u64) {
        let mut files_scanned: u64 = 0;
        let mut threats_found: u64 = 0;

        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            let mut entries = match fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(e) => {
                    warn!("cannot read directory {:?}: {}", dir, e);
                    continue;
                }
            };

            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(e)) => e,
                    Ok(None) => break,
                    Err(e) => {
                        warn!("error reading entry in {:?}: {}", dir, e);
                        continue;
                    }
                };

                let path = entry.path();
                let file_type = match entry.file_type().await {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };

                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file() {
                    files_scanned += 1;
                    if self.scan_file(&path).await.is_some() {
                        threats_found += 1;
                    }
                }
            }
        }

        let _ = self.event_tx.send(ScanEvent::ScanComplete {
            files_scanned,
            threats_found,
        }).await;

        (files_scanned, threats_found)
    }
}

/// Simple byte-pattern search (Knuth-Morris-Pratt would be faster for
/// large files, but this is sufficient for an initial implementation).
fn contains_pattern(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_search_finds_match() {
        let data = b"some prefix EVIL_PAYLOAD some suffix";
        let pattern = b"EVIL_PAYLOAD";
        assert!(contains_pattern(data, pattern));
    }

    #[test]
    fn pattern_search_no_match() {
        let data = b"clean file contents";
        let pattern = b"EVIL_PAYLOAD";
        assert!(!contains_pattern(data, pattern));
    }

    #[test]
    fn hex_pattern_decode() {
        let sig = ThreatSignature {
            id: "TEST-001".into(),
            pattern_hex: "4556494c".into(), // "EVIL" in hex
            description: "test".into(),
            severity: "low".into(),
        };
        let bytes = sig.pattern_bytes().unwrap();
        assert_eq!(bytes, b"EVIL");
    }

    #[tokio::test]
    async fn scan_file_detects_threat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let evil_path = tmp.path().join("evil.bin");
        std::fs::write(&evil_path, b"clean stuff EVIL_BYTES more clean stuff").unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let scanner = Scanner::new(tx);
        scanner.load_signature(ThreatSignature {
            id: "TEST-EVIL".into(),
            pattern_hex: hex::encode(b"EVIL_BYTES"),
            description: "test evil bytes".into(),
            severity: "high".into(),
        });

        let result = scanner.scan_file(&evil_path).await;
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.matches.contains(&"TEST-EVIL".to_string()));

        // Verify the event was sent
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, ScanEvent::ThreatFound(_)));
    }
}
