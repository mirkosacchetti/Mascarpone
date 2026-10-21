//! The picker: lintel's launcher with music instead of apps. On Ctrl+F
//! what is on screen: the tab's albums, artists or playlists; on the
//! Queue tab albums and tracks to append; on an artist's page their
//! albums and tracks (a track opens its album with the track selected,
//! so Enter again plays it); on an album's or a playlist's page its
//! tracks (Enter selects the row there). On `/` everything. On `a` over
//! a track, the playlists to add it to. On Ctrl+K the commands, each with
//! its keys in blue, done when picked. Typing filters (fuzzy, the name
//! first, then the detail), Up/Down/Tab and Ctrl+n/p/k/l move, Enter
//! opens the page, Escape closes (the keys are the configuration's).
//! Eight rows at a time, the selection scrolls them.
//!
//! The other dialogs live here too, the picker's look: the name to save
//! the queue as, and the question before a playlist is deleted.

use std::path::PathBuf;
use std::sync::Arc;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

use crate::app::{Action, App, Page};
use crate::glyphs;
use crate::library::Library;
use crate::pages::count;
use crate::rl::{self, Input};
use crate::size::rem;
use crate::ui::{self, Entry, Style, Ui, R};

const ROWS: usize = 8;
/// The dialog's width, in rem.
const WIDTH: f32 = 38.0;

/// What the picker lists.
#[derive(Clone, PartialEq, Eq)]
pub enum Kind {
    /// The tabs' lists.
    Albums,
    Artists,
    Playlists,
    /// An artist's albums and tracks together.
    Artist(usize),
    /// The playlists, to add this track to one, or to a new one named
    /// by what is typed.
    AddTo(String),
    /// Albums, and every track once something is typed, to append to
    /// the queue (on the Queue tab).
    ToQueue,
    /// The rows of the tracks page on screen (an album, a playlist):
    /// Enter selects the row there, so a second Enter plays it.
    Tracks,
    /// The commands, each with its keys: picked, done.
    Commands,
    /// Everything, from `/`: albums, artists, playlists, and every track
    /// once something is typed. Enter opens the page; on a track, its
    /// album's with the track selected, so a second Enter plays it.
    All,
}

/// What a line opens: indexes into the library.
#[derive(Clone, Copy)]
enum Target {
    Album(usize),
    Artist(usize),
    Playlist(usize),
    /// The album's page with the track selected.
    Track(usize, usize),
    /// A row of the tracks page on screen.
    Row(usize),
    Command(crate::config::Shortcut),
}

/// One line: the name, what goes after it in blue, where it leads. A
/// track among the albums shows only once something is typed.
struct Line {
    name: String,
    detail: String,
    target: Target,
    typed_only: bool,
}

/// The line to make a new playlist, shown under the matches of `AddTo`
/// when the text names none.
const NEW: usize = usize::MAX;

pub struct Picker {
    library: Arc<Library>,
    kind: Kind,
    prompt: &'static str,
    lines: Vec<Line>,
    entry: Entry,
    matcher: SkimMatcherV2,
    /// Indexes into `lines` that match, best first; the selected one; the
    /// first row shown.
    shown: Vec<usize>,
    selected: usize,
    top: usize,
}

