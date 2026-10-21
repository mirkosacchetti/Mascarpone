//! The shape of a track: how it goes along, where bliss's vector says
//! how it sounds on the whole. Eight numbers in -1..1, from what
//! bliss's descriptors see window by window and sum up (the
//! `Series` the vendored bliss keeps), each between fixed bounds like
//! bliss's own, so that a track's shape is the same in any library.

use bliss_audio::Series;

pub const N: usize = 8;
pub type Shape = [f32; N];
/// The columns of the shape in the database, in the shape's order.
pub const NAMES: [&str; N] = ["rise", "arch", "sections", "repetition", "harmony", "changes", "pulse", "onsets"];
/// Bumped when the shape changes meaning: every track is shaped again.
pub const VERSION: i64 = 1;

pub const RISE: usize = 0;
pub const ARCH: usize = 1;
pub const SECTIONS: usize = 2;
pub const REPETITION: usize = 3;
pub const HARMONY: usize = 4;
pub const CHANGES: usize = 5;
pub const PULSE: usize = 6;
pub const ONSETS: usize = 7;

/// bliss's sample rate, and the hops of its series, in samples.
const RATE: usize = 22050;
const LEVEL_HOP: usize = 1024;
const CENTROID_HOP: usize = 128;
const CHROMA_HOP: usize = 2205;
const CHROMA: usize = 12;

/// The bounds of each raw measure: a value at the low end is -1, at
/// the high end 1, beyond either is clamped. The rise and the arch
/// are in dB, the sections in doublings (log2), the rest shares.
const BOUNDS: [(f32, f32); N] = [(-10.0, 10.0), (-10.0, 10.0), (0.0, 5.0), (0.0, 1.0), (0.0, 0.6), (0.0, 0.3), (0.0, 1.0), (1.0, 7.0)];
/// Quieter than this is silence, for the rise and the arch: left
/// out at either end, no quieter than this between.
const FLOOR: f32 = -50.0;
/// How far a beat may stray from the median interval to count as on
/// the pulse.
const PULSE_SLACK: f32 = 0.12;

/// The structure is read on the track's seconds. For the sections
/// each is smoothed with its neighbours over `SMOOTH` seconds so that
/// a section is a colour and not a run of chords; a boundary is a
/// peak of novelty over `KERNEL` seconds on either side, above
/// `NOVELTY`. For the repetition the seconds are as they are: one is
/// repeated when a run of `RUN` seconds from it comes back, at least
/// `APART` seconds away, within `TWIN`, once the track has gone
/// farther than `LEAVE` from it. Structure is read on the first
/// `LONGEST` seconds at most.
const SMOOTH: usize = 8;
const KERNEL: usize = 12;
const NOVELTY: f32 = 0.4;
/// Less than this between the two runs is no boundary, however
/// still each run is: a loop's breathing is not sections.
const CONTRAST: f32 = 0.15;
const RUN: usize = 4;
const APART: usize = 8;
const TWIN: f32 = 0.25;
const LEAVE: f32 = 0.5;
const LONGEST: usize = 2000;

fn norm(v: f32, lo: f32, hi: f32) -> f32 {
    (2.0 * (v - lo) / (hi - lo) - 1.0).clamp(-1.0, 1.0)
}

fn db(level: f32) -> f32 {
    10.0 * level.max(1e-9).log10()
}

fn mean(xs: &[f32]) -> f32 {
    match xs.is_empty() {
        true => 0.0,
        false => xs.iter().sum::<f32>() / xs.len() as f32,
    }
}

/// A second of the track, as the structure sees it.
#[derive(Clone, Copy, Default)]
struct Second {
    /// The chroma, a unit vector; all zero for silence.
    chroma: [f32; CHROMA],
    /// The centroid in 0..1 and the loudness in 0..1, and in dB.
    centroid: f32,
    loudness: f32,
    db: f32,
}

