//! The pages with a list of tracks (an album, a playlist, the queue's
//! tab) and the artist's page. A tracks page: a picture when there is
//! one, title, subtitle and facts, Play and Add to queue, then the rows,
//! one selected. Play, and Enter anywhere on the page, plays the list
//! from the selected row; when that row is the song on, it pauses and
//! resumes instead, and reads Pause. Up and Down (l and k) move the
//! selection over the playable rows; a click on a row plays from there,
//! on the song on it pauses and resumes too; `a` adds the selected track
//! to a playlist. The song on shows the playing mark in place of its
//! number.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::app::{Action, App, Grid};
use crate::glyphs;
use crate::library::{self, Library, Playlist};
use crate::master::Source;
use crate::mpd::Command;
use crate::picker::{Kind, Modal};
use crate::radio::Seed;
use crate::rl::{self, Input};
use crate::size::{self, rem};
use crate::ui::{self, Look, Style, Ui, R};
use crate::view;

/// A number after the glyph of what it counts: no words.
pub fn count(n: usize, glyph: &str) -> String {
    format!("{glyph} {n}")
}

/// A whole length after the clock's glyph.
pub fn length(d: Duration) -> String {
    format!("{} {}", glyphs::LENGTH, library::clock(d))
}

/// What a list page's Play does.
#[derive(PartialEq, Eq)]
pub enum Plays {
    /// Loads the list on the queue, the master pointing to the source.
    Load(Source),
    /// The page is the queue itself: plays it from the row.
    Queue,
}

/// A track's row: its number, the title, under it the artist and the
/// album as links to their pages (or a line of plain text, for a file
/// the library lacks), the length.
pub struct Row {
    pub number: String,
    pub title: String,
    pub artist: Option<String>,
    /// The album's index in the page's library and the track's place in
    /// it, for the album's link.
    pub album: Option<(usize, usize)>,
    pub plain: Option<String>,
    pub duration: Option<Duration>,
    /// Its place among the playable rows; none for a file the library
    /// does not have.
    pub start: Option<usize>,
}

pub struct Tracks {
    pub library: Option<Arc<Library>>,
    pub plays: Plays,
    /// The album's cover: its key and the file mpd is asked it for.
    pub cover: Option<(String, String)>,
    pub title: String,
    pub subtitle: String,
    /// The subtitle a link to this artist's page.
    pub to_artist: Option<String>,
    pub facts: String,
    /// A playlist's shuffle, after the buttons.
    pub shuffle: Option<(PathBuf, bool)>,
    /// A playlist's delete, at the bottom.
    pub delete: Option<(PathBuf, String)>,
    /// The queue's save as a playlist, with the name proposed: the
    /// radio's queue, "Radio" and what it started from.
    pub save: Option<String>,
    /// What the radio starts from on the head's button: the album.
    pub seed: Option<Seed>,
    pub rows: Vec<Row>,
    /// The playable rows' uris, in order.
    pub uris: Vec<String>,
    pub selected: Option<usize>,
    pub scroll: ui::Scroll,
    /// The selected row scrolled into view at the next frame.
    pub reveal: bool,
    /// The first time in the middle of the view, not just at its edge.
    pub centre: bool,
    /// The rows' top and bottom in the content, as last laid out.
    tops: Vec<(f32, f32)>,
}

impl Tracks {
    fn new(library: Option<Arc<Library>>, plays: Plays, title: String, subtitle: String, facts: String, rows: Vec<Row>, selected: usize) -> Tracks {
        let uris = Vec::new();
        let mut t = Tracks {
            library,
            plays,
            cover: None,
            title,
            subtitle,
            to_artist: None,
            facts,
            shuffle: None,
            delete: None,
            save: None,
            seed: None,
            rows,
            uris,
            selected: None,
            scroll: ui::Scroll::default(),
            reveal: true,
            centre: true,
            tops: Vec::new(),
        };
        t.selected = t.rows.get(selected).filter(|r| r.start.is_some()).map(|_| selected);
        t
    }

