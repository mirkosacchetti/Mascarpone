//! The radio: a queue that never ends, each track like the ones before
//! it. bliss listens to every file of the library once and keeps, in the
//! database, what it heard as a vector of numbers (tempo, timbre,
//! loudness, chroma), and YAMNet with Essentia's head (`voice.rs`)
//! whether a voice sings in it; two tracks are alike when their vectors,
//! the voice counted in, are near.
//! The analysis runs on a thread of its own, at the lowest priority,
//! a few files at a time, after every load of the library: the files
//! new or changed since (by their stamp), and the rows of files gone are
//! dropped. The vectors go to the window as they come, so the radio
//! can start while the analysis runs.
//!
//! The radio starts from a track, which plays first; from an album,
//! whose tracks together are what the path starts from, none of them
//! played; or from an artist, twenty of whose tracks open it at random,
//! and whose tracks are all it plays. Then, whenever fewer than five
//! tracks remain after the song on, it adds twenty: an arc from the
//! seed, always from the seed and never from what played last (the
//! equilibrium, the break, the adventure, the resolution, the new
//! equilibrium, four tracks each, each part passing into the next a
//! quarter at a time), each track the nearest to the seed's vector
//! shifted for its place in the arc, one of the six
//! nearest (the nearer the likelier: the chance is what keeps it
//! varied), never one already in the queue, never one more vocal than
//! where the radio started (from an instrumental, instrumentals only).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use bliss_audio::decoder::symphonia::SymphoniaDecoder;
use bliss_audio::decoder::Decoder;
use bliss_audio::Song;
use serde::{Deserialize, Serialize};

use crate::db::{self, Done, Link, Relation, Stamp};
use crate::library::{Library, Playlist};
use crate::mpd::{Event, Out};
use crate::voice::{Models, Voice};

pub const N: usize = bliss_audio::NUMBER_FEATURES;
pub type Vector = [f32; N];

/// How many tracks the radio adds at a time, and how many of the
/// artist's open their radio.
pub const STEP: usize = 20;
/// The radio adds a step when fewer than this remain after the song on.
pub const LOW: usize = 5;
/// A step goes to one of this many nearest tracks.
const NEAREST: usize = 6;
/// How many tracks a part of the arc lasts, the passage to the next
/// part spread over them: five parts of four, a block.
const PER_PART: usize = STEP / ARC.len();
/// The radio picks no track longer than this (a whole side, a live
/// set): the seed, if it is, plays as it is.
const LONGEST: Duration = Duration::from_secs(15 * 60);
/// How much a link to the last tracks' artists shortens a distance: by
/// this much of it, for a full link.
const PULL: f32 = 0.35;

/// The analyses, by uri, as the window has them: bliss's vectors, and
/// the voice heard.
#[derive(Debug, Default, Clone)]
pub struct Vectors {
    pub by_uri: HashMap<String, Vector>,
    pub voice: HashMap<String, Voice>,
}

/// How much the voice counts in the distance between two tracks: each
/// of its two numbers, taken to -1..1, weighs this much against bliss's
/// features, which are in that range too.
const VOICE_WEIGHT: f32 = 1.0;

/// A track's vector with its voice at the end, for the distance.
type Full = [f32; N + 2];

fn full(vector: &Vector, voice: Option<&Voice>) -> Full {
    let mut f = [0f32; N + 2];
    f[..N].copy_from_slice(vector);
    if let Some(v) = voice {
        f[N] = (2.0 * v.mean - 1.0) * VOICE_WEIGHT;
        f[N + 1] = (2.0 * v.share - 1.0) * VOICE_WEIGHT;
    }
    f
}

/// Where a radio started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seed {
    Track(String),
    /// By its key.
    Album(String),
    /// By name: only their tracks.
    Artist(String),
}

pub enum Job {
    Library(Arc<Library>),
    /// The playlists read again.
    Playlists(Vec<Playlist>),
    /// Where a playlist just written came from (see `Db::set_origin`).
    Origin { path: PathBuf, name: String, origin: String, url: Option<String> },
    /// What the net said of an artist (see `net::links`).
    Links { name: String, mbid: Option<String>, deezer: Option<i64>, links: Vec<Link>, error: Option<String> },
}

/// The artists' links, by a library artist's name: the library artists
/// linked to them, each with the link's weight (1 for a relation of
/// MusicBrainz's or an influence, 0.7 for one Deezer finds alike),
/// both ways.
#[derive(Debug, Default, Clone)]
pub struct Relations {
    pub by_artist: HashMap<String, HashMap<String, f32>>,
}