impl Picker {
    pub fn open(app: &App, kind: Kind) -> Option<Picker> {
        // the commands need no library
        let library = match (app.library.clone(), &kind) {
            (Some(l), _) => l,
            (None, Kind::Commands) => Arc::new(Library::default()),
            (None, _) => return None,
        };
        let album_lines = || library.albums.iter().enumerate().map(|(index, a)| Line { name: a.title.clone(), detail: a.artist.clone(), target: Target::Album(index), typed_only: false });
        let artist_lines = || library.artists.iter().enumerate().map(|(index, a)| Line { name: a.name.clone(), detail: count(a.albums.len(), glyphs::ALBUM), target: Target::Artist(index), typed_only: false });
        let playlist_lines = || -> Vec<Line> { library.playlists().iter().enumerate().map(|(index, p)| Line { name: p.name.clone(), detail: count(p.entries.len(), glyphs::TRACK), target: Target::Playlist(index), typed_only: false }).collect() };
        // every track, shown once something is typed
        let track_lines = || {
            library.albums.iter().enumerate().flat_map(|(a, album)| {
                album.tracks.iter().enumerate().map(move |(t, track)| Line { name: track.title.clone(), detail: format!("{}  ·  {}", track.artist, album.title), target: Target::Track(a, t), typed_only: true })
            })
        };
        let lines: Vec<Line> = match &kind {
            Kind::Albums => album_lines().collect(),
            Kind::ToQueue => album_lines().chain(track_lines()).collect(),
            Kind::All => album_lines().chain(artist_lines()).chain(playlist_lines()).chain(track_lines()).collect(),
            Kind::Artists => artist_lines().collect(),
            Kind::Playlists | Kind::AddTo(_) => playlist_lines(),
            Kind::Commands => crate::config::commands()
                .into_iter()
                .map(|(shortcut, title, keys)| Line { name: title.to_owned(), detail: keys, target: Target::Command(shortcut), typed_only: false })
                .collect(),
            Kind::Tracks => match app.pages.last() {
                Some(Page::Album(t) | Page::Playlist(t)) => t.lines().into_iter().map(|(name, detail, i)| Line { name, detail, target: Target::Row(i), typed_only: false }).collect(),
                _ => Vec::new(),
            },
            Kind::Artist(artist) => {
                let artist = &library.artists[*artist];
                let albums = artist.albums.iter().map(|&i| {
                    let a = &library.albums[i];
                    Line { name: a.title.clone(), detail: a.artist.clone(), target: Target::Album(i), typed_only: false }
                });
                let tracks = artist.albums.iter().flat_map(|&i| {
                    let album = &library.albums[i];
                    album.tracks.iter().enumerate().map(move |(t, track)| Line { name: track.title.clone(), detail: album.title.clone(), target: Target::Track(i, t), typed_only: false })
                });
                albums.chain(tracks).collect()
            }
        };
        let prompt = match kind {
            Kind::Albums => "Album",
            Kind::Artists => "Artist",
            Kind::Playlists => "Playlist",
            Kind::Artist(_) => "Album or track",
            Kind::Tracks => "Track",
            Kind::AddTo(_) => "Add to playlist",
            Kind::ToQueue => "Add to queue",
            Kind::All => "Search",
            Kind::Commands => "Command",
        };
        let mut picker = Picker { library, kind, prompt, lines, entry: Entry::default(), matcher: SkimMatcherV2::default().ignore_case(), shown: Vec::new(), selected: 0, top: 0 };
        picker.refilter();
        Some(picker)
    }

