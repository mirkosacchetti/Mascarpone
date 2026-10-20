//! The frame's tools: the input of this frame, the palette (lintel's),
//! the four kinds of text, the shapes, the clip, and the few widgets
//! every page uses (links, round buttons, switches, a scroll), drawn and
//! asked at once: a click is a release where the press began, on a
//! widget that has not given it to one inside it first (each draws its
//! insides first and asks after).
//!
//! Flat: no frames, no shadows, no rounded corners but on buttons (and
//! the round portraits), no animations.

use std::rc::Rc;
use std::time::Instant;

use crate::rl::{self, Color, Input, Rectangle, Texture, Vector2};
use crate::size::rem;
use crate::text::{Face, Fonts, Weight};

const fn hex(rgb: u32) -> Color {
    Color { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: 255 }
}

/// The window's background, and the header's.
pub const BG: Color = hex(0x16242d);
pub const FG: Color = hex(0xd6e2ee);
/// The card under the pointer, a list's rows.
pub const CARD: Color = hex(0x1c2f3e);
/// The selected card; a cover waiting for its picture.
pub const SHADE: Color = hex(0x243d56);
/// A shade under the pointer.
pub const HOVER: Color = hex(0x304860);
pub const ACCENT_BG: Color = hex(0x4d86b0);
pub const ACCENT: Color = hex(0x8bc9eb);
pub const DESTRUCTIVE: Color = hex(0xbf616a);
pub const BIO: Color = hex(0xc0cdd9);
/// Over the window, under a dialog.
pub const DIM: Color = Color { r: 0, g: 0, b: 0, a: 92 };
/// A row of a list under the pointer, and selected.
pub const ROW_HOVER: Color = hex(0x213647);
pub const ROW_SELECTED: Color = hex(0x2a4256);
pub const ROW_SELECTED_HOVER: Color = hex(0x2e475c);
/// A plain button: white at a tenth over the background; more of it
/// under the pointer, more still held.
pub const BUTTON: Color = hex(0x2d3a42);
pub const BUTTON_HOVER: Color = hex(0x35424a);
pub const BUTTON_ACTIVE: Color = hex(0x454f57);
/// A flat button under the pointer.
pub const FLAT_HOVER: Color = hex(0x1f2d36);

/// A rectangle, in the screen's pixels.
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

    /// A `w` by `h` rectangle centred on this one.
    pub fn centered(&self, w: f32, h: f32) -> R {
        R::new((self.x + (self.w - w) / 2.0).round(), (self.y + (self.h - h) / 2.0).round(), w, h)
    }

    fn rl(&self) -> Rectangle {
        Rectangle { x: self.x, y: self.y, width: self.w, height: self.h }
    }
}

/// The four kinds of text: a page's name; a name over other words; the
/// words under or beside a name, the name's size in blue; plain text.
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

/// How a button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// A plain button, light glyph on a light shade.
    Plain,
    /// Play: in the accent.
    Suggested,
    /// Clear: in red.
    Destructive,
    /// Only the glyph, in red, a shade under the pointer: a playlist's
    /// delete.
    Danger,
}

/// A view that scrolls: where it is, and how tall its content was.
#[derive(Default, Clone, Copy)]
pub struct Scroll {
    pub offset: f32,
    pub content: f32,
    pub view: f32,
}

impl Scroll {
    /// The offset kept within the content.
    pub fn clamp(&mut self) {
        self.offset = self.offset.clamp(0.0, (self.content - self.view).max(0.0));
    }

    /// Scrolled so that `top..bottom` (content coordinates) is in view.
    pub fn reveal(&mut self, top: f32, bottom: f32) {
        if top < self.offset {
            self.offset = top;
        } else if bottom > self.offset + self.view {
            self.offset = bottom - self.view;
        }
    }
}

/// A line of text being typed: the picker's, a playlist's name.
#[derive(Default, Clone)]
pub struct Entry {
    pub text: String,
    /// The caret, as a byte offset.
    pub caret: usize,
}

impl Entry {
    /// A key or a character for the entry; true when it took it.
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
    /// The window, in the screen's pixels.
    pub w: f32,
    pub h: f32,
    pub mouse: Vector2,
    /// The pointer moved since the last frame.
    pub moved: bool,
    pub pressed: bool,
    pub released: bool,
    pub down: bool,
    pub press_at: Vector2,
    /// The scroll this frame, down positive, taken by the first view under
    /// the pointer: a touchpad's (and the fling's) in pixels, a wheel's in
    /// notches.
    wheel: f32,
    notches: f32,
    /// The touchpad's last moves, for the fling's speed when the fingers
    /// lift: when, and how far.
    swipe: Vec<(Instant, f32)>,
    /// The scroll going on after the fingers lifted: pixels a second,
    /// slowing down, and when it was last moved.
    fling: f32,
    fling_at: Instant,
    /// The keys and characters of this frame.
    pub inputs: Vec<Input>,
    consumed: bool,
    /// Under a dialog: no pointer.
    pub blocked: bool,
    clips: Vec<R>,
    alpha: f32,
    hand: bool,
    shown_hand: bool,
    /// What is being dragged (the time bar), until the release.
    pub drag: Option<&'static str>,
    /// The next frame wanted within this many seconds (an animation, a
    /// clock).
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

