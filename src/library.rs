//! The library as Mascarpone sees it: albums with their tracks, artists with
//! their albums, built from mpd's listing of every song, and
//! the playlists read from the m3u files in the music folder's
//! `Playlists`. Plain data, built on the mpd thread and shared with the
//! window as is.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard};
use std::time::Duration;

use mpd_client::responses::Song;
use mpd_client::tag::Tag;

#[derive(Debug)]
pub struct Track {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub disc: u64,
    pub number: u64,
    pub duration: Option<Duration>,
}

#[derive(Debug)]
pub struct Album {
    /// Album artist and title together: what tells one album from another,
    /// and the name of its cover in the cache. For a folder of loose
    /// tracks, an empty artist, "folder" and the folder's path.
    pub key: String,
    pub title: String,
    pub artist: String,
    pub year: Option<String>,
    pub tracks: Vec<Track>,
    /// Not an album but a folder: the tracks there without an album tag.
    pub loose: bool,
}

impl Album {
    /// A file of the album, the one mpd is asked the cover for.
    pub fn cover_uri(&self) -> &str {
        &self.tracks[0].uri
    }

    pub fn duration(&self) -> Duration {
        self.tracks.iter().filter_map(|t| t.duration).sum()
    }

    pub fn uris(&self) -> Vec<String> {
        self.tracks.iter().map(|t| t.uri.clone()).collect()
    }

    /// Discs in the album, 1 when untagged.
    pub fn discs(&self) -> u64 {
        self.tracks.iter().map(|t| t.disc).max().unwrap_or(0).max(1)
    }
}

#[derive(Debug)]
pub struct Artist {
    pub name: String,
    /// Indexes into `Library::albums`.
    pub albums: Vec<usize>,
}

/// A playlist's entry: the track as the library knows it, or, for a file
/// the library does not have, just its name from the path.
#[derive(Debug, Clone)]
pub struct Entry {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: Option<usize>,
    pub duration: Option<Duration>,
    /// In the library, so mpd can play it.
    pub found: bool,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub name: String,
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

impl Playlist {
    pub fn duration(&self) -> Duration {
        self.entries.iter().filter_map(|e| e.duration).sum()
    }

    /// The playable entries' uris, in order.
    pub fn uris(&self) -> Vec<String> {
        self.entries.iter().filter(|e| e.found).map(|e| e.uri.clone()).collect()
    }

    pub fn missing(&self) -> usize {
        self.entries.iter().filter(|e| !e.found).count()
    }
}

#[derive(Debug, Default)]
pub struct Library {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    /// Behind a lock: they are read again when Mascarpone writes one, the
    /// rest of the library stays.
    playlists: RwLock<Vec<Playlist>>,
    /// Album index by key, artist index by name, a track's place by uri.
    album_by_key: HashMap<String, usize>,
    artist_by_name: HashMap<String, usize>,
    by_uri: HashMap<String, (usize, usize)>,
}

/// Where the music is, mpd's `music_directory` too: the configuration's,
/// else the user's Music folder. The playlists are m3u files in their
/// folder, by default `Playlists` inside it.
pub fn music_dir() -> PathBuf {
    crate::config::get().music.clone()
}

pub fn playlists_dir() -> PathBuf {
    crate::config::get().playlists.clone()
}

const UNKNOWN_ARTIST: &str = "Unknown artist";

/// The artist an album is filed under: the album artist, else the artist.
fn album_artist(song: &Song) -> &str {
    song.album_artists()
        .first()
        .or_else(|| song.artists().first())
        .map(String::as_str)
        .unwrap_or(UNKNOWN_ARTIST)
}

/// The key of the album a song belongs to; for a song without an album,
/// the folder it is in, which stands in for one.
pub fn album_key(song: &Song) -> Option<String> {
    match song.album() {
        Some(album) => Some(format!("{}\u{1f}{}", album_artist(song), album)),
        None => Some(format!("\u{1f}folder\u{1f}{}", song.file_path().parent()?.to_string_lossy())),
    }
}

/// What a name sorts by: as written, case aside ("The Bad Plus" under T).
fn sort_name(name: &str) -> String {
    name.to_lowercase()
}

impl Library {
    pub fn build(songs: Vec<Song>) -> Library {
        let mut albums: Vec<Album> = Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for song in songs {
            let Some(key) = album_key(&song) else { continue };
            let index = *by_key.entry(key.clone()).or_insert_with(|| {
                let loose = song.album().is_none();
                albums.push(Album {
                    key,
                    title: match song.album() {
                        Some(album) => album.to_owned(),
                        // the folder's name, or the root's
                        None => song
                            .file_path()
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Loose tracks".to_owned()),
                    },
                    artist: album_artist(&song).to_owned(),
                    year: None,
                    tracks: Vec::new(),
                    loose,
                });
                albums.len() - 1
            });
            let album = &mut albums[index];
            if album.year.is_none() {
                let date = song
                    .tags
                    .get(&Tag::OriginalDate)
                    .or_else(|| song.tags.get(&Tag::Date))
                    .and_then(|v| v.first());
                // a date of 0000 is no date
                album.year = date.map(|d| d.chars().take(4).collect::<String>()).filter(|y| y.parse::<u32>().is_ok_and(|n| n > 0));
            }
            let (disc, number) = song.number();
            album.tracks.push(Track {
                title: song
                    .title()
                    .map(str::to_owned)
                    .unwrap_or_else(|| song.file_path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
                artist: song.artists().first().cloned().unwrap_or_else(|| album.artist.clone()),
                disc,
                number,
                duration: song.duration,
                uri: song.url,
            });
        }

        for album in &mut albums {
            album.tracks.sort_by_key(|t| (t.disc, t.number));
        }
        // albums by title; artists by name, each one's albums by year
        albums.sort_by_cached_key(|a| (sort_name(&a.title), sort_name(&a.artist)));
        let mut by_artist: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, album) in albums.iter().enumerate() {
            by_artist.entry(&album.artist).or_default().push(i);
        }
        let mut artists: Vec<Artist> = by_artist
            .into_iter()
            .map(|(name, mut list)| {
                list.sort_by_cached_key(|&i| (albums[i].year.clone().unwrap_or_default(), sort_name(&albums[i].title)));
                Artist { name: name.to_owned(), albums: list }
            })
            .collect();
        artists.sort_by_cached_key(|a| sort_name(&a.name));

        let album_by_key = albums.iter().enumerate().map(|(i, a)| (a.key.clone(), i)).collect();
        let artist_by_name = artists.iter().enumerate().map(|(i, a)| (a.name.clone(), i)).collect();
        let by_uri = albums.iter().enumerate().flat_map(|(a, album)| album.tracks.iter().enumerate().map(move |(t, track)| (track.uri.clone(), (a, t)))).collect();
        let library = Library { albums, artists, playlists: RwLock::new(Vec::new()), album_by_key, artist_by_name, by_uri };
        let playlists = library.read_playlists();
        *library.playlists.write().unwrap() = playlists;
        library
    }

