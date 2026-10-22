//! The archive: the thread that keeps the database. It takes the
//! library as the app loads it and writes it down, albums, tracks,
//! the artists an album is credited to, the playlists; it finds the
//! tracks not yet analyzed and hears them with bliss and for a voice,
//! at the lowest priority; it asks the net for the artists' links
//! and keeps them; and it sends the app what the radio measures by,
//! the vectors, and how far along it is.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bliss_audio::decoder::symphonia::SymphoniaDecoder;
use bliss_audio::decoder::Decoder;
use bliss_audio::Song;

use crate::db::{self, Done, Link, Stamp};
use crate::library::{Library, Playlist};
use crate::mpd::{Event, Out};
use crate::radio::{Vector, Vectors, N};
use crate::shape::{self, Shape};
use crate::voice::{Models, Voice};

/// How long after the start the analysis waits: the window, the
/// library and the covers come first, on a quiet machine.
const SETTLE: Duration = Duration::from_secs(5);

pub enum Job {
    Library(Arc<Library>),
    Playlists(Vec<Playlist>),
    Origin { path: PathBuf, name: String, origin: String, url: Option<String> },
    Links { name: String, mbid: Option<String>, deezer: Option<i64>, links: Vec<Link>, error: Option<String> },
}

pub fn start(jobs: mpsc::Receiver<Job>, out: Out, commands: tokio::sync::mpsc::UnboundedSender<crate::mpd::Command>, db: db::Shared) {
    std::thread::Builder::new()
        .name("archive".into())
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
            eprintln!("archive: {} tracks analyzed, {} shaped, {} heard for a voice, {} artists linked", vectors.by_uri.len(), vectors.shape.len(), vectors.voice.len(), artists.len());
            out.send(Event::Vectors(Arc::new(vectors.clone())));
            let mut archive = Archive { db, out, commands, vectors, artists, library: None, linking: (0, 0), models: Arc::new(OnceLock::new()), started: Instant::now() };
            archive.run(jobs);
        })
        .expect("archive thread");
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
                eprintln!("archive: the voice models: {e}: no voice heard");
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

struct Archive {
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

impl Archive {
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
                    eprintln!("archive: {e}");
                }
                match db.linked() {
                    Ok(a) => self.artists = a,
                    Err(e) => eprintln!("archive: {e}"),
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
            eprintln!("archive: {total} tracks to analyze");
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
            eprintln!("archive: {done} analyzed");
        }
    }

    fn diff(&mut self, library: &Library) -> Option<Vec<Todo>> {
        let stamps = match self.db.lock().unwrap().stamps() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("archive: {e}");
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
                eprintln!("archive: {}: {e}", track.uri);
            }
            if let Err(e) = self.db.lock().unwrap().write_analysis(&track.uri, t.stamp, &result) {
                eprintln!("archive: {}: {e}", track.uri);
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
                eprintln!("archive: {}: voice: {e}", track.uri);
            }
            if let Err(e) = self.db.lock().unwrap().write_voice(&track.uri, t.stamp, &result) {
                eprintln!("archive: {}: {e}", track.uri);
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
            eprintln!("archive: {} artists to link", missing.len());
            let _ = self.commands.send(crate::mpd::Command::Links(missing));
        }
    }

    fn other(&mut self, job: Job) {
        match job {
            Job::Library(_) => {}
            Job::Playlists(playlists) => {
                if let Err(e) = self.db.lock().unwrap().write_playlists(&playlists) {
                    eprintln!("archive: {e}");
                }
            }
            Job::Origin { path, name, origin, url } => {
                if let Err(e) = self.db.lock().unwrap().set_origin(&path, &name, &origin, url.as_deref()) {
                    eprintln!("archive: {e}");
                }
            }
            Job::Links { name, mbid, deezer, links, error } => {
                if let Err(e) = self.db.lock().unwrap().write_links(&name, mbid.as_deref(), deezer, error.as_deref(), &links) {
                    eprintln!("archive: {name}: {e}");
                    return;
                }
                if error.is_none() {
                    self.artists.insert(name, mbid);
                }
                self.linking.0 += 1;
                self.out.send(Event::Linking { done: self.linking.0, total: self.linking.1.max(self.linking.0) });
            }
        }
    }
}

fn stamp_of(path: &std::path::Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(Stamp { mtime, size: meta.len() as i64 })
}

#[cfg(target_os = "linux")]
fn lowest_priority() {
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 19);
    }
}

/// macOS: the thread in the utility class, under the window's; not the
/// background class, which keeps a thread on the efficiency cores only.
#[cfg(target_os = "macos")]
fn lowest_priority() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}