impl Relations {
    /// From the links between two artists of the library, as the
    /// database resolves them (by id).
    fn build(rows: &[Relation]) -> Relations {
        let mut relations = Relations::default();
        for row in rows {
            if row.other == row.artist {
                continue;
            }
            let weight = if row.source == "deezer" { 0.7 } else { 1.0 };
            for (a, b) in [(&row.artist, &row.other), (&row.other, &row.artist)] {
                let w = relations.by_artist.entry(a.clone()).or_default().entry(b.clone()).or_insert(0.0);
                *w = w.max(weight);
            }
        }
        relations
    }

    /// How linked an artist is to any of these.
    fn weight(&self, artist: &str, to: &[&str]) -> f32 {
        let Some(links) = self.by_artist.get(artist) else { return 0.0 };
        to.iter().filter_map(|a| links.get(*a)).copied().fold(0.0, f32::max)
    }
}

/// Starts the radio thread: the analysis, the artists' links. What it
/// finds goes out as events; the links it wants it asks mpd's thread for.
pub fn start(jobs: mpsc::Receiver<Job>, out: Out, commands: tokio::sync::mpsc::UnboundedSender<crate::mpd::Command>, db: db::Shared) {
    std::thread::Builder::new()
        .name("radio".into())
        .spawn(move || {
            let (vectors, artists) = {
                let db = db.lock().unwrap();
                match (db.vectors(), db.linked()) {
                    (Ok((by_uri, voice)), Ok(a)) => (Vectors { by_uri, voice }, a),
                    (Err(e), _) | (_, Err(e)) => {
                        eprintln!("{}: {e}", db::path().display());
                        return;
                    }
                }
            };
            eprintln!("radio: {} tracks analyzed, {} heard for a voice, {} artists linked", vectors.by_uri.len(), vectors.voice.len(), artists.len());
            out.send(Event::Vectors(Arc::new(vectors.clone())));
            let models = match Models::load() {
                Ok(m) => Some(m),
                Err(e) => {
                    eprintln!("radio: the voice models: {e}: no voice heard");
                    None
                }
            };
            let mut radio = Radio { db, out, commands, vectors, artists, library: None, linking: (0, 0), models: Arc::new(models) };
            radio.run(jobs);
        })
        .expect("radio thread");
}

/// A file to analyze: its track in the library, its path and stamp,
/// and what it lacks: bliss's features, the voice.
struct Todo {
    album: usize,
    track: usize,
    path: PathBuf,
    stamp: Stamp,
    bliss: bool,
    voice: bool,
}

/// What a file gave: bliss's features and the voice, each when asked
/// for, each the error when the file could not be heard.
struct Heard {
    bliss: Option<Result<Vector, String>>,
    voice: Option<Result<Voice, String>>,
}

/// A file decoded once, then given to bliss and to the voice models.
fn hear(models: &Option<Models>, t: &Todo) -> Heard {
    let pre = match <SymphoniaDecoder as Decoder>::decode(&t.path) {
        Ok(pre) => pre,
        Err(e) => {
            let e = e.to_string();
            return Heard { bliss: t.bliss.then(|| Err(e.clone())), voice: t.voice.then_some(Err(e)) };
        }
    };
    let voice = match (t.voice, models) {
        (true, Some(models)) => Some(models.hear(&pre.sample_array, 22050).map_err(|e| e.to_string())),
        _ => None,
    };
    let bliss = t.bliss.then(|| {
        Song::try_from(pre).map_err(|e| e.to_string()).map(|s| {
            let mut v = [0f32; N];
            v.copy_from_slice(&s.analysis.as_vec());
            v
        })
    });
    Heard { bliss, voice }
}

struct Radio {
    db: db::Shared,
    out: Out,
    commands: tokio::sync::mpsc::UnboundedSender<crate::mpd::Command>,
    vectors: Vectors,
    /// The library's artists the net has answered about, with their id.
    artists: HashMap<String, Option<String>>,
    library: Option<Arc<Library>>,
    /// The links being fetched: done, of how many asked.
    linking: (usize, usize),
    /// The voice models, shared with the workers; none when they could
    /// not be loaded.
    models: Arc<Option<Models>>,
}