    /// The lines that match the text, best first: the name counts most,
    /// and more when the text is its start or a word's start; the detail
    /// next. Empty text lists the library in its order.
    fn refilter(&mut self) {
        let text = self.entry.text.to_lowercase();
        let text = text.trim();
        let mut scored: Vec<(i64, usize)> = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            if text.is_empty() {
                if !line.typed_only {
                    scored.push((0, i));
                }
                continue;
            }
            // a track weighs less than an album of the same name
            let weight = if line.typed_only { 40 } else { 100 };
            let name = self.matcher.fuzzy_match(&line.name, text).map(|s| s + weight + start_bonus(&line.name, text));
            let detail = self.matcher.fuzzy_match(&line.detail, text).map(|s| s + weight / 2 + start_bonus(&line.detail, text) / 2);
            if let Some(best) = name.into_iter().chain(detail).max() {
                scored.push((best, i));
            }
        }
        // stable: equal scores keep the library's order
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        self.shown = scored.into_iter().map(|(_, i)| i).collect();
        if matches!(self.kind, Kind::AddTo(_)) && !text.is_empty() && !self.lines.iter().any(|l| l.name.to_lowercase() == text) {
            self.shown.push(NEW);
        }
        self.selected = 0;
        self.top = 0;
    }

    fn step(&mut self, by: i64) {
        let n = self.shown.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected as i64 + by).clamp(0, n as i64 - 1) as usize;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + ROWS {
            self.top = self.selected + 1 - ROWS;
        }
    }

    /// The selected line done; false when nothing is to be done.
    fn pick(&self, app: &App) -> bool {
        let Some(&i) = self.shown.get(self.selected) else { return false };
        if let Kind::AddTo(uri) = &self.kind {
            let path = if i == NEW {
                let path = crate::master::path_for(self.entry.text.trim());
                if let Err(e) = crate::master::write_playlist(&path, &[]) {
                    eprintln!("{}: {e}", path.display());
                    return false;
                }
                path
            } else {
                let Target::Playlist(p) = self.lines[i].target else { return false };
                let Some(path) = self.library.playlists().get(p).map(|p| p.path.clone()) else { return false };
                path
            };
            app.act(Action::AddToPlaylist(path, vec![uri.clone()]));
            return true;
        }
        if self.kind == Kind::ToQueue {
            let uris = match self.lines[i].target {
                Target::Album(a) => self.library.albums[a].uris(),
                Target::Track(a, t) => vec![self.library.albums[a].tracks[t].uri.clone()],
                _ => return false,
            };
            app.act(Action::Enqueue(uris));
            return true;
        }
        let action = match self.lines[i].target {
            Target::Command(shortcut) => Action::Run(shortcut),
            Target::Row(row) => Action::SelectRow(row),
            Target::Album(a) => Action::OpenAlbum { key: self.library.albums[a].key.clone(), track: 0 },
            Target::Track(a, t) => Action::OpenAlbum { key: self.library.albums[a].key.clone(), track: t },
            Target::Artist(a) => Action::OpenArtist(self.library.artists[a].name.clone()),
            Target::Playlist(p) => match self.library.playlists().get(p) {
                Some(p) => Action::OpenPlaylist(p.path.clone()),
                None => return false,
            },
        };
        app.act(action);
        true
    }

    /// A key; false when the picker closes.
    fn key(&mut self, app: &App, input: &Input) -> bool {
        use crate::config::Shortcut as S;
        // characters are the entry's: only keys move and close
        if let Input::Key { code, mods } = *input {
            let keys = [S::PickerOpen, S::PickerClose, S::PickerNext, S::PickerPrevious, S::PickerPageDown, S::PickerPageUp];
            match crate::config::get().which(&keys, input) {
                Some(S::PickerOpen) => return !self.pick(app),
                Some(S::PickerClose) => return false,
                Some(S::PickerNext) => self.step(1),
                Some(S::PickerPrevious) => self.step(-1),
                Some(S::PickerPageDown) => self.step(5),
                Some(S::PickerPageUp) => self.step(-5),
                // not the window's tab switch under the dialog
                _ if mods & rl::MOD_CONTROL != 0 && code == rl::KEY_TAB => {}
                _ => return self.type_in(input),
            }
            return true;
        }
        self.type_in(input)
    }

    /// A key or a character for the entry: the lines filtered again.
    fn type_in(&mut self, input: &Input) -> bool {
        let before = self.entry.text.clone();
        if self.entry.input(input) && self.entry.text != before {
            self.refilter();
        }
        true
    }

    fn draw(&mut self, app: &App, ui: &mut Ui) -> bool {
        let pad = rem(0.5);
        let row_h = rem(2.0).max(rem(1.75)) + 2.0 * rem(0.25);
        let head_h = ui.line(Style::Header) + 2.0 * rem(0.5);
        let w = rem(WIDTH).min(ui.w - 2.0 * rem(1.0));
        let h = 2.0 * pad + head_h + rem(0.5) + ROWS as f32 * row_h;
        let r = R::new(0.0, 0.0, ui.w, ui.h).centered(w, h);
        ui.rect(r, ui::BG);
        head(ui, R::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, head_h), self.prompt, &self.entry, "");

        let mut y = r.y + pad + head_h + rem(0.5);
        let mut picked = false;
        for n in 0..ROWS {
            let pos = self.top + n;
            let row = R::new(r.x + pad, y, r.w - 2.0 * pad, row_h);
            y += row_h;
            let Some(&i) = self.shown.get(pos) else { continue };
            if pos == self.selected {
                ui.rect(row, ui::SHADE);
            }
            let inner = row.inset(rem(0.5), rem(0.25));
            let mid = inner.y + inner.h / 2.0;
            let lh = ui.line(Style::Label);
            let ty = (mid - lh / 2.0).round();
            let mut x = inner.x;
            let mark = |ui: &Ui, glyph: &str, x: &mut f32| {
                ui.icon(glyph, R::new(*x, ty, lh, lh), 1.0, ui::ACCENT);
                *x += lh + rem(0.5);
            };
            let pic = rem(2.0);
            let (name, detail) = if i == NEW {
                mark(ui, glyphs::ADD, &mut x);
                (format!("New playlist \"{}\"", self.entry.text.trim()), String::new())
            } else {
                let line = &self.lines[i];
                match line.target {
                    Target::Album(a) | Target::Track(a, _) => {
                        // what the line is, a disc or a note, then the cover
                        mark(ui, if matches!(line.target, Target::Track(..)) { glyphs::TRACK } else { glyphs::ALBUM }, &mut x);
                        let album = &self.library.albums[a];
                        let pr = R::new(x, (mid - pic / 2.0).round(), pic, pic);
                        match app.pictures.cover_now(&album.key, album.cover_uri()) {
                            Some(t) => ui.picture(t, pr),
                            None => ui.placeholder(glyphs::ALBUM, pr, false),
                        }
                        x += pic + rem(0.5);
                    }
                    Target::Artist(a) => {
                        let pr = R::new(x, (mid - pic / 2.0).round(), pic, pic);
                        match app.pictures.face(&self.library.artists[a].name) {
                            Some(t) => {
                                ui.circle(pr.center(), pic / 2.0, ui::SHADE);
                                ui.picture(t, pr);
                            }
                            None => ui.placeholder(glyphs::ARTIST, pr, true),
                        }
                        x += pic + rem(0.5);
                    }
                    Target::Playlist(_) => mark(ui, glyphs::PLAYLIST, &mut x),
                    Target::Row(_) => mark(ui, glyphs::TRACK, &mut x),
                    Target::Command(_) => mark(ui, glyphs::COMMAND, &mut x),
                }
                (line.name.clone(), line.detail.clone())
            };
            let end = inner.right();
            let nw = ui.text(Style::Label, &name, x, ty, end - x);
            x += nw + rem(0.5) + rem(0.5);
            if x < end && !detail.is_empty() {
                ui.text(Style::Subtitle, &detail, x, ty, end - x);
            }
            if ui.click(row) {
                self.selected = pos;
                picked = true;
            }
        }
        if picked && self.pick(app) {
            return false;
        }
        // a click outside closes it
        ui.click(r);
        !ui.click(R::new(0.0, 0.0, ui.w, ui.h))
    }
}