    /// The album: cover, facts, the tracks, the one at `selected`
    /// selected.
    pub fn album(library: &Arc<Library>, index: usize, selected: usize) -> Tracks {
        let album = &library.albums[index];
        let mut facts = Vec::new();
        if album.loose {
            facts.push(glyphs::FOLDER.to_owned());
        }
        facts.push(count(album.tracks.len(), glyphs::TRACK));
        facts.push(length(album.duration()));
        if let Some(year) = &album.year {
            facts.push(format!("{} {year}", glyphs::YEAR));
        }
        let discs = album.discs();
        let rows = album
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| Row {
                number: if discs > 1 && track.disc > 0 { format!("{}.{}", track.disc, track.number) } else { track.number.to_string() },
                title: track.title.clone(),
                // the artist only when not the album's; no link to the
                // album the page is
                artist: (track.artist != album.artist).then(|| track.artist.clone()),
                album: None,
                plain: None,
                duration: track.duration,
                start: Some(i),
            })
            .collect();
        let mut t = Tracks::new(Some(library.clone()), Plays::Load(Source::Album { key: album.key.clone() }), album.title.clone(), album.artist.clone(), facts.join("  ·  "), rows, selected);
        t.uris = album.uris();
        t.cover = Some((album.key.clone(), album.cover_uri().to_owned()));
        t.to_artist = Some(album.artist.clone());
        t.seed = Some(Seed::Album(album.key.clone()));
        t
    }

    /// The playlist: its facts and its entries, the ones the library
    /// misses greyed out and not playable; shuffle after the buttons,
    /// delete at the bottom.
    pub fn playlist(library: &Arc<Library>, playlist: &Playlist) -> Tracks {
        let mut facts = vec![count(playlist.entries.len(), glyphs::TRACK), length(playlist.duration())];
        let missing = playlist.missing();
        if missing > 0 {
            facts.push(count(missing, glyphs::MISSING));
        }
        let mut playable = 0;
        let rows = playlist
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let start = entry.found.then(|| {
                    playable += 1;
                    playable - 1
                });
                Row {
                    number: (i + 1).to_string(),
                    title: entry.title.clone(),
                    artist: entry.found.then(|| entry.artist.clone()).filter(|a| !a.is_empty()),
                    album: if entry.found { entry.album.and_then(|a| library.place(&entry.uri).map(|(_, t)| (a, t))) } else { None },
                    plain: (!entry.found).then(|| entry.uri.clone()),
                    duration: entry.duration,
                    start,
                }
            })
            .collect();
        let mut t = Tracks::new(Some(library.clone()), Plays::Load(Source::Playlist { path: playlist.path.clone() }), playlist.name.clone(), String::new(), facts.join("  ·  "), rows, 0);
        t.uris = playlist.uris();
        t.shuffle = Some((playlist.path.clone(), crate::master::shuffled(&playlist.path)));
        t.delete = Some((playlist.path.clone(), playlist.name.clone()));
        t
    }

    /// The master, for its tab: the queue as mpd has it, with where it
    /// came from, the song on selected (or the row selected before); a
    /// row plays from there. Every row has keys to move it up and down
    /// and to remove it (L, K, d or Delete on the selected one); on a
    /// radio, Save writes the queue as a playlist file ("Radio" and what
    /// it started from, to be changed), Clear empties it.
    pub fn queue(app: &App) -> Tracks {
        let library = app.library.clone();
        let (title, subtitle) = match &library {
            Some(l) => app.master.describe(l),
            None => ("Queue".into(), String::new()),
        };
        let uris = app.master.uris.clone();
        let current = app.current().0;
        let mut on = 0;
        let mut whole = Duration::ZERO;
        let rows: Vec<Row> = uris
            .iter()
            .enumerate()
            .map(|(i, uri)| {
                if current == Some(uri.as_str()) {
                    on = i;
                }
                let place = library.as_ref().and_then(|l| l.place(uri));
                let row = match (&library, place) {
                    (Some(l), Some((a, t))) => {
                        let track = &l.albums[a].tracks[t];
                        Row { number: (i + 1).to_string(), title: track.title.clone(), artist: Some(track.artist.clone()).filter(|a| !a.is_empty()), album: Some((a, t)), plain: None, duration: track.duration, start: Some(i) }
                    }
                    _ => Row {
                        number: (i + 1).to_string(),
                        title: Path::new(uri).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                        artist: None,
                        album: None,
                        plain: Some(uri.clone()),
                        duration: None,
                        start: Some(i),
                    },
                };
                whole += row.duration.unwrap_or_default();
                row
            })
            .collect();
        let n = rows.len();
        let facts = format!("{}  ·  {}", count(n, glyphs::TRACK), length(whole));
        // the selection: the one from before a rebuild, else the song on
        let selected = app.queue_row.filter(|&r| r < n).unwrap_or(on);
        let mut t = Tracks::new(library, Plays::Queue, title, subtitle, facts, rows, selected);
        t.uris = uris;
        // a playlist's shuffle, as on its page; a radio's save
        if let crate::master::Source::Playlist { path } = &app.master.source {
            t.shuffle = Some((path.clone(), crate::master::shuffled(path)));
        }
        t.save = t.library.as_ref().and_then(|l| app.master.radio_name(l));
        t
    }

    fn is_queue(&self) -> bool {
        matches!(self.plays, Plays::Queue)
    }

    /// The selected row's start and uri, if it plays.
    fn selection(&self) -> Option<(usize, &str)> {
        let start = self.rows.get(self.selected?)?.start?;
        Some((start, self.uris[start].as_str()))
    }

    /// The selected row's track, if it plays.
    pub fn selected_uri(&self) -> Option<String> {
        self.selection().map(|(_, uri)| uri.to_owned())
    }

    /// A row selected (the picker's), the one that plays on Enter here.
    pub fn select(&mut self, i: usize) {
        if self.rows.get(i).is_some_and(|r| r.start.is_some()) {
            self.selected = Some(i);
            self.reveal = true;
        }
    }

    /// The playable rows as the picker lists them: title, detail, index.
    pub fn lines(&self) -> Vec<(String, String, usize)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.start.is_some())
            .map(|(i, r)| {
                let album = r.album.and_then(|(a, _)| self.library.as_ref().map(|l| l.albums[a].title.clone()));
                let detail: Vec<String> = r.artist.clone().into_iter().chain(album).chain(r.plain.clone()).collect();
                (r.title.clone(), detail.join("  ·  "), i)
            })
            .collect()
    }

    /// Plays the list from a playable row: loaded from there, or the
    /// queue from there.
    fn play_from(&self, app: &App, start: usize) {
        match &self.plays {
            Plays::Load(source) => app.act(Action::Load { source: source.clone(), uris: self.uris.clone(), start }),
            Plays::Queue => app.act(Action::Mpd(Command::PlayPos(start))),
        }
    }

    /// Plays the selected row, or pauses and resumes when it is the song
    /// on.
    fn play_selected(&self, app: &App) {
        if self.uris.is_empty() {
            return;
        }
        let (start, uri) = self.selection().unwrap_or((0, self.uris[0].as_str()));
        if app.current().0 == Some(uri) {
            app.act(Action::Mpd(Command::Toggle));
        } else {
            self.play_from(app, start);
        }
    }

    /// The selection moved over the playable rows.
    fn step(&mut self, app: &App, by: i64) {
        let mut i = self.selected.map_or(-1, |s| s as i64) + by;
        while i >= 0 && (i as usize) < self.rows.len() {
            if self.rows[i as usize].start.is_some() {
                self.selected = Some(i as usize);
                self.reveal = true;
                if self.is_queue() {
                    app.act(Action::QueueRow(self.selected));
                }
                return;
            }
            i += by;
        }
    }

    /// The queue's edits, by position: the queue comes back changed from
    /// mpd and the tab is built again, the selection kept.
    fn remove(&self, app: &App, i: usize) {
        app.act(Action::QueueRow(Some(i.min(self.rows.len().saturating_sub(2)))));
        app.act(Action::Mpd(Command::Remove(i)));
    }

    fn shift(&self, app: &App, i: usize, by: i64) {
        let n = self.rows.len() as i64;
        let to = (i as i64 + by).clamp(0, n - 1) as usize;
        if to != i {
            app.act(Action::QueueRow(Some(to)));
            app.act(Action::Mpd(Command::Move { from: i, to }));
        }
    }
}

