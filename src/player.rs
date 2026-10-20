//! The bar at the bottom: the time along its top edge (a click or a
//! drag seeks); under it, in three equal columns, the song's title over
//! the album or playlist the queue plays on the left, the transport in
//! the middle, the song's time and repeat one over the queue's time and
//! repeat on the right:
//!   ───────────────── time ─────────────────
//!   ♫ title               0:03 / 16:24 ⟲1
//!   ◉ album    ⏮  ⏯  ⏭  3:20 / 42:24 ⟲

use std::time::Duration;

use mpd_client::commands::SingleMode;

use crate::app::{Action, App, Tab};
use crate::glyphs;
use crate::library;
use crate::master::Source;
use crate::mpd::Command;
use crate::rl::Vector2;
use crate::size::rem;
use crate::ui::{self, Style, Ui, R};

/// The player's room under its time's bar and at its bottom, in rem: the
/// header's, above and under the tabs.
const EDGE: f32 = 0.5;
/// Between the player's columns, in rem.
const PAD: f32 = 1.0;

fn row_height(ui: &Ui) -> f32 {
    ui.line(Style::Header).max(ui::switch_size().1)
}

fn lines_height(ui: &Ui) -> f32 {
    (2.0 * row_height(ui) + rem(0.25)).max(rem(3.25))
}

pub fn height(ui: &Ui) -> f32 {
    rem(0.5) + rem(EDGE) + lines_height(ui) + rem(EDGE)
}

pub fn draw(app: &App, ui: &mut Ui, r: R) {
    ui.rect(r, ui::BG);
    let blocked = ui.blocked;
    // mpd unreachable: the player shown, not usable
    ui.blocked = blocked || !app.connected;
    ui.with_alpha(if app.connected { 1.0 } else { 0.5 }, |ui| bar(app, ui, r));
    ui.blocked = blocked;
}

