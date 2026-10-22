use std::rc::Rc;
use std::time::Instant;

use crate::rl::{self, Color, Input, Rectangle, Texture, Vector2};
use crate::size::rem;
use crate::text::{Face, Fonts, Weight};

const fn hex(rgb: u32) -> Color {
    Color { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: 255 }
}

pub const BG: Color = hex(0x16242d);
pub const FG: Color = hex(0xd6e2ee);
pub const CARD: Color = hex(0x1c2f3e);
pub const SHADE: Color = hex(0x243d56);
pub const HOVER: Color = hex(0x304860);
pub const ACCENT_BG: Color = hex(0x4d86b0);
pub const ACCENT: Color = hex(0x8bc9eb);
pub const DESTRUCTIVE: Color = hex(0xbf616a);
const DESTRUCTIVE_HOVER: Color = hex(0xc86e77);
const DESTRUCTIVE_ACTIVE: Color = hex(0xa9545c);
const SUGGESTED_HOVER: Color = hex(0x5a92bb);
const SUGGESTED_ACTIVE: Color = hex(0x3f7199);
const WHITE: Color = hex(0xffffff);
pub const BIO: Color = hex(0xc0cdd9);
pub const DIM: Color = Color { r: 0, g: 0, b: 0, a: 92 };
pub const ROW_HOVER: Color = hex(0x213647);
pub const ROW_SELECTED: Color = hex(0x2a4256);
pub const ROW_SELECTED_HOVER: Color = hex(0x2e475c);
pub const BUTTON: Color = hex(0x2d3a42);
pub const BUTTON_HOVER: Color = hex(0x35424a);
pub const BUTTON_ACTIVE: Color = hex(0x454f57);
pub const FLAT_HOVER: Color = hex(0x1f2d36);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }

    pub fn contains(&self, p: Vector2) -> bool {
        p.x >= self.x && p.x < self.x + self.w && p.y >= self.y && p.y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Vector2 {
        Vector2 { x: self.x + self.w / 2.0, y: self.y + self.h / 2.0 }
    }

    pub fn inset(&self, dx: f32, dy: f32) -> R {
        R::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }

    pub fn intersect(&self, o: &R) -> R {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        R::new(x, y, (self.right().min(o.right()) - x).max(0.0), (self.bottom().min(o.bottom()) - y).max(0.0))
    }

    pub fn centered(&self, w: f32, h: f32) -> R {
        R::new((self.x + (self.w - w) / 2.0).round(), (self.y + (self.h - h) / 2.0).round(), w, h)
    }

    fn rl(&self) -> Rectangle {
        Rectangle { x: self.x, y: self.y, width: self.w, height: self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Title,
    Header,
    Subtitle,
    Label,
}

impl Style {
    pub fn color(self) -> Color {
        match self {
            Style::Subtitle => ACCENT,
            _ => FG,
        }
    }
}

pub const CORNER: f32 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Plain,
    Suggested,
    Destructive,
    Danger,
}

impl Look {
    fn colors(self, over: bool, held: bool) -> (Option<Color>, Color) {
        match self {
            Look::Plain => (Some(if held { BUTTON_ACTIVE } else if over { BUTTON_HOVER } else { BUTTON }), FG),
            Look::Suggested => (Some(if held { SUGGESTED_ACTIVE } else if over { SUGGESTED_HOVER } else { ACCENT_BG }), BG),
            Look::Destructive => (Some(if held { DESTRUCTIVE_ACTIVE } else if over { DESTRUCTIVE_HOVER } else { DESTRUCTIVE }), WHITE),
            Look::Danger => (if held { Some(BUTTON_HOVER) } else if over { Some(FLAT_HOVER) } else { None }, DESTRUCTIVE),
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Scroll {
    pub offset: f32,
    pub content: f32,
    pub view: f32,
}

impl Scroll {
    pub fn clamp(&mut self) {
        self.offset = self.offset.clamp(0.0, (self.content - self.view).max(0.0));
    }

    pub fn reveal(&mut self, top: f32, bottom: f32) {
        if top < self.offset {
            self.offset = top;
        } else if bottom > self.offset + self.view {
            self.offset = bottom - self.view;
        }
    }
}

#[derive(Default, Clone)]
pub struct Entry {
    pub text: String,
    pub caret: usize,
}

impl Entry {
    pub fn input(&mut self, input: &Input) -> bool {
        match *input {
            Input::Char(c) => {
                self.text.insert(self.caret, c);
                self.caret += c.len_utf8();
            }
            Input::Key { code, mods } => {
                let ctrl = mods & rl::MOD_CONTROL != 0;
                let before = |e: &Entry| e.text[..e.caret].chars().next_back().map_or(0, char::len_utf8);
                let after = |e: &Entry| e.text[e.caret..].chars().next().map_or(0, char::len_utf8);
                match code {
                    rl::KEY_BACKSPACE if ctrl => {
                        let start = self.word_start();
                        self.text.replace_range(start..self.caret, "");
                        self.caret = start;
                    }
                    rl::KEY_BACKSPACE => {
                        let n = before(self);
                        self.text.replace_range(self.caret - n..self.caret, "");
                        self.caret -= n;
                    }
                    rl::KEY_DELETE => {
                        let n = after(self);
                        self.text.replace_range(self.caret..self.caret + n, "");
                    }
                    rl::KEY_LEFT => self.caret -= before(self),
                    rl::KEY_RIGHT => self.caret += after(self),
                    rl::KEY_HOME => self.caret = 0,
                    rl::KEY_END => self.caret = self.text.len(),
                    _ if ctrl && rl::key_char(code) == Some('v') => {
                        let paste: String = rl::clipboard().chars().filter(|c| !c.is_control()).collect();
                        self.text.insert_str(self.caret, &paste);
                        self.caret += paste.len();
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    fn word_start(&self) -> usize {
        let head = &self.text[..self.caret];
        let trimmed = head.trim_end();
        trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0)
    }
}

pub struct Ui {
    pub fonts: Fonts,
    pub w: f32,
    pub h: f32,
    pub mouse: Vector2,
    pub moved: bool,
    pub pressed: bool,
    pub released: bool,
    pub down: bool,
    pub press_at: Vector2,
    wheel: f32,
    notches: f32,
    swipe: Vec<(Instant, f32)>,
    fling: f32,
    fling_at: Instant,
    pub inputs: Vec<Input>,
    consumed: bool,
    pub blocked: bool,
    clips: Vec<R>,
    alpha: f32,
    hand: bool,
    shown_hand: bool,
    pub drag: Option<&'static str>,
    pub wake_in: Option<f64>,
    pub start: std::time::Instant,
}

impl Ui {
    pub fn new() -> Ui {
        Ui {
            fonts: Fonts::new(),
            w: 0.0,
            h: 0.0,
            mouse: Vector2 { x: -1.0, y: -1.0 },
            moved: false,
            pressed: false,
            released: false,
            down: false,
            press_at: Vector2::default(),
            wheel: 0.0,
            notches: 0.0,
            swipe: Vec::new(),
            fling: 0.0,
            fling_at: Instant::now(),
            inputs: Vec::new(),
            consumed: false,
            blocked: false,
            clips: Vec::new(),
            alpha: 1.0,
            hand: false,
            shown_hand: false,
            drag: None,
            wake_in: None,
            start: std::time::Instant::now(),
        }
    }

    fn pointer(scale: f32) -> Vector2 {
        let m = unsafe { rl::GetMousePosition() };
        if unsafe { rl::IsCursorOnScreen() } {
            Vector2 { x: m.x * scale, y: m.y * scale }
        } else {
            Vector2 { x: -1.0, y: -1.0 }
        }
    }

    fn screen_scale() -> f32 {
        let (sw, rw) = unsafe { (rl::GetScreenWidth() as f32, rl::GetRenderWidth() as f32) };
        if sw > 0.0 { rw / sw } else { 1.0 }
    }

    pub fn stirred(&self) -> bool {
        let size = unsafe { (rl::GetRenderWidth() as f32, rl::GetRenderHeight() as f32) };
        rl::input_waiting() || Ui::pointer(Ui::screen_scale()) != self.mouse || size != (self.w, self.h)
    }

    pub fn begin_frame(&mut self) -> bool {
        let scale = Ui::screen_scale();
        crate::size::set_scale(scale);
        self.fonts.begin_frame();
        unsafe {
            self.w = rl::GetRenderWidth() as f32;
            self.h = rl::GetRenderHeight() as f32;
        }
        let mouse = Ui::pointer(scale);
        self.moved = mouse != self.mouse;
        self.mouse = mouse;
        self.pressed = false;
        self.released = false;
        self.wheel = 0.0;
        self.notches = 0.0;
        self.inputs.clear();
        self.consumed = false;
        self.blocked = false;
        self.hand = false;
        self.wake_in = None;
        for input in rl::take_input() {
            match input {
                Input::Press(0) => {
                    self.pressed = true;
                    self.press_at = self.mouse;
                }
                Input::Release(0) => self.released = true,
                Input::Scroll { y, wheel } => {
                    self.fling = 0.0;
                    if wheel {
                        self.notches -= y;
                    } else {
                        let px = -y * 10.0 * scale;
                        self.wheel += px;
                        self.swipe.push((Instant::now(), px));
                    }
                }
                Input::ScrollStop => self.fling = Ui::fling_speed(std::mem::take(&mut self.swipe), scale),
                i @ Input::Press(rl::MOUSE_BACK | rl::MOUSE_FORWARD) => self.inputs.push(i),
                Input::Press(_) | Input::Release(_) => {}
                i => {
                    self.fling = 0.0;
                    self.inputs.push(i);
                }
            }
        }
        if self.pressed {
            self.fling = 0.0;
        }
        self.fling_on(scale);
        self.down = unsafe { rl::IsMouseButtonDown(0) } || (self.pressed && !self.released);
        self.moved || self.pressed || self.released || self.wheel != 0.0 || self.notches != 0.0 || !self.inputs.is_empty()
    }

    pub fn end_frame(&mut self) {
        if self.released {
            self.drag = None;
        }
        if self.hand != self.shown_hand {
            self.shown_hand = self.hand;
            unsafe { rl::SetMouseCursor(if self.hand { rl::MOUSE_CURSOR_POINTING_HAND } else { rl::MOUSE_CURSOR_DEFAULT }) };
        }
    }

    pub fn wake(&mut self, seconds: f64) {
        self.wake_in = Some(self.wake_in.map_or(seconds, |w| w.min(seconds)));
    }

    pub fn seconds(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn clip(&self) -> Option<&R> {
        self.clips.last()
    }

    pub fn hovered(&self, r: R) -> bool {
        !self.blocked && r.contains(self.mouse) && self.clip().is_none_or(|c| c.contains(self.mouse))
    }

    pub fn click(&mut self, r: R) -> bool {
        if self.released && !self.consumed && self.hovered(r) && r.contains(self.press_at) && self.clip().is_none_or(|c| c.contains(self.press_at)) {
            self.consumed = true;
            return true;
        }
        false
    }

    pub fn press(&mut self, r: R) -> bool {
        if self.pressed && !self.consumed && self.hovered(r) {
            self.consumed = true;
            return true;
        }
        false
    }

    pub fn held(&self, r: R) -> bool {
        self.down && !self.blocked && r.contains(self.press_at)
    }

    pub fn take_wheel(&mut self, r: R, page: f32) -> f32 {
        if (self.wheel == 0.0 && self.notches == 0.0) || !self.hovered(r) {
            return 0.0;
        }
        let scale = crate::size::scale();
        let step = 1.5 * (page / scale).max(1.0).powf(2.0 / 3.0) * scale;
        std::mem::take(&mut self.notches) * step + std::mem::take(&mut self.wheel)
    }

    fn fling_speed(swipe: Vec<(Instant, f32)>, scale: f32) -> f32 {
        let now = Instant::now();
        let recent: Vec<&(Instant, f32)> = swipe.iter().filter(|(t, _)| now.duration_since(*t).as_secs_f32() < 0.1).collect();
        let Some((first, _)) = recent.first() else { return 0.0 };
        let span = now.duration_since(*first).as_secs_f32().max(1.0 / 60.0);
        let speed = recent.iter().map(|(_, px)| px).sum::<f32>() / span;
        if speed.abs() < 150.0 * scale {
            0.0
        } else {
            speed
        }
    }

    fn fling_on(&mut self, scale: f32) {
        const TAU: f32 = 0.33;
        let now = Instant::now();
        let dt = now.duration_since(self.fling_at).as_secs_f32().min(0.1);
        self.fling_at = now;
        if self.fling == 0.0 {
            return;
        }
        let decay = (-dt / TAU).exp();
        self.wheel += self.fling * TAU * (1.0 - decay);
        self.fling *= decay;
        if self.fling.abs() < 20.0 * scale {
            self.fling = 0.0;
        } else {
            self.wake(0.0);
        }
    }

    pub fn set_hand(&mut self) {
        self.hand = true;
    }

    pub fn with_alpha<T>(&mut self, alpha: f32, f: impl FnOnce(&mut Ui) -> T) -> T {
        let before = self.alpha;
        self.alpha *= alpha;
        let out = f(self);
        self.alpha = before;
        out
    }

    fn tint(&self, c: Color) -> Color {
        Color { a: (c.a as f32 * self.alpha).round() as u8, ..c }
    }

    pub fn push_clip(&mut self, r: R) {
        let r = match self.clip() {
            Some(c) => r.intersect(c),
            None => r,
        };
        self.clips.push(r);
        self.scissor();
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
        self.scissor();
    }

    fn scissor(&self) {
        unsafe {
            rl::rlDrawRenderBatchActive();
            match self.clip() {
                Some(c) => {
                    rl::rlEnableScissorTest();
                    rl::rlScissor(c.x as i32, (self.h - c.bottom()) as i32, c.w as i32, c.h as i32);
                }
                None => rl::rlDisableScissorTest(),
            }
        }
    }

    pub fn rect(&self, r: R, c: Color) {
        unsafe { rl::DrawRectangleRec(r.rl(), self.tint(c)) };
    }

    pub fn rounded(&self, r: R, radius: f32, c: Color) {
        let roundness = (2.0 * radius / r.w.min(r.h)).min(1.0);
        unsafe { rl::DrawRectangleRounded(r.rl(), roundness, 24, self.tint(c)) };
    }

    pub fn round_corners(&self, r: R, radius: f32, bg: Color) {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0);
        if radius <= 0.0 {
            return;
        }
        const STEPS: usize = 8;
        let corners = [
            (Vector2 { x: r.x, y: r.y }, Vector2 { x: r.x + radius, y: r.y + radius }, 180.0f32),
            (Vector2 { x: r.right(), y: r.y }, Vector2 { x: r.right() - radius, y: r.y + radius }, 270.0),
            (Vector2 { x: r.right(), y: r.bottom() }, Vector2 { x: r.right() - radius, y: r.bottom() - radius }, 0.0),
            (Vector2 { x: r.x, y: r.bottom() }, Vector2 { x: r.x + radius, y: r.bottom() - radius }, 90.0),
        ];
        for (corner, centre, from) in corners {
            let mut points = [corner; STEPS + 2];
            for (i, p) in points[1..].iter_mut().enumerate() {
                let a = (from + 90.0 * i as f32 / STEPS as f32).to_radians();
                *p = Vector2 { x: centre.x + radius * a.cos(), y: centre.y + radius * a.sin() };
            }
            unsafe { rl::DrawTriangleFan(points.as_ptr(), points.len() as i32, self.tint(bg)) };
        }
    }

    pub fn circle(&self, center: Vector2, radius: f32, c: Color) {
        unsafe { rl::DrawCircleV(center, radius, self.tint(c)) };
    }

    pub fn picture(&self, t: Texture, r: R) {
        let (tw, th) = (t.width as f32, t.height as f32);
        let k = (r.w / tw).min(r.h / th);
        let (w, h) = ((tw * k).round(), (th * k).round());
        let dst = r.centered(w, h);
        unsafe { rl::DrawTexturePro(t, Rectangle { x: 0.0, y: 0.0, width: tw, height: th }, dst.rl(), Vector2::default(), 0.0, self.tint(rl::Color { r: 255, g: 255, b: 255, a: 255 })) };
    }

    pub fn placeholder(&self, glyph: &str, r: R, round: bool) {
        if round {
            self.circle(r.center(), r.w / 2.0, SHADE);
        } else {
            self.rect(r, SHADE);
        }
        let face = self.fonts.icons(r.w * 0.45);
        face.draw_centered(glyph, r.center(), self.tint(ACCENT_BG));
    }

    pub fn face(&self, style: Style) -> Rc<Face> {
        match style {
            Style::Title => self.fonts.face(Weight::Bold, 1.5),
            Style::Header | Style::Subtitle => self.fonts.face(Weight::Bold, 1.0),
            Style::Label => self.fonts.face(Weight::Regular, 1.0),
        }
    }

    pub fn line(&self, style: Style) -> f32 {
        self.face(style).line
    }

    pub fn text(&self, style: Style, s: &str, x: f32, y: f32, max: f32) -> f32 {
        self.text_in(style, s, x, y, max, style.color())
    }

    pub fn text_in(&self, style: Style, s: &str, x: f32, y: f32, max: f32, color: Color) -> f32 {
        let face = self.face(style);
        let s = face.ellipsize(s, max);
        face.draw(&s, x, y, self.tint(color))
    }

    pub fn text_right(&self, style: Style, s: &str, right: f32, y: f32) -> f32 {
        let face = self.face(style);
        let w = face.width(s);
        face.draw(s, right - w, y, self.tint(style.color()));
        w
    }

    pub fn width(&self, style: Style, s: &str) -> f32 {
        self.face(style).width(s)
    }

    pub fn link(&mut self, style: Style, s: &str, x: f32, y: f32, max: f32) -> (bool, f32) {
        self.link_in(style, s, x, y, max, style.color())
    }

    pub fn link_in(&mut self, style: Style, s: &str, x: f32, y: f32, max: f32, color: Color) -> (bool, f32) {
        let face = self.face(style);
        let shown = face.ellipsize(s, max);
        let w = face.width(&shown);
        let r = R::new(x, y, w, face.line);
        let over = self.hovered(r);
        face.draw(&shown, x, y, self.tint(color));
        if over {
            self.set_hand();
            let thick = (face.px / 14.0).round().max(1.0);
            self.rect(R::new(x.round(), (y + face.line * 0.86).round(), w, thick), color);
        }
        (self.click(r), w)
    }

    /// A line of names, a link each, separated by commas: the artists an
    /// album is credited to. What does not fit is cut with an ellipsis,
    /// the names beyond it dropped. The index of the name clicked.
    pub fn links(&mut self, style: Style, names: &[String], x: f32, y: f32, max: f32) -> Option<usize> {
        let face = self.face(style);
        let comma = face.width(", ");
        let mut clicked = None;
        let mut at = x;
        for (i, name) in names.iter().enumerate() {
            let room = x + max - at;
            if room <= 0.0 {
                break;
            }
            let (hit, w) = self.link(style, name, at, y, room);
            if hit {
                clicked = Some(i);
            }
            at += w;
            if face.width(name) > room {
                break;
            }
            if i + 1 < names.len() {
                face.draw(", ", at, y, self.tint(style.color()));
                at += comma;
            }
        }
        clicked
    }

    pub fn icon(&self, glyph: &str, r: R, em: f32, c: Color) {
        let face = self.fonts.icons(em * crate::size::px());
        face.draw_centered(glyph, r.center(), self.tint(c));
    }

    pub fn square(&mut self, x: f32, y: f32, glyph: &str, look: Look, sensitive: bool) -> bool {
        let side = self.square_side();
        let r = R::new(x, y, side, side);
        let over = sensitive && self.hovered(r);
        let (bg, fg) = look.colors(over, over && self.held(r));
        self.with_alpha(if sensitive { 1.0 } else { 0.5 }, |ui| {
            if let Some(bg) = bg {
                ui.rounded(r, rem(CORNER), bg);
            }
            ui.icon(glyph, r, 1.25, fg);
        });
        sensitive && self.click(r)
    }

    pub fn square_side(&self) -> f32 {
        self.line(Style::Header).max(rem(2.0))
    }
}
