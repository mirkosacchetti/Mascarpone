use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::ffi::{c_int, CString};
use std::rc::Rc;

use crate::rl::{self, Color, Rectangle, Vector2};

const FAMILY: &str = "JetBrainsMono Nerd Font Propo";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Set {
    Text,
    Icons,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    weight: Weight,
    px: u32,
    set: Set,
}

pub struct Face {
    font: rl::Font,
    index: HashMap<u32, usize>,
    pub px: f32,
    pub line: f32,
    missing: Option<Rc<RefCell<BTreeSet<u32>>>>,
}

impl Face {
    fn glyph(&self, c: char) -> Option<(rl::GlyphInfo, Rectangle)> {
        let Some(&i) = self.index.get(&(c as u32)) else {
            if let Some(m) = &self.missing {
                if !c.is_control() {
                    m.borrow_mut().insert(c as u32);
                }
            }
            return None;
        };
        unsafe { Some((*self.font.glyphs.add(i), *self.font.recs.add(i))) }
    }

    pub fn advance(&self, c: char) -> f32 {
        match self.glyph(c) {
            Some((g, _)) if g.advanceX > 0 => g.advanceX as f32,
            Some((_, r)) => r.width,
            None => 0.0,
        }
    }

    pub fn width(&self, s: &str) -> f32 {
        s.chars().map(|c| self.advance(c)).sum()
    }

    pub fn draw(&self, s: &str, x: f32, y: f32, color: Color) -> f32 {
        let (x0, y) = (x.round(), y.round());
        let mut x = x0;
        let pad = self.font.glyphPadding as f32;
        for c in s.chars() {
            let Some((g, r)) = self.glyph(c) else { continue };
            if r.width > 0.0 && c != ' ' {
                let src = Rectangle { x: r.x - pad, y: r.y - pad, width: r.width + 2.0 * pad, height: r.height + 2.0 * pad };
                let dst = Rectangle { x: x + g.offsetX as f32 - pad, y: y + g.offsetY as f32 - pad, width: src.width, height: src.height };
                unsafe { rl::DrawTexturePro(self.font.texture, src, dst, Vector2::default(), 0.0, color) };
            }
            x += if g.advanceX > 0 { g.advanceX as f32 } else { r.width };
        }
        x - x0
    }

    pub fn draw_centered(&self, glyph: &str, center: Vector2, color: Color) {
        let Some(c) = glyph.chars().next() else { return };
        let Some((_, r)) = self.glyph(c) else { return };
        let pad = self.font.glyphPadding as f32;
        let src = Rectangle { x: r.x - pad, y: r.y - pad, width: r.width + 2.0 * pad, height: r.height + 2.0 * pad };
        let dst = Rectangle { x: (center.x - r.width / 2.0).round() - pad, y: (center.y - r.height / 2.0).round() - pad, width: src.width, height: src.height };
        unsafe { rl::DrawTexturePro(self.font.texture, src, dst, Vector2::default(), 0.0, color) };
    }

    pub fn ellipsize<'a>(&self, s: &'a str, max: f32) -> std::borrow::Cow<'a, str> {
        if self.width(s) <= max {
            return s.into();
        }
        let room = max - self.advance('…');
        let mut w = 0.0;
        let mut end = 0;
        for (i, c) in s.char_indices() {
            w += self.advance(c);
            if w > room {
                break;
            }
            end = i + c.len_utf8();
        }
        format!("{}…", s[..end].trim_end()).into()
    }

    pub fn wrap(&self, s: &str, max: f32, lines: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for paragraph in s.split('\n') {
            let mut line = String::new();
            let mut w = 0.0;
            let space = self.advance(' ');
            for word in paragraph.split(' ').filter(|w| !w.is_empty()) {
                let ww = self.width(word);
                if !line.is_empty() && w + space + ww <= max {
                    line.push(' ');
                    line.push_str(word);
                    w += space + ww;
                    continue;
                }
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                    w = 0.0;
                }
                if ww <= max {
                    line.push_str(word);
                    w = ww;
                    continue;
                }
                for c in word.chars() {
                    let cw = self.advance(c);
                    if w + cw > max && !line.is_empty() {
                        out.push(std::mem::take(&mut line));
                        w = 0.0;
                    }
                    line.push(c);
                    w += cw;
                }
            }
            out.push(line);
        }
        if lines > 0 && out.len() > lines {
            let rest = out[lines - 1..].join(" ");
            out.truncate(lines - 1);
            out.push(self.ellipsize(&rest, max).into_owned());
        }
        out
    }
}

pub struct Fonts {
    files: [(Vec<u8>, f32); 2],
    text: RefCell<BTreeSet<u32>>,
    missing: Rc<RefCell<BTreeSet<u32>>>,
    faces: RefCell<HashMap<Key, Rc<Face>>>,
    made_for: Cell<f32>,
}