impl Radio {
    fn run(&mut self, jobs: mpsc::Receiver<Job>) {
        let mut pending: Option<Arc<Library>> = None;
        loop {
            let mut library = match pending.take() {
                Some(l) => l,
                None => match jobs.recv() {
                    Ok(Job::Library(l)) => l,
                    Ok(job) => {
                        self.other(job);
                        continue;
                    }
                    Err(_) => return,
                },
            };
            // only the last library counts
            while let Ok(job) = jobs.try_recv() {
                match job {
                    Job::Library(l) => library = l,
                    job => self.other(job),
                }
            }
            self.library = Some(library.clone());
            {
                let mut db = self.db.lock().unwrap();
                if let Err(e) = db.write_library(&library) {
                    eprintln!("radio: {e}");
                }
                // the writing may have an artist asked about again: one
                // whose tags name another than the net found
                match db.linked() {
                    Ok(a) => self.artists = a,
                    Err(e) => eprintln!("radio: {e}"),
                }
            }
            self.ask_links(&library);
            let Some(todo) = self.diff(&library) else { continue };
            if todo.is_empty() {
                continue;
            }
            let total = todo.len();
            eprintln!("radio: {total} tracks to analyze");
            self.out.send(Event::Analyzing { done: 0, total });
            // the pool: the workers take files from the list, the results
            // come back here one by one
            let work = Arc::new(Mutex::new(todo));
            let (done_tx, done_rx) = mpsc::channel::<(Todo, Heard)>();
            for _ in 0..crate::config::get().threads {
                let (work, done_tx, models) = (work.clone(), done_tx.clone(), self.models.clone());
                std::thread::Builder::new()
                    .name("bliss".into())
                    .spawn(move || {
                        lowest_priority();
                        loop {
                            let Some(t) = work.lock().unwrap().pop() else { break };
                            let heard = hear(&models, &t);
                            if done_tx.send((t, heard)).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("bliss thread");
            }
            drop(done_tx);
            let mut done = 0;
            let mut sent = Instant::now();
            for (t, heard) in done_rx {
                self.store(&library, t, heard);
                done += 1;
                // a new library: the rest is done over against it
                while let Ok(job) = jobs.try_recv() {
                    match job {
                        Job::Library(l) => {
                            work.lock().unwrap().clear();
                            pending = Some(l);
                        }
                        job => self.other(job),
                    }
                }
                if sent.elapsed() > Duration::from_secs(2) || done == total {
                    self.out.send(Event::Vectors(Arc::new(self.vectors.clone())));
                    self.out.send(Event::Analyzing { done, total });
                    sent = Instant::now();
                }
            }
            if done < total {
                self.out.send(Event::Vectors(Arc::new(self.vectors.clone())));
                self.out.send(Event::Analyzing { done: total, total });
            }
            eprintln!("radio: {done} analyzed");
        }
    }

    /// The files to analyze: new or changed since their row; the rows of
    /// files gone dropped. None when the database could not be read.
    fn diff(&mut self, library: &Library) -> Option<Vec<Todo>> {
        let stamps = match self.db.lock().unwrap().stamps() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("radio: {e}");
                return None;
            }
        };
        let music = crate::library::music_dir();
        let mut todo: Vec<Todo> = Vec::new();
        let mut present: HashSet<&str> = HashSet::new();
        let hearing = self.models.is_some();
        for (a, album) in library.albums.iter().enumerate() {
            for (t, track) in album.tracks.iter().enumerate() {
                present.insert(&track.uri);
                let path = music.join(&track.uri);
                let Some(stamp) = stamp_of(&path) else { continue };
                // the file as it was analyzed: only what is missing
                let done = stamps.get(&track.uri).filter(|d| d.stamp == stamp).copied().unwrap_or(Done { stamp, bliss: false, voice: false });
                let (bliss, voice) = (!done.bliss, !done.voice && hearing);
                if bliss || voice {
                    todo.push(Todo { album: a, track: t, path, stamp, bliss, voice });
                }
            }
        }
        // the rows of files gone went with the library's writing
        let gone: Vec<String> = stamps.keys().filter(|u| !present.contains(u.as_str())).cloned().collect();
        if !gone.is_empty() {
            for u in &gone {
                self.vectors.by_uri.remove(u);
                self.vectors.voice.remove(u);
            }
            self.out.send(Event::Vectors(Arc::new(self.vectors.clone())));
        }
        // the first in the library first: the list is popped from its end
        todo.reverse();
        Some(todo)
    }

    /// What a file gave written onto the track's row, and kept.
    fn store(&mut self, library: &Library, t: Todo, heard: Heard) {
        let track = &library.albums[t.album].tracks[t.track];
        if let Some(result) = heard.bliss {
            if let Err(e) = &result {
                eprintln!("radio: {}: {e}", track.uri);
            }
            if let Err(e) = self.db.lock().unwrap().write_analysis(&track.uri, t.stamp, &result) {
                eprintln!("radio: {}: {e}", track.uri);
            }
            match result {
                Ok(v) => {
                    self.vectors.by_uri.insert(track.uri.clone(), v);
                }
                Err(_) => {
                    self.vectors.by_uri.remove(&track.uri);
                }
            }
        }
        if let Some(result) = heard.voice {
            if let Err(e) = &result {
                eprintln!("radio: {}: voice: {e}", track.uri);
            }
            if let Err(e) = self.db.lock().unwrap().write_voice(&track.uri, t.stamp, &result) {
                eprintln!("radio: {}: {e}", track.uri);
            }
            match result {
                Ok(v) => {
                    self.vectors.voice.insert(track.uri.clone(), v);
                }
                Err(_) => {
                    self.vectors.voice.remove(&track.uri);
                }
            }
        }
    }

    /// The artists of the library the net has not answered about, asked
    /// for; the links between the library's artists sent.
    fn ask_links(&mut self, library: &Library) {
        let names: HashSet<&str> = library.artists.iter().map(|a| a.name.as_str()).collect();
        self.artists.retain(|n, _| names.contains(n.as_str()));
        let missing: Vec<(String, Vec<String>)> = library.artists.iter().filter(|a| !self.artists.contains_key(&a.name)).map(|a| (a.name.clone(), a.mbids.clone())).collect();
        self.linking = (0, missing.len());
        self.out.send(Event::Linking { done: 0, total: missing.len() });
        if !missing.is_empty() {
            eprintln!("radio: {} artists to link", missing.len());
            let _ = self.commands.send(crate::mpd::Command::Links(missing));
        }
        self.send_relations();
    }

    /// A job other than a library: the playlists written; what the net
    /// said of an artist, written and counted (one the net erred on is
    /// written with its error and asked again at the next library load).
    fn other(&mut self, job: Job) {
        match job {
            Job::Library(_) => {}
            Job::Playlists(playlists) => {
                if let Err(e) = self.db.lock().unwrap().write_playlists(&playlists) {
                    eprintln!("radio: {e}");
                }
            }
            Job::Origin { path, name, origin, url } => {
                if let Err(e) = self.db.lock().unwrap().set_origin(&path, &name, &origin, url.as_deref()) {
                    eprintln!("radio: {e}");
                }
            }
            Job::Links { name, mbid, deezer, links, error } => {
                if let Err(e) = self.db.lock().unwrap().write_links(&name, mbid.as_deref(), deezer, error.as_deref(), &links) {
                    eprintln!("radio: {name}: {e}");
                    return;
                }
                if error.is_none() {
                    self.artists.insert(name, mbid);
                }
                self.linking.0 += 1;
                self.out.send(Event::Linking { done: self.linking.0, total: self.linking.1.max(self.linking.0) });
                self.send_relations();
            }
        }
    }

    fn send_relations(&self) {
        match self.db.lock().unwrap().relations() {
            Ok(rows) => {
                self.out.send(Event::Relations(Arc::new(Relations::build(&rows))));
            }
            Err(e) => eprintln!("radio: {e}"),
        }
    }
}

/// A file's modification time and size.
fn stamp_of(path: &std::path::Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(Stamp { mtime, size: meta.len() as i64 })
}

/// This thread at the lowest priority: the analysis yields to everything.
fn lowest_priority() {
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 19);
    }
}

