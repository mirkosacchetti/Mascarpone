//! The radio: what a track is measured by, bliss's vector, the voice
//! and the shape in one; the eleven axes those fold into, each a
//! percentile of the library; the world of a seed, the five world's
//! axes the seed's own tracks keep to; the arc of five parts a radio
//! follows from its seed on the six levers, and the path through the
//! library that follows it, each track glued to the one before.
//! Functions on a library, its vectors and a seed: no thread, no
//! database (that is the archive's).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::library::Library;
use crate::shape::{self, Shape};
use crate::voice::Voice;

pub const N: usize = bliss_audio::NUMBER_FEATURES;
pub type Vector = [f32; N];

/// The tracks of an arc, and the arcs a radio starts with.
pub const STEP: usize = 15;
pub const ARCS: usize = 3;
const NEAREST: usize = 6;
const PER_PART: usize = STEP / ARC.len();
const LONGEST: Duration = Duration::from_secs(15 * 60);

/// The world of a seed: of the library, the tenth nearest to the
/// seed on the world's axes, never fewer than this many.
const WORLD_SHARE: usize = 10;
const WORLD_LEAST: usize = 120;

/// What each term of a track's cost is worth: its miss of the part's
/// target on the levers, its distance from the seed on the world's
/// axes, its distance from the track before on the glue's, and its
/// author heard among the last few.
const ARC_WEIGHT: f32 = 1.0;
const WORLD_WEIGHT: f32 = 0.5;
const GLUE_WEIGHT: f32 = 0.5;
const AUTHOR_COST: f32 = 0.3;
const RECENT: usize = 3;

#[derive(Debug, Default, Clone)]
pub struct Vectors {
    pub by_uri: HashMap<String, Vector>,
    pub voice: HashMap<String, Voice>,
    pub shape: HashMap<String, Shape>,
}

/// The whole of a track as measured: bliss's vector, the two of the
/// voice, the eight of the shape.
type Full = [f32; F];
const F: usize = N + 2 + shape::N;
/// Where the voice and the shape start in it.
const VOICE_AT: usize = N;
const SHAPE_AT: usize = N + 2;

