//! Playlists, Liked songs and play history (PLAN.md §4), in the replicated document. Every
//! change is durable before it returns and reaches the other devices like any other change.

use anyhow::Result;

use super::Node;
use crate::{
    library::TrackRef,
    model::{self, Playlist, PlayRecord},
};

impl Node {
    pub fn playlists(&self) -> Vec<Playlist> {
        model::read_playlists(&self.0.state.lock().unwrap().doc)
    }

    /// Returns the new playlist's id.
    pub async fn create_playlist(&self, name: String, tracks: Vec<TrackRef>) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let pid = id.clone();
        self.mutate(move |doc| {
            model::create_playlist(doc, &pid, &name)?;
            model::playlist_add(doc, &pid, &tracks, None)
        })
        .await?;
        Ok(id)
    }

    pub async fn rename_playlist(&self, id: String, name: String) -> Result<()> {
        self.mutate(move |doc| model::rename_playlist(doc, &id, &name)).await
    }

    pub async fn delete_playlist(&self, id: String) -> Result<()> {
        self.mutate(move |doc| model::delete_playlist(doc, &id)).await
    }

    pub async fn playlist_add(&self, id: String, tracks: Vec<TrackRef>, at: Option<usize>) -> Result<()> {
        self.mutate(move |doc| model::playlist_add(doc, &id, &tracks, at)).await
    }

    pub async fn playlist_remove(&self, id: String, positions: Vec<usize>) -> Result<()> {
        self.mutate(move |doc| model::playlist_remove(doc, &id, &positions)).await
    }

    pub async fn playlist_move(&self, id: String, from: usize, to: usize) -> Result<()> {
        self.mutate(move |doc| model::playlist_move(doc, &id, from, to)).await
    }

    /// Liked songs: (reference, liked since).
    pub fn favorites(&self) -> Vec<(TrackRef, String)> {
        model::read_favorites(&self.0.state.lock().unwrap().doc)
    }

    pub async fn like(&self, track: TrackRef) -> Result<()> {
        self.mutate(move |doc| model::like(doc, &track)).await
    }

    /// `ids`: every stored key that resolves to the track (the caller resolves them).
    pub async fn unlike(&self, ids: Vec<String>) -> Result<()> {
        self.mutate(move |doc| model::unlike(doc, &ids)).await
    }

    /// Counts a play on this device.
    pub async fn record_play(&self, track: TrackRef) -> Result<()> {
        let me = self.0.id.to_string();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.mutate(move |doc| model::record_play(doc, &me, &track, now)).await
    }

    /// Every device's play records: (device, track id, record).
    pub fn plays(&self) -> Vec<(String, String, PlayRecord)> {
        model::read_plays(&self.0.state.lock().unwrap().doc)
    }
}

impl Node {
    /// Hidden songs (references) and folders ("<volume>/<dir>").
    pub fn hidden(&self) -> (Vec<TrackRef>, Vec<String>) {
        let st = self.0.state.lock().unwrap();
        (model::read_hidden(&st.doc), model::read_hidden_folders(&st.doc))
    }

    pub async fn hide(&self, tracks: Vec<TrackRef>) -> Result<()> {
        self.mutate(move |doc| tracks.iter().try_for_each(|r| model::hide(doc, r))).await
    }

    /// `ids`: every stored key that resolves to the tracks shown again.
    pub async fn unhide(&self, ids: Vec<String>) -> Result<()> {
        self.mutate(move |doc| model::unhide(doc, &ids)).await
    }

    pub async fn set_folder_hidden(&self, folder: String, hidden: bool) -> Result<()> {
        self.mutate(move |doc| model::set_folder_hidden(doc, &folder, hidden)).await
    }
}
