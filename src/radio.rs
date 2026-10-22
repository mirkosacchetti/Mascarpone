//! The radio: what a track is measured by, bliss's vector, the voice
//! and the shape in one, the library's spread, the distance; the arc
//! of five parts a radio follows from its seed, and the path through
//! the library that follows it. Functions on a library, its vectors
//! and a seed: no thread, no database (that is the archive's).

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

#[derive(Debug, Default, Clone)]
pub struct Vectors {
    pub by_uri: HashMap<String, Vector>,
    pub voice: HashMap<String, Voice>,
    pub shape: HashMap<String, Shape>,
}

/// The distance is in standard deviations of the library on every
/// feature, each capped at `CAP` (a feature the library hardly
/// spans would otherwise tell everything), and each group weighted:
/// bliss's 23 of the sound, the 2 of the voice, the 8 of the shape.
const SOUND_WEIGHT: f32 = 1.0;
const VOICE_WEIGHT: f32 = 0.5;
const SHAPE_WEIGHT: f32 = 0.75;
const CAP: f32 = 3.0;

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
        f[VOICE] = 2.0 * v.mean - 1.0;
        f[VOICE + 1] = 2.0 * v.share - 1.0;
    }
    if let Some(s) = shape {
        f[SHAPE..].copy_from_slice(s);
    }
    f
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seed {
    Track(String),
    Album(String),
    Artist(String),
}


/// What a difference on each feature is worth: its group's weight
/// over the library's deviation, nothing where nothing varies.
fn scale(spread: &Full) -> Full {
    let mut s = [0f32; F];
    for (i, x) in s.iter_mut().enumerate() {
        let weight = match i {
            ..VOICE => SOUND_WEIGHT,
            VOICE..SHAPE => VOICE_WEIGHT,
            _ => SHAPE_WEIGHT,
        };
        if spread[i] > 0.0 {
            *x = weight / spread[i];
        }
    }
    s
}

