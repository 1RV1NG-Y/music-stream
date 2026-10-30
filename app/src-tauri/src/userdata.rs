//! The `music` commands that are the same on the PC and the phone: the library views,
//! playlists, Liked songs, hiding songs and folders. They work on whichever index the platform
//! has: the PC's cached copy of the phone's, or the phone's own.

use std::collections::HashSet;

use anyhow::{Context, Result};
use musicstream_core::{
    Node,
    library::{Index, Track, TrackId, TrackRef, parse_hex},
    views,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Params {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    at: Option<usize>,
    #[serde(default)]
    positions: Vec<usize>,
    #[serde(default)]
    from: usize,
    #[serde(default)]
    to: usize,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    hidden: bool,
}

pub fn tracks(index: &Index, ids: &[String]) -> Result<Vec<Track>> {
    ids.iter()
        .map(|id| {
            let id = parse_hex::<16>(id)?;
            index.resolve(&id).cloned().context("That track is no longer in the library.")
        })
        .collect()
}

fn refs(index: &Index, ids: &[String]) -> Result<Vec<TrackRef>> {
    Ok(tracks(index, ids)?.iter().map(Track::reference).collect())
}

fn targets(index: &Index, ids: &[String]) -> Result<HashSet<TrackId>> {
    Ok(tracks(index, ids)?.iter().map(|t| t.id).collect())
}

/// `None` when `method` isn't one of these.
pub async fn dispatch(node: &Node, index: &Index, cached: impl Fn(&Track) -> bool, method: &str, p: Value) -> Option<Result<Value>> {
    let params = || -> Result<Params> { Ok(serde_json::from_value(p.clone())?) };
    let result = match method {
        "library" => Ok(views::library(&views::Context::new(node, index), cached)),
        "playlists" => Ok(views::playlists(&views::Context::new(node, index), node)),
        "folders" => Ok(views::folders(&views::Context::new(node, index))),
        "createPlaylist" => async {
            let p = params()?;
            let id = node.create_playlist(p.name, refs(index, &p.ids)?).await?;
            Ok(json!({ "id": id }))
        }
        .await,
        "renamePlaylist" => async {
            let p = params()?;
            node.rename_playlist(p.id, p.name).await?;
            Ok(Value::Null)
        }
        .await,
        "deletePlaylist" => async {
            node.delete_playlist(params()?.id).await?;
            Ok(Value::Null)
        }
        .await,
        "playlistAdd" => async {
            let p = params()?;
            node.playlist_add(p.id, refs(index, &p.ids)?, p.at).await?;
            Ok(Value::Null)
        }
        .await,
        "playlistRemove" => async {
            let p = params()?;
            node.playlist_remove(p.id, p.positions).await?;
            Ok(Value::Null)
        }
        .await,
        "playlistMove" => async {
            let p = params()?;
            node.playlist_move(p.id, p.from, p.to).await?;
            Ok(Value::Null)
        }
        .await,
        "like" => async {
            for r in refs(index, &params()?.ids)? {
                node.like(r).await?;
            }
            Ok(Value::Null)
        }
        .await,
        "unlike" => async {
            let t = targets(index, &params()?.ids)?;
            node.unlike(views::liked_keys(node, index, &t)).await?;
            Ok(Value::Null)
        }
        .await,
        "hide" => async {
            node.hide(refs(index, &params()?.ids)?).await?;
            Ok(Value::Null)
        }
        .await,
        "unhide" => async {
            let t = targets(index, &params()?.ids)?;
            node.unhide(views::hidden_keys(node, index, &t)).await?;
            Ok(Value::Null)
        }
        .await,
        "setFolderHidden" => async {
            let p = params()?;
            node.set_folder_hidden(p.folder, p.hidden).await?;
            Ok(Value::Null)
        }
        .await,
        _ => return None,
    };
    Some(result)
}