fn full(vector: &Vector, voice: Option<&Voice>, shape: Option<&Shape>) -> Full {
    let mut f = [0f32; F];
    f[..N].copy_from_slice(vector);
    if let Some(v) = voice {
        f[VOICE_AT] = 2.0 * v.mean - 1.0;
        f[VOICE_AT + 1] = 2.0 * v.share - 1.0;
    }
    if let Some(s) = shape {
        f[SHAPE_AT..].copy_from_slice(s);
    }
    f
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seed {
    Track(String),
    Album(String),
    Artist(String),
}

/// The axes a track is on, the measures folded by what they measure
/// together (zcr, centroid, rolloff and flatness go as one, nine in
/// ten): the five of the world, which an album keeps to and the arc
/// leaves alone (the timbre and how it varies, the loudness, the
/// voice, how chordal the sound is), and the six levers the arc
/// pulls, which an album itself goes through (the tension, far from
/// a key and dissonant; the motion, through-composed over repeated;
/// the drive, fast and dense; the steady pulse; the rise; the arch).
pub const AXES: usize = 11;
pub type Axis = [f32; AXES];
pub const BRIGHT: usize = 0;
pub const TEXTURE: usize = 1;
pub const LOUD: usize = 2;
pub const VOICE: usize = 3;
pub const CHORDAL: usize = 4;
pub const TENSION: usize = 5;
pub const MOTION: usize = 6;
pub const DRIVE: usize = 7;
pub const STEADY: usize = 8;
pub const RISE: usize = 9;
pub const ARCH: usize = 10;
#[cfg(test)]
pub const NAMES: [&str; AXES] = ["bright", "texture", "loud", "voice", "chordal", "tension", "motion", "drive", "steady", "rise", "arch"];

/// The measures each axis is the mean of, each with its sign, by
/// index in the full vector: 0 tempo, 1 zcr, 2-7 the mean and
/// deviation of centroid, rolloff and flatness, 8-9 of loudness,
/// 10-15 the interval classes from the minor second to the tritone,
/// 16-19 the major, minor, diminished and augmented triads, 20-22
/// the weight of the dyads and of the triads, then the voice and the
/// shape.
const MEMBERS: [&[(usize, f32)]; AXES] = [
    &[(1, 1.0), (2, 1.0), (4, 1.0), (6, 1.0)],
    &[(3, 1.0), (5, 1.0), (7, 1.0)],
    &[(8, 1.0), (9, 1.0)],
    &[(VOICE_AT, 1.0), (VOICE_AT + 1, 1.0)],
    &[(20, 1.0), (21, 1.0), (22, 1.0)],
    &[(10, 1.0), (15, 1.0), (14, -1.0), (16, -1.0), (17, 1.0), (18, 1.0), (19, 1.0), (SHAPE_AT + shape::HARMONY, 1.0)],
    &[(SHAPE_AT + shape::CHANGES, 1.0), (SHAPE_AT + shape::SECTIONS, 1.0), (SHAPE_AT + shape::REPETITION, -1.0)],
    &[(0, 1.0), (SHAPE_AT + shape::ONSETS, 1.0)],
    &[(SHAPE_AT + shape::PULSE, 1.0)],
    &[(SHAPE_AT + shape::RISE, 1.0)],
    &[(SHAPE_AT + shape::ARCH, 1.0)],
];
const WORLD: [usize; 5] = [BRIGHT, TEXTURE, LOUD, VOICE, CHORDAL];
const LEVERS: [usize; 6] = [TENSION, MOTION, DRIVE, STEADY, RISE, ARCH];
/// What holds a track to the one before: the sound and the pace.
const GLUE: [usize; 3] = [BRIGHT, LOUD, DRIVE];

/// Every track of the library on the axes, each a percentile of the
/// library: 0 the lowest, 1 the highest, ties in the middle of their
/// run, an axis the library does not span at a half.
pub struct Axes {
    pub of: HashMap<String, Axis>,
}

impl Axes {
    pub fn of(vectors: &Vectors) -> Axes {
        let fulls: Vec<(&str, Full)> = vectors.by_uri.keys().filter_map(|u| Some((u.as_str(), vectors.full(u)?))).collect();
        let n = fulls.len();
        let mut mean = [0f32; F];
        let mut sd = [0f32; F];
        for (_, f) in &fulls {
            for i in 0..F {
                mean[i] += f[i];
            }
        }
        for x in &mut mean {
            *x /= n.max(1) as f32;
        }
        for (_, f) in &fulls {
            for i in 0..F {
                sd[i] += (f[i] - mean[i]).powi(2);
            }
        }
        for x in &mut sd {
            *x = (*x / n.max(1) as f32).sqrt();
        }
        // each axis the mean of its measures' deviations, signed
        let raw: Vec<Axis> = fulls
            .iter()
            .map(|(_, f)| {
                let mut a = [0f32; AXES];
                for (k, members) in MEMBERS.iter().enumerate() {
                    a[k] = members.iter().filter(|(i, _)| sd[*i] > 0.0).map(|(i, sign)| sign * (f[*i] - mean[*i]) / sd[*i]).sum::<f32>() / members.len() as f32;
                }
                a
            })
            .collect();
        // then its rank
        let mut axes: Vec<Axis> = vec![[0.5; AXES]; n];
        for k in 0..AXES {
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|&a, &b| raw[a][k].total_cmp(&raw[b][k]));
            let mut i = 0;
            while i < n {
                let mut j = i + 1;
                while j < n && raw[order[j]][k] == raw[order[i]][k] {
                    j += 1;
                }
                let p = (i + j) as f32 / 2.0 / n as f32;
                for &t in &order[i..j] {
                    axes[t][k] = p;
                }
                i = j;
            }
        }
        Axes { of: fulls.iter().zip(axes).map(|((u, _), a)| (u.to_string(), a)).collect() }
    }
}

/// How far apart two tracks are on some axes: the root mean square
/// of their differences, in percentiles, 0 to 1.
fn rms(a: &Axis, b: &Axis, over: &[usize]) -> f32 {
    (over.iter().map(|&i| (a[i] - b[i]).powi(2)).sum::<f32>() / over.len() as f32).sqrt()
}