fn distance(a: &Full, b: &Full, scale: &Full) -> f32 {
    a.iter().zip(b).zip(scale).map(|((x, y), s)| (((x - y) * s).abs().min(CAP)).powi(2)).sum::<f32>().sqrt()
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
                VOICE..SHAPE => self.voice.values().map(|v| 2.0 * if i == VOICE { v.mean } else { v.share } - 1.0).collect(),
                _ => self.shape.values().map(|v| v[i - SHAPE]).collect(),
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
/// features (by index: 0 tempo, 1 zcr, 2-7 the mean and deviation of
/// centroid, rolloff and flatness, 8-9 of loudness, 16-19 the major,
/// minor, diminished and augmented triads), the voice's and the
/// shape's, in standard deviations of the library. The equilibrium is
/// the seed itself; the break a first dissonance, a dirtied timbre,
/// the loudness and the pulse broken, the repetition too; the
/// adventure the longest unrest, a key that keeps moving, chromatic
/// bridges, layers and registers, loudness in waves, the pulse put to
/// the test, the motif varied; the climax the densest and loudest,
/// the tension sustained (far from the key, still), the pulse at its
/// thickest, a motif hammered, and the victory an arch that resolves
/// in the major; the new equilibrium the home key again, the major,
/// lighter and cleaner, quieter, steady, the repetition of the start.
const ARC: [(&str, &[(usize, f32)]); 5] = [
    ("equilibrium", &[]),
    ("break", &[(18, 0.75), (19, 0.25), (17, 0.5), (16, -0.5), (HARMONY, 0.5), (CHANGES, 0.5), (6, 0.5), (1, 0.25), (9, 0.75), (ARCH, -0.5), (PULSE, -0.75), (REPETITION, -0.5), (SECTIONS, 0.5)]),
    ("adventure", &[(HARMONY, 0.75), (CHANGES, 0.75), (18, 0.5), (19, 0.5), (16, -0.25), (3, 0.75), (5, 0.5), (4, 0.5), (RISE, 0.75), (9, 0.5), (0, 0.5), (ONSETS, 0.5), (PULSE, -0.25), (REPETITION, -0.5), (SECTIONS, 0.75), (VOICE, 0.5), (VOICE + 1, 0.5)]),
    ("climax", &[(8, 1.0), (2, 0.75), (4, 0.75), (1, 0.5), (6, 0.25), (HARMONY, 0.75), (CHANGES, -0.25), (18, 0.5), (19, 0.25), (16, 0.25), (0, 0.75), (ONSETS, 1.0), (PULSE, 0.5), (REPETITION, 0.5), (ARCH, 0.75), (9, 0.5)]),
    ("new equilibrium", &[(HARMONY, -0.75), (16, 0.75), (17, -0.5), (18, -0.75), (19, -0.5), (6, -0.5), (3, -0.5), (4, -0.25), (8, -0.75), (RISE, -0.5), (9, -0.75), (PULSE, 0.5), (ONSETS, -0.5), (0, -0.5), (REPETITION, 0.75), (SECTIONS, -0.5), (CHANGES, -0.75), (VOICE, -0.5), (VOICE + 1, -0.5)]),
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

pub fn path(library: &Library, vectors: &Vectors, seed: &Seed, queue: &[String], n: usize) -> Vec<String> {
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
    let scale = scale(&spread);
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
            .map(|(i, (_, v, _))| (distance(&target, v, &scale), i))
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
        let (u, _, _) = pool[best[chosen].1];
        out.push(u.to_owned());
        excluded.insert(u);
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
        assert!(shift(res)[ARCH] > 0.0 && shift(res)[HARMONY] > 0.0 && shift(res)[CHANGES] < 0.0, "the climax: tension sustained, an arch that resolves");
        assert!(shift(brk)[HARMONY] > 0.0 && shift(brk)[CHANGES] > 0.0, "the break: a dissonance that moves on");
        assert!(shift(new)[RISE] < 0.0 && shift(new)[PULSE] > 0.0 && shift(new)[HARMONY] < 0.0, "the new equilibrium: fading, steady, home");
        for at in 0..STEP {
            assert!(shift(at).iter().all(|x| x.abs() <= 1.0));
        }
    }

    #[test]
    fn the_arc_is_well_formed() {
        for (name, shifts) in ARC {
            let mut seen = HashSet::new();
            for &(i, d) in shifts {
                assert!(i < F, "{name}: feature {i} is beyond the vector");
                assert!(seen.insert(i), "{name}: feature {i} shifted twice");
                assert!(d != 0.0 && d.abs() <= 1.0, "{name}: feature {i} shifted by {d}");
            }
        }
        assert_eq!(ARC.len() * PER_PART, STEP, "the parts fill the arc exactly");
    }

    /// `ARC_SEED=<uri> cargo test --release radio::tests::arc_probe -- --ignored --nocapture`
    /// builds an arc from the library in the database, from the seed
    /// (a track's uri; an album's key; an artist's name), and prints
    /// each track with its part, the target and its own values on a
    /// few features, to see the arc rise and fall.
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
        let seeds = seed_vectors(&library, &vectors, &seed);
        assert!(!seeds.is_empty(), "no seed: {seed:?}");
        let centre = mean(&seeds);
        let spread = vectors.spread();
        if std::env::var("ARC_SPREAD").is_ok() {
            // every feature: the library's mean, deviation and span, the seed's value, the largest shift of the arc in -1..1 units
            let names: Vec<String> = db::FEATURES.iter().map(|f| f.to_string()).chain(["voice".into(), "voiced".into()]).chain(shape::NAMES.iter().map(|f| f.to_string())).collect();
            let fulls: Vec<Full> = vectors.by_uri.keys().filter_map(|u| vectors.full(u)).collect();
            println!("{:24} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}", "feature", "mean", "sd", "min", "max", "seed", "shift");
            for i in 0..F {
                let xs: Vec<f32> = fulls.iter().map(|f| f[i]).collect();
                let mean = xs.iter().sum::<f32>() / xs.len() as f32;
                let (min, max) = xs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
                let largest = ARC.iter().flat_map(|(_, s)| s.iter()).filter(|(j, _)| *j == i).map(|(_, d)| d.abs()).fold(0.0, f32::max);
                println!("{:24} {mean:7.2} {:7.2} {min:7.2} {max:7.2} {:7.2} {:7.2}", names[i], spread[i], centre[i], largest * spread[i]);
            }
            return;
        }
        if let Ok(runs) = std::env::var("ARC_RUNS").map(|r| r.parse::<usize>().unwrap()) {
            // where the seed track itself lands, over many arcs
            let Seed::Track(uri) = &seed else { panic!("ARC_RUNS wants a track") };
            let mut at = [0; STEP + 1];
            // how far the chosen tracks miss their targets, rms in sd units, on the sound, the voice and the shape
            let (mut miss, mut n) = ([0f64; 3], 0usize);
            for _ in 0..runs {
                let p = path(&library, &vectors, &seed, &[], STEP);
                at[p.iter().position(|u| u == uri).unwrap_or(STEP)] += 1;
                for (i, u) in p.iter().enumerate() {
                    let (target, v) = (shifted(&centre, &shift(i), &spread), vectors.full(u).unwrap());
                    for (g, range) in [0..VOICE, VOICE..SHAPE, SHAPE..F].into_iter().enumerate() {
                        let k = range.len();
                        miss[g] += range.filter(|j| spread[*j] > 0.0).map(|j| ((target[j] - v[j]) / spread[j]).powi(2) as f64).sum::<f64>() / k as f64;
                    }
                    n += 1;
                }
            }
            println!("the seed at: {}", at.iter().enumerate().map(|(i, n)| format!("{i}:{n} ")).collect::<String>());
            println!("the miss, rms in sd: sound {:.2}  voice {:.2}  shape {:.2}", (miss[0] / n as f64).sqrt(), (miss[1] / n as f64).sqrt(), (miss[2] / n as f64).sqrt());
            return;
        }
        let path = path(&library, &vectors, &seed, &[], STEP);
        let columns: [(&str, usize); 8] = [("loud", 8), ("tempo", 0), ("major", 16), ("dim", 18), ("harm", HARMONY), ("pulse", PULSE), ("onset", ONSETS), ("rise", RISE)];
        println!("{:3} {:16} {:6} {}", "", "", "", columns.iter().map(|(n, _)| format!("{n:>6}")).collect::<String>());
        println!("{:3} {:16} {:6} {}", "", "seed", "", columns.iter().map(|(_, i)| format!("{:6.2}", centre[*i])).collect::<String>());
        for (at, uri) in path.iter().enumerate() {
            let part = ARC[(at / PER_PART) % ARC.len()].0;
            let target = shifted(&centre, &shift(at), &spread);
            let v = vectors.full(uri).unwrap();
            let (a, t) = library.place(uri).unwrap();
            let track = &library.albums[a].tracks[t];
            println!("{at:3} {:16} target {}", part, columns.iter().map(|(_, i)| format!("{:6.2}", target[*i])).collect::<String>());
            println!("{:3} {:16} {:6.2} {}  {} - {}", "", "", distance(&target, &v, &scale(&spread)), columns.iter().map(|(_, i)| format!("{:6.2}", v[*i])).collect::<String>(), track.artist, track.title);
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
        let scale = scale(&vectors.spread());
        assert_eq!(distance(&a, &b, &scale), 0.0, "B rises like A");
        let sd = vectors.spread()[RISE];
        assert!((distance(&a, &c, &scale) - 2.0 / sd * SHAPE_WEIGHT).abs() < 1e-5, "C falls: two rises apart, in the library's deviations, by the shape's weight");
        let none = full(&at(0.0), None, None);
        assert!((distance(&a, &none, &scale) - 1.0 / sd * SHAPE_WEIGHT).abs() < 1e-5, "a track not shaped yet is halfway from either");
        let mut far = [0f32; F];
        far[RISE] = 100.0;
        assert_eq!(distance(&a, &far, &scale), CAP, "no farther than the cap on one feature");
        assert_eq!(scale[0], 0.0, "a feature the library does not span counts nothing");
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
        let (brk, adv, new) = (of(1, 0), of(2, 0), of(4, 0));
        assert!(brk == 0.0 && adv > 0.0 && new < 0.0, "the tempo: untouched by the break, up in the adventure, down at the end");
        let adventure = 2 * PER_PART;
        assert_eq!(shift(adventure)[0], adv, "the first track of the adventure is all adventure");
        assert!((shift(adventure - 2)[0] - adv / 3.0).abs() < 1e-6, "a third through the break, a third of the way to the adventure");
        assert!((shift(adventure - 1)[0] - 2.0 * adv / 3.0).abs() < 1e-6);
        assert!((shift(STEP - 1)[0] - new / 3.0).abs() < 1e-6, "the last track, nearly back at the equilibrium");
        assert_eq!(shift(STEP)[0], 0.0, "the next block starts over");
    }

}
