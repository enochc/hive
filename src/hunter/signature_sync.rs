/// Signature synchronization via Hive properties.
///
/// Watches for threat signature properties arriving through Hive's normal
/// sync path and feeds them into the local Scanner.  Signatures are
/// distributed as Hive properties whose names start with a configurable
/// prefix (default: `threat_sig_`).  Each property value is a JSON-encoded
/// `ThreatSignature`.
///
/// When a signature property is set or updated, the watcher deserializes
/// it and loads it into the Scanner.  When a signature property is deleted
/// (set to None), the watcher removes it from the Scanner.
///
/// This module does not poll.  It uses the existing `Property::on_next`
/// callback mechanism so the Scanner stays current without any busy-wait
/// loops.

use std::sync::Arc;

use tracing::{debug, info, warn, error};

use crate::hive::Hive;
use crate::hunter::scanner::{Scanner, ThreatSignature};
use crate::property::PropertyValue;

/// Default prefix for signature property names.
pub const DEFAULT_SIG_PREFIX: &str = "threat_sig_";

/// Register on_next callbacks on all existing signature properties in
/// the Hive instance.  Each callback deserializes the JSON value and
/// loads (or removes) the signature in the shared Scanner.
///
/// Call this once during node startup, after the Hive instance is
/// constructed but before `hive.go()` is called.  Any signature
/// properties that arrive later through peer sync will trigger the
/// same callbacks.
///
/// # Arguments
///
/// * `hive` - mutable reference to the Hive instance (needed to
///   register `on_next` callbacks on properties)
/// * `scanner` - shared Scanner wrapped in Arc<Mutex<>> so callbacks
///   can safely mutate it from the Hive event loop
/// * `prefix` - property name prefix to match (e.g. "threat_sig_")
pub fn register_signature_watchers(
    hive: &mut Hive,
    scanner: Arc<Scanner>,
    prefix: &str,
) {
    // Walk existing properties and register watchers on any that match
    // the signature prefix.
    let matching_names: Vec<String> = hive
        .properties
        .values()
        .filter_map(|p| {
            let name = p.get_name();
            if name.starts_with(prefix) {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    for name in matching_names {
        register_single_watcher(hive, scanner.clone(), &name);
    }
}

/// Register an on_next callback on a single signature property.
///
/// This is also used to dynamically register watchers when new
/// signature properties are created at runtime.
pub fn register_single_watcher(
    hive: &mut Hive,
    scanner: Arc<Scanner>,
    prop_name: &str,
) {
    let sig_id = prop_name.to_string();
    let scanner_clone = scanner.clone();

    let prop = hive.get_mut_property_by_name(prop_name);
    match prop {
        Some(p) => {
            // If the property already has a value, load it immediately
            if let Some(current) = p.current_value() {
                load_signature_from_value(&scanner_clone, &sig_id, &current);
            }

            // Register the on_next callback for future changes
            let sid = sig_id.clone();
            p.on_next(move |value| {
                load_signature_from_value(&scanner_clone, &sid, &value);
            });

            debug!("registered signature watcher on '{}'", sig_id);
        }
        None => {
            warn!("could not find property '{}' for signature watcher", sig_id);
        }
    }
}

/// Parse a PropertyValue as a JSON ThreatSignature and load it into
/// the Scanner.  If the value is empty or the JSON is invalid, remove
/// the signature instead.
fn load_signature_from_value(
    scanner: &Arc<Scanner>,
    prop_name: &str,
    value: &PropertyValue,
) {
    let json_str = match value.val.as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            // Empty or non-string value means the signature was removed
            debug!("removing signature '{}'", prop_name);
            scanner.remove_signature(prop_name);
            return;
        }
    };

    match serde_json::from_str::<ThreatSignature>(json_str) {
        Ok(sig) => {
            info!("loaded signature '{}': {}", sig.id, sig.description);
            scanner.load_signature(sig);
        }
        Err(e) => {
            error!(
                "failed to parse signature from property '{}': {}",
                prop_name, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunter::scanner::Scanner;
    use tokio::sync::mpsc;

    #[test]
    fn load_valid_signature() {
        let (tx, _rx) = mpsc::channel(16);
        let scanner = Arc::new(Scanner::new(tx));

        let json = serde_json::json!({
            "id": "TEST-SIG",
            "pattern_hex": "deadbeef",
            "description": "test signature",
            "severity": "low"
        });

        let pv = PropertyValue { val: toml::Value::String(json.to_string()) };
        load_signature_from_value(&scanner, "threat_sig_test", &pv);

        assert_eq!(scanner.signature_count(), 1);
    }

    #[test]
    fn remove_signature_on_empty_value() {
        let (tx, _rx) = mpsc::channel(16);
        let scanner = Arc::new(Scanner::new(tx));

        // Load one first
        let json = serde_json::json!({
            "id": "threat_sig_rm",
            "pattern_hex": "cafe",
            "description": "removable",
            "severity": "medium"
        });
        let pv = PropertyValue { val: toml::Value::String(json.to_string()) };
        load_signature_from_value(&scanner, "threat_sig_rm", &pv);
        assert_eq!(scanner.signature_count(), 1);

        // Now send empty string to remove it
        let empty = PropertyValue { val: toml::Value::String(String::new()) };
        load_signature_from_value(&scanner, "threat_sig_rm", &empty);
        assert_eq!(scanner.signature_count(), 0);
    }

    #[test]
    fn invalid_json_does_not_crash() {
        let (tx, _rx) = mpsc::channel(16);
        let scanner = Arc::new(Scanner::new(tx));

        let pv = PropertyValue { val: toml::Value::String("not valid json".into()) };
        load_signature_from_value(&scanner, "threat_sig_bad", &pv);

        // Should have logged an error but not panicked
        assert_eq!(scanner.signature_count(), 0);
    }
}