fn mean(xs: &[Axis]) -> Axis {
    let mut m = [0f32; AXES];
    for x in xs {
        for (i, v) in x.iter().enumerate() {
            m[i] += v;
        }
    }
    for v in &mut m {
        *v /= xs.len() as f32;
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

fn seed_uris(library: &Library, seed: &Seed) -> Vec<String> {
    match seed {
        Seed::Track(uri) => vec![uri.clone()],
        Seed::Album(key) => library.album_by_key(key).map(|a| a.uris()).unwrap_or_default(),
        Seed::Artist(name) => own_tracks(library, name),
    }
}

impl Vectors {
    fn full(&self, uri: &str) -> Option<Full> {
        Some(full(self.by_uri.get(uri)?, self.voice.get(uri), self.shape.get(uri)))
    }

    fn vocality(&self, library: &Library, seed: &Seed) -> Option<f32> {
        seed_uris(library, seed).iter().filter_map(|u| self.voice.get(u)).map(|v| v.mean).reduce(f32::max)
    }
}

fn fits(library: &Library, uri: &str) -> bool {
    library.place(uri).and_then(|(a, t)| library.albums[a].tracks[t].duration).is_none_or(|d| d <= LONGEST)
}

fn own_tracks(library: &Library, name: &str) -> Vec<String> {
    library.artist(name).map(|i| library.artists[i].albums.iter().flat_map(|&a| library.albums[a].uris()).collect()).unwrap_or_default()
}

/// The five parts of the arc: each a shift of the seed along the
/// levers, in percentiles of the library. The equilibrium is the seed
/// itself; the break a first dissonance and the harmony moving, the
/// pulse broken, the repetition too, a dip; the adventure the longest
/// unrest, far off the key and moving, through-composed, a crescendo,
/// faster and denser, the pulse put to the test; the climax the
/// tension sustained, the fastest and thickest pulse, a motif
/// hammered, an arch that resolves; the new equilibrium home: on a
/// key, the repetition of the start, slower and sparser, steady, a
/// fade.
const ARC: [(&str, &[(usize, f32)]); 5] = [
    ("equilibrium", &[]),
    ("break", &[(TENSION, 0.2), (MOTION, 0.2), (STEADY, -0.25), (ARCH, -0.15)]),
    ("adventure", &[(TENSION, 0.25), (MOTION, 0.3), (DRIVE, 0.15), (STEADY, -0.1), (RISE, 0.25)]),
    ("climax", &[(TENSION, 0.25), (MOTION, -0.1), (DRIVE, 0.3), (STEADY, 0.15), (ARCH, 0.25)]),
    ("new equilibrium", &[(TENSION, -0.25), (MOTION, -0.25), (DRIVE, -0.15), (STEADY, 0.15), (RISE, -0.15)]),
];

fn shift(at: usize) -> Axis {
    let part = (at / PER_PART) % ARC.len();
    let t = (at % PER_PART) as f32 / PER_PART as f32;
    let mut s = [0f32; AXES];
    for &(i, d) in ARC[part].1 {
        s[i] += (1.0 - t) * d;
    }
    for &(i, d) in ARC[(part + 1) % ARC.len()].1 {
        s[i] += t * d;
    }
    s
}

fn shifted(centre: &Axis, deltas: &Axis) -> Axis {
    let mut target = *centre;
    for i in 0..AXES {
        target[i] = (target[i] + deltas[i]).clamp(0.0, 1.0);
    }
    target
}

struct Candidate<'a> {
    uri: &'a str,
    axis: Axis,
    album: usize,
}

/// The world of the seed: the nearest tenth of the pool on the
/// world's axes, the whole of a small pool.
fn world<'a>(mut pool: Vec<Candidate<'a>>, centre: &Axis) -> Vec<Candidate<'a>> {
    let keep = (pool.len() / WORLD_SHARE).max(WORLD_LEAST);
    if pool.len() > keep {
        let mut far: Vec<(f32, Candidate)> = pool.drain(..).map(|c| (rms(&c.axis, centre, &WORLD), c)).collect();
        far.sort_by(|a, b| a.0.total_cmp(&b.0));
        far.truncate(keep);
        pool = far.into_iter().map(|(_, c)| c).collect();
    }
    pool
}