fn distance(a: &Full, b: &Full) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

fn mean(vs: &[Full]) -> Full {
    let mut m = [0f32; N + 2];
    for v in vs {
        for (i, x) in v.iter().enumerate() {
            m[i] += x;
        }
    }
    for x in &mut m {
        *x /= vs.len() as f32;
    }
    m
}

/// What plays first: the track; nothing for an album, which is only
/// where the path starts from; twenty of the artist's tracks at random.
pub fn opening(library: &Library, seed: &Seed) -> Vec<String> {
    match seed {
        Seed::Track(uri) => library.place(uri).map(|_| vec![uri.clone()]).unwrap_or_default(),
        Seed::Album(_) => Vec::new(),
        Seed::Artist(name) => {
            let mut theirs: Vec<String> = own_tracks(library, name).into_iter().filter(|u| fits(library, u)).collect();
            fastrand::shuffle(&mut theirs);
            theirs.truncate(STEP);
            theirs
        }
    }
}

/// What the path starts from: the seed's own tracks (a track's, an
/// album's, an artist's), as analyzed.
fn seed_vectors(library: &Library, vectors: &Vectors, seed: &Seed) -> Vec<Full> {
    let uris = match seed {
        Seed::Track(uri) => vec![uri.clone()],
        Seed::Album(key) => library.album_by_key(key).map(|a| a.uris()).unwrap_or_default(),
        Seed::Artist(name) => own_tracks(library, name),
    };
    uris.iter().filter_map(|u| vectors.full(u)).collect()
}