/// A tracks page's keys (the configuration's): the queue's edits (out,
/// down, up), play, add to a playlist, the selection up and down.
pub fn tracks_key(app: &App, t: &mut Tracks, input: &Input) {
    use crate::config::Shortcut as S;
    let queue = t.is_queue();
    let keys = [S::Remove, S::MoveDown, S::MoveUp, S::Open, S::AddToPlaylist, S::Up, S::Down];
    match crate::config::get().which(&keys, input) {
        Some(S::Remove) if queue => {
            if let Some(i) = t.selected {
                t.remove(app, i);
            }
        }
        Some(S::MoveDown) if queue => {
            if let Some(i) = t.selected {
                t.shift(app, i, 1);
            }
        }
        Some(S::MoveUp) if queue => {
            if let Some(i) = t.selected {
                t.shift(app, i, -1);
            }
        }
        Some(S::Open) => t.play_selected(app),
        Some(S::AddToPlaylist) => {
            if let Some((_, uri)) = t.selection() {
                app.act(Action::Picker(Kind::AddTo(uri.to_owned())));
            }
        }
        Some(S::Up) => t.step(app, -1),
        Some(S::Down) => t.step(app, 1),
        _ => {}
    }
}

/// The column every page keeps to, centred in `area`: three album cards
/// wide, or the window when narrower.
pub fn column(area: R) -> (f32, f32) {
    let w = size::column().min(area.w);
    ((area.x + (area.w - w) / 2.0).round(), w)
}

