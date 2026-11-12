//! Strict readers for TOML config values. A key that is present but has the
//! wrong type or an out-of-range value is an error naming the key, never
//! treated as absent or cast into something else (a negative timeout
//! becoming centuries, a quoted number being ignored). Callers turn the
//! error into a panic at construction, like the security config.

// Some readers are only used by the `tls` and `mqtt` features.
#![allow(dead_code)]

use toml::Value;

/// "`key`" for the top level, "[TABLE] `key`" inside a table.
fn label(table: &str, key: &str) -> String {
    if table.is_empty() {
        format!("`{}`", key)
    } else {
        format!("{} `{}`", table, key)
    }
}

/// Refuse keys outside `known`, so a typo can't silently leave the option
/// it was meant to set at its default.
pub(crate) fn check_keys(t: &Value, table: &str, known: &[&str]) -> Result<(), String> {
    let map = t.as_table().ok_or_else(|| format!("{} must be a table", table))?;
    match map.keys().find(|k| !known.contains(&k.as_str())) {
        Some(unknown) => Err(format!(
            "{} has an unknown key `{}` (expected one of: {})",
            table,
            unknown,
            known.join(", ")
        )),
        None => Ok(()),
    }
}

pub(crate) fn get_str<'a>(t: &'a Value, table: &str, key: &str) -> Result<Option<&'a str>, String> {
    match t.get(key) {
        None => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or_else(|| format!("{} must be a string", label(table, key))),
    }
}

/// `"true"` in quotes is an error rather than a quiet `false`.
pub(crate) fn get_bool(t: &Value, table: &str, key: &str) -> Result<Option<bool>, String> {
    match t.get(key) {
        None => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| format!("{} must be true or false", label(table, key))),
    }
}

/// An integer in `min..=max`, converted to `T` (which must hold that range).
pub(crate) fn get_int<T: TryFrom<i64>>(t: &Value, table: &str, key: &str, min: i64, max: i64) -> Result<Option<T>, String> {
    let Some(v) = t.get(key) else { return Ok(None) };
    let n = v
        .as_integer()
        .ok_or_else(|| format!("{} must be an integer, not {}", label(table, key), v))?;
    if n < min || n > max {
        return Err(format!("{} = {} is out of range, expected {}..={}", label(table, key), n, min, max));
    }
    T::try_from(n)
        .map(Some)
        .map_err(|_| format!("{} = {} does not fit", label(table, key), n))
}

/// An array of strings.
pub(crate) fn get_str_list(t: &Value, table: &str, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(v) = t.get(key) else { return Ok(None) };
    let err = || format!("{} must be an array of strings", label(table, key));
    v.as_array()
        .ok_or_else(err)?
        .iter()
        .map(|e| e.as_str().map(String::from).ok_or_else(err))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn integers_are_range_and_type_checked() {
        let v = t("a = 5\nb = -1\nc = \"5\"\nd = 70000");
        assert_eq!(get_int::<u16>(&v, "", "a", 1, 65535), Ok(Some(5)));
        assert_eq!(get_int::<u16>(&v, "", "missing", 1, 65535), Ok(None));
        assert!(get_int::<u64>(&v, "", "b", 0, 10).unwrap_err().contains("`b` = -1 is out of range"));
        assert!(get_int::<u16>(&v, "[MQTT]", "c", 1, 65535).unwrap_err().contains("[MQTT] `c` must be an integer"));
        assert!(get_int::<u16>(&v, "", "d", 1, 65535).unwrap_err().contains("out of range"));
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_named() {
        let v = t("hots = \"x\"\nflag = \"true\"\nlist = [1]");
        assert!(check_keys(&v, "[MQTT]", &["host", "flag", "list"]).unwrap_err().contains("unknown key `hots`"));
        assert!(get_bool(&v, "[X]", "flag").unwrap_err().contains("[X] `flag` must be true or false"));
        assert!(get_str_list(&v, "", "list").unwrap_err().contains("`list` must be an array of strings"));
        assert!(get_str(&v, "", "flag").is_ok());
    }
}