impl Second {
    fn features(&self) -> [f32; CHROMA + 2] {
        let mut f = [0f32; CHROMA + 2];
        f[..CHROMA].copy_from_slice(&self.chroma);
        f[CHROMA] = 0.5 * self.centroid;
        f[CHROMA + 1] = 0.5 * self.loudness;
        f
    }
}

fn unit(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-6 {
        for x in v {
            *x /= n;
        }
    }
}

fn seconds(series: &Series) -> Vec<Second> {
    let n = series.levels.len() * LEVEL_HOP / RATE;
    (0..n)
        .map(|t| {
            let levels = &series.levels[t * RATE / LEVEL_HOP..((t + 1) * RATE / LEVEL_HOP).min(series.levels.len())];
            let centroids = &series.centroids[(t * RATE / CENTROID_HOP).min(series.centroids.len())..((t + 1) * RATE / CENTROID_HOP).min(series.centroids.len())];
            let frames = &series.chroma[(t * RATE / CHROMA_HOP).min(series.chroma.len())..((t + 1) * RATE / CHROMA_HOP).min(series.chroma.len())];
            let mut chroma = [0f32; CHROMA];
            for f in frames {
                for (c, x) in chroma.iter_mut().zip(f) {
                    *c += x;
                }
            }
            unit(&mut chroma);
            let db = db(mean(levels));
            Second { chroma, centroid: (mean(centroids) / (RATE as f32 / 2.0)).clamp(0.0, 1.0), loudness: ((db + 90.0) / 90.0).clamp(0.0, 1.0), db }
        })
        .collect()
}

fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

/// The distances between the features of every two seconds, each
/// second the mean of its `smooth` neighbours.
fn distances(seconds: &[Second], smooth: usize) -> Vec<Vec<f32>> {
    let raw: Vec<[f32; CHROMA + 2]> = seconds.iter().map(Second::features).collect();
    let smooth: Vec<[f32; CHROMA + 2]> = (0..raw.len())
        .map(|t| {
            let (lo, hi) = (t.saturating_sub(smooth / 2), (t + smooth / 2 + 1).min(raw.len()));
            let mut f = [0f32; CHROMA + 2];
            for r in &raw[lo..hi] {
                for (x, y) in f.iter_mut().zip(r) {
                    *x += y;
                }
            }
            for x in &mut f {
                *x /= (hi - lo) as f32;
            }
            f
        })
        .collect();
    (0..smooth.len()).map(|t| (0..smooth.len()).map(|s| distance(&smooth[t], &smooth[s])).collect()).collect()
}

/// The novelty at every second: how unlike the `KERNEL` seconds before
/// are to the `KERNEL` after, less how unlike each run is within, as
/// a share of the former: 1 where two still stretches meet, 0 where
/// nothing changes or everything does.
fn novelty(d: &[Vec<f32>]) -> Vec<f32> {
    contrast(d).into_iter().map(|(novelty, _)| novelty).collect()
}

/// The novelty at every second and the distance between the runs
/// before and after it.
fn contrast(d: &[Vec<f32>]) -> Vec<(f32, f32)> {
    let n = d.len();
    (0..n)
        .map(|t| {
            if t < KERNEL || t + KERNEL > n {
                return (0.0, 0.0);
            }
            let (before, after) = (t - KERNEL..t, t..t + KERNEL);
            let between = before.clone().flat_map(|i| after.clone().map(move |j| (i, j))).map(|(i, j)| d[i][j]).sum::<f32>() / (KERNEL * KERNEL) as f32;
            let within = |r: std::ops::Range<usize>| r.clone().flat_map(|i| r.clone().map(move |j| (i, j))).map(|(i, j)| d[i][j]).sum::<f32>() / (KERNEL * KERNEL) as f32;
            if between < CONTRAST {
                return (0.0, between);
            }
            ((between - (within(before) + within(after)) / 2.0) / between, between)
        })
        .collect()
}