impl Vectors {
    /// How spread each feature is over the library: its standard
    /// deviation, the unit of the arc's shifts. Over the tracks
    /// analyzed; the voice's, over the tracks heard.
    fn spread(&self) -> Full {
        let mut s = [0f32; N + 2];
        for i in 0..N + 2 {
            let xs: Vec<f32> = match i < N {
                true => self.by_uri.values().map(|v| v[i]).collect(),
                false => self.voice.values().map(|v| (2.0 * if i == N { v.mean } else { v.share } - 1.0) * VOICE_WEIGHT).collect(),
            };
            if xs.is_empty() {
                continue;
            }
            let mean = xs.iter().sum::<f32>() / xs.len() as f32;
            s[i] = (xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32).sqrt();
        }
        s
    }

    /// A track's vector with its voice, if analyzed.
    fn full(&self, uri: &str) -> Option<Full> {
        Some(full(self.by_uri.get(uri)?, self.voice.get(uri)))
    }

    /// How vocal the seed is: a track's voice, the most vocal of an
    /// album's or an artist's tracks; none until heard.
    fn vocality(&self, library: &Library, seed: &Seed) -> Option<f32> {
        let uris = match seed {
            Seed::Track(uri) => vec![uri.clone()],
            Seed::Album(key) => library.album_by_key(key).map(|a| a.uris()).unwrap_or_default(),
            Seed::Artist(name) => own_tracks(library, name),
        };
        uris.iter().filter_map(|u| self.voice.get(u)).map(|v| v.mean).reduce(f32::max)
    }
}

/// Every track of an artist's albums.
/// Whether a track is short enough for the radio (one of unknown
/// length is).
fn fits(library: &Library, uri: &str) -> bool {
    library.place(uri).and_then(|(a, t)| library.albums[a].tracks[t].duration).is_none_or(|d| d <= LONGEST)
}

fn own_tracks(library: &Library, name: &str) -> Vec<String> {
    library.artist(name).map(|i| library.artists[i].albums.iter().flat_map(|&a| library.albums[a].uris()).collect()).unwrap_or_default()
}

/// The arc a block follows, four tracks a part: each part a shift of
/// the seed's vector along bliss's features (by index: tempo
/// 0, zero-crossings 1, the spectral centroid 2 and its deviation 3,
/// the rolloff 4 and 5, the flatness 6 and 7, the loudness 8 and 9,
/// the major, minor, diminished and augmented triads 16 to 19; the
/// voice's mean and share after them), in standard deviations of the
/// feature over the library: bliss's -1..1 is a theoretical range, and
/// a feature the library spans a tenth of would otherwise be shifted
/// out of it. The equilibrium: the seed, steadier. The break: tension,
/// the minor and diminished up, the loudness and the brightness less
/// steady. The adventure: faster, louder, brighter, more varied, more
/// vocal. The resolution: loud and open, the major up. The new
/// equilibrium: slower, quieter, darker, sparser, less vocal, at peace.
const ARC: [(&str, &[(usize, f32)]); 5] = [
    ("equilibrium", &[(3, -0.5), (5, -0.5), (7, -0.5), (9, -0.5)]),
    ("break", &[(3, 0.75), (5, 0.75), (7, 0.5), (9, 0.75), (16, -0.5), (17, 0.75), (18, 0.5)]),
    ("adventure", &[(0, 1.0), (1, 0.75), (2, 1.0), (4, 1.0), (8, 1.0), (9, 0.75), (17, 0.5), (N, 0.75), (N + 1, 0.75)]),
    ("resolution", &[(8, 0.75), (9, 0.5), (16, 1.0), (17, -0.75), (18, -0.5)]),
    ("new equilibrium", &[(0, -1.0), (1, -0.75), (2, -1.0), (4, -1.0), (6, -0.5), (8, -1.0), (9, -0.75), (16, 0.5), (N, -0.75), (N + 1, -0.75)]),
];

