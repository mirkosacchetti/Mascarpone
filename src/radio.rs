use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bliss_audio::decoder::symphonia::SymphoniaDecoder;
use bliss_audio::decoder::Decoder;
use bliss_audio::Song;
use serde::{Deserialize, Serialize};

use crate::db::{self, Done, Link, Relation, Stamp};
use crate::library::{Library, Playlist};
use crate::mpd::{Event, Out};
use crate::shape::{self, Shape};
use crate::voice::{Models, Voice};

pub const N: usize = bliss_audio::NUMBER_FEATURES;
pub type Vector = [f32; N];

/// The tracks of an arc, and the arcs a radio starts with.
pub const STEP: usize = 15;
pub const ARCS: usize = 3;
const NEAREST: usize = 6;
const PER_PART: usize = STEP / ARC.len();
const LONGEST: Duration = Duration::from_secs(15 * 60);
const PULL: f32 = 0.35;
/// How long after the start the analysis waits: the window, the
/// library and the covers come first, on a quiet machine.
const SETTLE: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Clone)]
pub struct Vectors {
    pub by_uri: HashMap<String, Vector>,
    pub voice: HashMap<String, Voice>,
    pub shape: HashMap<String, Shape>,
}

const VOICE_WEIGHT: f32 = 1.0;
const SHAPE_WEIGHT: f32 = 1.0;

/// The whole of a track as the radio measures it: bliss's vector,
/// the two of the voice, the eight of the shape.
type Full = [f32; F];
const F: usize = N + 2 + shape::N;
/// Where the voice and the shape start in it.
const VOICE: usize = N;
const SHAPE: usize = N + 2;

fn full(vector: &Vector, voice: Option<&Voice>, shape: Option<&Shape>) -> Full {
    let mut f = [0f32; F];
    f[..N].copy_from_slice(vector);
    if let Some(v) = voice {
        f[VOICE] = (2.0 * v.mean - 1.0) * VOICE_WEIGHT;
        f[VOICE + 1] = (2.0 * v.share - 1.0) * VOICE_WEIGHT;
    }
    if let Some(s) = shape {
        for (i, x) in s.iter().enumerate() {
            f[SHAPE + i] = x * SHAPE_WEIGHT;
        }
    }
    f
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seed {
    Track(String),
    Album(String),
    Artist(String),
}

pub enum Job {
    Library(Arc<Library>),
    Playlists(Vec<Playlist>),
    Origin { path: PathBuf, name: String, origin: String, url: Option<String> },
    Links { name: String, mbid: Option<String>, deezer: Option<i64>, links: Vec<Link>, error: Option<String> },
}

#[derive(Debug, Default, Clone)]
pub struct Relations {
    pub by_artist: HashMap<String, HashMap<String, f32>>,
}

impl Relations {
    fn build(rows: &[Relation]) -> Relations {
        let mut relations = Relations::default();
        for row in rows {
            if row.other == row.artist {
                continue;
            }
            let weight = if row.source == "deezer" { 0.7 } else { 1.0 };
            let (artist, other) = (row.artist.to_lowercase(), row.other.to_lowercase());
            for (a, b) in [(&artist, &other), (&other, &artist)] {
                let w = relations.by_artist.entry(a.clone()).or_default().entry(b.clone()).or_insert(0.0);
                *w = w.max(weight);
            }
        }
        relations
    }

    fn weight(&self, artist: &str, to: &[&str]) -> f32 {
        let Some(links) = self.by_artist.get(&artist.to_lowercase()) else { return 0.0 };
        to.iter().filter_map(|a| links.get(&a.to_lowercase())).copied().fold(0.0, f32::max)
    }
}

pub fn start(jobs: mpsc::Receiver<Job>, out: Out, commands: tokio::sync::mpsc::UnboundedSender<crate::mpd::Command>, db: db::Shared) {
    std::thread::Builder::new()
        .name("radio".into())
        .spawn(move || {
            let (vectors, artists) = {
                let db = db.lock().unwrap();
                match (db.vectors(), db.linked()) {
                    (Ok(vectors), Ok(a)) => (vectors, a),
                    (Err(e), _) | (_, Err(e)) => {
                        eprintln!("{}: {e}", db::path().display());
                        return;
                    }
                }
            };
            eprintln!("radio: {} tracks analyzed, {} shaped, {} heard for a voice, {} artists linked", vectors.by_uri.len(), vectors.shape.len(), vectors.voice.len(), artists.len());
            out.send(Event::Vectors(Arc::new(vectors.clone())));
            let mut radio = Radio { db, out, commands, vectors, artists, library: None, linking: (0, 0), models: Arc::new(OnceLock::new()), started: Instant::now() };
            radio.run(jobs);
        })
        .expect("radio thread");
}

struct Todo {
    album: usize,
    track: usize,
    path: PathBuf,
    stamp: Stamp,
    bliss: bool,
    voice: bool,
}

struct Heard {
    bliss: Option<Result<(Vector, Shape), String>>,
    voice: Option<Result<Voice, String>>,
}

/// The voice models, loaded (and optimized by tract, half a second of
/// CPU) the first time a voice is to be heard; None when they failed.
fn models(models: &OnceLock<Option<Models>>) -> Option<&Models> {
    models
        .get_or_init(|| match Models::load() {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("radio: the voice models: {e}: no voice heard");
                None
            }
        })
        .as_ref()
}