/// A thin bar at the right edge while the view has more than it shows.
pub fn scrollbar(ui: &Ui, area: R, s: &ui::Scroll) {
    if s.content <= s.view + 1.0 {
        return;
    }
    let w = rem(0.25);
    let h = (area.h * s.view / s.content).max(rem(2.0));
    let y = area.y + (area.h - h) * (s.offset / (s.content - s.view));
    ui.rounded(R::new(area.right() - w - rem(0.25), y, w, h), w / 2.0, rl::Color { a: 60, ..ui::FG });
}

/// The wheel on a view: its offset moved and kept within its content.
pub fn wheel(ui: &mut Ui, area: R, s: &mut ui::Scroll) {
    s.view = area.h;
    s.offset += ui.take_wheel(area, area.h);
    s.clamp();
}

/// The head of every page with a list (an album's, a playlist's, the
/// queue's, an artist's), in rem: the cover, the portrait, and the words
/// beside them, as tall on all of them.
const HEAD: f32 = 14.0;

/// The head's height on a page, and the block under it that scrolls:
/// the rows, the albums. A rem between the blocks, from the tabs to the
/// player: half of it above the head and under the body is the header's
/// and the player's own.
fn head_and_body(area: R) -> (R, R) {
    let head = R::new(area.x, area.y + rem(0.5), area.w, rem(HEAD));
    let top = head.bottom() + rem(1.0);
    (head, R::new(area.x, top, area.w, (area.bottom() - rem(0.5) - top).max(0.0)))
}