impl Fonts {
    pub fn new() -> Fonts {
        let mut text: BTreeSet<u32> = (32..=126).chain(160..=383).chain(0x2010..=0x2027).chain(0x2030..=0x203a).collect();
        for g in crate::glyphs::ALL {
            text.extend(g.chars().map(|c| c as u32));
        }
        Fonts {
            files: [load(Weight::Regular), load(Weight::Bold)],
            text: RefCell::new(text),
            missing: Rc::new(RefCell::new(BTreeSet::new())),
            faces: RefCell::new(HashMap::new()),
            made_for: Cell::new(0.0),
        }
    }

    pub fn learn(&self, s: &str) {
        let text = self.text.borrow();
        let mut missing = self.missing.borrow_mut();
        for c in s.chars() {
            if !c.is_control() && !text.contains(&(c as u32)) {
                missing.insert(c as u32);
            }
        }
    }

    pub fn begin_frame(&self) {
        let px = crate::size::px();
        if (px - self.made_for.get()).abs() > 0.01 {
            self.made_for.set(px);
            self.drop_faces(|_| true);
        }
        let missing = std::mem::take(&mut *self.missing.borrow_mut());
        if !missing.is_empty() {
            self.text.borrow_mut().extend(missing);
            self.drop_faces(|k| k.set == Set::Text);
        }
    }

    fn drop_faces(&self, which: impl Fn(&Key) -> bool) {
        let mut faces = self.faces.borrow_mut();
        let gone: Vec<Key> = faces.keys().filter(|k| which(k)).copied().collect();
        for k in gone {
            if let Some(face) = faces.remove(&k) {
                if let Ok(face) = Rc::try_unwrap(face) {
                    unsafe { rl::UnloadFont(face.font) };
                }
            }
        }
    }

    pub fn face(&self, weight: Weight, rem: f32) -> Rc<Face> {
        self.get(Key { weight, px: (rem * crate::size::px()).round() as u32, set: Set::Text })
    }

    pub fn icons(&self, px: f32) -> Rc<Face> {
        self.get(Key { weight: Weight::Regular, px: px.round().max(1.0) as u32, set: Set::Icons })
    }

    fn get(&self, key: Key) -> Rc<Face> {
        if let Some(f) = self.faces.borrow().get(&key) {
            return f.clone();
        }
        let codepoints: Vec<c_int> = match key.set {
            Set::Text => self.text.borrow().iter().map(|&c| c as c_int).collect(),
            Set::Icons => crate::glyphs::ALL.iter().flat_map(|g| g.chars()).map(|c| c as c_int).collect(),
        };
        let (data, ratio) = &self.files[key.weight as usize];
        let size = (key.px as f32 * ratio).round() as c_int;
        let ext = CString::new(".ttf").unwrap();
        let font = unsafe { rl::LoadFontFromMemory(ext.as_ptr(), data.as_ptr(), data.len() as c_int, size, codepoints.as_ptr(), codepoints.len() as c_int) };
        let mut index = HashMap::new();
        for i in 0..font.glyphCount.max(0) as usize {
            let g = unsafe { *font.glyphs.add(i) };
            index.insert(g.value as u32, i);
        }
        let face = Rc::new(Face {
            font,
            index,
            px: key.px as f32,
            line: font.baseSize as f32,
            missing: (key.set == Set::Text).then(|| self.missing.clone()),
        });
        self.faces.borrow_mut().insert(key, face.clone());
        face
    }
}

fn load(weight: Weight) -> (Vec<u8>, f32) {
    let style = match weight {
        Weight::Regular => "Regular",
        Weight::Bold => "Bold",
    };
    let path = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", &format!("{FAMILY}:style={style}")])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .filter(|p| !p.is_empty());
    let data = path.as_deref().and_then(|p| std::fs::read(p).ok()).unwrap_or_else(|| panic!("no font file for {FAMILY} {style} (fc-match)"));
    let ratio = line_per_em(&data).unwrap_or(1.3);
    (data, ratio)
}

fn line_per_em(ttf: &[u8]) -> Option<f32> {
    let u16_at = |at: usize| -> Option<u16> { Some(u16::from_be_bytes(ttf.get(at..at + 2)?.try_into().ok()?)) };
    let u32_at = |at: usize| -> Option<u32> { Some(u32::from_be_bytes(ttf.get(at..at + 4)?.try_into().ok()?)) };
    let tables = u16_at(4)? as usize;
    let mut head = None;
    let mut hhea = None;
    for i in 0..tables {
        let at = 12 + 16 * i;
        let tag = ttf.get(at..at + 4)?;
        let offset = u32_at(at + 8)? as usize;
        match tag {
            b"head" => head = Some(offset),
            b"hhea" => hhea = Some(offset),
            _ => {}
        }
    }
    let upem = u16_at(head? + 18)? as f32;
    let ascent = u16_at(hhea? + 4)? as i16 as f32;
    let descent = u16_at(hhea? + 6)? as i16 as f32;
    (upem > 0.0).then(|| (ascent - descent) / upem)
}
