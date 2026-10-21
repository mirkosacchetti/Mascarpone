use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;

use crate::mpd::Command;
use crate::rl::{self, Texture};

pub const SIZE: u32 = 320;

const KEEP: usize = 400;

pub fn cache_dir() -> PathBuf {
    crate::dirs::cache().join("mascarpone").join("covers")
}

pub fn path_for(key: &str) -> PathBuf {
    cache_dir().join(format!("{}.jpg", hash(key)))
}

pub fn hash(name: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

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

struct Pixels {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

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
    Waiting,
    Ready(Texture, u64),
    None,
}

#[derive(Clone, Default)]
pub struct Known {
    pub bio: Option<String>,
    pub url: Option<String>,
    portrait: bool,
}

pub struct Pictures {
    commands: UnboundedSender<Command>,
    slots: RefCell<HashMap<String, Slot>>,
    frame: Cell<u64>,
    jobs: mpsc::Sender<(String, PathBuf, bool)>,
    done: mpsc::Receiver<(String, Option<Pixels>)>,
    early: RefCell<Vec<(String, Option<Pixels>)>>,
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

    pub fn news(&self) -> bool {
        let mut early = self.early.borrow_mut();
        early.extend(self.done.try_iter());
        !early.is_empty()
    }

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

    pub fn retry(&self, name: &str) {
        self.known.borrow_mut().remove(name);
        if let Some(Slot::Ready(t, _)) = self.slots.borrow_mut().remove(&format!("a:{name}")) {
            self.dropped.borrow_mut().push(t);
        }
        self.asked.borrow_mut().insert(name.to_owned());
        let _ = self.commands.send(Command::ForgetArtist(name.to_owned()));
    }
}