/// A tracks page, drawn in `area`: the head, still, and under it the rows
/// in a block of their own that scrolls, down to the player.
pub fn draw_tracks(app: &App, t: &mut Tracks, ui: &mut Ui, area: R) {
    let (x0, cw) = column(area);
    let (head, body) = head_and_body(area);
    let (current, playing) = app.current();

    // the head: the picture, and beside it from the top the title (two
    // lines at most), the subtitle, the facts and the buttons; without a
    // picture (a playlist, the queue) the words alone
    let pic = if t.cover.is_some() { head.h } else { 0.0 };
    let fx = x0 + if pic > 0.0 { pic + rem(1.5) } else { 0.0 };
    let fw = (x0 + cw - fx).max(rem(4.0));
    let s = rem(0.5);
    let title_lines = ui.face(Style::Title).wrap(&t.title, fw, 2);
    let sub_lines = if t.subtitle.is_empty() { Vec::new() } else { ui.face(Style::Header).wrap(&t.subtitle, fw, 1) };
    let (lt, lh) = (ui.line(Style::Title), ui.line(Style::Header));
    let button = ui.square_side();
    if let Some((key, uri)) = &t.cover {
        let r = R::new(x0, head.y, pic, pic);
        match app.pictures.cover(key, uri) {
            Some(tex) => ui.picture(tex, r),
            None => ui.placeholder(glyphs::ALBUM, r, false),
        }
    }
    let mut fy = head.y;
    for line in &title_lines {
        ui.text(Style::Title, line, fx, fy, fw);
        fy += lt;
    }
    if !sub_lines.is_empty() {
        fy += s;
        for line in &sub_lines {
            match &t.to_artist {
                Some(name) => {
                    if ui.link(Style::Header, line, fx, fy, fw).0 {
                        app.act(Action::OpenArtist(name.clone()));
                    }
                }
                None => {
                    ui.text(Style::Header, line, fx, fy, fw);
                }
            }
            fy += lh;
        }
    }
    fy += s;
    ui.text(Style::Subtitle, &t.facts, fx, fy, fw);
    fy += lh + s;
    let mut bx = fx;
    let mut next = |ui: &mut Ui, glyph: &str, look: Look, sensitive: bool| {
        let x = bx;
        bx += button + rem(0.5);
        ui.square(x, fy, glyph, look, sensitive)
    };
    let any = !t.uris.is_empty();
    let shuffle = t.shuffle.clone().map(|(path, on)| (path, on, if on { Look::Suggested } else { Look::Plain }));
    match t.plays {
        // play from the selection, a playlist's shuffle, add to the queue
        Plays::Load(_) => {
            let here = t.selection().is_some_and(|(_, uri)| current == Some(uri));
            if next(ui, if here && playing { glyphs::PAUSE } else { glyphs::PLAY }, Look::Suggested, any) {
                t.play_selected(app);
            }
            if let Some((path, on, look)) = shuffle {
                if next(ui, glyphs::SHUFFLE, look, true) {
                    app.act(Action::Shuffle(path, !on));
                }
            }
            if next(ui, glyphs::QUEUE_ADD, Look::Plain, any) {
                app.act(Action::Append(t.uris.clone()));
            }
            // the radio from the album
            if let Some(seed) = &t.seed {
                if next(ui, glyphs::RADIO, Look::Plain, app.vectors.is_some()) {
                    app.act(Action::Radio(seed.clone()));
                }
            }
        }
        // the player plays it: the shuffle of the playlist it plays, a
        // radio's save as a playlist, and clear
        Plays::Queue => {
            if let Some((path, on, look)) = shuffle {
                if next(ui, glyphs::SHUFFLE, look, true) {
                    app.act(Action::Shuffle(path, !on));
                }
            }
            if let Some(name) = &t.save {
                if next(ui, glyphs::SAVE, Look::Plain, any) {
                    app.act(Action::Modal(Modal::name(Some(name.clone()))));
                }
            }
            if next(ui, glyphs::CLEAR, Look::Destructive, any) {
                app.act(Action::QueueRow(None));
                app.act(Action::Mpd(Command::Clear));
            }
        }
    }

    // the rows, in their block
    wheel(ui, body, &mut t.scroll);
    ui.push_clip(body);
    let top = body.y - t.scroll.offset;
    let mut y = top;
    t.tops.clear();
    let n = t.rows.len();
    for i in 0..n {
        let h = row_height(ui, &t.rows[i]);
        let r = R::new(x0, y, cw, h);
        t.tops.push((y - top, y - top + h));
        if r.bottom() >= body.y && r.y <= body.bottom() {
            draw_row(app, t, i, r, ui, current, playing);
        }
        y += h;
    }
    // the block's corners rounded, where it shows
    ui.round_corners(R::new(x0, body.y, cw, (y.min(body.bottom()) - body.y).max(0.0)), rem(ui::CORNER), ui::BG);

    if let Some((path, name)) = &t.delete {
        y += rem(1.0);
        if ui.square(x0, y, glyphs::DELETE, Look::Danger, true) {
            app.act(Action::Modal(Modal::delete(path.clone(), name.clone())));
        }
        y += button;
    }
    y += rem(1.0);
    t.scroll.content = y - top;
    if std::mem::take(&mut t.reveal) {
        if let Some(&(a, b)) = t.selected.and_then(|i| t.tops.get(i)) {
            if std::mem::take(&mut t.centre) {
                t.scroll.offset = (a + b - t.scroll.view) / 2.0;
            } else {
                t.scroll.reveal(a, b);
            }
            t.scroll.clamp();
            ui.wake(0.0);
        }
    }
    ui.pop_clip();
    scrollbar(ui, body, &t.scroll);
}

