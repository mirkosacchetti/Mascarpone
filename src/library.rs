use std::collections::{HashMap, HashSet};
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
    pub mbid: Option<String>,
}

/// One of the artists an album is credited to, as the tags name it, with
/// the MusicBrainz ids the tags give it: one, usually; none for the
/// untagged; several when the credit is a collaboration the tags did
/// not split, "Charlie Haden & Pat Metheny" with both ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub mbids: Vec<String>,
}

#[derive(Debug)]
pub struct Album {
    pub key: String,
    pub title: String,
    /// The credit as written: "The Velvet Underground & Nico".
    pub artist: String,
    pub year: Option<String>,
    pub tracks: Vec<Track>,
    pub loose: bool,
    pub mbid: Option<String>,
    /// Who the credit names: the credit itself, whole, until the tags
    /// split it (see `split`), then one member a name.
    pub members: Vec<Member>,
}

impl Album {
    pub fn whole(artist: &str, mbids: Vec<String>) -> Vec<Member> {
        vec![Member { name: artist.to_owned(), mbids }]
    }

    /// The names of the members, for the album's artist line.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|m| m.name.as_str())
    }

    /// Splits the credit by its ids, in the tags' order, into the
    /// artists named for them: all of them, or none yet.
    pub fn split_by_ids(&mut self, names: &HashMap<String, String>) -> bool {
        let [Member { mbids, .. }] = self.members.as_slice() else { return false };
        if mbids.len() < 2 {
            return false;
        }
        let Some(found) = mbids.iter().map(|id| names.get(id).cloned()).collect::<Option<Vec<String>>>() else { return false };
        self.split(&found)
    }

    /// Splits a credit without ids on its semicolons, "Anton Webern;
    /// Pierre Boulez": the one join no band's name contains, the one
    /// classical tagging uses between composer and performers. The
    /// members go without ids, to be looked up by name.
    pub fn split_joined(&mut self) -> bool {
        let [Member { mbids, .. }] = self.members.as_slice() else { return false };
        if !mbids.is_empty() || !self.artist.contains("; ") {
            return false;
        }
        let names: Vec<String> = self.artist.split("; ").map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect();
        self.split(&names)
    }

    /// Splits the credit into the artists the tags name one by one
    /// (`ALBUMARTISTS`, a value each), when they are distinct and, if
    /// the tags give ids, as many as the ids, which pair with them in
    /// order; without ids the members go nameless of id, to be looked
    /// up by name. A name repeated, or names that do not match the
    /// ids, leaves the credit whole.
    pub fn split(&mut self, names: &[String]) -> bool {
        let [Member { mbids, .. }] = self.members.as_slice() else { return false };
        if names.len() < 2 || (!mbids.is_empty() && names.len() != mbids.len()) {
            return false;
        }
        let distinct: HashSet<&str> = names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()).collect();
        if distinct.len() != names.len() {
            return false;
        }
        let ids = mbids.iter().map(|m| vec![m.clone()]).chain(std::iter::repeat(Vec::new()));
        self.members = names.iter().zip(ids).map(|(name, mbids)| Member { name: name.trim().to_owned(), mbids }).collect();
        true
    }

    pub fn cover_uri(&self) -> &str {
        &self.tracks[0].uri
    }

    pub fn duration(&self) -> Duration {
        self.tracks.iter().filter_map(|t| t.duration).sum()
    }

    pub fn uris(&self) -> Vec<String> {
        self.tracks.iter().map(|t| t.uri.clone()).collect()
    }

    pub fn discs(&self) -> u64 {
        self.tracks.iter().map(|t| t.disc).max().unwrap_or(0).max(1)
    }
}

#[derive(Debug)]
pub struct Artist {
    pub name: String,
    pub albums: Vec<usize>,
    pub mbids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: Option<usize>,
    pub duration: Option<Duration>,
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
    playlists: RwLock<Vec<Playlist>>,
    album_by_key: HashMap<String, usize>,
    artist_by_name: HashMap<String, usize>,
    by_uri: HashMap<String, (usize, usize)>,
}

pub fn music_dir() -> PathBuf {
    crate::config::get().music.clone()
}

pub fn playlists_dir() -> PathBuf {
    crate::config::get().playlists.clone()
}

const UNKNOWN_ARTIST: &str = "Unknown artist";

fn album_artist(song: &Song) -> &str {
    song.album_artists()
        .first()
        .or_else(|| song.artists().first())
        .map(String::as_str)
        .unwrap_or(UNKNOWN_ARTIST)
}

fn ids(song: &Song, tag: &Tag) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in song.tags.get(tag).into_iter().flatten() {
        for id in value.split(['/', ';']).map(str::trim).filter(|id| !id.is_empty()) {
            if !out.iter().any(|o| o == id) {
                out.push(id.to_owned());
            }
        }
    }
    out
}