fn bar(app: &App, ui: &mut Ui, r: R) {
    let (x0, cw) = crate::pages::column(r);
    let now = app.now.as_ref();
    let playing = now.is_some_and(|n| n.playing());

    // the time: where the song is, as the clock runs between statuses
    let clock = app.clock;
    let length = clock.map(|c| c.length).unwrap_or_default();
    let mut at = clock.map(|c| if playing { c.elapsed + c.since.elapsed() } else { c.elapsed }.min(c.length)).unwrap_or_default();
    let bar = R::new(x0, r.y, cw, rem(0.5));
    let grip = R::new(bar.x, bar.y, bar.w, bar.h + rem(0.25));
    let seekable = !length.is_zero();
    let to = |ui: &Ui| Duration::from_secs_f64((((ui.mouse.x - bar.x) / bar.w).clamp(0.0, 1.0) as f64) * length.as_secs_f64());
    if seekable && ui.press(grip) {
        ui.drag = Some("seek");
        app.act(Action::Mpd(Command::Seek(to(ui))));
    } else if seekable && ui.drag == Some("seek") && ui.moved {
        app.act(Action::Mpd(Command::Seek(to(ui))));
    }
    if seekable && ui.drag == Some("seek") {
        at = to(ui);
    }
    ui.rect(bar, ui::SHADE);
    let k = if seekable { (at.as_secs_f64() / length.as_secs_f64()) as f32 } else { 0.0 };
    ui.rect(R::new(bar.x, bar.y, (bar.w * k).round(), bar.h), ui::ACCENT_BG);
    if seekable && (ui.hovered(grip) || ui.drag == Some("seek")) {
        ui.circle(Vector2 { x: bar.x + bar.w * k, y: bar.y + bar.h / 2.0 }, rem(0.5), ui::ACCENT);
    }
    if playing {
        ui.wake(1.0 - at.as_secs_f64().fract());
    }

    let top = r.y + rem(0.5) + rem(EDGE);
    let lines = lines_height(ui);
    let rh = row_height(ui);
    let rows = [top + (lines - 2.0 * rh - rem(0.25)) / 2.0, top + (lines + rem(0.25)) / 2.0];
    let col = ((cw - 2.0 * rem(PAD)) / 3.0).floor();
    let lh = ui.line(Style::Header);
    let text_y = |row: f32| (row + (rh - lh) / 2.0).round();

    // the left: the song's title (it opens the queue), the album or
    // playlist the queue plays (it opens its page)
    let mark = lh;
    let title = match now.and_then(|n| n.song.as_ref()) {
        Some(entry) => entry.song.title().unwrap_or(&entry.song.url).to_owned(),
        // stopped at the end of the list: the list stays, its first track
        // ready, the way a paused one shows
        None => match app.waiting() {
            Some((library, a, t)) => library.albums[a].tracks[t].title.clone(),
            None => glyphs::NOTHING.to_owned(),
        },
    };
    let tx = x0 + mark + rem(0.5);
    ui.icon(glyphs::TRACK, R::new(x0, text_y(rows[0]), mark, mark), 1.0, ui::FG);
    if ui.link(Style::Header, &title, tx, text_y(rows[0]), col - mark - rem(0.5)).0 {
        app.act(Action::Tab(Tab::Queue));
    }
    let (glyph, source) = match app.master.playlist() {
        Some(list) => (glyphs::PLAYLIST, Some(list)),
        None => (
            glyphs::ALBUM,
            now.and_then(|n| n.song.as_ref())
                .and_then(|s| s.song.album().map(str::to_owned))
                .or_else(|| app.waiting().map(|(library, a, _)| library.albums[a].title.clone())),
        ),
    };
    if let Some(source) = source {
        ui.icon(glyph, R::new(x0, text_y(rows[1]), mark, mark), 1.0, ui::ACCENT);
        if ui.link(Style::Subtitle, &source, tx, text_y(rows[1]), col - mark - rem(0.5)).0 {
            open_source(app);
        }
    }

    // the transport: previous and next, round and flat, and between them
    // the big one, play while paused, pause while playing
    let (small, big, margin) = (rem(2.25), rem(3.25), rem(0.5));
    let all = 2.0 * small + big + 6.0 * margin;
    let mut x = (x0 + col + rem(PAD) + (col - all) / 2.0).round();
    let mid = top + lines / 2.0;
    for (glyph, size, command, main) in [
        (glyphs::PREVIOUS, small, Command::Previous, false),
        (if playing { glyphs::PAUSE } else { glyphs::PLAY }, big, Command::Toggle, true),
        (glyphs::NEXT, small, Command::Next, false),
    ] {
        x += margin;
        let b = R::new(x, (mid - size / 2.0).round(), size, size);
        x += size + margin;
        let over = ui.hovered(b);
        let held = over && ui.held(b);
        let (bg, fg) = if main {
            (Some(if held { ui::ACCENT_BG } else if over { ui::ACCENT } else { ui::FG }), ui::BG)
        } else {
            (if held { Some(ui::HOVER) } else if over { Some(ui::SHADE) } else { None }, ui::FG)
        };
        if let Some(bg) = bg {
            ui.circle(b.center(), size / 2.0, bg);
        }
        ui.icon(glyph, b, 1.75, fg);
        if ui.click(b) {
            app.act(Action::Mpd(command));
        }
    }

    // the right: the times, and after them repeat one (the song on, over
    // and over) and repeat (the queue), a switch each
    let right = x0 + cw;
    let (sw, sh) = ui::switch_size();
    let single = now.is_some_and(|n| n.status.single == SingleMode::Enabled);
    let repeat = now.is_some_and(|n| n.status.repeat);
    let switches = [(glyphs::REPEAT_ONE, repeat && single), (glyphs::REPEAT, repeat && !single)];
    let times = clock.map(|c| {
        [
            (Style::Header, format!("{} / {}", library::clock(at), library::clock(length))),
            (Style::Subtitle, format!("{} / {}", library::clock(c.before + at), library::clock(c.whole))),
        ]
    });
    for (i, (glyph, on)) in switches.into_iter().enumerate() {
        let sy = (rows[i] + (rh - sh) / 2.0).round();
        if ui.switch(Vector2 { x: right - sw, y: sy }, glyph, on, true) {
            // repeat one off: no repeat at all; repeat off from repeat one too
            app.act(Action::Mpd(match (i, on) {
                (0, false) => Command::RepeatOne,
                (1, false) => Command::RepeatAll,
                _ => Command::RepeatOff,
            }));
        }
        if let Some(times) = &times {
            let (style, text) = &times[i];
            ui.text_right(*style, text, right - sw - rem(0.5), text_y(rows[i]));
        }
    }
}

/// The album's page, the saved playlist's, or the queue's tab.
pub fn open_source(app: &App) {
    match &app.master.source {
        Source::Playlist { path } => app.act(Action::OpenPlaylist(path.clone())),
        Source::Anon => app.act(Action::Tab(Tab::Queue)),
        Source::Album { .. } | Source::None => {
            let (Some(uri), _) = app.current() else { return };
            if let Some((album, track)) = app.library.as_ref().and_then(|l| l.place(uri)) {
                app.act(Action::OpenAlbum { album, track });
            }
        }
    }
}