fn hear(models: &OnceLock<Option<Models>>, t: &Todo) -> Heard {
    let pre = match <SymphoniaDecoder as Decoder>::decode(&t.path) {
        Ok(pre) => pre,
        Err(e) => {
            let e = e.to_string();
            return Heard { bliss: t.bliss.then(|| Err(e.clone())), voice: t.voice.then_some(Err(e)) };
        }
    };
    let voice = if t.voice { self::models(models).map(|m| m.hear(&pre.sample_array, 22050).map_err(|e| e.to_string())) } else { None };
    let bliss = t.bliss.then(|| {
        Song::analyze_series(&pre.sample_array).map_err(|e| e.to_string()).map(|(analysis, series)| {
            let mut v = [0f32; N];
            v.copy_from_slice(&analysis.as_vec());
            (v, shape::of(&series))
        })
    });
    Heard { bliss, voice }
}

struct Radio {
    db: db::Shared,
    out: Out,
    commands: tokio::sync::mpsc::UnboundedSender<crate::mpd::Command>,
    vectors: Vectors,
    artists: HashMap<String, Option<String>>,
    library: Option<Arc<Library>>,
    linking: (usize, usize),
    models: Arc<OnceLock<Option<Models>>>,
    started: Instant,
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
            // the first analysis waits for the start to settle, still
            // taking the jobs that come meanwhile
            let mut later = false;
            while let Some(left) = (self.started + SETTLE).checked_duration_since(Instant::now()) {
                match jobs.recv_timeout(left) {
                    Ok(Job::Library(l)) => {
                        pending = Some(l);
                        later = true;
                        break;
                    }
                    Ok(job) => self.other(job),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            if later {
                continue;
            }
            let total = todo.len();
            eprintln!("radio: {total} tracks to analyze");
            self.out.send(Event::Analyzing { done: 0, total });
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
        let hearing = self.models.get().is_none_or(|m| m.is_some());
        for (a, album) in library.albums.iter().enumerate() {
            for (t, track) in album.tracks.iter().enumerate() {
                present.insert(&track.uri);
                let path = music.join(&track.uri);
                let Some(stamp) = stamp_of(&path) else { continue };
                let done = stamps.get(&track.uri).filter(|d| d.stamp == stamp).copied().unwrap_or(Done { stamp, bliss: false, voice: false });
                let (bliss, voice) = (!done.bliss, !done.voice && hearing);
                if bliss || voice {
                    todo.push(Todo { album: a, track: t, path, stamp, bliss, voice });
                }
            }
        }
        let gone: Vec<String> = stamps.keys().filter(|u| !present.contains(u.as_str())).cloned().collect();
        if !gone.is_empty() {
            for u in &gone {
                self.vectors.by_uri.remove(u);
                self.vectors.voice.remove(u);
                self.vectors.shape.remove(u);
            }
            self.out.send(Event::Vectors(Arc::new(self.vectors.clone())));
        }
        todo.reverse();
        Some(todo)
    }

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
                Ok((v, s)) => {
                    self.vectors.by_uri.insert(track.uri.clone(), v);
                    self.vectors.shape.insert(track.uri.clone(), s);
                }
                Err(_) => {
                    self.vectors.by_uri.remove(&track.uri);
                    self.vectors.shape.remove(&track.uri);
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

    fn ask_links(&mut self, library: &Library) {
        let names: HashSet<String> = library.artists.iter().map(|a| a.name.to_lowercase()).collect();
        self.artists.retain(|n, _| names.contains(&n.to_lowercase()));
        let linked: HashSet<String> = self.artists.keys().map(|n| n.to_lowercase()).collect();
        let missing: Vec<(String, Vec<String>)> = library.artists.iter().filter(|a| !linked.contains(&a.name.to_lowercase())).map(|a| (a.name.clone(), a.mbids.clone())).collect();
        self.linking = (0, missing.len());
        self.out.send(Event::Linking { done: 0, total: missing.len() });
        if !missing.is_empty() {
            eprintln!("radio: {} artists to link", missing.len());
            let _ = self.commands.send(crate::mpd::Command::Links(missing));
        }
        self.send_relations();
    }

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

fn stamp_of(path: &std::path::Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(Stamp { mtime, size: meta.len() as i64 })
}

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
    let mut m = [0f32; F];
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

/// What a radio opens with before its arcs: nothing for a track or an
/// album, which are only the seed (the track is nearest to itself and
/// may well come first, or not), a handful of an artist's.
pub fn opening(library: &Library, seed: &Seed) -> Vec<String> {
    match seed {
        Seed::Track(_) | Seed::Album(_) => Vec::new(),
        Seed::Artist(name) => {
            let mut theirs: Vec<String> = own_tracks(library, name).into_iter().filter(|u| fits(library, u)).collect();
            fastrand::shuffle(&mut theirs);
            theirs.truncate(STEP);
            theirs
        }
    }
}

fn seed_vectors(library: &Library, vectors: &Vectors, seed: &Seed) -> Vec<Full> {
    let uris = match seed {
        Seed::Track(uri) => vec![uri.clone()],
        Seed::Album(key) => library.album_by_key(key).map(|a| a.uris()).unwrap_or_default(),
        Seed::Artist(name) => own_tracks(library, name),
    };
    uris.iter().filter_map(|u| vectors.full(u)).collect()
}

impl Vectors {
    fn spread(&self) -> Full {
        let mut s = [0f32; F];
        for i in 0..F {
            let xs: Vec<f32> = match i {
                ..VOICE => self.by_uri.values().map(|v| v[i]).collect(),
                VOICE..SHAPE => self.voice.values().map(|v| (2.0 * if i == VOICE { v.mean } else { v.share } - 1.0) * VOICE_WEIGHT).collect(),
                _ => self.shape.values().map(|v| v[i - SHAPE] * SHAPE_WEIGHT).collect(),
            };
            if xs.is_empty() {
                continue;
            }
            let mean = xs.iter().sum::<f32>() / xs.len() as f32;
            s[i] = (xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32).sqrt();
        }
        s
    }

    fn full(&self, uri: &str) -> Option<Full> {
        Some(full(self.by_uri.get(uri)?, self.voice.get(uri), self.shape.get(uri)))
    }

    fn vocality(&self, library: &Library, seed: &Seed) -> Option<f32> {
        let uris = match seed {
            Seed::Track(uri) => vec![uri.clone()],
            Seed::Album(key) => library.album_by_key(key).map(|a| a.uris()).unwrap_or_default(),
            Seed::Artist(name) => own_tracks(library, name),
        };
        uris.iter().filter_map(|u| self.voice.get(u)).map(|v| v.mean).reduce(f32::max)
    }
}

fn fits(library: &Library, uri: &str) -> bool {
    library.place(uri).and_then(|(a, t)| library.albums[a].tracks[t].duration).is_none_or(|d| d <= LONGEST)
}

fn own_tracks(library: &Library, name: &str) -> Vec<String> {
    library.artist(name).map(|i| library.artists[i].albums.iter().flat_map(|&a| library.albums[a].uris()).collect()).unwrap_or_default()
}

/// The shape's features in the full vector.
const RISE: usize = SHAPE + shape::RISE;
const ARCH: usize = SHAPE + shape::ARCH;
const SECTIONS: usize = SHAPE + shape::SECTIONS;
const REPETITION: usize = SHAPE + shape::REPETITION;
const HARMONY: usize = SHAPE + shape::HARMONY;
const CHANGES: usize = SHAPE + shape::CHANGES;
const PULSE: usize = SHAPE + shape::PULSE;
const ONSETS: usize = SHAPE + shape::ONSETS;

/// The five parts of the arc: each a shift of the seed along bliss's
/// features (by index), the voice's and the shape's, in standard
/// deviations of the library; the equilibrium is the seed itself.
const ARC: [(&str, &[(usize, f32)]); 5] = [
    ("equilibrium", &[]),
    ("break", &[(3, 0.75), (5, 0.75), (7, 0.5), (9, 0.75), (16, -0.5), (17, 0.75), (18, 0.5), (CHANGES, 0.75), (SECTIONS, 0.5), (HARMONY, 0.75), (PULSE, -0.5), (ARCH, -0.5)]),
    ("adventure", &[(0, 1.0), (1, 0.75), (2, 1.0), (4, 1.0), (8, 1.0), (9, 0.75), (17, 0.5), (VOICE, 0.75), (VOICE + 1, 0.75), (RISE, 0.75), (SECTIONS, 0.75), (ONSETS, 0.75), (HARMONY, 0.5), (REPETITION, -0.75)]),
    ("resolution", &[(8, 0.75), (9, 0.5), (16, 1.0), (17, -0.75), (18, -0.5), (ARCH, 0.75), (REPETITION, 0.5), (CHANGES, -0.5), (RISE, -0.5)]),
    ("new equilibrium", &[(0, -1.0), (1, -0.75), (2, -1.0), (4, -1.0), (6, -0.5), (8, -1.0), (9, -0.75), (16, 0.5), (VOICE, -0.75), (VOICE + 1, -0.75), (RISE, -0.75), (ONSETS, -0.75), (CHANGES, -0.5), (SECTIONS, -0.5), (REPETITION, 0.5), (PULSE, 0.5)]),
];

fn shift(at: usize) -> Full {
    let part = (at / PER_PART) % ARC.len();
    let t = (at % PER_PART) as f32 / PER_PART as f32;
    let mut s = [0f32; F];
    for &(i, d) in ARC[part].1 {
        s[i] += (1.0 - t) * d;
    }
    for &(i, d) in ARC[(part + 1) % ARC.len()].1 {
        s[i] += t * d;
    }
    s
}

fn shifted(centre: &Full, deltas: &Full, spread: &Full) -> Full {
    let mut target = *centre;
    for i in 0..F {
        target[i] = (target[i] + deltas[i] * spread[i]).clamp(-1.0, 1.0);
    }
    target
}

fn seed_artists<'a>(library: &'a Library, seed: &'a Seed) -> Vec<&'a str> {
    match seed {
        Seed::Track(uri) => library.place(uri).map(|(a, _)| vec![library.albums[a].artist.as_str()]).unwrap_or_default(),
        Seed::Album(key) => library.album_by_key(key).map(|a| vec![a.artist.as_str()]).unwrap_or_default(),
        Seed::Artist(name) => vec![name.as_str()],
    }
}

pub fn path(library: &Library, vectors: &Vectors, relations: &Relations, seed: &Seed, queue: &[String], n: usize) -> Vec<String> {
    let ceiling = vectors.vocality(library, seed);
    let quiet = |u: &str| fits(library, u) && ceiling.is_none_or(|c| vectors.voice.get(u).is_none_or(|v| v.mean <= c));
    let pool: Vec<(&str, Full, usize)> = match seed {
        Seed::Artist(name) => own_tracks(library, name)
            .into_iter()
            .filter_map(|u| {
                let (a, _) = library.place(&u)?;
                let (k, _) = vectors.by_uri.get_key_value(&u)?;
                Some((k.as_str(), vectors.full(k)?, a))
            })
            .filter(|(u, _, _)| quiet(u))
            .collect(),
        _ => vectors
            .by_uri
            .keys()
            .filter(|u| quiet(u))
            .filter_map(|u| {
                let (a, _) = library.place(u)?;
                Some((u.as_str(), vectors.full(u)?, a))
            })
            .collect(),
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
    if pool.iter().all(|(u, _, _)| excluded.contains(u)) {
        for keep in [10, 1, 0] {
            excluded = queue.iter().rev().take(keep).map(String::as_str).collect();
            if pool.iter().any(|(u, _, _)| !excluded.contains(u)) {
                break;
            }
        }
    }
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
        for (artist, base) in [("A", 0.0), ("B", 1.0), ("C", 5.0)] {
            for i in 1..=3 {
                vectors.by_uri.insert(format!("{artist}/{}/{i}.ogg", artist.to_lowercase()), at(base + 0.1 * i as f32));
            }
        }
        let relations = Relations::default();
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = opening(&lib, &seed);
        assert!(queue.is_empty(), "the seed is not forced in");
        let path = path(&lib, &vectors, &relations, &seed, &queue, 8);
        assert_eq!(path.len(), 8, "eight of the nine, once each");
        let mut all = path.clone();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 8);
        let first = (0..200).filter(|_| super::path(&lib, &vectors, &relations, &seed, &queue, 1)[0] == "A/a/1.ogg").count();
        assert!(first > 30 && first < 90, "the seed, nearest to itself, opens about three times in ten: {first} of 200");
        let far = (0..200).filter(|_| super::path(&lib, &vectors, &relations, &seed, &queue, 1)[0].starts_with("C/")).count();
        assert!(far < 40, "{far} of 200 went far");
        let mut whole: Vec<String> = queue.iter().cloned().chain(path).collect();
        let last = super::path(&lib, &vectors, &relations, &seed, &whole, 3);
        assert_eq!(last.len(), 1, "the ninth and last, then nothing new");
        whole.extend(last);
        assert_eq!(super::path(&lib, &vectors, &relations, &seed, &whole, 3).len(), 3, "all played, the radio goes round again");
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
        assert_eq!(more.len(), 15, "the fifteen of B's left");
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
        vectors.by_uri.insert("D/d/1.ogg".into(), at(0.0));
        let relations = Relations::default();
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = vec!["A/a/1.ogg".to_string()];
        let mut path = path(&lib, &vectors, &relations, &seed, &queue, 5);
        path.sort();
        assert_eq!(path, vec!["C/c/1.ogg".to_string(), "D/d/1.ogg".to_string()]);
        let seed = Seed::Track("B/b/1.ogg".into());
        assert_eq!(super::path(&lib, &vectors, &relations, &seed, &["B/b/1.ogg".to_string()], 5).len(), 3);
    }

    #[test]
    fn a_shift_is_in_the_library_s_units() {
        let mut vectors = Vectors::default();
        for i in 0..10 {
            let mut v = at(-1.0 + 0.2 * i as f32);
            v[1] = -1.0 + 0.02 * i as f32;
            vectors.by_uri.insert(format!("A/a/{i}.ogg"), v);
        }
        let spread = vectors.spread();
        assert!((spread[0] / spread[1] - 10.0).abs() < 0.01, "{} and {}", spread[0], spread[1]);
        assert_eq!(spread[VOICE], 0.0, "no voice heard, no spread");
        assert_eq!(spread[SHAPE], 0.0, "no shape, no spread");
        let centre = [0f32; F];
        let mut deltas = [0f32; F];
        for i in [0, 1, VOICE] {
            deltas[i] = 1.0;
        }
        let target = shifted(&centre, &deltas, &spread);
        assert!((target[0] - spread[0]).abs() < 1e-6);
        assert!((target[0] / target[1] - 10.0).abs() < 0.01);
        assert_eq!(target[VOICE], 0.0);
    }

    #[test]
    fn the_arc_wants_a_shape() {
        let (eq, brk, adv, res, new) = (0, PER_PART, 2 * PER_PART, 3 * PER_PART, 4 * PER_PART);
        assert!(shift(eq).iter().all(|x| *x == 0.0), "the equilibrium is the seed itself");
        assert!(shift(brk)[CHANGES] > 0.0 && shift(brk)[PULSE] < 0.0, "the break: the harmony moves, the pulse loosens");
        assert!(shift(adv)[RISE] > 0.0 && shift(adv)[ONSETS] > 0.0 && shift(adv)[REPETITION] < 0.0, "the adventure: a crescendo, dense, through-composed");
        assert!(shift(res)[ARCH] > 0.0 && shift(res)[RISE] < 0.0, "the resolution: an arch that comes down");
        assert!(shift(new)[RISE] < 0.0 && shift(new)[PULSE] > 0.0, "the new equilibrium: fading, steady");
        for at in 0..STEP {
            assert!(shift(at).iter().all(|x| x.abs() <= 1.0));
        }
    }

    #[test]
    fn a_shape_tells_tracks_apart() {
        let mut vectors = Vectors::default();
        for u in ["A/a/1.ogg", "B/b/1.ogg", "C/c/1.ogg"] {
            vectors.by_uri.insert(u.into(), at(0.0));
        }
        let mut rising = [0f32; shape::N];
        rising[shape::RISE] = 1.0;
        let mut falling = [0f32; shape::N];
        falling[shape::RISE] = -1.0;
        vectors.shape.insert("A/a/1.ogg".into(), rising);
        vectors.shape.insert("B/b/1.ogg".into(), rising);
        vectors.shape.insert("C/c/1.ogg".into(), falling);
        let (a, b, c) = (vectors.full("A/a/1.ogg").unwrap(), vectors.full("B/b/1.ogg").unwrap(), vectors.full("C/c/1.ogg").unwrap());
        assert_eq!(distance(&a, &b), 0.0, "B rises like A");
        assert_eq!(distance(&a, &c), 2.0 * SHAPE_WEIGHT, "C falls");
        let none = full(&at(0.0), None, None);
        assert_eq!(distance(&a, &none), SHAPE_WEIGHT, "a track not shaped yet is halfway from either");
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
        let seed = Seed::Artist("B".into());
        assert!(opening(&lib, &seed).is_empty());
    }

    #[test]
    fn a_part_passes_into_the_next_a_third_a_track() {
        assert_eq!(PER_PART, 3);
        let adventure = 2 * PER_PART;
        assert_eq!(shift(adventure)[0], 1.0, "the first track of the adventure is all adventure");
        assert!((shift(adventure - 2)[0] - 1.0 / 3.0).abs() < 1e-6, "a third through the break, a third of the way to the adventure");
        assert!((shift(adventure - 1)[0] - 2.0 / 3.0).abs() < 1e-6);
        assert!((shift(STEP - 1)[0] + 1.0 / 3.0).abs() < 1e-6, "the last track, nearly back at the equilibrium");
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
        let rows = vec![Relation { artist: "A".into(), other: "B".into(), source: "musicbrainz".into() }];
        let relations = Relations::build(&rows);
        assert_eq!(relations.weight("B", &["A"]), 1.0);
        assert_eq!(relations.weight("A", &["B"]), 1.0);
        assert_eq!(relations.weight("C", &["A"]), 0.0);
        let mut firsts = HashSet::new();
        for _ in 0..200 {
            firsts.insert(path(&lib, &vectors, &relations, &seed, &queue, 1).remove(0));
        }
        assert!(firsts.contains("B/b/1.ogg"));
    }
}