/// The text as the name's start, or a word's start in it.
fn start_bonus(name: &str, text: &str) -> i64 {
    let name = name.to_lowercase();
    if name.starts_with(text) {
        150
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(text)) {
        100
    } else {
        0
    }
}

/// A dialog's head: the prompt in the accent, the entry after it with
/// its caret.
fn head(ui: &mut Ui, r: R, prompt: &str, entry: &Entry, placeholder: &str) {
    let lh = ui.line(Style::Header);
    let y = r.y + rem(0.5);
    let x = r.x + rem(0.5);
    let pw = ui.text_in(Style::Header, prompt, x, y, f32::MAX, ui::ACCENT);
    let ex = x + pw + rem(0.5);
    let room = r.right() - rem(0.5) - ex;
    let face = ui.face(Style::Header);
    let caret = face.width(&entry.text[..entry.caret]);
    // the text slides left when the caret would go past the end
    let shift = (caret - room).max(0.0);
    let clip = R::new(ex, r.y, room.max(0.0), r.h);
    if entry.text.is_empty() && !placeholder.is_empty() {
        ui.text_in(Style::Header, placeholder, ex, y, room, rl::Color { a: 128, ..ui::FG });
    } else {
        ui.push_clip(clip);
        ui.text(Style::Header, &entry.text, ex - shift, y, f32::MAX);
        ui.pop_clip();
    }
    let cw = crate::size::scale().round().max(1.0);
    ui.rect(R::new((ex + caret - shift).round(), y, cw, lh), ui::FG);
}

/// A dialog up over the window.
pub enum Modal {
    Picker(Box<Picker>),
    /// The name to save the queue as.
    Name(Entry),
    /// Asked before a playlist is deleted: its file, its name.
    Delete(PathBuf, String),
}

impl Modal {
    /// The name to save the queue as, with a name proposed (the radio's),
    /// the caret at its end.
    pub fn name(proposed: Option<String>) -> Modal {
        let text = proposed.unwrap_or_default();
        Modal::Name(Entry { caret: text.len(), text })
    }

