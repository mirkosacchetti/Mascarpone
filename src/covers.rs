//! Album covers, and the artists' portraits. mpd hands the picture over
//! (embedded or the file next to the album), or the net does; it is
//! shrunk once to a thumbnail in `~/.cache/mascarpone/covers`, read back
//! and decoded on two threads of their own, and kept as a texture while
//! it is shown: the last few hundred shown stay (a whole library of them
//! would be hundreds of MB). A thumbnail stays until its file is deleted.
//!
//! The textures are made and dropped at the start of a frame only, never
//! between two draws of one.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;

use crate::mpd::Command;
use crate::rl::{self, Texture};

/// Thumbnail side, in pixels: a 12 rem grid cover (264 px with a 15 px
/// font on a 1.5x screen), the 16 rem cover of the album page a little
/// soft. In memory a texture is four bytes a pixel, and a grid keeps a
/// few hundred alive.
pub const SIZE: u32 = 320;

/// Textures kept, about 400 KB each (more with their mipmaps): more than
/// a big window's grid shows at once.
const KEEP: usize = 400;

pub fn cache_dir() -> PathBuf {
    crate::dirs::cache().join("mascarpone").join("covers")
}

/// Where an album's thumbnail is kept: a stable hash of its key.
pub fn path_for(key: &str) -> PathBuf {
    cache_dir().join(format!("{}.jpg", hash(key)))
}