fn artist_mbids<'a>(albums: impl Iterator<Item = &'a [String]> + Clone) -> Vec<String> {
    if albums.clone().any(|ids| ids.len() > 1) {
        let mut all: Vec<String> = Vec::new();
        for id in albums.flatten() {
            if !all.contains(id) {
                all.push(id.clone());
            }
        }
        return all;
    }
    let mut count: Vec<(&String, usize)> = Vec::new();
    for id in albums.flatten() {
        match count.iter_mut().find(|(i, _)| *i == id) {
            Some((_, n)) => *n += 1,
            None => count.push((id, 1)),
        }
    }
    count.into_iter().max_by_key(|(_, n)| *n).map(|(id, _)| vec![id.clone()]).unwrap_or_default()
}

pub fn album_key(song: &Song) -> Option<String> {
    match song.album() {
        Some(album) => Some(format!("{}\u{1f}{}", album_artist(song), album)),
        None => Some(format!("\u{1f}folder\u{1f}{}", song.file_path().parent()?.to_string_lossy())),
    }
}

fn sort_name(name: &str) -> String {
    name.to_lowercase()
}

impl Library {
    /// The albums the songs make, each credited whole; the library is
    /// assembled once the credits that the tags split are split.
    pub fn gather(songs: Vec<Song>) -> Vec<Album> {
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
                    mbid: None,
                    members: Vec::new(),
                });
                albums.len() - 1
            });
            let album = &mut albums[index];
            if album.mbid.is_none() {
                album.mbid = ids(&song, &Tag::MusicBrainzReleaseId).pop();
            }
            if album.members.is_empty() || album.members[0].mbids.is_empty() {
                let tag = if song.album_artists().is_empty() { Tag::MusicBrainzArtistId } else { Tag::MusicBrainzReleaseArtistId };
                album.members = Album::whole(&album.artist, ids(&song, &tag));
            }
            if album.year.is_none() {
                let date = song
                    .tags
                    .get(&Tag::OriginalDate)
                    .or_else(|| song.tags.get(&Tag::Date))
                    .and_then(|v| v.first());
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
                mbid: ids(&song, &Tag::MusicBrainzRecordingId).pop(),
                uri: song.url,
            });
        }

        for album in &mut albums {
            album.tracks.sort_by_key(|t| (t.disc, t.number));
        }
        albums
    }

    pub(crate) fn assemble(mut albums: Vec<Album>) -> Library {
        albums.sort_by_cached_key(|a| (sort_name(&a.title), sort_name(&a.artist)));
        // an album under each of its members: a collaboration the tags
        // split is on both artists' pages, the credit itself no artist
        let mut by_artist: HashMap<&str, Vec<(usize, &[String])>> = HashMap::new();
        for (i, album) in albums.iter().enumerate() {
            for member in &album.members {
                by_artist.entry(&member.name).or_default().push((i, &member.mbids));
            }
        }
        let mut artists: Vec<Artist> = by_artist
            .into_iter()
            .map(|(name, mut list)| {
                list.sort_by_cached_key(|&(i, _)| (albums[i].year.clone().unwrap_or_default(), sort_name(&albums[i].title)));
                let mbids = artist_mbids(list.iter().map(|(_, ids)| *ids));
                Artist { name: name.to_owned(), albums: list.into_iter().map(|(i, _)| i).collect(), mbids }
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

    #[cfg(test)]
    pub fn of(albums: &[(&str, &str, usize)]) -> Library {
        let albums = albums
            .iter()
            .map(|(artist, title, n)| Album {
                key: format!("{artist}\u{1f}{title}"),
                title: title.to_string(),
                artist: artist.to_string(),
                year: None,
                tracks: (1..=*n).map(|i| Track { uri: format!("{artist}/{title}/{i}.ogg"), title: format!("{title} {i}"), artist: artist.to_string(), disc: 1, number: i as u64, duration: None, mbid: None }).collect(),
                loose: false,
                mbid: None,
                members: Album::whole(artist, Vec::new()),
            })
            .collect();
        Library::assemble(albums)
    }

    pub fn playlists(&self) -> RwLockReadGuard<'_, Vec<Playlist>> {
        self.playlists.read().unwrap()
    }

    pub fn read_playlists(&self) -> Vec<Playlist> {
        read_playlists(self, &music_dir(), &playlists_dir())
    }

    pub fn set_playlists(&self, playlists: Vec<Playlist>) {
        *self.playlists.write().unwrap() = playlists;
    }

    pub fn album_by_key(&self, key: &str) -> Option<&Album> {
        self.album_by_key.get(key).map(|&i| &self.albums[i])
    }

    pub fn album_index(&self, key: &str) -> Option<usize> {
        self.album_by_key.get(key).copied()
    }

    pub fn place(&self, uri: &str) -> Option<(usize, usize)> {
        self.by_uri.get(uri).copied()
    }

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

pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(artist: &str, title: &str, ids: &[&str]) -> Album {
        let mut lib = Library::of(&[(artist, title, 1)]);
        let mut album = lib.albums.remove(0);
        album.members = Album::whole(artist, ids.iter().map(|i| i.to_string()).collect());
        album
    }

    #[test]
    fn a_split_credit_puts_the_album_under_each_member() {
        let mut nico = album("The Velvet Underground & Nico", "The Velvet Underground & Nico", &["vu", "nico"]);
        assert!(nico.split(&["The Velvet Underground".into(), "Nico".into()]));
        let loaded = album("The Velvet Underground", "Loaded", &["vu"]);
        let lib = Library::assemble(vec![nico, loaded]);
        let names: Vec<&str> = lib.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Nico", "The Velvet Underground"], "the credit is no artist");
        let vu = &lib.artists[lib.artist("The Velvet Underground").unwrap()];
        assert_eq!(vu.albums.len(), 2);
        assert_eq!(vu.mbids, ["vu"]);
        let nico = &lib.artists[lib.artist("Nico").unwrap()];
        assert_eq!(nico.albums.len(), 1);
        assert_eq!(nico.mbids, ["nico"]);
        let album = &lib.albums[nico.albums[0]];
        assert_eq!(album.artist, "The Velvet Underground & Nico", "the credit stays as written");
        assert_eq!(album.names().collect::<Vec<_>>(), ["The Velvet Underground", "Nico"]);
    }

    #[test]
    fn a_credit_the_tags_do_not_split_stays_whole() {
        let mut haden = album("Charlie Haden & Pat Metheny", "Beyond the Missouri Sky", &["charlie", "pat"]);
        assert!(!haden.split(&["Charlie Haden".into(), "Charlie Haden".into()]), "the tagger's guess, the first name twice");
        assert!(!haden.split(&["Charlie Haden".into()]), "one name for two ids");
        assert!(!haden.split(&["Charlie Haden".into(), "Pat Metheny".into(), "Someone".into()]), "more names than ids");
        assert_eq!(haden.members.len(), 1);
        let mut reznor = album("Trent Reznor & Atticus Ross", "Before the Flood", &[]);
        assert!(!reznor.split(&["Trent Reznor".into(), "Trent Reznor".into()]), "the first name twice, no ids");
        assert!(reznor.split(&["Trent Reznor".into(), "Atticus Ross".into()]), "distinct names without ids: the tagger did split them");
        assert!(reznor.members.iter().all(|m| m.mbids.is_empty()));
        let lib = Library::assemble(vec![haden, reznor]);
        let names: Vec<&str> = lib.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Atticus Ross", "Charlie Haden & Pat Metheny", "Trent Reznor"]);
        assert!(lib.artists[0].mbids.is_empty(), "looked up by name");
        let haden = &lib.artists[lib.artist("Charlie Haden & Pat Metheny").unwrap()];
        assert_eq!(haden.mbids, ["charlie", "pat"], "both ids, for the members to be asked about");
    }

    #[test]
    fn a_credit_without_ids_splits_on_its_semicolons() {
        let mut webern = album("Anton Webern; Pierre Boulez", "Complete Works", &[]);
        assert!(webern.split_joined());
        assert_eq!(webern.names().collect::<Vec<_>>(), ["Anton Webern", "Pierre Boulez"]);
        let mut tagged = album("Ludwig van Beethoven; Takács Quartet", "Quartets", &["lvb", "takacs"]);
        assert!(!tagged.split_joined(), "with ids, the tags' names decide");
        let mut amp = album("Iggy & the Stooges", "Raw Power", &[]);
        assert!(!amp.split_joined(), "an ampersand is a band's");
        let mut once = album("Anton Webern; Pierre Boulez", "Complete Works", &[]);
        assert!(once.split_joined() && !once.split_joined());
    }

    #[test]
    fn a_member_s_id_is_the_one_most_of_its_albums_give() {
        let mut a = album("Brian Eno & Harold Budd", "The Pearl", &["eno", "budd"]);
        assert!(a.split(&["Brian Eno".into(), "Harold Budd".into()]));
        let b = album("Brian Eno", "Another Green World", &["eno"]);
        let c = album("Brian Eno", "Discreet Music", &["eno-other"]);
        let lib = Library::assemble(vec![a, b, c]);
        let eno = &lib.artists[lib.artist("Brian Eno").unwrap()];
        assert_eq!(eno.albums.len(), 3);
        assert_eq!(eno.mbids, ["eno"]);
        let mut split_once = album("Brian Eno & Harold Budd", "The Pearl", &["eno", "budd"]);
        assert!(split_once.split(&[" Brian Eno ".into(), "Harold Budd".into()]), "trimmed");
        assert!(!split_once.split(&["Brian Eno".into(), "Harold Budd".into()]), "split once");
        assert_eq!(split_once.members[0].name, "Brian Eno");
    }
}
