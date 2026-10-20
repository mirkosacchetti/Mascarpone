//! The master list: mpd's queue, and where it was loaded from. Playing an
//! album or a playlist fills the queue with it and points the master
//! there; appending detaches the pointer and the queue becomes the Anon
//! Playlist, until it is saved as a playlist. The queue is mpd's,
//! other clients change it too: whenever it stops matching the list the
//! pointer came from, the pointer drops. The pointer is kept on disk, so
//! it survives a restart as long as the queue does.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::glyphs;
use crate::library::{self, Library};

/// What the queue is not yet saved as: put together by hand.
pub const ANON: &str = "Anon Playlist";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Empty, never loaded.
    None,
    Album { key: String },
    /// A saved playlist, its file.
    Playlist { path: PathBuf },
    /// The Anon Playlist: put together by hand, not saved.
    Anon,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Master {
    pub source: Source,
    /// The queue as the pointer last knew it.
    pub uris: Vec<String>,
}

fn state_dir() -> PathBuf {
    crate::dirs::state().join("mascarpone")
}

fn state_path() -> PathBuf {
    state_dir().join("master.json")
}

/// Deletes a playlist file, keeping it: moved to
/// `~/.local/state/mascarpone/deleted`, out of the music folder (which
/// syncs), its name stamped with the time. Returns where it went.
pub fn delete_playlist(path: &Path) -> Result<PathBuf> {
    let dir = state_dir().join("deleted");
    std::fs::create_dir_all(&dir)?;
    let stamp = crate::dirs::now("%Y-%m-%d %H.%M.%S");
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kept = dir.join(format!("{stamp} {name}"));
    // a rename, or a copy where the folders are on two disks
    if std::fs::rename(path, &kept).is_err() {
        std::fs::copy(path, &kept)?;
        std::fs::remove_file(path)?;
    }
    Ok(kept)
}

impl Master {
    pub fn load() -> Master {
        std::fs::read(state_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Master { source: Source::None, uris: Vec::new() })
    }

    pub fn save(&self) {
        let path = state_path();
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        if let Ok(json) = serde_json::to_vec(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Loaded whole from a list.
    pub fn point(&mut self, source: Source, uris: Vec<String>) {
        self.source = source;
        self.uris = uris;
        self.save();
    }

    /// Appended to: the pointer drops to the Anon Playlist.
    pub fn append(&mut self, uris: &[String]) {
        self.source = Source::Anon;
        self.uris.extend_from_slice(uris);
        self.save();
    }

    /// The queue as mpd has it now: when it is not what the pointer says,
    /// the pointer drops to the Anon Playlist (or to None when the queue
    /// is empty).
    pub fn reconcile(&mut self, queue: &[String]) {
        if self.uris == queue {
            return;
        }
        self.uris = queue.to_vec();
        self.source = if queue.is_empty() { Source::None } else { Source::Anon };
        self.save();
    }

    /// Name and origin, for the queue's tab.
    pub fn describe(&self, library: &Library) -> (String, String) {
        match &self.source {
            Source::None => ("Queue".into(), "empty".into()),
            // what it is by its glyph beside the name, not by a word
            Source::Album { key } => match library.album_by_key(key) {
                Some(a) => (format!("{}  {}", glyphs::ALBUM, a.title), a.artist.clone()),
                None => ("Queue".into(), glyphs::ALBUM.into()),
            },
            Source::Playlist { path } => (format!("{}  {}", glyphs::PLAYLIST, path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()), String::new()),
            Source::Anon => (format!("{}  {ANON}", glyphs::PLAYLIST), String::new()),
        }
    }

    /// The playlist the queue plays, by name: a saved one's, or the Anon
    /// Playlist; none for an album or an empty queue.
    pub fn playlist(&self) -> Option<String> {
        match &self.source {
            Source::Playlist { path } => Some(path.file_stem()?.to_string_lossy().into_owned()),
            Source::Anon => Some(ANON.into()),
            Source::None | Source::Album { .. } => None,
        }
    }
}

/// A playlist file's line for a uri: relative to the Playlists folder,
/// the way the files are written by hand (`../Artist/Album/track`).
fn line_for(uri: &str) -> String {
    format!("../{uri}")
}

/// The file of a playlist to be, from its name: in the Playlists folder,
/// `.m3u8`, the name's slashes made harmless.
pub fn path_for(name: &str) -> PathBuf {
    library::playlists_dir().join(format!("{}.m3u8", name.trim().replace('/', "-")))
}

/// Writes a new playlist whole.
pub fn write_playlist(path: &Path, uris: &[String]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut text = String::from("#EXTM3U\n");
    for uri in uris {
        text.push_str(&line_for(uri));
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// Whether a playlist plays in a random order: kept in
/// `~/.local/state/mascarpone/shuffle.json` by the playlist's name, not
/// in its file (the files sync, and another machine rewrites them); on
/// unless switched off.
pub fn shuffled(playlist: &Path) -> bool {
    shuffled_in(&shuffle_path(), playlist)
}

pub fn set_shuffle(playlist: &Path, on: bool) -> Result<()> {
    set_shuffle_in(&shuffle_path(), playlist, on)
}

fn shuffle_path() -> PathBuf {
    state_dir().join("shuffle.json")
}

/// The playlists switched, by name.
fn shuffles(file: &Path) -> std::collections::BTreeMap<String, bool> {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn name_of(playlist: &Path) -> String {
    playlist.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn shuffled_in(file: &Path, playlist: &Path) -> bool {
    shuffles(file).get(&name_of(playlist)).copied().unwrap_or(true)
}

fn set_shuffle_in(file: &Path, playlist: &Path, on: bool) -> Result<()> {
    let mut all = shuffles(file);
    all.insert(name_of(playlist), on);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_vec_pretty(&all)?)?;
    Ok(())
}

/// Appends a track to a playlist file.
pub fn append_to_playlist(path: &Path, uri: &str) -> Result<()> {
    use std::io::Write;
    // a last line without its newline gets one first
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if !text.is_empty() && !text.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    writeln!(f, "{}", line_for(uri))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_is_on_until_switched_off() {
        let file = std::env::temp_dir().join(format!("mascarpone-shuffle-{}.json", std::process::id()));
        let list = Path::new("/music/Playlists/Go Radio.m3u8");
        assert!(shuffled_in(&file, list));
        set_shuffle_in(&file, list, false).unwrap();
        assert!(!shuffled_in(&file, list));
        assert!(shuffled_in(&file, Path::new("/music/Playlists/Other.m3u8")));
        set_shuffle_in(&file, list, true).unwrap();
        assert!(shuffled_in(&file, list));
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn a_deleted_playlist_is_kept() {
        let root = std::env::temp_dir().join(format!("mascarpone-delete-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("XDG_STATE_HOME", &root);
        let path = root.join("Gone.m3u8");
        std::fs::write(&path, "#EXTM3U\n../a.flac\n").unwrap();
        let kept = delete_playlist(&path).unwrap();
        assert!(!path.exists());
        assert!(kept.starts_with(root.join("mascarpone").join("deleted")));
        assert!(kept.to_string_lossy().ends_with(" Gone.m3u8"));
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "#EXTM3U\n../a.flac\n");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
