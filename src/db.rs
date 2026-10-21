use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::library::{Library, Playlist};
use crate::net::Info;
use crate::radio::{Vector, Vectors, N};
use crate::shape::{self, Shape};
use crate::voice::Voice;

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

pub const VERSION: i64 = bliss_audio::FeaturesVersion::LATEST as i64;

const SCHEMA: i64 = 8;

pub fn path() -> PathBuf {
    crate::dirs::data().join("mascarpone").join("mascarpone.db")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub mtime: i64,
    pub size: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct Done {
    pub stamp: Stamp,
    pub bliss: bool,
    pub voice: bool,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub other: String,
    pub other_mbid: Option<String>,
    pub other_deezer: Option<i64>,
    pub kind: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Relation {
    pub artist: String,
    pub other: String,
    pub source: String,
}

pub type Shared = Arc<Mutex<Db>>;

pub struct Db {
    conn: Connection,
}

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

    fn prepare(&mut self) -> Result<()> {
        self.conn.pragma_update(None, "foreign_keys", "on")?;
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version >= SCHEMA {
            return Ok(());
        }
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
            if version < 7 {
                self.conn.execute_batch("alter table album add column mbid text; alter table track add column mbid text;")?;
            }
            if version < 8 {
                for column in shape::NAMES {
                    self.conn.execute_batch(&format!("alter table track add column {column} real"))?;
                }
                self.conn.execute_batch("alter table track add column shaped integer")?;
            }
            self.conn.pragma_update(None, "user_version", SCHEMA)?;
            return Ok(());
        }
        let old = version == 0 && self.conn.table_exists(None, "track")?;
        if old {
            self.conn.execute_batch("alter table track rename to old_track; alter table artist rename to old_artist; alter table relation rename to old_relation;")?;
        }
        let features = FEATURES.iter().chain(&shape::NAMES).map(|f| format!("{f} real")).collect::<Vec<_>>().join(", ");
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
                loose integer not null default 0,
                mbid text
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
                shaped integer,
                voice real,
                voiced real,
                vocal integer,
                mbid text
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

    pub fn write_library(&mut self, library: &Library) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("update artist set in_library = 0", [])?;
        {
            let mut artist = tx.prepare_cached("insert into artist (name, in_library) values (?1, 1) on conflict (name) do update set in_library = 1, name = excluded.name")?;
            for a in &library.artists {
                artist.execute(params![a.name])?;
                let id: i64 = tx.query_row("select id from artist where name = ?1", params![a.name], |r| r.get(0))?;
                match a.mbids.as_slice() {
                    [mbid] => claim_mbid(&tx, id, mbid)?,
                    [_, _, ..] => {
                        let linked: bool = tx.query_row("select exists (select 1 from relation where artist_id = ?1 and source = 'tags')", params![id], |r| r.get(0))?;
                        if !linked {
                            tx.execute("update artist set links_fetched = null, links_error = null where id = ?1", params![id])?;
                        }
                    }
                    [] => {}
                }
            }
            let mut track_artist = tx.prepare_cached("insert or ignore into artist (name) values (?1)")?;
            let mut album = tx.prepare_cached(
                "insert into album (key, title, artist_id, year, folder, loose, mbid) values (?1, ?2, (select id from artist where name = ?3), ?4, ?5, ?6, ?7)
                 on conflict (key) do update set title = excluded.title, artist_id = excluded.artist_id, year = excluded.year, folder = excluded.folder, loose = excluded.loose, mbid = excluded.mbid",
            )?;
            let mut track = tx.prepare_cached(
                "insert into track (uri, album_id, artist_id, title, disc, number, duration, mbid) values (?1, (select id from album where key = ?2), (select id from artist where name = ?3), ?4, ?5, ?6, ?7, ?8)
                 on conflict (uri) do update set album_id = excluded.album_id, artist_id = excluded.artist_id, title = excluded.title, disc = excluded.disc, number = excluded.number, duration = excluded.duration, mbid = excluded.mbid",
            )?;
            tx.execute_batch("create temp table if not exists keep_album (key text primary key); create temp table if not exists keep_track (uri text primary key); delete from keep_album; delete from keep_track;")?;
            let mut keep_album = tx.prepare_cached("insert or ignore into keep_album (key) values (?1)")?;
            let mut keep_track = tx.prepare_cached("insert or ignore into keep_track (uri) values (?1)")?;
            for a in &library.albums {
                let folder = Path::new(&a.tracks[0].uri).parent().map(|p| p.to_string_lossy().into_owned());
                album.execute(params![a.key, a.title, a.artist, a.year, folder, a.loose as i64, a.mbid])?;
                keep_album.execute(params![a.key])?;
                for t in &a.tracks {
                    track_artist.execute(params![t.artist])?;
                    track.execute(params![t.uri, a.key, t.artist, t.title, t.disc as i64, t.number as i64, t.duration.map(|d| d.as_secs_f64()), t.mbid])?;
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

    pub fn write_playlists(&mut self, playlists: &[Playlist]) -> Result<()> {
        let tx = self.conn.transaction()?;
        write_playlists(&tx, playlists)?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_origin(&self, path: &Path, name: &str, origin: &str, url: Option<&str>) -> Result<()> {
        self.conn.execute(
            "insert into playlist (name, path, origin, origin_url) values (?1, ?2, ?3, ?4) on conflict (path) do update set origin = excluded.origin, origin_url = excluded.origin_url",
            params![name, path.to_string_lossy(), origin, url],
        )?;
        Ok(())
    }

    pub fn stamps(&self) -> Result<HashMap<String, Done>> {
        let mut stmt = self.conn.prepare("select uri, mtime, size, (version = ?1 and shaped is ?2) or error is not null, voice is not null or error is not null from track where mtime is not null")?;
        let rows = stmt.query_map(params![VERSION, shape::VERSION], |r| Ok((r.get::<_, String>(0)?, Done { stamp: Stamp { mtime: r.get(1)?, size: r.get(2)? }, bliss: r.get(3)?, voice: r.get(4)? })))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn vectors(&self) -> Result<Vectors> {
        let sql = format!("select uri, {}, voice, voiced, shaped, {} from track where (version = ?1 or voice is not null) and error is null", FEATURES.join(", "), shape::NAMES.join(", "));
        let mut stmt = self.conn.prepare(&sql)?;
        let mut vectors = Vectors::default();
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
            let mut shape = [0f32; shape::N];
            let mut shaped = r.get::<_, Option<i64>>(N + 3)? == Some(shape::VERSION);
            for (i, x) in shape.iter_mut().enumerate() {
                match r.get::<_, Option<f64>>(N + 4 + i)? {
                    Some(f) => *x = f as f32,
                    None => shaped = false,
                }
            }
            Ok((uri, whole.then_some(v), voice, shaped.then_some(shape)))
        })?;
        for row in rows {
            let (uri, vector, voice, shape) = row?;
            if let Some(v) = vector {
                vectors.by_uri.insert(uri.clone(), v);
            }
            if let Some(v) = voice {
                vectors.voice.insert(uri.clone(), v);
            }
            if let Some(s) = shape {
                vectors.shape.insert(uri, s);
            }
        }
        Ok(vectors)
    }

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

    pub fn write_analysis(&self, uri: &str, stamp: Stamp, analysis: &Result<(Vector, Shape), String>) -> Result<()> {
        let (error, heard) = match analysis {
            Ok(v) => (None, Some(v)),
            Err(e) => (Some(e.clone()), None),
        };
        let sets = FEATURES.iter().chain(&shape::NAMES).enumerate().map(|(i, f)| format!("{f} = ?{}", 7 + i)).collect::<Vec<_>>().join(", ");
        let sql = format!("update track set mtime = ?2, size = ?3, version = ?4, analyzed = datetime('now'), error = ?5, shaped = ?6, {sets} where uri = ?1");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let shaped = heard.map(|_| shape::VERSION);
        let mut values: Vec<rusqlite::types::Value> = vec![uri.to_owned().into(), stamp.mtime.into(), stamp.size.into(), VERSION.into(), error.into(), shaped.into()];
        for i in 0..N + shape::N {
            values.push(match heard {
                Some((v, s)) => (if i < N { v[i] } else { s[i - N] } as f64).into(),
                None => rusqlite::types::Value::Null,
            });
        }
        stmt.execute(rusqlite::params_from_iter(values))?;
        Ok(())
    }

    pub fn artist_mbid(&self, name: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached("select mbid from artist where name = ?1")?;
        Ok(stmt.query_row(params![name], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
    }

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

    pub fn forget_info(&self, name: &str) -> Result<()> {
        self.conn.execute("update artist set bio = null, wikipedia = null, portrait = null, info_fetched = null where name = ?1", params![name])?;
        Ok(())
    }

    pub fn linked(&self) -> Result<HashMap<String, Option<String>>> {
        let mut stmt = self.conn.prepare("select name, mbid from artist where links_fetched is not null and links_error is null")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

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

    pub fn relations(&self) -> Result<Vec<Relation>> {
        let mut stmt = self.conn.prepare(
            "select a.name, b.name, r.source from relation r join artist a on a.id = r.artist_id join artist b on b.id = r.other_id where a.in_library = 1 and b.in_library = 1",
        )?;
        let rows = stmt.query_map([], |r| Ok(Relation { artist: r.get(0)?, other: r.get(1)?, source: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

fn claim_mbid(conn: &Connection, id: i64, mbid: &str) -> Result<()> {
    let old: Option<String> = conn.query_row("select mbid from artist where id = ?1", params![id], |r| r.get(0))?;
    if old.as_deref() == Some(mbid) {
        return Ok(());
    }
    let theirs = conn.query_row("select id, in_library from artist where mbid = ?1 and id != ?2", params![mbid, id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?))).optional()?;
    if theirs.is_some_and(|(_, in_library)| in_library) {
        return Ok(());
    }
    if let Some(old) = old {
        conn.execute(
            "insert or ignore into artist (name, mbid, deezer_id, bio, wikipedia, portrait, info_fetched, links_fetched, links_error)
             select name || ' (' || ?2 || ')', mbid, deezer_id, bio, wikipedia, portrait, info_fetched, links_fetched, links_error from artist where id = ?1",
            params![id, old],
        )?;
        let namesake: i64 = conn.query_row("select id from artist where name = (select name || ' (' || ?2 || ')' from artist where id = ?1)", params![id, old], |r| r.get(0))?;
        conn.execute("update or ignore relation set artist_id = ?2 where artist_id = ?1", params![id, namesake])?;
        conn.execute("update or ignore relation set other_id = ?2 where other_id = ?1", params![id, namesake])?;
        conn.execute("delete from relation where artist_id = ?1 or other_id = ?1", params![id])?;
        conn.execute("update artist set deezer_id = null, bio = null, wikipedia = null, portrait = null, info_fetched = null, links_fetched = null, links_error = null where id = ?1", params![id])?;
    }
    if let Some((theirs, _)) = theirs {
        merge_artist(conn, theirs, id)?;
    }
    conn.execute("update artist set mbid = ?2, links_fetched = null, links_error = null where id = ?1", params![id, mbid])?;
    Ok(())
}

fn merge_artist(conn: &Connection, from: i64, into: i64) -> Result<()> {
    conn.execute("update or ignore relation set artist_id = ?2 where artist_id = ?1", params![from, into])?;
    conn.execute("update or ignore relation set other_id = ?2 where other_id = ?1", params![from, into])?;
    conn.execute("delete from relation where artist_id = ?1 or other_id = ?1 or artist_id = other_id", params![from])?;
    conn.execute("update album set artist_id = ?2 where artist_id = ?1", params![from, into])?;
    conn.execute("update track set artist_id = ?2 where artist_id = ?1", params![from, into])?;
    conn.execute(
        "update artist set
            deezer_id = coalesce(deezer_id, (select deezer_id from artist where id = ?1)),
            bio = coalesce(bio, (select bio from artist where id = ?1)),
            wikipedia = coalesce(wikipedia, (select wikipedia from artist where id = ?1)),
            portrait = coalesce(portrait, (select portrait from artist where id = ?1)),
            info_fetched = coalesce(info_fetched, (select info_fetched from artist where id = ?1))
         where id = ?2",
        params![from, into],
    )?;
    conn.execute("update artist set mbid = null where id = ?1", params![from])?;
    conn.execute("delete from artist where id = ?1", params![from])?;
    Ok(())
}

fn artist_id(conn: &Connection, name: &str, mbid: Option<&str>, deezer: Option<i64>) -> Result<i64> {
    let by_mbid = |m: &str| conn.query_row("select id from artist where mbid = ?1", params![m], |r| r.get::<_, i64>(0)).optional();
    let by_deezer = |d: i64| conn.query_row("select id from artist where deezer_id = ?1", params![d], |r| r.get::<_, i64>(0)).optional();
    let by_name = || conn.query_row("select id, mbid from artist where name = ?1", params![name], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).optional();
    let id = match (mbid.map(by_mbid).transpose()?.flatten(), deezer.map(by_deezer).transpose()?.flatten(), by_name()?) {
        (Some(id), _, _) | (None, Some(id), _) => id,
        (None, None, Some((id, theirs))) if mbid.is_none() || theirs.is_none() => id,
        (None, None, namesake) => {
            let name = match (namesake, mbid) {
                (Some(_), Some(mbid)) => format!("{name} ({mbid})"),
                _ => name.to_owned(),
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn link(other: &str, mbid: Option<&str>, kind: &str, source: &str) -> Link {
        Link { other: other.into(), other_mbid: mbid.map(str::to_owned), other_deezer: None, kind: kind.into(), source: source.into() }
    }

    #[test]
    fn the_shape_is_analyzed_with_the_vector_and_again_when_it_changes() {
        let mut db = Db::in_memory().unwrap();
        db.write_library(&Library::of(&[("Can", "Tago Mago", 1)])).unwrap();
        let uri = "Can/Tago Mago/1.ogg";
        let stamp = Stamp { mtime: 1, size: 2 };
        let (mut vector, mut shape) = ([0f32; N], [0f32; shape::N]);
        vector[0] = 0.5;
        shape[shape::RISE] = -0.25;
        db.write_analysis(uri, stamp, &Ok((vector, shape))).unwrap();
        let done = db.stamps().unwrap()[uri];
        assert!(done.bliss && !done.voice);
        let vectors = db.vectors().unwrap();
        assert_eq!(vectors.by_uri[uri], vector);
        assert_eq!(vectors.shape[uri], shape);
        db.conn.execute("update track set shaped = null", []).unwrap();
        assert!(!db.stamps().unwrap()[uri].bliss, "no shape yet: listened to again");
        db.conn.execute("update track set shaped = ?1", params![shape::VERSION - 1]).unwrap();
        assert!(!db.stamps().unwrap()[uri].bliss, "an older shape is listened to again");
        let vectors = db.vectors().unwrap();
        assert_eq!(vectors.by_uri[uri], vector, "the vector serves meanwhile");
        assert!(!vectors.shape.contains_key(uri));
        db.write_analysis(uri, stamp, &Err("no".into())).unwrap();
        assert!(db.stamps().unwrap()[uri].bliss, "an error is not tried again");
        assert!(db.vectors().unwrap().by_uri.is_empty());
    }

    #[test]
    fn a_database_from_before_the_shape_gets_its_columns() {
        let mut db = Db::in_memory().unwrap();
        for column in shape::NAMES.iter().chain(&["shaped"]) {
            db.conn.execute_batch(&format!("alter table track drop column {column}")).unwrap();
        }
        db.conn.pragma_update(None, "user_version", 7).unwrap();
        db.prepare().unwrap();
        db.write_library(&Library::of(&[("Can", "Tago Mago", 1)])).unwrap();
        db.write_analysis("Can/Tago Mago/1.ogg", Stamp { mtime: 1, size: 2 }, &Ok(([0f32; N], [0f32; shape::N]))).unwrap();
        assert_eq!(db.vectors().unwrap().shape.len(), 1);
    }

    #[test]
    fn the_row_takes_the_library_s_spelling() {
        let mut db = Db::in_memory().unwrap();
        db.conn.execute("insert into artist (name) values ('CAN')", []).unwrap();
        db.write_library(&Library::of(&[("Can", "Tago Mago", 2)])).unwrap();
        db.write_links("Can", Some("can"), None, None, &[]).unwrap();
        assert!(db.linked().unwrap().contains_key("Can"));
        assert_eq!(db.conn.query_row("select count(*) from artist", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn the_tags_id_wins_over_the_net_s() {
        let mut db = Db::in_memory().unwrap();
        let mut lib = Library::of(&[("Beck", "Odelay", 2), ("Pavement", "Wowee Zowee", 2)]);
        db.write_library(&lib).unwrap();
        db.write_links("Beck", Some("namesake"), None, None, &[link("Jeff Beck", Some("jeff"), "member of band", "musicbrainz")]).unwrap();
        db.write_links("Pavement", Some("pavement"), None, None, &[link("Beck", Some("beck"), "collaboration", "musicbrainz")]).unwrap();
        assert_eq!(db.linked().unwrap().len(), 2);
        assert_eq!(db.relations().unwrap().len(), 0, "the real Beck is not in the library yet");
        let beck = lib.artists.iter().position(|a| a.name == "Beck").unwrap();
        lib.artists[beck].mbids = vec!["beck".into()];
        db.write_library(&lib).unwrap();
        assert_eq!(db.artist_mbid("Beck").unwrap().as_deref(), Some("beck"));
        assert_eq!(db.artist_mbid("Beck (namesake)").unwrap().as_deref(), Some("namesake"));
        let linked = db.linked().unwrap();
        assert!(!linked.contains_key("Beck"), "asked again, by id");
        assert!(linked.contains_key("Pavement"));
        let rows: Vec<(i64, i64)> = {
            let mut stmt = db.conn.prepare("select artist_id, other_id from relation").unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(rows.len(), 2, "the namesake's link and Pavement's, the merged row gone");
        let relations = db.relations().unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!((relations[0].artist.as_str(), relations[0].other.as_str()), ("Pavement", "Beck"));
        let count: i64 = db.conn.query_row("select count(*) from artist where mbid = 'beck'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
        db.write_links("Beck", Some("beck"), None, None, &[]).unwrap();
        db.write_library(&lib).unwrap();
        assert!(db.linked().unwrap().contains_key("Beck"));
    }

    #[test]
    fn a_collaboration_is_asked_for_its_members_once() {
        let mut db = Db::in_memory().unwrap();
        let mut lib = Library::of(&[("Charlie Haden & Pat Metheny", "Beyond the Missouri Sky", 2), ("Pat Metheny", "Bright Size Life", 2)]);
        db.write_library(&lib).unwrap();
        db.write_links("Charlie Haden & Pat Metheny", None, None, None, &[]).unwrap();
        assert!(db.linked().unwrap().contains_key("Charlie Haden & Pat Metheny"));
        lib.artists.iter_mut().find(|a| a.name == "Pat Metheny").unwrap().mbids = vec!["pat".into()];
        lib.artists.iter_mut().find(|a| a.name.starts_with("Charlie")).unwrap().mbids = vec!["charlie".into(), "pat".into()];
        db.write_library(&lib).unwrap();
        assert!(!db.linked().unwrap().contains_key("Charlie Haden & Pat Metheny"), "asked about its members");
        db.write_links("Charlie Haden & Pat Metheny", None, None, None, &[link("Charlie Haden", Some("charlie"), "member", "tags"), link("Pat Metheny", Some("pat"), "member", "tags")]).unwrap();
        db.write_library(&lib).unwrap();
        assert!(db.linked().unwrap().contains_key("Charlie Haden & Pat Metheny"), "settled");
        let relations = db.relations().unwrap();
        assert_eq!(relations.len(), 1, "Pat Metheny is in the library, Charlie Haden is not");
        assert_eq!(relations[0].other, "Pat Metheny");
        let count: i64 = db.conn.query_row("select count(*) from artist", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 3);
    }
}