fn row_height(ui: &Ui, row: &Row) -> f32 {
    let mut words = ui.line(Style::Label);
    if row.artist.is_some() || row.album.is_some() || row.plain.is_some() {
        words += rem(0.25) + ui.line(Style::Subtitle);
    }
    words.max(rem(2.75)) + 2.0 * rem(0.5)
}

/// A row: the number (the playing mark in its place on the song on),
/// the words, the length; on the queue the edits.
fn draw_row(app: &App, t: &mut Tracks, i: usize, r: R, ui: &mut Ui, current: Option<&str>, playing: bool) {
    let queue = t.is_queue();
    let n = t.rows.len();
    let row = &t.rows[i];
    let playable = row.start.is_some();
    let over = playable && ui.hovered(r);
    let selected = t.selected == Some(i);
    ui.rect(
        r,
        match (selected, over) {
            (true, true) => ui::ROW_SELECTED_HOVER,
            (true, false) => ui::ROW_SELECTED,
            (false, true) => ui::ROW_HOVER,
            (false, false) => ui::CARD,
        },
    );
    let inner = r.inset(rem(0.75), rem(0.5));
    let mid = inner.y + inner.h / 2.0;
    let mut clicked_inside = false;
    ui.with_alpha(if playable { 1.0 } else { 0.5 }, |ui| {
        // the number, four characters wide, right-aligned; the mark over it
        let face = ui.face(Style::Subtitle);
        let nw = 4.0 * face.advance('0');
        let ls = face.line;
        let on = row.start.is_some_and(|s| current == Some(t.uris[s].as_str()));
        if on {
            let side = rem(1.0);
            crate::bars::draw(ui, R::new(inner.x + nw - side, (mid - side / 2.0).round(), side, side), playing);
        } else {
            ui.text_right(Style::Subtitle, &row.number, inner.x + nw, (mid - ls / 2.0).round());
        }

        // the end: the length, then on the queue up, down and out
        let mut right = inner.right();
        if queue {
            let key = rem(2.0);
            let margin = rem(0.25);
            let lit = over || selected;
            let mut x = right - 3.0 * (key + 2.0 * margin);
            for (glyph, sensitive, edit) in [(glyphs::UP, i > 0, -1i64), (glyphs::DOWN, i + 1 < n, 1), (glyphs::REMOVE, true, 0)] {
                let b = R::new(x + margin, (mid - key / 2.0).round(), key, key);
                x += key + 2.0 * margin;
                if edit_key(ui, b, glyph, sensitive, lit) {
                    clicked_inside = true;
                    if edit == 0 {
                        t.remove(app, i);
                    } else {
                        t.shift(app, i, edit);
                    }
                }
            }
            right -= 3.0 * (key + 2.0 * margin) + rem(0.75);
        }
        if let Some(d) = row.duration {
            let w = ui.text_right(Style::Subtitle, &library::clock(d), right, (mid - ls / 2.0).round());
            right -= w + rem(0.75);
        }

        // the words: the title, under it the artist and the album
        let wx = inner.x + nw + rem(0.75);
        let ww = (right - wx).max(0.0);
        let ll = ui.line(Style::Label);
        let two = row.artist.is_some() || row.album.is_some() || row.plain.is_some();
        let wh = if two { ll + rem(0.25) + ls } else { ll };
        let wy = (mid - wh / 2.0).round();
        ui.text(Style::Label, &row.title, wx, wy, ww);
        if two {
            let ly = wy + ll + rem(0.25);
            let mut x = wx;
            let end = wx + ww;
            if let Some(name) = &row.artist {
                let (click, w) = ui.link(Style::Subtitle, name, x, ly, end - x);
                if click {
                    clicked_inside = true;
                    app.act(Action::OpenArtist(name.clone()));
                }
                x += w + rem(0.5);
            }
            if let (Some((a, track)), Some(library)) = (row.album, &t.library) {
                if row.artist.is_some() {
                    x += ui.text(Style::Subtitle, "·", x, ly, end - x) + rem(0.5);
                }
                let (click, _) = ui.link(Style::Subtitle, &library.albums[a].title, x, ly, (end - x).max(0.0));
                if click {
                    clicked_inside = true;
                    app.act(Action::OpenAlbum { key: library.albums[a].key.clone(), track });
                }
            }
            if let Some(text) = &row.plain {
                ui.text(Style::Subtitle, text, x, ly, (end - x).max(0.0));
            }
        }
    });
    // a click on the row selects it and plays from there, or pauses and
    // resumes the song on when it is that row
    if !clicked_inside && playable && ui.click(r) {
        t.selected = Some(i);
        if queue {
            app.act(Action::QueueRow(Some(i)));
        }
        let start = t.rows[i].start.unwrap();
        if current == Some(t.uris[start].as_str()) {
            app.act(Action::Mpd(Command::Toggle));
        } else {
            t.play_from(app, start);
        }
    }
}