    pub fn delete(path: PathBuf, name: String) -> Modal {
        Modal::Delete(path, name)
    }

    /// A key for the dialog; false when it closes.
    pub fn key(&mut self, app: &App, input: &Input) -> bool {
        match self {
            Modal::Picker(p) => p.key(app, input),
            Modal::Name(entry) => match crate::config::get().which(&[crate::config::Shortcut::PickerClose, crate::config::Shortcut::PickerOpen], input) {
                Some(crate::config::Shortcut::PickerClose) => false,
                Some(_) => {
                    let name = entry.text.trim().to_owned();
                    if name.is_empty() {
                        return true;
                    }
                    app.act(Action::SaveQueueAs(name));
                    false
                }
                _ => {
                    entry.input(input);
                    true
                }
            },
            // open is Cancel, the default
            Modal::Delete(..) => crate::config::get().which(&[crate::config::Shortcut::PickerClose, crate::config::Shortcut::PickerOpen], input).is_none(),
        }
    }

    /// Drawn over the dimmed window; false when it closes.
    pub fn draw(&mut self, app: &App, ui: &mut Ui) -> bool {
        match self {
            Modal::Picker(p) => p.draw(app, ui),
            Modal::Name(entry) => {
                let pad = rem(0.5);
                let head_h = ui.line(Style::Header) + 2.0 * rem(0.5);
                let w = rem(WIDTH).min(ui.w - 2.0 * rem(1.0));
                let r = R::new(0.0, 0.0, ui.w, ui.h).centered(w, head_h + 2.0 * pad);
                ui.rect(r, ui::BG);
                head(ui, R::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, head_h), "Save the queue as", entry, "Playlist name");
                ui.click(r);
                !ui.click(R::new(0.0, 0.0, ui.w, ui.h))
            }
            Modal::Delete(path, name) => {
                let w = rem(26.0).min(ui.w - 2.0 * rem(1.0));
                let pad = rem(1.5);
                let inner = w - 2.0 * pad;
                let heading = ui.face(Style::Title).wrap(&format!("Delete {name}?"), inner, 0);
                let body = ui.face(Style::Label).wrap("The file is moved to ~/.local/state/mascarpone/deleted, where it can be taken back from.", inner, 0);
                let (lt, ll) = (ui.line(Style::Title), ui.line(Style::Label));
                let button = rem(2.75);
                let h = pad + heading.len() as f32 * lt + rem(0.75) + body.len() as f32 * ll + pad + button + pad;
                let r = R::new(0.0, 0.0, ui.w, ui.h).centered(w, h);
                ui.rect(r, ui::BG);
                let mut y = r.y + pad;
                for (style, lines, lh) in [(Style::Title, &heading, lt), (Style::Label, &body, ll)] {
                    for line in lines {
                        let lw = ui.width(style, line);
                        ui.text(style, line, (r.x + (r.w - lw) / 2.0).round(), y, inner + 1.0);
                        y += lh;
                    }
                    y += rem(0.75);
                }
                let y = r.bottom() - pad - button;
                let bw = ((inner - rem(0.75)) / 2.0).floor();
                let mut keep = true;
                for (i, (label, color)) in [("Cancel", ui::FG), ("Delete", ui::DESTRUCTIVE)].into_iter().enumerate() {
                    let b = R::new(r.x + pad + i as f32 * (bw + rem(0.75)), y, bw, button);
                    let over = ui.hovered(b);
                    ui.rounded(b, rem(ui::CORNER), if over && ui.held(b) { ui::BUTTON_ACTIVE } else if over { ui::BUTTON_HOVER } else { ui::BUTTON });
                    let lw = ui.width(Style::Header, label);
                    ui.text_in(Style::Header, label, (b.x + (b.w - lw) / 2.0).round(), (b.y + (b.h - ui.line(Style::Header)) / 2.0).round(), bw, color);
                    if ui.click(b) {
                        keep = false;
                        if i == 1 {
                            app.act(Action::Delete(path.clone()));
                        }
                    }
                }
                ui.click(r);
                keep && !ui.click(R::new(0.0, 0.0, ui.w, ui.h))
            }
        }
    }
}
