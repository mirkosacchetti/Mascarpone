//! Mascarpone's database: SQLite, `~/.local/share/mascarpone/mascarpone.db`,
//! everything by integer ids. The library as mpd gives it, written
//! whole at every load (`artist`, `album`, `track`, `playlist` with its
//! shuffle and its origin, `playlist_entry`), the rows of what is gone
//! dropped. A playlist's origin is two columns: what it came from
//! (`queue`, `radio:track`, `radio:album`, `radio:artist`, or a word
//! for a service, `spotify`) and the origin's address (the track's uri,
//! the album's key, the artist's name; the web link of a service's
//! playlist), both kept over the rewrites, both empty for a file from
//! nowhere known. On `track`,
//! bliss's analysis too: the file's stamp (modification time and size,
//! so a file changed is analyzed again), the features' version, and
//! every feature in a column of its own (tempo, zero-crossing rate, the
//! spectral centroid, rolloff and flatness, the loudness, as mean and
//! deviation, the thirteen chroma features), or the error when the file
//! could not be analyzed (a codec symphonia does not decode, such as
//! Opus); and the voice heard in it (`voice`, the mean likelihood;
//! `voiced`, the share of frames with one; `vocal`, 1 when the mean is
//! over a half). On `artist`, what the net says: the MusicBrainz and Deezer
//! ids, the biography, the Wikipedia article, whether there is a
//! portrait; and in `relation` their links to other artists, a row a
//! link (a band's member, a collaboration, an influence, one alike),
//! with its kind and source, the other artist a row of `artist` too
//! (`in_library` 0 when the library has none of theirs), found by
//! MusicBrainz id, else Deezer id, else name. The window keeps what it
//! needs in memory; the file is for the record, and for whatever else
//! wants to look (`sqlite3`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::library::{Library, Playlist};
use crate::net::Info;
use crate::radio::{Vector, N};
use crate::voice::Voice;

/// The columns of the analysis, in bliss's order (`AnalysisIndex`).
pub const FEATURES: [&str; N] = [
    "tempo",
    "zcr",
    "mean_spectral_centroid",
    "std_spectral_centroid",
    "mean_spectral_rolloff",
    "std_spectral_rolloff",
    "mean_spectral_flatness",
    "std_spectral_flatness",
    "mean_loudness",
    "std_loudness",
    "chroma1",
    "chroma2",
    "chroma3",
    "chroma4",
    "chroma5",
    "chroma6",
    "chroma7",
    "chroma8",
    "chroma9",
    "chroma10",
    "chroma11",
    "chroma12",
    "chroma13",
];

/// The version of bliss's features the rows hold: rows of another are
/// analyzed again.
pub const VERSION: i64 = bliss_audio::FeaturesVersion::LATEST as i64;

/// The schema's version, `pragma user_version`.
const SCHEMA: i64 = 6;

pub fn path() -> PathBuf {
    crate::dirs::data().join("mascarpone").join("mascarpone.db")
}

/// A file's stamp: modification time (seconds) and size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub mtime: i64,
    pub size: i64,
}

/// What a track's row has of its file: the stamp it was analyzed at,
/// and whether bliss's features and the voice are there.
#[derive(Clone, Copy, Debug)]
pub struct Done {
    pub stamp: Stamp,
    pub bliss: bool,
    pub voice: bool,
}

/// A link from an artist to another, as the net gives it: the other by
/// name, with their MusicBrainz or Deezer id when the net has one.
#[derive(Debug, Clone)]
pub struct Link {
    pub other: String,
    pub other_mbid: Option<String>,
    pub other_deezer: Option<i64>,
    /// MusicBrainz's words ("member of band", "collaboration"...),
    /// Wikidata's "influenced by", Deezer's "similar".
    pub kind: String,
    pub source: String,
}

/// A link between two artists of the library, by name, and where it
/// came from (what weighs it).
#[derive(Debug, Clone)]
pub struct Relation {
    pub artist: String,
    pub other: String,
    pub source: String,
}

/// The database, shared by the threads that write it (the radio's, mpd's).
pub type Shared = Arc<Mutex<Db>>;

pub struct Db {
    conn: Connection,
}

/// The database opened, its schema made or brought up to date; in
/// memory, for the session only, when the file cannot be had.
pub fn open() -> Shared {
    let path = path();
    let db = match Db::open(&path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{}: {e}: nothing kept this session", path.display());
            Db::in_memory().expect("sqlite in memory")
        }
    };
    Arc::new(Mutex::new(db))
}