fn sections(novelty: &[f32]) -> usize {
    1 + (0..novelty.len())
        .filter(|&t| {
            let n = novelty[t];
            n > NOVELTY && (t.saturating_sub(KERNEL)..(t + KERNEL + 1).min(novelty.len())).all(|s| s == t || novelty[s] < n || (novelty[s] == n && s > t))
        })
        .count()
}

/// The share of seconds a run from which comes back later, after
/// the track has gone elsewhere: a verse again, a chorus. A stretch
/// that merely goes on is not a return.
fn repetition(d: &[Vec<f32>]) -> f32 {
    let n = d.len();
    if n <= RUN + APART {
        return 0.0;
    }
    let runs = n - RUN;
    let mut back = vec![false; runs];
    for t in 0..runs {
        let mut left = false;
        for s in t + 1..runs {
            if !left {
                left = d[t][s] > LEAVE;
                continue;
            }
            if s - t >= APART && (0..RUN).map(|k| d[t + k][s + k]).sum::<f32>() / RUN as f32 <= TWIN {
                back[t] = true;
                back[s] = true;
            }
        }
    }
    back.iter().filter(|b| **b).count() as f32 / runs as f32
}

/// Krumhansl's key profiles, how much each degree of a major and of
/// a minor key is heard in it.
const MAJOR: [f32; CHROMA] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; CHROMA] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let (ma, mb) = (mean(a), mean(b));
    let cov = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum::<f32>();
    let (va, vb) = (a.iter().map(|x| (x - ma).powi(2)).sum::<f32>(), b.iter().map(|y| (y - mb).powi(2)).sum::<f32>());
    match va * vb > 0.0 {
        true => cov / (va * vb).sqrt(),
        false => 0.0,
    }
}

/// How far the track's chroma is from any one key: 0 for a track
/// that sits in a key, towards 1 for one that is in none.
fn harmony(series: &Series) -> f32 {
    let mut sum = [0f32; CHROMA];
    for f in &series.chroma {
        for (s, x) in sum.iter_mut().zip(f) {
            *s += x;
        }
    }
    if sum.iter().sum::<f32>() <= 0.0 {
        return 0.0;
    }
    let fit = (0..CHROMA)
        .flat_map(|key| [MAJOR, MINOR].into_iter().map(move |profile| (key, profile)))
        .map(|(key, profile)| {
            let rotated: Vec<f32> = (0..CHROMA).map(|i| profile[(i + CHROMA - key) % CHROMA]).collect();
            correlation(&sum, &rotated)
        })
        .fold(f32::MIN, f32::max);
    (1.0 - fit).clamp(0.0, 1.0)
}

/// How far the chroma moves from a second to the next, on average.
fn changes(seconds: &[Second]) -> f32 {
    let steps: Vec<f32> = seconds.windows(2).filter(|w| w[0].chroma.iter().any(|x| *x != 0.0) && w[1].chroma.iter().any(|x| *x != 0.0)).map(|w| 1.0 - w[0].chroma.iter().zip(&w[1].chroma).map(|(x, y)| x * y).sum::<f32>()).collect();
    mean(&steps).max(0.0)
}

/// The share of beats on the pulse: an interval within `PULSE_SLACK`
/// of the median one.
fn pulse(beats: &[usize]) -> f32 {
    let mut intervals: Vec<f32> = beats.windows(2).map(|w| w[1].abs_diff(w[0]) as f32).filter(|i| *i > 0.0).collect();
    if intervals.len() < 4 {
        return 0.0;
    }
    let mut sorted = intervals.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    intervals.retain(|i| (i - median).abs() <= PULSE_SLACK * median);
    intervals.len() as f32 / sorted.len() as f32
}