/// The queue's row edits: small flat keys, shown plainly only under the
/// pointer and on the selected row.
fn edit_key(ui: &mut Ui, r: R, glyph: &str, sensitive: bool, lit: bool) -> bool {
    let over = sensitive && ui.hovered(r);
    ui.with_alpha(if sensitive { 1.0 } else { 0.5 }, |ui| {
        if over {
            ui.rounded(r, rem(ui::CORNER), ui::HOVER);
        }
        let color = if over { ui::FG } else if lit { ui::ACCENT } else { ui::ACCENT_BG };
        ui.icon(glyph, r, 1.0, color);
    });
    sensitive && ui.click(r)
}

/// The artist: face, name and the words from the net in the head, the
/// words in a block of their own that scrolls; under it the grid of their
/// albums, in another.
pub struct ArtistPage {
    pub library: Arc<Library>,
    pub index: usize,
    /// Their albums, the newest first, those without a year last.
    pub grid: Grid,
    /// The words' scroll.
    pub words: ui::Scroll,
}

impl ArtistPage {
    pub fn name(&self) -> &str {
        &self.library.artists[self.index].name
    }

    pub fn new(library: &Arc<Library>, index: usize) -> ArtistPage {
        let mut albums = library.artists[index].albums.clone();
        albums.sort_by_key(|&a| std::cmp::Reverse(library.albums[a].year.as_deref().and_then(|y| y.get(..4)?.parse::<u32>().ok())));
        ArtistPage { library: library.clone(), index, grid: Grid { order: albums, dealt: Some(library.clone()), ..Grid::default() }, words: ui::Scroll::default() }
    }
}

