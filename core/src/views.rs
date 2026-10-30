//! What the apps show (PLAN.md §7), built the same way on the PC (from the cached index) and on
//! the phone (from its own library): tracks with likes, plays and hidden state, playlists,
//! the automatic playlists, and the folders that make up the library.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

use crate::{
    Node,
    library::{Index, Track, TrackId, TrackRef, hex},
};

const DAY_MS: u64 = 86_400_000;

/// Plays of one track, summed over every device.
#[derive(Default, Clone)]
pub struct Stats {
    pub count: u64,
    pub last: u64,
    pub recent: Vec<u32>,
}

/// User data resolved against one index.
pub struct Context<'a> {
    pub index: &'a Index,
    pub stats: HashMap<TrackId, Stats>,
    /// Liked tracks, most recently liked first.
    pub liked: Vec<TrackId>,
    hidden: HashSet<TrackId>,
    hidden_folders: Vec<String>,
}

impl<'a> Context<'a> {
    pub fn new(node: &Node, index: &'a Index) -> Self {
        let mut stats: HashMap<TrackId, Stats> = HashMap::new();
        for (_, id, rec) in node.plays() {
            let r = TrackRef { id, fallback: rec.fallback.clone() };
            let Some(t) = index.resolve_ref(&r) else { continue };
            let s = stats.entry(t.id).or_default();
            s.count += rec.count;
            s.last = s.last.max(rec.last);
            s.recent.extend(rec.recent);
        }
        let mut favorites = node.favorites();
        favorites.sort_by(|a, b| b.1.cmp(&a.1));
        let mut seen = HashSet::new();
        let liked = favorites.iter().filter_map(|(r, _)| index.resolve_ref(r)).map(|t| t.id).filter(|id| seen.insert(*id)).collect();
        let (hidden_refs, hidden_folders) = node.hidden();
        let hidden = hidden_refs.iter().filter_map(|r| index.resolve_ref(r)).map(|t| t.id).collect();
        Context { index, stats, liked, hidden, hidden_folders }
    }

    /// Hidden by itself, or under a folder switched off.
    pub fn is_hidden(&self, t: &Track) -> bool {
        self.hidden.contains(&t.id) || self.hidden_folders.iter().any(|f| in_folder(t, f))
    }

    fn shown(&self) -> impl Iterator<Item = &Track> {
        self.index.tracks.iter().filter(|t| !self.is_hidden(t))
    }
}

/// "<volume>/<dir>" of a track ("<volume>" for one at the volume's top).
pub fn folder_of(t: &Track) -> String {
    match t.path.rsplit_once('/') {
        Some((dir, _)) => format!("{}/{dir}", t.volume),
        None => t.volume.clone(),
    }
}

fn in_folder(t: &Track, folder: &str) -> bool {
    let own = folder_of(t);
    own == folder || own.starts_with(&format!("{folder}/"))
}

pub fn track_view(ctx: &Context, t: &Track, cached: bool) -> Value {
    let stats = ctx.stats.get(&t.id);
    json!({
        "id": hex(&t.id),
        "title": t.title,
        "artist": t.artist,
        "album": t.album,
        "albumArtist": t.album_artist,
        "disc": t.disc,
        "track": t.track,
        "year": t.year,
        "genre": t.genre,
        "durationMs": t.duration_ms,
        "format": t.format,
        "sampleRate": t.sample_rate,
        "bitDepth": t.bit_depth,
        "channels": t.channels,
        "size": t.size(),
        "path": t.path,
        "folder": folder_of(t),
        "art": t.art.map(|a| hex(&a)),
        "cached": cached,
        "liked": ctx.liked.contains(&t.id),
        "hidden": ctx.is_hidden(t),
        "plays": stats.map_or(0, |s| s.count),
        "lastPlayed": stats.map_or(0, |s| s.last),
        // The file's modification time on the phone: when it was added, near enough.
        "added": t.fp.mtime.0,
    })
}

/// Every track, hidden ones included (flagged), so the Hidden and Folders pages can show them.
pub fn library(ctx: &Context, cached: impl Fn(&Track) -> bool) -> Value {
    let tracks: Vec<Value> = ctx.index.tracks.iter().map(|t| track_view(ctx, t, cached(t))).collect();
    json!({ "tracks": tracks, "generation": ctx.index.generation })
}

/// Liked songs, the automatic playlists and the user's playlists, as track ids (hidden left out).
pub fn playlists(ctx: &Context, node: &Node) -> Value {
    let visible = |id: &TrackId| ctx.index.tracks.iter().find(|t| &t.id == id).is_some_and(|t| !ctx.is_hidden(t));
    let user: Vec<Value> = node
        .playlists()
        .into_iter()
        .map(|pl| {
            let entries: Vec<Value> = pl
                .entries
                .iter()
                .map(|r| match ctx.index.resolve_ref(r) {
                    Some(t) if ctx.is_hidden(t) => json!({ "id": null, "hidden": true }),
                    Some(t) => json!({ "id": hex(&t.id) }),
                    None => json!({ "id": null, "title": r.fallback.title, "artist": r.fallback.album_artist }),
                })
                .collect();
            json!({ "id": pl.id, "name": pl.name, "created": pl.created, "entries": entries })
        })
        .collect();
    let liked: Vec<String> = ctx.liked.iter().filter(|id| visible(id)).map(|id| hex(id)).collect();
    json!({ "liked": liked, "smart": smart_playlists(ctx), "user": user })
}