impl Db {
    fn open(path: &Path) -> Result<Db> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "synchronous", "normal")?;
        let mut db = Db { conn };
        db.prepare()?;
        Ok(db)
    }

    fn in_memory() -> Result<Db> {
        let mut db = Db { conn: Connection::open_in_memory()? };
        db.prepare()?;
        Ok(db)
    }

    /// The schema; a database of the first days (the tracks' analysis
    /// by uri, the artists and their links by name) carried over.
    fn prepare(&mut self) -> Result<()> {
        self.conn.pragma_update(None, "foreign_keys", "on")?;
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version >= SCHEMA {
            return Ok(());
        }
        // from the second schema on: the playlists' shuffle, their origin
        if version >= 2 {
            if version < 3 {
                self.conn.execute_batch("alter table playlist add column shuffle integer not null default 1")?;
            }
            if version < 4 {
                self.conn.execute_batch("alter table playlist add column origin text")?;
            }
            if version < 5 {
                self.conn.execute_batch("alter table playlist add column origin_url text")?;
            }
            if version < 6 {
                self.conn.execute_batch("alter table track add column voice real; alter table track add column voiced real; alter table track add column vocal integer;")?;
            }
            self.conn.pragma_update(None, "user_version", SCHEMA)?;
            return Ok(());
        }
        let old = version == 0 && self.conn.table_exists(None, "track")?;
        if old {
            self.conn.execute_batch("alter table track rename to old_track; alter table artist rename to old_artist; alter table relation rename to old_relation;")?;
        }
        let features = FEATURES.iter().map(|f| format!("{f} real")).collect::<Vec<_>>().join(", ");
        self.conn.execute_batch(&format!(
            "create table artist (
                id integer primary key,
                name text not null unique collate nocase,
                in_library integer not null default 0,
                mbid text,
                deezer_id integer,
                bio text,
                wikipedia text,
                portrait integer,
                info_fetched text,
                links_fetched text,
                links_error text
            );
            create index artist_mbid on artist (mbid);
            create index artist_deezer on artist (deezer_id);
            create table album (
                id integer primary key,
                key text not null unique,
                title text not null,
                artist_id integer not null references artist (id),
                year text,
                folder text,
                loose integer not null default 0
            );
            create table track (
                id integer primary key,
                uri text not null unique,
                album_id integer references album (id) on delete set null,
                artist_id integer references artist (id),
                title text,
                disc integer,
                number integer,
                duration real,
                mtime integer,
                size integer,
                version integer,
                analyzed text,
                error text,
                {features},
                voice real,
                voiced real,
                vocal integer
            );
            create index track_album on track (album_id);
            create table playlist (
                id integer primary key,
                name text not null,
                path text not null unique,
                shuffle integer not null default 1,
                origin text,
                origin_url text
            );
            create table playlist_entry (
                playlist_id integer not null references playlist (id) on delete cascade,
                position integer not null,
                uri text not null,
                track_id integer references track (id) on delete set null,
                primary key (playlist_id, position)
            );
            create table relation (
                artist_id integer not null references artist (id) on delete cascade,
                other_id integer not null references artist (id) on delete cascade,
                kind text not null,
                source text not null,
                primary key (artist_id, other_id, kind, source)
            );"
        ))?;
        if old {
            self.carry_over()?;
        }
        self.conn.pragma_update(None, "user_version", SCHEMA)?;
        Ok(())
    }

    /// The first schema's rows into this one: the analyses, the artists
    /// answered about and their links.
    fn carry_over(&mut self) -> Result<()> {
        let columns = FEATURES.join(", ");
        self.conn.execute(
            &format!("insert into track (uri, title, disc, number, duration, mtime, size, version, analyzed, error, {columns}) select uri, title, disc, number, duration, mtime, size, version, analyzed, error, {columns} from old_track"),
            [],
        )?;
        let artists: Vec<(String, Option<String>, String)> = {
            let mut stmt = self.conn.prepare("select name, mbid, fetched from old_artist where error is null")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let links: Vec<(String, Link)> = {
            let mut stmt = self.conn.prepare("select artist, other, other_mbid, kind, source from old_relation")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, Link { other: r.get(1)?, other_mbid: r.get(2)?, other_deezer: None, kind: r.get(3)?, source: r.get(4)? })))?;
            rows.collect::<Result<_, _>>()?
        };
        for (name, mbid, fetched) in artists {
            let theirs: Vec<Link> = links.iter().filter(|(a, _)| *a == name).map(|(_, l)| l.clone()).collect();
            self.write_links(&name, mbid.as_deref(), None, None, &theirs)?;
            self.conn.execute("update artist set links_fetched = ?2 where name = ?1", params![name, fetched])?;
        }
        self.conn.execute_batch("drop table old_relation; drop table old_artist; drop table old_track;")?;
        Ok(())
    }

    // the library

    /// The library written whole: the artists (the albums' marked as the
    /// library's, the rest not), the albums, the tracks with their tags
    /// (the analysis kept), the playlists; the rows of what is gone
    /// dropped.
    pub fn write_library(&mut self, library: &Library) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("update artist set in_library = 0", [])?;
        {
            let mut artist = tx.prepare_cached("insert into artist (name, in_library) values (?1, 1) on conflict (name) do update set in_library = 1")?;
            for a in &library.artists {
                artist.execute(params![a.name])?;
            }
            let mut track_artist = tx.prepare_cached("insert or ignore into artist (name) values (?1)")?;
            let mut album = tx.prepare_cached(
                "insert into album (key, title, artist_id, year, folder, loose) values (?1, ?2, (select id from artist where name = ?3), ?4, ?5, ?6)
                 on conflict (key) do update set title = excluded.title, artist_id = excluded.artist_id, year = excluded.year, folder = excluded.folder, loose = excluded.loose",
            )?;
            let mut track = tx.prepare_cached(
                "insert into track (uri, album_id, artist_id, title, disc, number, duration) values (?1, (select id from album where key = ?2), (select id from artist where name = ?3), ?4, ?5, ?6, ?7)
                 on conflict (uri) do update set album_id = excluded.album_id, artist_id = excluded.artist_id, title = excluded.title, disc = excluded.disc, number = excluded.number, duration = excluded.duration",
            )?;
            tx.execute_batch("create temp table if not exists keep_album (key text primary key); create temp table if not exists keep_track (uri text primary key); delete from keep_album; delete from keep_track;")?;
            let mut keep_album = tx.prepare_cached("insert or ignore into keep_album (key) values (?1)")?;
            let mut keep_track = tx.prepare_cached("insert or ignore into keep_track (uri) values (?1)")?;
            for a in &library.albums {
                let folder = Path::new(&a.tracks[0].uri).parent().map(|p| p.to_string_lossy().into_owned());
                album.execute(params![a.key, a.title, a.artist, a.year, folder, a.loose as i64])?;
                keep_album.execute(params![a.key])?;
                for t in &a.tracks {
                    track_artist.execute(params![t.artist])?;
                    track.execute(params![t.uri, a.key, t.artist, t.title, t.disc as i64, t.number as i64, t.duration.map(|d| d.as_secs_f64())])?;
                    keep_track.execute(params![t.uri])?;
                }
            }
        }
        tx.execute("delete from track where uri not in (select uri from keep_track)", [])?;
        tx.execute("delete from album where key not in (select key from keep_album)", [])?;
        write_playlists(&tx, &library.playlists())?;
        tx.commit()?;
        Ok(())
    }

    /// The playlists written whole, each with its shuffle as `master`
    /// keeps it, its origin kept; the rows of files gone dropped.
    pub fn write_playlists(&mut self, playlists: &[Playlist]) -> Result<()> {
        let tx = self.conn.transaction()?;
        write_playlists(&tx, playlists)?;
        tx.commit()?;
        Ok(())
    }

    /// Where a playlist came from, on its row (made if the file has not
    /// been read yet): what, and its address.
    pub fn set_origin(&self, path: &Path, name: &str, origin: &str, url: Option<&str>) -> Result<()> {
        self.conn.execute(
            "insert into playlist (name, path, origin, origin_url) values (?1, ?2, ?3, ?4) on conflict (path) do update set origin = excluded.origin, origin_url = excluded.origin_url",
            params![name, path.to_string_lossy(), origin, url],
        )?;
        Ok(())
    }

    // the analysis

    /// Every analyzed track's stamp, by uri, with what it has: bliss's
    /// features of this version (a row of another version counts for
    /// nothing, so its file is analyzed again), the voice. A file that
    /// erred has both, nothing to do over.
    pub fn stamps(&self) -> Result<HashMap<String, Done>> {
        let mut stmt = self.conn.prepare("select uri, mtime, size, version = ?1 or error is not null, voice is not null or error is not null from track where mtime is not null")?;
        let rows = stmt.query_map(params![VERSION], |r| Ok((r.get::<_, String>(0)?, Done { stamp: Stamp { mtime: r.get(1)?, size: r.get(2)? }, bliss: r.get(3)?, voice: r.get(4)? })))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The analyses, by uri: the rows of this version without an error;
    /// and the voice heard, by uri.
    pub fn vectors(&self) -> Result<(HashMap<String, Vector>, HashMap<String, Voice>)> {
        let sql = format!("select uri, {}, voice, voiced from track where (version = ?1 or voice is not null) and error is null", FEATURES.join(", "));
        let mut stmt = self.conn.prepare(&sql)?;
        let mut vectors = HashMap::new();
        let mut voices = HashMap::new();
        let rows = stmt.query_map(params![VERSION], |r| {
            let uri: String = r.get(0)?;
            let mut v = [0f32; N];
            let mut whole = true;
            for (i, x) in v.iter_mut().enumerate() {
                match r.get::<_, Option<f64>>(i + 1)? {
                    Some(f) => *x = f as f32,
                    None => whole = false,
                }
            }
            let voice = match (r.get::<_, Option<f64>>(N + 1)?, r.get::<_, Option<f64>>(N + 2)?) {
                (Some(mean), Some(share)) => Some(Voice { mean: mean as f32, share: share as f32 }),
                _ => None,
            };
            Ok((uri, whole.then_some(v), voice))
        })?;
        for row in rows {
            let (uri, vector, voice) = row?;
            if let Some(v) = vector {
                vectors.insert(uri.clone(), v);
            }
            if let Some(v) = voice {
                voices.insert(uri, v);
            }
        }
        Ok((vectors, voices))
    }

    /// The voice heard in a file, or the error, onto its track's row.
    pub fn write_voice(&self, uri: &str, stamp: Stamp, voice: &Result<Voice, String>) -> Result<()> {
        match voice {
            Ok(v) => self.conn.execute(
                "update track set mtime = ?2, size = ?3, voice = ?4, voiced = ?5, vocal = ?6 where uri = ?1",
                params![uri, stamp.mtime, stamp.size, v.mean as f64, v.share as f64, v.vocal() as i64],
            )?,
            Err(e) => self.conn.execute("update track set mtime = ?2, size = ?3, voice = null, voiced = null, vocal = null, error = coalesce(error, ?4) where uri = ?1", params![uri, stamp.mtime, stamp.size, e])?,
        };
        Ok(())
    }

    /// A file's analysis, or the error, onto its track's row.
    pub fn write_analysis(&self, uri: &str, stamp: Stamp, analysis: &Result<Vector, String>) -> Result<()> {
        let (error, vector) = match analysis {
            Ok(v) => (None, Some(v)),
            Err(e) => (Some(e.clone()), None),
        };
        let sets = FEATURES.iter().enumerate().map(|(i, f)| format!("{f} = ?{}", 6 + i)).collect::<Vec<_>>().join(", ");
        let sql = format!("update track set mtime = ?2, size = ?3, version = ?4, analyzed = datetime('now'), error = ?5, {sets} where uri = ?1");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mut values: Vec<rusqlite::types::Value> = vec![uri.to_owned().into(), stamp.mtime.into(), stamp.size.into(), VERSION.into(), error.into()];
        for i in 0..N {
            values.push(match vector {
                Some(v) => (v[i] as f64).into(),
                None => rusqlite::types::Value::Null,
            });
        }
        stmt.execute(rusqlite::params_from_iter(values))?;
        Ok(())
    }

    // the artists from the net

    /// What the net said of an artist, once it has been asked.
    pub fn artist_info(&self, name: &str) -> Result<Option<Info>> {
        let mut stmt = self.conn.prepare_cached("select bio, wikipedia, portrait from artist where name = ?1 and info_fetched is not null")?;
        Ok(stmt.query_row(params![name], |r| Ok(Info { bio: r.get(0)?, wikipedia: r.get(1)?, portrait: r.get::<_, i64>(2)? != 0 })).optional()?)
    }

    pub fn write_info(&self, name: &str, info: &Info) -> Result<()> {
        self.conn.execute(
            "insert into artist (name, bio, wikipedia, portrait, info_fetched) values (?1, ?2, ?3, ?4, datetime('now'))
             on conflict (name) do update set bio = excluded.bio, wikipedia = excluded.wikipedia, portrait = excluded.portrait, info_fetched = excluded.info_fetched",
            params![name, info.bio, info.wikipedia, info.portrait as i64],
        )?;
        Ok(())
    }

    /// The artist's words forgotten, to be asked again.
    pub fn forget_info(&self, name: &str) -> Result<()> {
        self.conn.execute("update artist set bio = null, wikipedia = null, portrait = null, info_fetched = null where name = ?1", params![name])?;
        Ok(())
    }

    /// The artists whose links the net has given, with their MusicBrainz
    /// id when it had one; one it erred on is asked again.
    pub fn linked(&self) -> Result<HashMap<String, Option<String>>> {
        let mut stmt = self.conn.prepare("select name, mbid from artist where links_fetched is not null and links_error is null")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// What the net said of an artist's links: their ids, the error if
    /// the net erred, and their links anew, each other artist a row.
    pub fn write_links(&mut self, name: &str, mbid: Option<&str>, deezer: Option<i64>, error: Option<&str>, links: &[Link]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let id = artist_id(&tx, name, mbid, deezer)?;
        tx.execute("update artist set links_fetched = datetime('now'), links_error = ?2 where id = ?1", params![id, error])?;
        tx.execute("delete from relation where artist_id = ?1", params![id])?;
        for l in links {
            let other = artist_id(&tx, &l.other, l.other_mbid.as_deref(), l.other_deezer)?;
            if other != id {
                tx.execute("insert or ignore into relation (artist_id, other_id, kind, source) values (?1, ?2, ?3, ?4)", params![id, other, l.kind, l.source])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every link between two artists of the library, by name.
    pub fn relations(&self) -> Result<Vec<Relation>> {
        let mut stmt = self.conn.prepare(
            "select a.name, b.name, r.source from relation r join artist a on a.id = r.artist_id join artist b on b.id = r.other_id where a.in_library = 1 and b.in_library = 1",
        )?;
        let rows = stmt.query_map([], |r| Ok(Relation { artist: r.get(0)?, other: r.get(1)?, source: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The artist's row, found by MusicBrainz id, else Deezer id, else name
/// (case aside), made when there is none; the ids filled in.
fn artist_id(conn: &Connection, name: &str, mbid: Option<&str>, deezer: Option<i64>) -> Result<i64> {
    let by_mbid = |m: &str| conn.query_row("select id from artist where mbid = ?1", params![m], |r| r.get::<_, i64>(0)).optional();
    let by_deezer = |d: i64| conn.query_row("select id from artist where deezer_id = ?1", params![d], |r| r.get::<_, i64>(0)).optional();
    let by_name = || conn.query_row("select id from artist where name = ?1", params![name], |r| r.get::<_, i64>(0)).optional();
    let id = match (mbid.map(by_mbid).transpose()?.flatten(), deezer.map(by_deezer).transpose()?.flatten(), by_name()?) {
        (Some(id), _, _) | (None, Some(id), _) | (None, None, Some(id)) => id,
        (None, None, None) => {
            conn.execute("insert into artist (name, mbid, deezer_id) values (?1, ?2, ?3)", params![name, mbid, deezer])?;
            conn.last_insert_rowid()
        }
    };
    conn.execute("update artist set mbid = coalesce(mbid, ?2), deezer_id = coalesce(deezer_id, ?3) where id = ?1", params![id, mbid, deezer])?;
    Ok(id)
}

fn write_playlists(tx: &Connection, playlists: &[Playlist]) -> Result<()> {
    tx.execute_batch("create temp table if not exists keep_playlist (path text primary key); delete from keep_playlist;")?;
    let mut playlist = tx.prepare_cached("insert into playlist (name, path, shuffle) values (?1, ?2, ?3) on conflict (path) do update set name = excluded.name, shuffle = excluded.shuffle")?;
    let mut id_of = tx.prepare_cached("select id from playlist where path = ?1")?;
    let mut keep = tx.prepare_cached("insert or ignore into keep_playlist (path) values (?1)")?;
    let mut clear = tx.prepare_cached("delete from playlist_entry where playlist_id = ?1")?;
    let mut entry = tx.prepare_cached("insert into playlist_entry (playlist_id, position, uri, track_id) values (?1, ?2, ?3, (select id from track where uri = ?3))")?;
    for p in playlists {
        let path = p.path.to_string_lossy();
        playlist.execute(params![p.name, path, crate::master::shuffled(&p.path) as i64])?;
        let id: i64 = id_of.query_row(params![path], |r| r.get(0))?;
        keep.execute(params![path])?;
        clear.execute(params![id])?;
        for (i, e) in p.entries.iter().enumerate() {
            entry.execute(params![id, i as i64, e.uri])?;
        }
    }
    tx.execute("delete from playlist where path not in (select path from keep_playlist)", [])?;
    Ok(())
}