/// A piece of the artist's words: a word, or a glyph that is a link.
#[allow(clippy::large_enum_variant)]
enum Bit<'a> {
    Word(&'a str),
    Break,
    Link(&'a str, Action),
}

pub fn draw_artist(app: &App, page: &mut ArtistPage, ui: &mut Ui, area: R) {
    let artist = &page.library.artists[page.index];
    let (x0, cw) = column(area);
    let (head, body) = head_and_body(area);

    let side = head.h;
    let portrait = R::new(x0, head.y, side, side);
    match app.pictures.face(&artist.name) {
        Some(tex) => {
            ui.circle(portrait.center(), side / 2.0, ui::SHADE);
            ui.picture(tex, portrait);
        }
        None => ui.placeholder(glyphs::ARTIST, portrait, true),
    }
    let wx = x0 + side + rem(1.5);
    // the radio from the artist, at the end of the name's line
    let button = ui.square_side();
    let bx = x0 + cw - button;
    if ui.square(bx, head.y, glyphs::RADIO, Look::Plain, app.vectors.is_some()) {
        app.act(Action::Radio(Seed::Artist(artist.name.clone())));
    }
    let ww = (bx - rem(1.0) - wx).max(rem(4.0));
    let mut wy = head.y;
    let lt = ui.line(Style::Title);
    for line in ui.face(Style::Title).wrap(&artist.name, ww, 1) {
        ui.text(Style::Title, &line, wx, wy, ww);
        wy += lt;
    }
    let ww = (x0 + cw - wx).max(rem(4.0));
    // the words, then glyphs as links: the article's, then the retry; in
    // the rest of the head, scrolling
    if let Some(known) = app.pictures.words(&artist.name) {
        wy += rem(0.5);
        let mut bits: Vec<Bit> = Vec::new();
        if let Some(bio) = &known.bio {
            for (i, paragraph) in bio.split('\n').enumerate() {
                if i > 0 {
                    bits.push(Bit::Break);
                }
                bits.extend(paragraph.split(' ').filter(|w| !w.is_empty()).map(Bit::Word));
            }
        }
        if let Some(url) = &known.url {
            bits.push(Bit::Link(glyphs::WIKIPEDIA, Action::OpenUrl(url.clone())));
        }
        bits.push(Bit::Link(glyphs::RETRY, Action::Retry(artist.name.clone())));
        let words = R::new(wx, wy, x0 + cw - wx, (head.bottom() - wy).max(0.0));
        wheel(ui, words, &mut page.words);
        ui.push_clip(words);
        let top = words.y - page.words.offset;
        page.words.content = flow(app, ui, bits, wx, top, ww - rem(1.0)) - top;
        ui.pop_clip();
        scrollbar(ui, words, &page.words);
    }

    // the albums, three across the column, in their block
    wheel(ui, body, &mut page.grid.scroll);
    ui.push_clip(body);
    let top = body.y - page.grid.scroll.offset;
    let gw = size::column() + size::gap();
    let gx = (area.x + (area.w - gw) / 2.0).round().max(area.x);
    let library = page.library.clone();
    let h = view::draw_grid(app, ui, &mut page.grid, view::Cards::Albums(&library), gx, gw, size::COLUMNS, top, body);
    page.grid.scroll.content = h + rem(1.0);
    if std::mem::take(&mut page.grid.reveal) {
        let row = page.grid.selected / size::COLUMNS;
        let rh = view::card_height(ui) + size::gap();
        let at = row as f32 * rh;
        page.grid.scroll.reveal(at, at + rh);
        page.grid.scroll.clamp();
        ui.wake(0.0);
    }
    ui.pop_clip();
    scrollbar(ui, body, &page.grid.scroll);
}

/// The words laid in lines `w` wide from `y`; the links underlined, a
/// click does theirs. Returns the bottom.
fn flow(app: &App, ui: &mut Ui, bits: Vec<Bit>, x0: f32, mut y: f32, w: f32) -> f32 {
    let face = ui.face(Style::Label);
    let line = face.line;
    let space = face.advance(' ');
    let mut x = x0;
    let mut first = true;
    for bit in bits {
        match bit {
            Bit::Break => {
                x = x0;
                y += line;
                first = true;
            }
            Bit::Word(word) => {
                let ww = face.width(word);
                if !first && x + space + ww > x0 + w {
                    x = x0;
                    y += line;
                    first = true;
                }
                if !first {
                    x += space;
                }
                ui.text_in(Style::Label, word, x, y, f32::MAX, ui::BIO);
                x += ww;
                first = false;
            }
            Bit::Link(glyph, action) => {
                let gap = 2.0 * space;
                let ww = face.width(glyph);
                if !first && x + gap + ww > x0 + w {
                    x = x0;
                    y += line;
                    first = true;
                }
                if !first {
                    x += gap;
                }
                let (click, _) = ui.link_in(Style::Label, glyph, x, y, f32::MAX, ui::ACCENT);
                ui.rect(R::new(x, (y + line * 0.86).round(), ww, (face.px / 14.0).round().max(1.0)), ui::ACCENT);
                if click {
                    app.act(action);
                }
                x += ww;
                first = false;
            }
        }
    }
    y + line
}