/// The shift of the arc's track number `at` (from 0, round and round):
/// its part's, passing into the next part's a quarter a track, so that
/// the fourth track of the break is three quarters of the way to the
/// adventure, and the last of the new equilibrium nearly back at the
/// equilibrium.
fn shift(at: usize) -> Full {
    let part = (at / PER_PART) % ARC.len();
    let t = (at % PER_PART) as f32 / PER_PART as f32;
    let mut s = [0f32; N + 2];
    for &(i, d) in ARC[part].1 {
        s[i] += (1.0 - t) * d;
    }
    for &(i, d) in ARC[(part + 1) % ARC.len()].1 {
        s[i] += t * d;
    }
    s
}

/// The seed's vector shifted, each shift scaled by the feature's
/// spread, kept in -1..1.
fn shifted(centre: &Full, deltas: &Full, spread: &Full) -> Full {
    let mut target = *centre;
    for i in 0..N + 2 {
        target[i] = (target[i] + deltas[i] * spread[i]).clamp(-1.0, 1.0);
    }
    target
}

/// The artists of the seed, whose links pull.
fn seed_artists<'a>(library: &'a Library, seed: &'a Seed) -> Vec<&'a str> {
    match seed {
        Seed::Track(uri) => library.place(uri).map(|(a, _)| vec![library.albums[a].artist.as_str()]).unwrap_or_default(),
        Seed::Album(key) => library.album_by_key(key).map(|a| vec![a.artist.as_str()]).unwrap_or_default(),
        Seed::Artist(name) => vec![name.as_str()],
    }
}