/// The eight measures, raw: dB, doublings and shares.
pub fn measure(series: &Series) -> Shape {
    let seconds = seconds(series);
    let n = seconds.len();
    // the sound between the silences at either end, in thirds
    let heard = seconds.iter().position(|s| s.db > FLOOR).unwrap_or(n)..seconds.iter().rposition(|s| s.db > FLOOR).map_or(0, |i| i + 1);
    let third = heard.len() / 3;
    let (rise, arch) = match third {
        0 => (0.0, 0.0),
        _ => {
            let loud = |r: std::ops::Range<usize>| mean(&seconds[r].iter().map(|s| s.db.max(FLOOR)).collect::<Vec<_>>());
            let (first, mid, last) = (loud(heard.start..heard.start + third), loud(heard.start + third..heard.end - third), loud(heard.end - third..heard.end));
            (last - first, mid - (first + last) / 2.0)
        }
    };
    let structure = &seconds[..n.min(LONGEST)];
    let mut raw = [0f32; N];
    raw[RISE] = rise;
    raw[ARCH] = arch;
    raw[SECTIONS] = (sections(&novelty(&distances(structure, SMOOTH))) as f32).log2();
    raw[REPETITION] = repetition(&distances(structure, 1));
    raw[HARMONY] = harmony(series);
    raw[CHANGES] = changes(&seconds);
    raw[PULSE] = pulse(&series.beats);
    raw[ONSETS] = series.onsets.len() as f32 / n.max(1) as f32;
    raw
}