    pub fn playlists(&self) -> RwLockReadGuard<'_, Vec<Playlist>> {
        self.playlists.read().unwrap()
    }

    /// The playlist files read again, against this library's tracks.
    pub fn read_playlists(&self) -> Vec<Playlist> {
        read_playlists(self, &music_dir(), &playlists_dir())
    }

    pub fn set_playlists(&self, playlists: Vec<Playlist>) {
        *self.playlists.write().unwrap() = playlists;
    }

    pub fn album_by_key(&self, key: &str) -> Option<&Album> {
        self.album_by_key.get(key).map(|&i| &self.albums[i])
    }

    /// The album a track is in, and its place there.
    pub fn place(&self, uri: &str) -> Option<(usize, usize)> {
        self.by_uri.get(uri).copied()
    }

    /// The folder an album lives in alone: all its tracks there, no other
    /// album's. Where a cover file can go without lying to anyone.
    pub fn own_folder(&self, key: &str) -> Option<PathBuf> {
        let album = self.album_by_key(key)?;
        let folder = Path::new(&album.tracks[0].uri).parent()?.to_path_buf();
        if album.tracks.iter().any(|t| Path::new(&t.uri).parent() != Some(&folder)) {
            return None;
        }
        let shared = self
            .albums
            .iter()
            .filter(|a| a.key != album.key)
            .any(|a| a.tracks.iter().any(|t| Path::new(&t.uri).parent() == Some(&folder)));
        if shared {
            return None;
        }
        Some(folder)
    }

    pub fn artist(&self, name: &str) -> Option<usize> {
        self.artist_by_name.get(name).copied()
    }

    pub fn tracks(&self) -> usize {
        self.albums.iter().map(|a| a.tracks.len()).sum()
    }
}

/// The m3u files of `dir`, by name. An entry's path is relative to the
/// file (or absolute); inside the music folder it is an mpd uri, and the
/// library's track when it has it.
fn read_playlists(library: &Library, music: &Path, dir: &Path) -> Vec<Playlist> {
    let Ok(files) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut playlists: Vec<Playlist> = files
        .flatten()
        .map(|f| f.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("m3u" | "m3u8")))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let entries = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|line| {
                    let file = normalize(&dir.join(line));
                    let uri = file.strip_prefix(music).ok().map(|u| u.to_string_lossy().into_owned());
                    match uri.as_deref().and_then(|u| library.place(u)) {
                        Some((album, t)) => {
                            let track = &library.albums[album].tracks[t];
                            Entry {
                                uri: track.uri.clone(),
                                title: track.title.clone(),
                                artist: track.artist.clone(),
                                album: Some(album),
                                duration: track.duration,
                                found: true,
                            }
                        }
                        None => Entry {
                            uri: uri.unwrap_or_else(|| line.to_owned()),
                            title: file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                            artist: String::new(),
                            album: None,
                            duration: None,
                            found: false,
                        },
                    }
                })
                .collect();
            Some(Playlist { name, path, entries })
        })
        .collect();
    playlists.sort_by_cached_key(|p| sort_name(&p.name));
    playlists
}

/// The path without `.` and `..`, lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// m:ss, or h:mm:ss from an hour up.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