    /// The pointer where it is now, in the screen's pixels; off the window
    /// far away.
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

    /// Something for a frame to show since the last one: input, the
    /// pointer moved, the window resized.
    pub fn stirred(&self) -> bool {
        let size = unsafe { (rl::GetRenderWidth() as f32, rl::GetRenderHeight() as f32) };
        rl::input_waiting() || Ui::pointer(Ui::screen_scale()) != self.mouse || size != (self.w, self.h)
    }

    /// The input since the last frame; true when there was any.
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
                        // tenths of the touchpad's points
                        let px = -y * 10.0 * scale;
                        self.wheel += px;
                        self.swipe.push((Instant::now(), px));
                    }
                }
                Input::ScrollStop => self.fling = Ui::fling_speed(std::mem::take(&mut self.swipe), scale),
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

    /// Another frame within `seconds`.
    pub fn wake(&mut self, seconds: f64) {
        self.wake_in = Some(self.wake_in.map_or(seconds, |w| w.min(seconds)));
    }

    pub fn seconds(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    // the pointer

    fn clip(&self) -> Option<&R> {
        self.clips.last()
    }

    pub fn hovered(&self, r: R) -> bool {
        !self.blocked && r.contains(self.mouse) && self.clip().is_none_or(|c| c.contains(self.mouse))
    }

    /// A click on `r`: released there, pressed there, not taken yet.
    pub fn click(&mut self, r: R) -> bool {
        if self.released && !self.consumed && self.hovered(r) && r.contains(self.press_at) && self.clip().is_none_or(|c| c.contains(self.press_at)) {
            self.consumed = true;
            return true;
        }
        false
    }

    /// A press on `r`, taken.
    pub fn press(&mut self, r: R) -> bool {
        if self.pressed && !self.consumed && self.hovered(r) {
            self.consumed = true;
            return true;
        }
        false
    }

    /// Held down on `r`, the press begun there.
    pub fn held(&self, r: R) -> bool {
        self.down && !self.blocked && r.contains(self.press_at)
    }

    /// The scroll taken by a view under the pointer, `page` tall, in
    /// pixels: a wheel's notch moves by half again the page's height (in
    /// points) to the two thirds, GTK's step, about a browser's hundred
    /// points; a touchpad by its pixels.
    pub fn take_wheel(&mut self, r: R, page: f32) -> f32 {
        if (self.wheel == 0.0 && self.notches == 0.0) || !self.hovered(r) {
            return 0.0;
        }
        let scale = crate::size::scale();
        let step = 1.5 * (page / scale).max(1.0).powf(2.0 / 3.0) * scale;
        std::mem::take(&mut self.notches) * step + std::mem::take(&mut self.wheel)
    }

    /// The fling's speed, in pixels a second, from the touchpad's moves
    /// in the last tenth of a second before the fingers lifted; none
    /// when they had slowed down to a crawl.
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

    /// The fling moved on since its last frame, slowing down by the time
    /// it has run (it loses two thirds of its speed every third of a
    /// second), until it is too slow to see.
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

    /// The pointer a hand this frame: over a link.
    pub fn set_hand(&mut self) {
        self.hand = true;
    }

    // drawing

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

    pub fn circle(&self, center: Vector2, radius: f32, c: Color) {
        unsafe { rl::DrawCircleV(center, radius, self.tint(c)) };
    }

    /// A picture in `r`, whole, centred on the shade.
    pub fn picture(&self, t: Texture, r: R) {
        let (tw, th) = (t.width as f32, t.height as f32);
        let k = (r.w / tw).min(r.h / th);
        let (w, h) = ((tw * k).round(), (th * k).round());
        if w < r.w || h < r.h {
            self.rect(r, SHADE);
        }
        let dst = r.centered(w, h);
        unsafe { rl::DrawTexturePro(t, Rectangle { x: 0.0, y: 0.0, width: tw, height: th }, dst.rl(), Vector2::default(), 0.0, self.tint(rl::Color { r: 255, g: 255, b: 255, a: 255 })) };
    }

    /// A glyph centred on a square of the cover's shade: what a picture
    /// shows while it has nothing better; round for a face.
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

    /// A line's height in a style.
    pub fn line(&self, style: Style) -> f32 {
        self.face(style).line
    }

    /// Text in a style, cut to `max` pixels; returns its width.
    pub fn text(&self, style: Style, s: &str, x: f32, y: f32, max: f32) -> f32 {
        self.text_in(style, s, x, y, max, style.color())
    }

    pub fn text_in(&self, style: Style, s: &str, x: f32, y: f32, max: f32, color: Color) -> f32 {
        let face = self.face(style);
        let s = face.ellipsize(s, max);
        face.draw(&s, x, y, self.tint(color))
    }

    /// Text right-aligned to `right`.
    pub fn text_right(&self, style: Style, s: &str, right: f32, y: f32) -> f32 {
        let face = self.face(style);
        let w = face.width(s);
        face.draw(s, right - w, y, self.tint(style.color()));
        w
    }

    pub fn width(&self, style: Style, s: &str) -> f32 {
        self.face(style).width(s)
    }

    /// Text made a link: the hand, an underline under the pointer; true
    /// on a click. Its width drawn in the second place.
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

    /// A glyph by its ink in the middle of `r`, at `em` rem.
    pub fn icon(&self, glyph: &str, r: R, em: f32, c: Color) {
        let face = self.fonts.icons(em * crate::size::px());
        face.draw_centered(glyph, r.center(), self.tint(c));
    }

    /// A round button filling `r`, only its glyph at `em` rem; true on a
    /// click.
    pub fn round_button(&mut self, r: R, glyph: &str, em: f32, look: Look, sensitive: bool) -> bool {
        self.button(r, None, glyph, em, look, sensitive)
    }

    /// A round button's square sister, its corners rounded half a rem,
    /// the tabs'.
    pub fn square_button(&mut self, r: R, glyph: &str, em: f32, look: Look, sensitive: bool) -> bool {
        self.button(r, Some(rem(0.5)), glyph, em, look, sensitive)
    }

    /// A button: round without a corner's radius, square with one.
    fn button(&mut self, r: R, corner: Option<f32>, glyph: &str, em: f32, look: Look, sensitive: bool) -> bool {
        let over = sensitive && self.hovered(r);
        let held = over && self.held(r);
        let (bg, fg) = match look {
            Look::Plain => (Some(if held { BUTTON_ACTIVE } else if over { BUTTON_HOVER } else { BUTTON }), FG),
            Look::Suggested => (Some(if held { hex(0x3f7199) } else if over { hex(0x5a92bb) } else { ACCENT_BG }), BG),
            Look::Destructive => (Some(if held { hex(0xa9545c) } else if over { hex(0xc86e77) } else { DESTRUCTIVE }), hex(0xffffff)),
            Look::Danger => (if held { Some(BUTTON_HOVER) } else if over { Some(FLAT_HOVER) } else { None }, DESTRUCTIVE),
        };
        self.with_alpha(if sensitive { 1.0 } else { 0.5 }, |ui| {
            match (bg, corner) {
                (Some(bg), Some(corner)) => ui.rounded(r, corner, bg),
                (Some(bg), None) => ui.circle(r.center(), r.w / 2.0, bg),
                (None, _) => {}
            }
            ui.icon(glyph, r, em, fg);
        });
        sensitive && self.click(r)
    }

    /// A switch with the glyph of what it switches in its knob: a track,
    /// the knob at its start while off and at its end, in the accent,
    /// while on. Its size: `switch_size`. True on a click.
    pub fn switch(&mut self, at: Vector2, glyph: &str, on: bool, sensitive: bool) -> bool {
        let (w, h) = switch_size();
        let r = R::new(at.x, at.y, w, h);
        let over = sensitive && self.hovered(r);
        self.with_alpha(if sensitive { 1.0 } else { 0.5 }, |ui| {
            let track = if on { ACCENT_BG } else if over { HOVER } else { SHADE };
            ui.rounded(r, h / 2.0, track);
            let pad = rem(0.25);
            let knob = rem(1.5);
            let x = if on { r.right() - pad - knob } else { r.x + pad };
            let k = R::new(x, r.y + pad, knob, knob);
            ui.circle(k.center(), knob / 2.0, FG);
            ui.icon(glyph, k, 1.0, BG);
        });
        sensitive && self.click(r)
    }
}

/// A switch's track: three rem by one and a half, a quarter around.
pub fn switch_size() -> (f32, f32) {
    (rem(3.0) + 2.0 * rem(0.25), rem(1.5) + 2.0 * rem(0.25))
}
