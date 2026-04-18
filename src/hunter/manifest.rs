/// Update manifest describing a new binary release.
///
/// Distributed as a Hive property so every connected node learns about
/// new versions through the normal sync path.  The manifest carries
/// everything a node needs to decide whether to update and to verify
/// the downloaded binary before replacing itself.

use std::fmt;
use serde::{Deserialize, Serialize};

/// Semantic version triple used for ordering releases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }

    /// Returns true when `self` is strictly newer than `other`.
    pub fn is_newer_than(&self, other: &Version) -> bool {
        (self.major, self.minor, self.patch) > (other.major, other.minor, other.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for Version {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("expected 3 version components, got {}", parts.len()));
        }
        let major = parts[0].parse::<u32>().map_err(|e| e.to_string())?;
        let minor = parts[1].parse::<u32>().map_err(|e| e.to_string())?;
        let patch = parts[2].parse::<u32>().map_err(|e| e.to_string())?;
        Ok(Version { major, minor, patch })
    }
}

/// Where to fetch the new binary from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UpdateSource {
    /// Download from a URL (HTTPS artifact server, S3 presigned URL, etc.)
    Url(String),
    /// Fetch from a Hive peer that already has the binary cached locally.
    Peer { name: String, address: String },
}

/// The manifest that gets serialized into a Hive property value.
///
/// When the orchestrator pushes an update, it sets the `hunter_update_manifest`
/// property to the JSON serialization of this struct.  Every hunter node
/// watches that property and reacts when a newer version appears.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// The version of the binary this manifest describes.
    pub version: Version,
    /// SHA-256 hex digest of the binary.
    pub sha256: String,
    /// Where to download the binary.
    pub source: UpdateSource,
    /// Target platform triple (e.g. "x86_64-unknown-linux-gnu").
    /// Nodes skip manifests that don't match their own platform.
    pub platform: String,
    /// Optional Ed25519 signature over the sha256 digest, hex-encoded.
    /// When present, nodes verify this against their built-in public key
    /// before accepting the binary.
    pub signature: Option<String>,
}

impl UpdateManifest {
    /// Serialize to a JSON string suitable for storing in a Hive property.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from a JSON string pulled from a Hive property value.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        let v1 = Version::new(1, 0, 0);
        let v2 = Version::new(1, 0, 1);
        let v3 = Version::new(2, 0, 0);

        assert!(!v1.is_newer_than(&v1));
        assert!(v2.is_newer_than(&v1));
        assert!(v3.is_newer_than(&v2));
        assert!(!v1.is_newer_than(&v3));
    }

    #[test]
    fn manifest_round_trip() {
        let m = UpdateManifest {
            version: Version::new(0, 3, 0),
            sha256: "abcdef1234567890".into(),
            source: UpdateSource::Url("https://releases.example.com/hunter/0.3.0".into()),
            platform: "x86_64-unknown-linux-gnu".into(),
            signature: Some("deadbeef".into()),
        };

        let json = m.to_json().unwrap();
        let parsed = UpdateManifest::from_json(&json).unwrap();

        assert_eq!(parsed.version, m.version);
        assert_eq!(parsed.sha256, m.sha256);
        assert_eq!(parsed.platform, m.platform);
    }
}
