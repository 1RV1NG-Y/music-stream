//! How user data is laid out inside the replicated Loro document (PLAN.md §4).
//!
//! - `settings`: setting name → JSON value, last-writer-wins.
//! - `devices` holds membership; `addresses` each device's current LAN addresses.
//!
//! Playlists and favorites (`TrackRef`s, PLAN.md §3–§4) are added with the library.

use anyhow::{Result, bail};
use loro::{LoroDoc, LoroMap, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SETTINGS: &str = "settings";
pub const DEVICES: &str = "devices";
/// Map of endpoint id → JSON list of socket addresses, written only by that device.
pub const ADDRESSES: &str = "addresses";

const MAX_SETTINGS: usize = 100;
const MAX_SETTING_JSON: usize = 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRecord {
    pub name: String,
    pub platform: String,
    pub added_at: String,
    pub added_by: String,
    #[serde(default)]
    pub removed: bool,
}

pub fn now_iso() -> String {
    humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string()
}

fn json_value<T: for<'de> Deserialize<'de>>(map: &LoroMap, key: &str) -> Option<T> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => serde_json::from_str(&s).ok(),
        _ => None,
    }
}

// ---- Settings ------------------------------------------------------------------------------

pub fn read_settings(doc: &LoroDoc) -> serde_json::Map<String, Value> {
    let m = doc.get_map(SETTINGS);
    m.keys().filter_map(|k| json_value::<Value>(&m, &k).map(|v| (k.to_string(), v))).collect()
}

/// Writes the settings in `patch` that differ; a `null` value removes the setting.
/// Returns whether anything changed. The caller commits.
pub fn write_settings(doc: &LoroDoc, patch: &serde_json::Map<String, Value>) -> Result<bool> {
    let current = read_settings(doc);
    let m = doc.get_map(SETTINGS);
    let mut changed = false;
    for (key, value) in patch {
        if key.is_empty() || key.len() > 64 || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            bail!("Invalid setting name.");
        }
        if value.is_null() {
            if current.contains_key(key) {
                m.delete(key)?;
                changed = true;
            }
            continue;
        }
        let json = serde_json::to_string(value)?;
        if json.len() > MAX_SETTING_JSON {
            bail!("The value of setting {key} is too large.");
        }
        if current.get(key) != Some(value) {
            m.insert(key, json)?;
            changed = true;
        }
    }
    if read_settings(doc).len() > MAX_SETTINGS {
        bail!("Too many settings.");
    }
    Ok(changed)
}

// ---- Devices -------------------------------------------------------------------------------

pub fn read_devices(doc: &LoroDoc) -> Vec<(String, DeviceRecord)> {
    let devices = doc.get_map(DEVICES);
    devices.keys().filter_map(|k| json_value::<DeviceRecord>(&devices, &k).map(|r| (k.to_string(), r))).collect()
}

pub fn write_device(doc: &LoroDoc, id: &str, record: &DeviceRecord) -> Result<()> {
    doc.get_map(DEVICES).insert(id, serde_json::to_string(record)?)?;
    Ok(())
}

pub fn read_addresses(doc: &LoroDoc, id: &str) -> Vec<std::net::SocketAddr> {
    match doc.get_map(ADDRESSES).get(id) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => serde_json::from_str(&s).unwrap_or_default(),
        _ => vec![],
    }
}

/// Publishes this device's addresses; returns whether they changed. The caller commits.
pub fn write_addresses(doc: &LoroDoc, id: &str, addrs: &[std::net::SocketAddr]) -> Result<bool> {
    let mut addrs = addrs.to_vec();
    addrs.sort();
    if read_addresses(doc, id) == addrs {
        return Ok(false);
    }
    doc.get_map(ADDRESSES).insert(id, serde_json::to_string(&addrs)?)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(a: &LoroDoc, b: &LoroDoc) {
        b.import(&a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap()).unwrap();
        a.import(&b.export(loro::ExportMode::updates(&a.oplog_vv())).unwrap()).unwrap();
    }

    #[test]
    fn settings_merge_and_validate() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        let patch = |k: &str, v: Value| serde_json::Map::from_iter([(k.to_owned(), v)]);
        assert!(write_settings(&a, &patch("volume", 0.5.into())).unwrap());
        assert!(!write_settings(&a, &patch("volume", 0.5.into())).unwrap(), "unchanged is not a change");
        a.commit();
        sync(&a, &b);
        assert_eq!(read_settings(&b)["volume"], 0.5);
        assert!(write_settings(&b, &patch("volume", Value::Null)).unwrap());
        b.commit();
        sync(&a, &b);
        assert!(read_settings(&a).is_empty());
        assert!(write_settings(&a, &patch("bad name", 1.into())).is_err());
        assert!(write_settings(&a, &patch("big", "x".repeat(2000).into())).is_err());
    }

    #[test]
    fn devices_and_addresses() {
        let doc = LoroDoc::new();
        let record = DeviceRecord { name: "Phone".into(), platform: "android".into(), added_at: now_iso(), added_by: "a".into(), removed: false };
        write_device(&doc, "a", &record).unwrap();
        assert_eq!(read_devices(&doc), vec![("a".to_owned(), record)]);
        let addrs: Vec<std::net::SocketAddr> = vec!["192.168.1.9:47120".parse().unwrap(), "192.168.1.2:47120".parse().unwrap()];
        assert!(write_addresses(&doc, "a", &addrs).unwrap());
        assert!(!write_addresses(&doc, "a", &addrs.iter().rev().copied().collect::<Vec<_>>()).unwrap(), "order doesn't matter");
        assert_eq!(read_addresses(&doc, "a")[0].to_string(), "192.168.1.2:47120");
    }
}
