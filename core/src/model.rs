//! How user data is laid out inside the replicated Loro document (PLAN.md §4).
//!
//! - `settings`: setting name → JSON value, last-writer-wins.
//! - `devices` holds membership; `addresses` each device's current LAN addresses.
//! - `library`: `head` → hex hash of the served library's current index, written by the
//!   device that serves it after a rescan, so other devices know to fetch it again.
//!
//! - `playlists`: playlist id → map { `name`, `created`, `entries`: movable list of JSON
//!   [`TrackRef`] }. Order is the user's; concurrent edits on two PCs merge.
//! - `favorites`: track id → JSON { `since`, `fallback` } (Liked songs; remove = unlike).
//! - `plays`: device id → map of track id → JSON [`PlayRecord`]. Each device writes only its
//!   own map, one entry per track, so counts merge without conflicts (PLAN.md §4).

use anyhow::{Result, bail};
use loro::{LoroDoc, LoroMap, LoroMovableList, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::TrackRef;

pub const SETTINGS: &str = "settings";
pub const DEVICES: &str = "devices";
/// Map of endpoint id → JSON list of socket addresses, written only by that device.
pub const ADDRESSES: &str = "addresses";
pub const LIBRARY: &str = "library";
pub const PLAYLISTS: &str = "playlists";
pub const FAVORITES: &str = "favorites";
pub const PLAYS: &str = "plays";

/// Plays remembered per track and device, for "in the last 30 days" lists.
const RECENT_PLAYS: usize = 32;
const MAX_NAME: usize = 200;

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

// ---- Library head -------------------------------------------------------------------------

pub fn read_library_head(doc: &LoroDoc) -> Option<String> {
    match doc.get_map(LIBRARY).get("head") {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

/// Returns whether it changed. The caller commits.
pub fn write_library_head(doc: &LoroDoc, hash: &str) -> Result<bool> {
    if read_library_head(doc).as_deref() == Some(hash) {
        return Ok(false);
    }
    doc.get_map(LIBRARY).insert("head", hash)?;
    Ok(true)
}

// ---- Playlists ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub created: String,
    pub entries: Vec<TrackRef>,
}

fn playlist_map(doc: &LoroDoc, id: &str) -> Result<LoroMap> {
    match doc.get_map(PLAYLISTS).get(id) {
        Some(ValueOrContainer::Container(c)) => c.into_map().map_err(|_| anyhow::anyhow!("That playlist is damaged.")),
        _ => bail!("That playlist no longer exists."),
    }
}

fn entries(map: &LoroMap) -> Result<LoroMovableList> {
    map.ensure_mergeable_movable_list("entries").map_err(Into::into)
}

fn text(map: &LoroMap, key: &str) -> String {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
        _ => String::new(),
    }
}

fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Give the playlist a name.");
    }
    Ok(name.chars().take(MAX_NAME).collect())
}

pub fn read_playlists(doc: &LoroDoc) -> Vec<Playlist> {
    let all = doc.get_map(PLAYLISTS);
    let mut out: Vec<Playlist> = all
        .keys()
        .filter_map(|id| {
            let map = playlist_map(doc, &id).ok()?;
            let entries = match map.get("entries") {
                Some(ValueOrContainer::Container(c)) => c
                    .into_movable_list()
                    .ok()
                    .map(|l| (0..l.len()).filter_map(|i| match l.get(i) {
                        Some(ValueOrContainer::Value(LoroValue::String(s))) => serde_json::from_str(&s).ok(),
                        _ => None,
                    }).collect())
                    .unwrap_or_default(),
                _ => vec![],
            };
            Some(Playlist { id: id.to_string(), name: text(&map, "name"), created: text(&map, "created"), entries })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.created.cmp(&b.created)));
    out
}

/// Creates a playlist with `id`. The caller commits.
pub fn create_playlist(doc: &LoroDoc, id: &str, name: &str) -> Result<()> {
    let name = check_name(name)?;
    let map = doc.get_map(PLAYLISTS).ensure_mergeable_map(id)?;
    map.insert("name", name)?;
    map.insert("created", now_iso())?;
    entries(&map)?;
    Ok(())
}

pub fn rename_playlist(doc: &LoroDoc, id: &str, name: &str) -> Result<()> {
    playlist_map(doc, id)?.insert("name", check_name(name)?)?;
    Ok(())
}

pub fn delete_playlist(doc: &LoroDoc, id: &str) -> Result<()> {
    playlist_map(doc, id)?;
    doc.get_map(PLAYLISTS).delete(id)?;
    Ok(())
}

/// Adds tracks at `at` (the end when `None`).
pub fn playlist_add(doc: &LoroDoc, id: &str, refs: &[TrackRef], at: Option<usize>) -> Result<()> {
    let list = entries(&playlist_map(doc, id)?)?;
    let mut pos = at.unwrap_or(list.len()).min(list.len());
    for r in refs {
        list.insert(pos, serde_json::to_string(r)?)?;
        pos += 1;
    }
    Ok(())
}

pub fn playlist_remove(doc: &LoroDoc, id: &str, positions: &[usize]) -> Result<()> {
    let list = entries(&playlist_map(doc, id)?)?;
    let mut positions = positions.to_vec();
    positions.sort_unstable();
    positions.dedup();
    for p in positions.into_iter().rev() {
        if p < list.len() {
            list.delete(p, 1)?;
        }
    }
    Ok(())
}