/// The next `n` tracks after `queue`: an arc from the seed, always
/// from the seed (a track's vector, the mean of an album's or an
/// artist's), never from what played last: four tracks a part, each
/// part passing into the next over them, each track the nearest to
/// the seed shifted for its place, one of the six nearest, the nearer
/// the likelier.
/// Fewer when there is nothing to go on (the seed not analyzed yet) or
/// nothing left to choose. The seed's vocality is a ceiling: no track
/// more vocal than it (one not heard yet passes).
pub fn path(library: &Library, vectors: &Vectors, relations: &Relations, seed: &Seed, queue: &[String], n: usize) -> Vec<String> {
    let ceiling = vectors.vocality(library, seed);
    let quiet = |u: &str| fits(library, u) && ceiling.is_none_or(|c| vectors.voice.get(u).is_none_or(|v| v.mean <= c));
    // the pool: the artist's tracks, or every analyzed track of the
    // library, under the ceiling and not too long
    let pool: Vec<(&str, Full, usize)> = match seed {
        Seed::Artist(name) => own_tracks(library, name).into_iter().filter_map(|u| library.place(&u).map(|(a, _)| (a, u))).filter_map(|(a, u)| vectors.by_uri.get_key_value(&u).map(|(k, v)| (k.as_str(), full(v, vectors.voice.get(k)), a))).filter(|(u, _, _)| quiet(u)).collect(),
        _ => vectors.by_uri.iter().filter(|(u, _)| quiet(u)).filter_map(|(u, v)| library.place(u).map(|(a, _)| (u.as_str(), full(v, vectors.voice.get(u)), a))).collect(),
    };
    if pool.is_empty() {
        return Vec::new();
    }
    let seeds = seed_vectors(library, vectors, seed);
    if seeds.is_empty() {
        return Vec::new();
    }
    let centre = mean(&seeds);
    let spread = vectors.spread();
    let artists = seed_artists(library, seed);
    let mut excluded: HashSet<&str> = queue.iter().map(String::as_str).collect();
    // everything played already: the radio starts over, the last ten
    // aside (the last one, when there are not even ten)
    if pool.iter().all(|(u, _, _)| excluded.contains(u)) {
        for keep in [10, 1, 0] {
            excluded = queue.iter().rev().take(keep).map(String::as_str).collect();
            if pool.iter().any(|(u, _, _)| !excluded.contains(u)) {
                break;
            }
        }
    }
    // the arc, a track at a time
    let mut out: Vec<String> = Vec::with_capacity(n);
    for at in 0..n {
        let target = shifted(&centre, &shift(at), &spread);
        let mut best: Vec<(f32, usize)> = pool
            .iter()
            .enumerate()
            .filter(|(_, (u, _, _))| !excluded.contains(u))
            .map(|(i, (_, v, a))| (distance(&target, v) * (1.0 - PULL * relations.weight(&library.albums[*a].artist, &artists)), i))
            .collect();
        if best.is_empty() {
            break;
        }
        best.sort_by(|x, y| x.0.total_cmp(&y.0));
        best.truncate(NEAREST);
        // the nearer the likelier: the first weighs NEAREST, the last 1
        let weights: Vec<usize> = (0..best.len()).map(|r| NEAREST - r).collect();
        let mut pick = fastrand::usize(..weights.iter().sum::<usize>());
        let mut chosen = 0;
        for (r, w) in weights.iter().enumerate() {
            if pick < *w {
                chosen = r;
                break;
            }
            pick -= w;
        }
        let (u, _, _) = pool[best[chosen].1];
        out.push(u.to_owned());
        excluded.insert(u);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library of albums `artist/album` with `n` tracks each.
    fn library(albums: &[(&str, &str, usize)]) -> Library {
        Library::of(albums)
    }

    fn at(x: f32) -> Vector {
        let mut v = [0f32; N];
        v[0] = x;
        v
    }

    #[test]
    fn the_path_goes_near_and_never_back() {
        let lib = library(&[("A", "a", 3), ("B", "b", 3), ("C", "c", 3)]);
        let mut vectors = Vectors::default();
        // A's tracks at 0.0, 0.1, 0.2; B's at 1.0..; C's at 5.0..
        for (artist, base) in [("A", 0.0), ("B", 1.0), ("C", 5.0)] {
            for i in 1..=3 {
                vectors.by_uri.insert(format!("{artist}/{}/{i}.ogg", artist.to_lowercase()), at(base + 0.1 * i as f32));
            }
        }
        let relations = Relations::default();
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = opening(&lib, &seed);
        assert_eq!(queue, vec!["A/a/1.ogg".to_string()]);
        let path = path(&lib, &vectors, &relations, &seed, &queue, 8);
        assert_eq!(path.len(), 8, "every other track, once");
        assert!(!path.contains(&"A/a/1.ogg".to_string()));
        let mut all = path.clone();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 8);
        // C is far: the first step hardly ever goes there (one chance in
        // twenty-one, as the sixth nearest)
        let far = (0..200).filter(|_| super::path(&lib, &vectors, &relations, &seed, &queue, 1)[0].starts_with("C/")).count();
        assert!(far < 40, "{far} of 200 went far");
        // everything played: it starts over
        let whole: Vec<String> = queue.iter().cloned().chain(path).collect();
        assert_eq!(super::path(&lib, &vectors, &relations, &seed, &whole, 3).len(), 3);
    }

    #[test]
    fn an_album_is_only_the_start_and_an_artist_is_all() {
        let lib = library(&[("A", "a", 3), ("B", "b", 30)]);
        let mut vectors = Vectors::default();
        for i in 1..=3 {
            vectors.by_uri.insert(format!("A/a/{i}.ogg"), at(0.0));
        }
        for i in 1..=30 {
            vectors.by_uri.insert(format!("B/b/{i}.ogg"), at(1.0));
        }
        let relations = Relations::default();
        let album = Seed::Album("A\u{1f}a".into());
        assert!(opening(&lib, &album).is_empty());
        let path = path(&lib, &vectors, &relations, &album, &[], 5);
        assert_eq!(path.len(), 5);
        assert!(path.iter().all(|u| u.starts_with("A/") || u.starts_with("B/")));
        let artist = Seed::Artist("B".into());
        let open = opening(&lib, &artist);
        assert_eq!(open.len(), STEP);
        assert!(open.iter().all(|u| u.starts_with("B/")));
        let more = super::path(&lib, &vectors, &relations, &artist, &open, STEP);
        assert_eq!(more.len(), 10, "the ten of B's left");
        assert!(more.iter().all(|u| u.starts_with("B/") && !open.contains(u)));
    }

    #[test]
    fn nothing_more_vocal_than_the_seed() {
        let lib = library(&[("A", "a", 1), ("B", "b", 1), ("C", "c", 1), ("D", "d", 1)]);
        let mut vectors = Vectors::default();
        for (u, voice) in [("A/a/1.ogg", 0.4), ("B/b/1.ogg", 0.9), ("C/c/1.ogg", 0.2)] {
            vectors.by_uri.insert(u.into(), at(0.0));
            vectors.voice.insert(u.into(), Voice { mean: voice, share: voice });
        }
        // D not heard yet: it passes
        vectors.by_uri.insert("D/d/1.ogg".into(), at(0.0));
        let relations = Relations::default();
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = vec!["A/a/1.ogg".to_string()];
        let mut path = path(&lib, &vectors, &relations, &seed, &queue, 5);
        path.sort();
        assert_eq!(path, vec!["C/c/1.ogg".to_string(), "D/d/1.ogg".to_string()]);
        // from the vocal one, everything
        let seed = Seed::Track("B/b/1.ogg".into());
        assert_eq!(super::path(&lib, &vectors, &relations, &seed, &["B/b/1.ogg".to_string()], 5).len(), 3);
    }

    #[test]
    fn a_shift_is_in_the_library_s_units() {
        let mut vectors = Vectors::default();
        // the tempo spans the range, the zero-crossings a tenth of it
        for i in 0..10 {
            let mut v = at(-1.0 + 0.2 * i as f32);
            v[1] = -1.0 + 0.02 * i as f32;
            vectors.by_uri.insert(format!("A/a/{i}.ogg"), v);
        }
        let spread = vectors.spread();
        assert!((spread[0] / spread[1] - 10.0).abs() < 0.01, "{} and {}", spread[0], spread[1]);
        assert_eq!(spread[N], 0.0, "no voice heard, no spread");
        let centre = [0f32; N + 2];
        let mut deltas = [0f32; N + 2];
        for i in [0, 1, N] {
            deltas[i] = 1.0;
        }
        let target = shifted(&centre, &deltas, &spread);
        assert!((target[0] - spread[0]).abs() < 1e-6);
        assert!((target[0] / target[1] - 10.0).abs() < 0.01);
        assert_eq!(target[N], 0.0);
    }

    #[test]
    fn nothing_longer_than_a_quarter_of_an_hour() {
        let mut lib = library(&[("A", "a", 1), ("B", "b", 1), ("C", "c", 1)]);
        let b = lib.album_index("B\u{1f}b").unwrap();
        lib.albums[b].tracks[0].duration = Some(LONGEST + Duration::from_secs(1));
        let c = lib.album_index("C\u{1f}c").unwrap();
        lib.albums[c].tracks[0].duration = Some(LONGEST);
        let mut vectors = Vectors::default();
        for u in ["A/a/1.ogg", "B/b/1.ogg", "C/c/1.ogg"] {
            vectors.by_uri.insert(u.into(), at(0.0));
        }
        let relations = Relations::default();
        let seed = Seed::Track("A/a/1.ogg".into());
        let path = path(&lib, &vectors, &relations, &seed, &["A/a/1.ogg".to_string()], 5);
        assert_eq!(path, vec!["C/c/1.ogg".to_string()], "B is too long, C just fits");
        // an artist's opening too
        let seed = Seed::Artist("B".into());
        assert!(opening(&lib, &seed).is_empty());
    }

    #[test]
    fn a_part_passes_into_the_next_a_quarter_a_track() {
        // the tempo (0): the break leaves it, the adventure raises it
        // by one, the new equilibrium lowers it by one
        let adventure = 2 * PER_PART;
        assert_eq!(shift(adventure)[0], 1.0, "the first track of the adventure is all adventure");
        assert_eq!(shift(adventure - 2)[0], 0.5, "halfway through the break, halfway to the adventure");
        assert_eq!(shift(adventure - 1)[0], 0.75);
        assert_eq!(shift(STEP - 1)[0], -0.25, "the last track, nearly back at the equilibrium");
        assert_eq!(shift(STEP)[0], 0.0, "the next block starts over");
    }

    #[test]
    fn a_link_pulls() {
        let lib = library(&[("A", "a", 1), ("B", "b", 1), ("C", "c", 1)]);
        let mut vectors = Vectors::default();
        vectors.by_uri.insert("A/a/1.ogg".into(), at(0.0));
        vectors.by_uri.insert("B/b/1.ogg".into(), at(1.0));
        vectors.by_uri.insert("C/c/1.ogg".into(), at(0.8));
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = vec!["A/a/1.ogg".to_string()];
        // B is linked to A: 1.0 * 0.65 = 0.65 < 0.8, it comes before C
        let rows = vec![Relation { artist: "A".into(), other: "B".into(), source: "musicbrainz".into() }];
        let relations = Relations::build(&rows);
        assert_eq!(relations.weight("B", &["A"]), 1.0);
        assert_eq!(relations.weight("A", &["B"]), 1.0);
        assert_eq!(relations.weight("C", &["A"]), 0.0);
        let mut firsts = HashSet::new();
        for _ in 0..200 {
            firsts.insert(path(&lib, &vectors, &relations, &seed, &queue, 1).remove(0));
        }
        // nearer, likelier; with the chance, both come
        assert!(firsts.contains("B/b/1.ogg"));
    }
}