pub fn of(series: &Series) -> Shape {
    let mut shape = measure(series);
    for (x, (lo, hi)) in shape.iter_mut().zip(BOUNDS) {
        *x = norm(*x, lo, hi);
    }
    shape
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(notes: &[usize]) -> Vec<f32> {
        let mut f = vec![0f32; CHROMA];
        for &n in notes {
            f[n] = 1.0 / notes.len() as f32;
        }
        f
    }

    /// `seconds` of a sound: a chord, a level and a centroid.
    fn stretch(series: &mut Series, seconds: usize, notes: &[usize], level: f32, centroid: f32) {
        series.levels.extend(std::iter::repeat_n(level, seconds * RATE / LEVEL_HOP));
        series.centroids.extend(std::iter::repeat_n(centroid, seconds * RATE / CENTROID_HOP));
        series.chroma.extend(std::iter::repeat_n(frame(notes), seconds * RATE / CHROMA_HOP));
    }

    #[test]
    fn a_crescendo_rises_and_an_arch_swells() {
        let mut up = Series::default();
        stretch(&mut up, 30, &[0, 4, 7], 1e-5, 2000.0);
        stretch(&mut up, 30, &[0, 4, 7], 1e-4, 2000.0);
        stretch(&mut up, 30, &[0, 4, 7], 1e-3, 2000.0);
        let s = of(&up);
        assert!((s[RISE] - 1.0).abs() < 1e-3, "20 dB up is all the rise and more: {}", s[RISE]);
        assert!(s[ARCH].abs() < 0.05, "a straight line is no arch: {}", s[ARCH]);
        let mut swell = Series::default();
        stretch(&mut swell, 30, &[0, 4, 7], 1e-4, 2000.0);
        stretch(&mut swell, 30, &[0, 4, 7], 1e-3, 2000.0);
        stretch(&mut swell, 30, &[0, 4, 7], 1e-4, 2000.0);
        let s = of(&swell);
        assert!(s[RISE].abs() < 0.01);
        assert!((s[ARCH] - 1.0).abs() < 0.05, "10 dB over the ends is the whole arch: {}", s[ARCH]);
    }

    #[test]
    fn a_drone_is_one_section_a_song_is_more_and_repeats() {
        let mut drone = Series::default();
        stretch(&mut drone, 120, &[0, 7], 1e-3, 1000.0);
        let s = of(&drone);
        assert_eq!(s[SECTIONS], -1.0, "one section");
        assert_eq!(s[REPETITION], -1.0, "and nothing comes back, nothing having left");
        assert!(s[CHANGES] < -0.99, "the chroma never moves");
        let mut song = Series::default();
        for _ in 0..2 {
            stretch(&mut song, 30, &[0, 4, 7], 1e-4, 1000.0);
            stretch(&mut song, 30, &[2, 5, 9], 1e-3, 3000.0);
        }
        stretch(&mut song, 30, &[0, 4, 7], 1e-4, 1000.0);
        let s = of(&song);
        assert!(s[SECTIONS] > -0.2, "five sections, {}", s[SECTIONS]);
        assert!(s[REPETITION] > 0.5, "verse and chorus come back: {}", s[REPETITION]);
        let mut through = Series::default();
        for n in 0..4 {
            stretch(&mut through, 30, &[n, n + 4, n + 8], 1e-3, 1000.0 + 500.0 * n as f32);
        }
        let s = of(&through);
        assert!(s[REPETITION] < -0.8, "nothing comes back: {}", s[REPETITION]);
    }

    #[test]
    fn a_steady_pulse_and_a_loose_one() {
        let mut series = Series::default();
        stretch(&mut series, 60, &[0], 1e-3, 1000.0);
        series.beats = (0..120).map(|b| b * RATE / 2).collect();
        series.onsets = (0..240).map(|b| b * RATE / 4).collect();
        let s = of(&series);
        assert_eq!(s[PULSE], 1.0);
        assert!(s[ONSETS].abs() < 0.05, "four a second is half the most: {}", s[ONSETS]);
        series.beats = (0..120).map(|b| b * RATE / 2 + if b % 3 == 0 { RATE / 5 } else { 0 }).collect();
        let s = of(&series);
        assert!(s[PULSE] < 0.0, "two thirds of the intervals are off: {}", s[PULSE]);
        series.beats.clear();
        assert_eq!(of(&series)[PULSE], -1.0, "no beats, no pulse");
    }

    /// `SHAPE_FILES=a.flac:b.mp3 cargo test --release shape::tests::probe -- --ignored --nocapture`
    /// prints the shape of each file, for tuning the bounds.
    #[test]
    #[ignore]
    fn probe() {
        use bliss_audio::decoder::{symphonia::SymphoniaDecoder, Decoder};
        let files = std::env::var("SHAPE_FILES").unwrap_or_default();
        println!("{:40} {}", "", NAMES.iter().map(|n| format!("{n:>10}")).collect::<String>());
        for file in files.split(':').filter(|f| !f.is_empty()) {
            let pre = <SymphoniaDecoder as Decoder>::decode(std::path::Path::new(file)).unwrap();
            let (_, series) = bliss_audio::Song::analyze_series(&pre.sample_array).unwrap();
            let (raw, s) = (measure(&series), of(&series));
            let name: String = std::path::Path::new(file).file_name().unwrap().to_string_lossy().chars().take(40).collect();
            println!("{name:40} {}", s.iter().map(|x| format!("{x:>10.2}")).collect::<String>());
            println!("{:40} {}", "", raw.iter().map(|x| format!("{x:>10.2}")).collect::<String>());
            let secs = seconds(&series);
            let d = distances(&secs[..secs.len().min(LONGEST)], SMOOTH);
            let c = contrast(&d);
            let nov: Vec<f32> = c.iter().map(|x| x.0).collect();
            let mut peaks: Vec<(f32, f32)> = (0..nov.len()).filter(|&t| nov[t] > 0.0 && (t.saturating_sub(KERNEL)..(t + KERNEL + 1).min(nov.len())).all(|u| u == t || nov[u] < nov[t] || (nov[u] == nov[t] && u > t))).map(|t| c[t]).collect();
            peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
            println!("{:40} {} s, peaks: {}", "", secs.len(), peaks.iter().map(|(n, b)| format!("{n:.2}/{b:.2} ")).collect::<String>());
        }
    }

    #[test]
    fn too_short_to_shape_is_still_a_shape() {
        let s = of(&Series::default());
        assert!(s.iter().all(|x| x.abs() <= 1.0));
        let mut short = Series::default();
        stretch(&mut short, 2, &[0], 1e-3, 1000.0);
        assert!(of(&short).iter().all(|x| x.abs() <= 1.0));
    }
}