/// The automatic playlists (PLAN.md §7), as track ids in order.
fn smart_playlists(ctx: &Context) -> Vec<Value> {
    let today = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) / DAY_MS) as u32;
    let shown: HashSet<TrackId> = ctx.shown().map(|t| t.id).collect();
    let stats: Vec<(&TrackId, &Stats)> = ctx.stats.iter().filter(|(id, _)| shown.contains(*id)).collect();
    let mut on_repeat: Vec<(&TrackId, &Stats, usize)> =
        stats.iter().map(|(id, s)| (*id, *s, s.recent.iter().filter(|d| **d + 30 >= today).count())).filter(|(_, _, n)| *n >= 2).collect();
    on_repeat.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.last.cmp(&a.1.last)));
    let mut recent = stats.clone();
    recent.sort_by(|a, b| b.1.last.cmp(&a.1.last));
    let mut most: Vec<(&TrackId, &Stats)> = stats.iter().copied().filter(|(_, s)| s.count >= 2).collect();
    most.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(b.1.last.cmp(&a.1.last)));
    let mut added: Vec<&Track> = ctx.shown().collect();
    added.sort_by(|a, b| b.fp.mtime.cmp(&a.fp.mtime));
    let ids = |v: Vec<&TrackId>| -> Vec<String> { v.into_iter().map(|id| hex(id)).collect() };
    vec![
        json!({ "id": "on-repeat", "name": "On Repeat", "hint": "Your most played songs of the last 30 days.",
                "tracks": ids(on_repeat.into_iter().take(50).map(|(id, _, _)| id).collect()) }),
        json!({ "id": "recently-played", "name": "Recently played", "hint": "What you listened to last.",
                "tracks": ids(recent.into_iter().take(100).map(|(id, _)| id).collect()) }),
        json!({ "id": "recently-added", "name": "Recently added", "hint": "The newest songs on your phone.",
                "tracks": added.into_iter().take(100).map(|t| hex(&t.id)).collect::<Vec<_>>() }),
        json!({ "id": "most-played", "name": "Most played", "hint": "Your most played songs of all time.",
                "tracks": ids(most.into_iter().take(100).map(|(id, _)| id).collect()) }),
    ]
}

/// Every folder that holds audio, with how many songs are directly in it and below it, and
/// whether it's switched off (itself, or through a folder above it).
pub fn folders(ctx: &Context) -> Value {
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for t in &ctx.index.tracks {
        let own = folder_of(t);
        counts.entry(own.clone()).or_default().0 += 1;
        // Every ancestor counts it too, so a parent folder can be switched off as a whole.
        let mut dir = own.as_str();
        loop {
            counts.entry(dir.to_owned()).or_default().1 += 1;
            match dir.rsplit_once('/') {
                Some((up, _)) => dir = up,
                None => break,
            }
        }
    }
    let off = |f: &str| ctx.hidden_folders.iter().any(|h| f == h || f.starts_with(&format!("{h}/")));
    let list: Vec<Value> = counts
        .into_iter()
        .map(|(f, (own, total))| json!({ "folder": f, "songs": own, "total": total, "off": off(&f), "switchedOff": ctx.hidden_folders.contains(&f) }))
        .collect();
    json!({ "folders": list, "hiddenSongs": ctx.hidden.len() })
}

/// Keys of hidden entries that resolve to these tracks (to show them again).
pub fn hidden_keys(node: &Node, index: &Index, targets: &HashSet<TrackId>) -> Vec<String> {
    node.hidden().0.into_iter().filter(|r| index.resolve_ref(r).is_some_and(|t| targets.contains(&t.id))).map(|r| r.id).collect()
}

/// Keys of Liked songs entries that resolve to these tracks (to unlike them).
pub fn liked_keys(node: &Node, index: &Index, targets: &HashSet<TrackId>) -> Vec<String> {
    node.favorites().into_iter().filter(|(r, _)| index.resolve_ref(r).is_some_and(|t| targets.contains(&t.id))).map(|(r, _)| r.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Fingerprint;

    fn track(path: &str) -> Track {
        Track {
            id: crate::library::track_id("v", path),
            rev: 1,
            volume: "v".into(),
            path: path.into(),
            fp: Fingerprint { size: 1, mtime: (0, 0), ctime: (0, 0), ino: 0, sample: [0; 32] },
            format: "flac".into(),
            duration_ms: 1000,
            sample_rate: 44100,
            bit_depth: 16,
            channels: 2,
            title: path.into(),
            artist: String::new(),
            album: String::new(),
            album_artist: String::new(),
            disc: 0,
            track: 0,
            year: 0,
            genre: String::new(),
            replay_gain: Default::default(),
            art: None,
        }
    }

    #[test]
    fn folders_count_and_hide_with_their_subfolders() {
        let index = Index { generation: 1, tracks: vec![track("Music/a.flac"), track("Music/Live/b.flac"), track("Download/note.m4a"), track("top.mp3")], aliases: vec![] };
        let ctx = Context { index: &index, stats: HashMap::new(), liked: vec![], hidden: HashSet::new(), hidden_folders: vec!["v/Music".into()] };
        assert!(ctx.is_hidden(&index.tracks[0]) && ctx.is_hidden(&index.tracks[1]));
        assert!(!ctx.is_hidden(&index.tracks[2]) && !ctx.is_hidden(&index.tracks[3]));
        let f = folders(&ctx);
        let row = |name: &str| f["folders"].as_array().unwrap().iter().find(|r| r["folder"] == name).unwrap().clone();
        assert_eq!((row("v/Music")["songs"].as_u64(), row("v/Music")["total"].as_u64()), (Some(1), Some(2)));
        assert_eq!(row("v")["total"].as_u64(), Some(4));
        assert_eq!(row("v/Music/Live")["off"], true);
        assert_eq!(row("v/Music/Live")["switchedOff"], false);
        // "v/Mus" is not a parent of "v/Music".
        let ctx = Context { hidden_folders: vec!["v/Mus".into()], ..ctx };
        assert!(!ctx.is_hidden(&index.tracks[0]));
    }
}