/// What a track costs at a point of the arc, each term 0 to 1 before
/// its weight: its miss of the target on the levers, its distance
/// from the seed on the world's axes, from the track before on the
/// glue's, and its author heard lately.
fn cost(axis: &Axis, target: &Axis, centre: &Axis, previous: Option<&Axis>, repeated: bool) -> f32 {
    ARC_WEIGHT * rms(axis, target, &LEVERS) + WORLD_WEIGHT * rms(axis, centre, &WORLD) + previous.map_or(0.0, |p| GLUE_WEIGHT * rms(axis, p, &GLUE)) + if repeated { AUTHOR_COST } else { 0.0 }
}

pub fn path(library: &Library, vectors: &Vectors, seed: &Seed, queue: &[String], n: usize) -> Vec<String> {
    let ceiling = vectors.vocality(library, seed);
    let quiet = |u: &str| fits(library, u) && ceiling.is_none_or(|c| vectors.voice.get(u).is_none_or(|v| v.mean <= c));
    let axes = Axes::of(vectors);
    let uris: Vec<&str> = match seed {
        Seed::Artist(name) => own_tracks(library, name).iter().filter_map(|u| axes.of.get_key_value(u)).map(|(k, _)| k.as_str()).collect(),
        _ => axes.of.keys().map(String::as_str).collect(),
    };
    let pool: Vec<Candidate> = uris.into_iter().filter(|u| quiet(u)).filter_map(|u| Some(Candidate { uri: u, axis: *axes.of.get(u)?, album: library.place(u)?.0 })).collect();
    let seeds: Vec<Axis> = seed_uris(library, seed).iter().filter_map(|u| axes.of.get(u)).copied().collect();
    if pool.is_empty() || seeds.is_empty() {
        return Vec::new();
    }
    let centre = mean(&seeds);
    let pool = match seed {
        Seed::Artist(_) => pool,
        _ => world(pool, &centre),
    };
    let mut excluded: HashSet<&str> = queue.iter().map(String::as_str).collect();
    if pool.iter().all(|c| excluded.contains(c.uri)) {
        for keep in [10, 1, 0] {
            excluded = queue.iter().rev().take(keep).map(String::as_str).collect();
            if pool.iter().any(|c| !excluded.contains(c.uri)) {
                break;
            }
        }
    }
    let author = |uri: &str| library.place(uri).map(|(a, _)| library.albums[a].artist.clone());
    let mut out: Vec<String> = Vec::with_capacity(n);
    for at in 0..n {
        let target = shifted(&centre, &shift(at));
        let previous: Option<Axis> = out.last().or(queue.last()).and_then(|u| axes.of.get(u)).copied();
        let recent: Vec<String> = queue.iter().chain(out.iter()).rev().take(RECENT).filter_map(|u| author(u)).collect();
        let mut best: Vec<(f32, usize)> = pool
            .iter()
            .enumerate()
            .filter(|(_, c)| !excluded.contains(c.uri))
            .map(|(i, c)| {
                let repeated = recent.iter().any(|r| *r == library.albums[c.album].artist);
                (cost(&c.axis, &target, &centre, previous.as_ref(), repeated), i)
            })
            .collect();
        if best.is_empty() {
            break;
        }
        best.sort_by(|x, y| x.0.total_cmp(&y.0));
        best.truncate(NEAREST);
        // the squares: the nearest two in three, the sixth once in ninety
        let weights: Vec<usize> = (0..best.len()).map(|r| (NEAREST - r).pow(2)).collect();
        let mut pick = fastrand::usize(..weights.iter().sum::<usize>());
        let mut chosen = 0;
        for (r, w) in weights.iter().enumerate() {
            if pick < *w {
                chosen = r;
                break;
            }
            pick -= w;
        }
        let uri = pool[best[chosen].1].uri;
        out.push(uri.to_owned());
        excluded.insert(uri);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

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
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = opening(&lib, &seed);
        assert!(queue.is_empty(), "the seed is not forced in");
        let path = path(&lib, &vectors, &seed, &queue, 8);
        assert_eq!(path.len(), 8, "eight of the nine, once each");
        let mut all = path.clone();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 8);
        let first = (0..200).filter(|_| super::path(&lib, &vectors, &seed, &queue, 1)[0] == "A/a/1.ogg").count();
        assert!(first > 50 && first < 110, "the seed, nearest to itself, opens about four times in ten: {first} of 200");
        let far = (0..200).filter(|_| super::path(&lib, &vectors, &seed, &queue, 1)[0].starts_with("C/")).count();
        assert!(far < 40, "{far} of 200 went far");
        let mut whole: Vec<String> = queue.iter().cloned().chain(path).collect();
        let last = super::path(&lib, &vectors, &seed, &whole, 3);
        assert_eq!(last.len(), 1, "the ninth and last, then nothing new");
        whole.extend(last);
        assert_eq!(super::path(&lib, &vectors, &seed, &whole, 3).len(), 3, "all played, the radio goes round again");
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
        let album = Seed::Album("A\u{1f}a".into());
        assert!(opening(&lib, &album).is_empty());
        let path = path(&lib, &vectors, &album, &[], 5);
        assert_eq!(path.len(), 5);
        assert!(path.iter().all(|u| u.starts_with("A/") || u.starts_with("B/")));
        let artist = Seed::Artist("B".into());
        let open = opening(&lib, &artist);
        assert_eq!(open.len(), STEP);
        assert!(open.iter().all(|u| u.starts_with("B/")));
        let more = super::path(&lib, &vectors, &artist, &open, STEP);
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
        let seed = Seed::Track("A/a/1.ogg".into());
        let queue = vec!["A/a/1.ogg".to_string()];
        let mut path = path(&lib, &vectors, &seed, &queue, 5);
        path.sort();
        assert_eq!(path, vec!["C/c/1.ogg".to_string(), "D/d/1.ogg".to_string()]);
        let seed = Seed::Track("B/b/1.ogg".into());
        assert_eq!(super::path(&lib, &vectors, &seed, &["B/b/1.ogg".to_string()], 5).len(), 3);
    }

    #[test]
    fn an_axis_is_a_percentile_of_the_library() {
        let mut vectors = Vectors::default();
        for i in 0..10 {
            let mut v = at(-1.0 + 0.2 * i as f32);
            v[1] = -1.0 + 0.02 * i as f32;
            vectors.by_uri.insert(format!("A/a/{i}.ogg"), v);
        }
        let axes = Axes::of(&vectors);
        for i in 0..10 {
            let a = axes.of[&format!("A/a/{i}.ogg")];
            assert!((a[DRIVE] - (i as f32 + 0.5) / 10.0).abs() < 1e-6, "the tempo in rank, however it is scaled: {}", a[DRIVE]);
            assert!((a[BRIGHT] - (i as f32 + 0.5) / 10.0).abs() < 1e-6, "the zcr too, ten times narrower: {}", a[BRIGHT]);
            assert_eq!(a[VOICE], 0.5, "no voice heard, no spread: halfway");
            assert_eq!(a[RISE], 0.5, "no shape, no spread: halfway");
        }
        let centre = axes.of["A/a/9.ogg"];
        let mut deltas = [0f32; AXES];
        deltas[DRIVE] = 0.3;
        deltas[RISE] = -0.3;
        let target = shifted(&centre, &deltas);
        assert_eq!(target[DRIVE], 1.0, "no higher than the library goes");
        assert!((target[RISE] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn the_arc_wants_a_shape() {
        let (eq, brk, adv, res, new) = (0, PER_PART, 2 * PER_PART, 3 * PER_PART, 4 * PER_PART);
        assert!(shift(eq).iter().all(|x| *x == 0.0), "the equilibrium is the seed itself");
        assert!(shift(brk)[TENSION] > 0.0 && shift(brk)[MOTION] > 0.0 && shift(brk)[STEADY] < 0.0, "the break: a dissonance that moves on, the pulse loosens");
        assert!(shift(adv)[RISE] > 0.0 && shift(adv)[DRIVE] > 0.0 && shift(adv)[MOTION] > 0.0, "the adventure: a crescendo, dense, through-composed");
        assert!(shift(res)[ARCH] > 0.0 && shift(res)[TENSION] > 0.0 && shift(res)[DRIVE] > 0.0 && shift(res)[STEADY] > 0.0, "the climax: tension sustained, the thickest pulse, an arch that resolves");
        assert!(shift(new)[RISE] < 0.0 && shift(new)[STEADY] > 0.0 && shift(new)[TENSION] < 0.0 && shift(new)[MOTION] < 0.0, "the new equilibrium: fading, steady, home, the repetition of the start");
        for at in 0..STEP {
            assert!(shift(at).iter().all(|x| x.abs() <= 0.5));
            assert!(WORLD.iter().all(|&w| shift(at)[w] == 0.0), "the world's axes are not the arc's");
        }
    }

    #[test]
    fn the_arc_is_well_formed() {
        for (name, shifts) in ARC {
            let mut seen = HashSet::new();
            for &(i, d) in shifts {
                assert!(LEVERS.contains(&i), "{name}: axis {i} is not a lever");
                assert!(seen.insert(i), "{name}: axis {i} shifted twice");
                assert!(d != 0.0 && d.abs() <= 1.0, "{name}: axis {i} shifted by {d}");
            }
        }
        assert_eq!(ARC.len() * PER_PART, STEP, "the parts fill the arc exactly");
        let mut all: Vec<usize> = WORLD.iter().chain(&LEVERS).copied().collect();
        all.sort();
        assert_eq!(all, (0..AXES).collect::<Vec<_>>(), "every axis is the world's or a lever, once");
        assert!(GLUE.iter().all(|g| all.contains(g)));
        for (k, members) in MEMBERS.iter().enumerate() {
            assert!(!members.is_empty(), "{}: no measure", NAMES[k]);
            assert!(members.iter().all(|(i, s)| *i < F && s.abs() == 1.0), "{}: a measure beyond the vector or an odd sign", NAMES[k]);
        }
    }

    #[test]
    fn a_shape_tells_tracks_apart() {
        let mut vectors = Vectors::default();
        for u in ["A/a/1.ogg", "B/b/1.ogg", "C/c/1.ogg", "D/d/1.ogg"] {
            vectors.by_uri.insert(u.into(), at(0.0));
        }
        let mut rising = [0f32; shape::N];
        rising[shape::RISE] = 1.0;
        let mut falling = [0f32; shape::N];
        falling[shape::RISE] = -1.0;
        vectors.shape.insert("A/a/1.ogg".into(), rising);
        vectors.shape.insert("B/b/1.ogg".into(), rising);
        vectors.shape.insert("C/c/1.ogg".into(), falling);
        let axes = Axes::of(&vectors);
        let (a, b, c, d) = (axes.of["A/a/1.ogg"], axes.of["B/b/1.ogg"], axes.of["C/c/1.ogg"], axes.of["D/d/1.ogg"]);
        assert_eq!(rms(&a, &b, &LEVERS), 0.0, "B rises like A");
        assert_eq!(a[RISE], 0.75, "A and B share the top two ranks of four");
        assert_eq!(c[RISE], 0.125, "C falls, the lowest");
        assert_eq!(d[RISE], 0.375, "a track not shaped yet is between");
        assert!(rms(&a, &c, &LEVERS) > rms(&a, &d, &LEVERS) && rms(&a, &d, &LEVERS) > 0.0);
        assert_eq!(rms(&a, &c, &WORLD), 0.0, "the rise is no part of the world");
    }

    #[test]
    fn the_world_is_the_nearest_tenth() {
        let mut pool: Vec<Candidate> = Vec::new();
        let uris: Vec<String> = (0..2000).map(|i| format!("{i}.ogg")).collect();
        for (i, u) in uris.iter().enumerate() {
            let mut axis = [0.5f32; AXES];
            // the loudness spread over the library, the levers alike everywhere
            axis[LOUD] = i as f32 / 2000.0;
            pool.push(Candidate { uri: u, axis, album: 0 });
        }
        let mut centre = [0.5f32; AXES];
        centre[LOUD] = 0.3;
        let kept = world(pool, &centre);
        assert_eq!(kept.len(), 200, "a tenth of two thousand");
        assert!(kept.iter().all(|c| (c.axis[LOUD] - 0.3).abs() <= 0.051), "the two hundred nearest in loudness");
        let few: Vec<Candidate> = uris.iter().take(50).map(|u| Candidate { uri: u, axis: [0.5; AXES], album: 0 }).collect();
        assert_eq!(world(few, &centre).len(), 50, "a small pool is kept whole");
    }

    #[test]
    fn the_glue_and_the_author_cost() {
        let mut centre = [0.5f32; AXES];
        centre[BRIGHT] = 0.5;
        let target = centre;
        let (mut near, mut far, mut previous) = (centre, centre, centre);
        near[BRIGHT] = 0.7;
        far[BRIGHT] = 0.3;
        previous[BRIGHT] = 0.9;
        assert_eq!(cost(&near, &target, &centre, None, false), cost(&far, &target, &centre, None, false), "as far from the seed either way");
        assert!(cost(&near, &target, &centre, Some(&previous), false) < cost(&far, &target, &centre, Some(&previous), false), "glued to the one before");
        assert!((cost(&near, &target, &centre, None, true) - cost(&near, &target, &centre, None, false) - AUTHOR_COST).abs() < 1e-6, "an author heard lately costs");
        assert_eq!(cost(&centre, &target, &centre, Some(&centre), false), 0.0, "the seed at the equilibrium, after itself");
    }

    #[test]
    fn an_author_waits_their_turn() {
        // five each of four authors, all alike: the authors take turns rather than run
        let lib = library(&[("A", "a", 5), ("B", "b", 5), ("C", "c", 5), ("D", "d", 5)]);
        let mut vectors = Vectors::default();
        for artist in ["A", "B", "C", "D"] {
            for i in 1..=5 {
                vectors.by_uri.insert(format!("{artist}/{}/{i}.ogg", artist.to_lowercase()), at(0.0));
            }
        }
        let seed = Seed::Track("A/a/1.ogg".into());
        let mut runs = 0;
        for _ in 0..50 {
            let path = path(&lib, &vectors, &seed, &[], 12);
            runs += path.windows(2).filter(|w| w[0].split('/').next() == w[1].split('/').next()).count();
        }
        assert!(runs < 50, "the same author twice in a row {runs} times in 550 pairs, 275 by chance");
    }

    /// `ARC_SEED=<uri> cargo test --release radio::tests::arc_probe -- --ignored --nocapture`
    /// builds an arc from the library in the database, from the seed
    /// (a track's uri; an album's key; an artist's name), and prints
    /// each track with its part, the target and its own place on the
    /// axes, to see the arc rise and fall.
    #[test]
    #[ignore]
    fn arc_probe() {
        use crate::library::{Album, Track};
        let db = db::Db::open(&db::path()).unwrap();
        let vectors = db.vectors().unwrap();
        let conn = rusqlite::Connection::open(db::path()).unwrap();
        let mut stmt = conn.prepare("select t.uri, t.title, al.title, al.credit, t.duration, al.key from track t join album al on al.id = t.album_id order by al.credit, al.title, t.disc, t.number").unwrap();
        let mut albums: Vec<Album> = Vec::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<f64>>(4)?, r.get::<_, String>(5)?))).unwrap() {
            let (uri, title, album, artist, duration, key) = row.unwrap();
            if albums.last().is_none_or(|a| a.key != key) {
                albums.push(Album { key, title: album.clone(), artist: artist.clone(), year: None, tracks: Vec::new(), loose: false, mbid: None, members: Album::whole(&artist, Vec::new()) });
            }
            let a = albums.last_mut().unwrap();
            let number = a.tracks.len() as u64 + 1;
            a.tracks.push(Track { uri, title: title.unwrap_or_default(), artist, disc: 1, number, duration: duration.map(Duration::from_secs_f64), mbid: None });
        }
        let library = Library::assemble(albums);
        let seed = std::env::var("ARC_SEED").unwrap_or_default();
        let seed = if library.place(&seed).is_some() {
            Seed::Track(seed)
        } else if library.album_by_key(&seed).is_some() {
            Seed::Album(seed)
        } else {
            Seed::Artist(seed)
        };
        let axes = Axes::of(&vectors);
        let seeds: Vec<Axis> = seed_uris(&library, &seed).iter().filter_map(|u| axes.of.get(u)).copied().collect();
        assert!(!seeds.is_empty(), "no seed: {seed:?}");
        let centre = mean(&seeds);
        if let Ok(runs) = std::env::var("ARC_RUNS").map(|r| r.parse::<usize>().unwrap()) {
            // where the seed track itself lands, over many arcs, and the terms of the cost, rms over the picks
            let Seed::Track(uri) = &seed else { panic!("ARC_RUNS wants a track") };
            let mut at = [0; STEP + 1];
            let (mut arc, mut world, mut glue, mut n) = (0f64, 0f64, 0f64, 0usize);
            for _ in 0..runs {
                let p = path(&library, &vectors, &seed, &[], STEP);
                at[p.iter().position(|u| u == uri).unwrap_or(STEP)] += 1;
                for (i, u) in p.iter().enumerate() {
                    let a = axes.of[u];
                    arc += rms(&a, &shifted(&centre, &shift(i)), &LEVERS).powi(2) as f64;
                    world += rms(&a, &centre, &WORLD).powi(2) as f64;
                    if i > 0 {
                        glue += rms(&a, &axes.of[&p[i - 1]], &GLUE).powi(2) as f64;
                    }
                    n += 1;
                }
            }
            println!("the seed at: {}", at.iter().enumerate().map(|(i, n)| format!("{i}:{n} ")).collect::<String>());
            println!("the miss, rms in percentiles: the arc {:.2}  the world {:.2}  the glue {:.2}", (arc / n as f64).sqrt(), (world / n as f64).sqrt(), (glue / (n - runs) as f64).sqrt());
            return;
        }
        let path = path(&library, &vectors, &seed, &[], STEP);
        println!("{:3} {:16} {:5} {}", "", "", "", NAMES.iter().map(|n| format!("{n:>8}")).collect::<String>());
        println!("{:3} {:16} {:5} {}", "", "seed", "", centre.iter().map(|x| format!("{x:8.2}")).collect::<String>());
        let mut previous: Option<Axis> = None;
        for (at, uri) in path.iter().enumerate() {
            let part = ARC[(at / PER_PART) % ARC.len()].0;
            let target = shifted(&centre, &shift(at));
            let a = axes.of[uri];
            let (al, t) = library.place(uri).unwrap();
            let track = &library.albums[al].tracks[t];
            println!("{at:3} {:16} {:5} {}", part, "", LEVERS.iter().fold(vec![format!("{:8}", ""); AXES], |mut row, &i| { row[i] = format!("{:8.2}", target[i]); row }).concat());
            println!("{:3} {:16} {:5.2} {}  {} - {}", "", "", cost(&a, &target, &centre, previous.as_ref(), false), a.iter().map(|x| format!("{x:8.2}")).collect::<String>(), track.artist, track.title);
            previous = Some(a);
        }
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
        let seed = Seed::Track("A/a/1.ogg".into());
        let path = path(&lib, &vectors, &seed, &["A/a/1.ogg".to_string()], 5);
        assert_eq!(path, vec!["C/c/1.ogg".to_string()], "B is too long, C just fits");
        let seed = Seed::Artist("B".into());
        assert!(opening(&lib, &seed).is_empty());
    }

    #[test]
    fn a_part_passes_into_the_next_a_third_a_track() {
        assert_eq!(PER_PART, 3);
        let of = |part: usize, i: usize| ARC[part].1.iter().filter(|(j, _)| *j == i).map(|(_, d)| d).sum::<f32>();
        let (brk, adv, new) = (of(1, DRIVE), of(2, DRIVE), of(4, DRIVE));
        assert!(brk == 0.0 && adv > 0.0 && new < 0.0, "the drive: untouched by the break, up in the adventure, down at the end");
        let adventure = 2 * PER_PART;
        assert_eq!(shift(adventure)[DRIVE], adv, "the first track of the adventure is all adventure");
        assert!((shift(adventure - 2)[DRIVE] - adv / 3.0).abs() < 1e-6, "a third through the break, a third of the way to the adventure");
        assert!((shift(adventure - 1)[DRIVE] - 2.0 * adv / 3.0).abs() < 1e-6);
        assert!((shift(STEP - 1)[DRIVE] - new / 3.0).abs() < 1e-6, "the last track, nearly back at the equilibrium");
        assert_eq!(shift(STEP)[DRIVE], 0.0, "the next block starts over");
    }
}