pub fn playlist_move(doc: &LoroDoc, id: &str, from: usize, to: usize) -> Result<()> {
    let list = entries(&playlist_map(doc, id)?)?;
    if from >= list.len() || to >= list.len() {
        bail!("No such entry.");
    }
    if from != to {
        list.mov(from, to)?;
    }
    Ok(())
}

// ---- Liked songs ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub since: String,
    pub fallback: crate::library::Fallback,
}

pub fn read_favorites(doc: &LoroDoc) -> Vec<(TrackRef, String)> {
    let m = doc.get_map(FAVORITES);
    m.keys()
        .filter_map(|k| json_value::<Favorite>(&m, &k).map(|f| (TrackRef { id: k.to_string(), fallback: f.fallback }, f.since)))
        .collect()
}

pub fn like(doc: &LoroDoc, r: &TrackRef) -> Result<()> {
    if doc.get_map(FAVORITES).get(&r.id).is_none() {
        doc.get_map(FAVORITES).insert(&r.id, serde_json::to_string(&Favorite { since: now_iso(), fallback: r.fallback.clone() })?)?;
    }
    Ok(())
}

/// Removes these keys (every key that resolves to the track being unliked).
pub fn unlike(doc: &LoroDoc, ids: &[String]) -> Result<()> {
    for id in ids {
        if doc.get_map(FAVORITES).get(id).is_some() {
            doc.get_map(FAVORITES).delete(id)?;
        }
    }
    Ok(())
}

// ---- Play history ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayRecord {
    pub count: u64,
    /// Milliseconds since the Unix epoch.
    pub last: u64,
    /// Days since the Unix epoch of the most recent plays, oldest first.
    pub recent: Vec<u32>,
    pub fallback: crate::library::Fallback,
}

/// Counts a play of `r` on device `me` at `now_ms`. The caller commits.
pub fn record_play(doc: &LoroDoc, me: &str, r: &TrackRef, now_ms: u64) -> Result<()> {
    let device = doc.get_map(PLAYS).ensure_mergeable_map(me)?;
    let mut rec: PlayRecord = json_value(&device, &r.id).unwrap_or_default();
    rec.count += 1;
    rec.last = now_ms;
    rec.recent.push((now_ms / 86_400_000) as u32);
    if rec.recent.len() > RECENT_PLAYS {
        let extra = rec.recent.len() - RECENT_PLAYS;
        rec.recent.drain(..extra);
    }
    rec.fallback = r.fallback.clone();
    device.insert(&r.id, serde_json::to_string(&rec)?)?;
    Ok(())
}

/// Every device's records: (device, track id, record).
pub fn read_plays(doc: &LoroDoc) -> Vec<(String, String, PlayRecord)> {
    let all = doc.get_map(PLAYS);
    let mut out = vec![];
    for device in all.keys() {
        if let Some(ValueOrContainer::Container(c)) = all.get(&device) {
            if let Ok(m) = c.into_map() {
                for k in m.keys() {
                    if let Some(rec) = json_value::<PlayRecord>(&m, &k) {
                        out.push((device.to_string(), k.to_string(), rec));
                    }
                }
            }
        }
    }
    out
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

    fn r(id: &str) -> TrackRef {
        TrackRef { id: id.into(), fallback: crate::library::Fallback { title: id.into(), ..Default::default() } }
    }

    fn ids(p: &Playlist) -> Vec<&str> {
        p.entries.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn playlists_merge_edits_made_apart() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        create_playlist(&a, "p1", "Gym").unwrap();
        playlist_add(&a, "p1", &[r("x"), r("y"), r("z")], None).unwrap();
        a.commit();
        sync(&a, &b);
        // Apart: A moves z to the front; B appends w and renames.
        playlist_move(&a, "p1", 2, 0).unwrap();
        a.commit();
        playlist_add(&b, "p1", &[r("w")], None).unwrap();
        rename_playlist(&b, "p1", "Gym 2").unwrap();
        b.commit();
        sync(&a, &b);
        for d in [&a, &b] {
            let p = &read_playlists(d)[0];
            assert_eq!((p.name.as_str(), ids(p)), ("Gym 2", vec!["z", "x", "y", "w"]));
        }
        playlist_remove(&a, "p1", &[0, 2]).unwrap();
        a.commit();
        assert_eq!(ids(&read_playlists(&a)[0]), vec!["x", "w"]);
        assert!(create_playlist(&a, "p2", "  ").is_err());
        delete_playlist(&a, "p1").unwrap();
        assert!(read_playlists(&a).is_empty());
    }

    #[test]
    fn likes_and_play_counts() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        like(&a, &r("x")).unwrap();
        like(&a, &r("x")).unwrap();
        a.commit();
        assert_eq!(read_favorites(&a).len(), 1);
        unlike(&a, &["x".into()]).unwrap();
        assert!(read_favorites(&a).is_empty());

        // Two PCs count their own plays; nothing conflicts.
        for day in 0..40u64 {
            record_play(&a, "pc-linux", &r("x"), day * 86_400_000).unwrap();
        }
        record_play(&b, "pc-windows", &r("x"), 5 * 86_400_000).unwrap();
        a.commit();
        b.commit();
        sync(&a, &b);
        let plays = read_plays(&b);
        assert_eq!(plays.len(), 2);
        let linux = &plays.iter().find(|p| p.0 == "pc-linux").unwrap().2;
        assert_eq!((linux.count, linux.recent.len(), linux.recent[0]), (40, 32, 8), "only recent days are kept");
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