/// A stable hash of a name, for a file name: FNV-1a, 16 hex digits.
pub fn hash(name: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Decodes a picture as mpd gave it and writes the thumbnail. Slow, for a
/// blocking task.
pub fn thumbnail(bytes: &[u8], path: &Path) -> Result<()> {
    let image = image::load_from_memory(bytes)?.thumbnail(SIZE, SIZE).into_rgb8();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("part");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88).encode_image(&image)?;
    drop(out);
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// A picture as the texture wants it.
struct Pixels {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// A thumbnail read and decoded; a face cut round (a square from its
/// middle, a soft edge).
fn decode(path: &Path, round: bool) -> Option<Pixels> {
    let image = image::open(path).ok()?;
    if !round {
        let rgba = image.into_rgba8();
        let (width, height) = rgba.dimensions();
        return Some(Pixels { rgba: rgba.into_raw(), width, height });
    }
    let side = image.width().min(image.height());
    let square = image.crop_imm((image.width() - side) / 2, (image.height() - side) / 2, side, side);
    let mut rgba = square.into_rgba8();
    let r = side as f32 / 2.0;
    for (x, y, p) in rgba.enumerate_pixels_mut() {
        let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
        let a = (r - d + 0.5).clamp(0.0, 1.0);
        p.0[3] = (p.0[3] as f32 * a) as u8;
    }
    Some(Pixels { rgba: rgba.into_raw(), width: side, height: side })
}

enum Slot {
    /// Asked: of mpd, of the net, or of the decoders.
    Waiting,
    /// The picture, last shown at that frame.
    Ready(Texture, u64),
    /// There is none: the placeholder.
    None,
}

/// What is known of an artist: the words, the article's url, whether the
/// net gave a face.
#[derive(Clone, Default)]
pub struct Known {
    pub bio: Option<String>,
    pub url: Option<String>,
    portrait: bool,
}

/// The pictures as textures, by name (`c:` an album's key, `a:` an
/// artist's), and what is known of the artists.
pub struct Pictures {
    commands: UnboundedSender<Command>,
    slots: RefCell<HashMap<String, Slot>>,
    frame: Cell<u64>,
    jobs: mpsc::Sender<(String, PathBuf, bool)>,
    done: mpsc::Receiver<(String, Option<Pixels>)>,
    /// Decoded, taken off the channel before a wait, for the next frame.
    early: RefCell<Vec<(String, Option<Pixels>)>>,
    /// Textures to drop at the next frame's start.
    dropped: RefCell<Vec<Texture>>,
    asked: RefCell<HashSet<String>>,
    known: RefCell<HashMap<String, Known>>,
}

impl Pictures {
    pub fn new(commands: UnboundedSender<Command>) -> Pictures {
        let (jobs, todo) = mpsc::channel::<(String, PathBuf, bool)>();
        let (out, done) = mpsc::channel();
        let todo = Arc::new(Mutex::new(todo));
        for i in 0..2 {
            let (todo, out) = (todo.clone(), out.clone());
            std::thread::Builder::new()
                .name(format!("decode{i}"))
                .spawn(move || loop {
                    let job = todo.lock().unwrap().recv();
                    let Ok((name, path, round)) = job else { return };
                    if out.send((name, decode(&path, round))).is_err() {
                        return;
                    }
                    rl::wake();
                })
                .expect("decoder thread");
        }
        Pictures {
            commands,
            slots: RefCell::new(HashMap::new()),
            frame: Cell::new(0),
            jobs,
            done,
            early: RefCell::new(Vec::new()),
            dropped: RefCell::new(Vec::new()),
            asked: RefCell::new(HashSet::new()),
            known: RefCell::new(HashMap::new()),
        }
    }

    /// At the start of a frame: the decoded pictures made textures, the
    /// least recently shown dropped when too many. True when one came.
    pub fn begin_frame(&self) -> bool {
        let frame = self.frame.get() + 1;
        self.frame.set(frame);
        for t in self.dropped.borrow_mut().drain(..) {
            unsafe { rl::UnloadTexture(t) };
        }
        let mut any = false;
        let mut slots = self.slots.borrow_mut();
        let early = std::mem::take(&mut *self.early.borrow_mut());
        for (name, pixels) in early.into_iter().chain(self.done.try_iter()) {
            any = true;
            // dropped meanwhile (a retry): not wanted any more
            let Some(slot) = slots.get_mut(&name) else { continue };
            *slot = match pixels {
                Some(p) => Slot::Ready(rl::texture(&p.rgba, p.width, p.height), frame),
                None => Slot::None,
            };
        }
        let ready = slots.values().filter(|s| matches!(s, Slot::Ready(..))).count();
        if ready > KEEP {
            let mut ages: Vec<(u64, String)> = slots.iter().filter_map(|(k, s)| if let Slot::Ready(_, at) = s { Some((*at, k.clone())) } else { None }).collect();
            ages.sort();
            for (at, key) in ages.into_iter().take(ready - KEEP) {
                // not one drawn on the last frame
                if at + 1 >= frame {
                    break;
                }
                if let Some(Slot::Ready(t, _)) = slots.remove(&key) {
                    unsafe { rl::UnloadTexture(t) };
                }
            }
        }
        any
    }

    /// A picture decoded and not yet shown: no waiting for events then.
    pub fn news(&self) -> bool {
        let mut early = self.early.borrow_mut();
        early.extend(self.done.try_iter());
        !early.is_empty()
    }

    /// Pictures asked and not yet in.
    pub fn waiting(&self) -> usize {
        let pictures = self.slots.borrow().values().filter(|s| matches!(s, Slot::Waiting)).count();
        let known = self.known.borrow();
        pictures + self.asked.borrow().iter().filter(|n| !known.contains_key(*n)).count()
    }

    fn texture(&self, name: &str) -> Option<Texture> {
        let mut slots = self.slots.borrow_mut();
        match slots.get_mut(name)? {
            Slot::Ready(t, at) => {
                *at = self.frame.get();
                Some(*t)
            }
            _ => None,
        }
    }

    fn decode(&self, name: String, path: PathBuf, round: bool) {
        self.slots.borrow_mut().insert(name.clone(), Slot::Waiting);
        let _ = self.jobs.send((name, path, round));
    }

    /// The album's cover, or none yet (the placeholder): asked the first
    /// time, read from the cache when it is there, else of mpd.
    pub fn cover(&self, key: &str, uri: &str) -> Option<Texture> {
        let name = format!("c:{key}");
        if self.slots.borrow().contains_key(&name) {
            return self.texture(&name);
        }
        let path = path_for(key);
        if path.exists() {
            self.decode(name, path, false);
        } else {
            self.slots.borrow_mut().insert(name, Slot::Waiting);
            let _ = self.commands.send(Command::Cover { key: key.to_owned(), uri: uri.to_owned() });
        }
        None
    }

    /// Like `cover`, but a thumbnail already on disk is read here and now:
    /// for the picker's few small covers, refilled at every keystroke,
    /// where a frame of the disc before the cover would flicker.
    pub fn cover_now(&self, key: &str, uri: &str) -> Option<Texture> {
        let name = format!("c:{key}");
        if !self.slots.borrow().contains_key(&name) {
            let path = path_for(key);
            if let Some(p) = path.exists().then(|| decode(&path, false)).flatten() {
                let t = rl::texture(&p.rgba, p.width, p.height);
                self.slots.borrow_mut().insert(name.clone(), Slot::Ready(t, self.frame.get()));
            }
        }
        self.cover(key, uri)
    }

    /// mpd's answer on a cover: its thumbnail, or none; a failed one is
    /// asked again the next time it shows.
    pub fn cover_arrived(&self, key: &str, path: Option<&Path>, failed: bool) {
        let name = format!("c:{key}");
        match (path, failed) {
            (Some(p), _) => self.decode(name, p.to_path_buf(), false),
            (None, false) => {
                self.slots.borrow_mut().insert(name, Slot::None);
            }
            (None, true) => {
                self.slots.borrow_mut().remove(&name);
            }
        }
    }

    fn ask(&self, name: &str) {
        if self.asked.borrow_mut().insert(name.to_owned()) {
            let _ = self.commands.send(Command::Artist(name.to_owned()));
        }
    }

    /// The artist's face, or none (yet): asked the first time.
    pub fn face(&self, name: &str) -> Option<Texture> {
        let portrait = match self.known.borrow().get(name) {
            Some(k) => k.portrait,
            None => {
                self.ask(name);
                return None;
            }
        };
        if !portrait {
            return None;
        }
        let key = format!("a:{name}");
        if self.slots.borrow().contains_key(&key) {
            return self.texture(&key);
        }
        self.decode(key, crate::net::portrait_path(name), true);
        None
    }

    /// The artist's words and the article's url, once they are in.
    pub fn words(&self, name: &str) -> Option<Known> {
        let known = self.known.borrow().get(name).cloned();
        if known.is_none() {
            self.ask(name);
        }
        known
    }

    pub fn artist_arrived(&self, name: &str, info: &crate::net::Info) {
        self.known.borrow_mut().insert(name.to_owned(), Known { bio: info.bio.clone(), url: info.wikipedia.clone(), portrait: info.portrait });
    }

    /// A new try from the net for an artist: the cache and what is kept
    /// forgotten, asked again.
    pub fn retry(&self, name: &str) {
        crate::net::forget(name);
        self.known.borrow_mut().remove(name);
        self.asked.borrow_mut().remove(name);
        if let Some(Slot::Ready(t, _)) = self.slots.borrow_mut().remove(&format!("a:{name}")) {
            self.dropped.borrow_mut().push(t);
        }
        self.ask(name);
    }
}
