//! The bar at the bottom: on top the time (a click or a drag seeks),
//! where the song is and its length just over its ends, the song's
//! title centred between; half a rem under it the transport, centred,
//! and beside it, as far under the time, the song's artist and the
//! album or playlist the queue plays, a button each, on the left, repeat
//! and repeat one side by side on the right:
//!   0:03                 title                16:24
//!   ───────────────── time ─────────────────
//!   [☺] [◉]           ⏮  ⏯  ⏭           [⟲] [⟲1]

use std::time::Duration;

use mpd_client::commands::SingleMode;

use crate::app::{Action, App, Tab};
use crate::glyphs;
use crate::library;
use crate::master::Source;
use crate::mpd::Command;
use crate::rl::Vector2;
use crate::size::rem;
use crate::ui::{self, Look, Style, Ui, R};

/// The player's room at its top and between its rows, in rem: the
/// header's, above and under the tabs.
const EDGE: f32 = 0.5;
/// The player's room at its bottom, in rem.
const BOTTOM: f32 = 2.0;
/// Around the song's title, in rem.
const PAD: f32 = 1.0;

fn row_height(ui: &Ui) -> f32 {
    ui.line(Style::Header).max(ui::switch_size().1)
}

pub fn height(ui: &Ui) -> f32 {
    // the times, the time, the transport
    rem(EDGE) + ui.line(Style::Header) + rem(0.25) + rem(0.5) + rem(EDGE) + row_height(ui) + rem(BOTTOM)
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
    // over it where the song is and its length
    let lh = ui.line(Style::Header);
    let times = r.y + rem(EDGE);
    let bar = R::new(x0, times + lh + rem(0.25), cw, rem(0.5));
    let grip = R::new(bar.x, bar.y - rem(0.25), bar.w, bar.h + rem(0.5));
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
    // its corners rounded as the tabs' are, which at its height is round
    ui.rounded(bar, rem(0.5), ui::SHADE);
    let k = if seekable { (at.as_secs_f64() / length.as_secs_f64()) as f32 } else { 0.0 };
    let done = (bar.w * k).round();
    if done > 0.0 {
        ui.rounded(R::new(bar.x, bar.y, done, bar.h), rem(0.5), ui::ACCENT_BG);
    }
    if seekable && (ui.hovered(grip) || ui.drag == Some("seek")) {
        ui.circle(Vector2 { x: bar.x + bar.w * k, y: bar.y + bar.h / 2.0 }, rem(0.5), ui::ACCENT);
    }
    if playing {
        ui.wake(1.0 - at.as_secs_f64().fract());
    }

    // the transport: previous and next, round and flat, and between them
    // play while paused, pause while playing, round and filled; the size
    // of the square buttons beside it, the album's and the artist's on the
    // left, the repeats on the right; all half a rem under the time
    let rh = row_height(ui);
    let row = bar.bottom() + rem(EDGE);
    let margin = rem(0.5);
    let all = 3.0 * rh + 6.0 * margin;
    let mut x = (x0 + (cw - all) / 2.0).round();
    for (glyph, command, main) in [
        (glyphs::PREVIOUS, Command::Previous, false),
        (if playing { glyphs::PAUSE } else { glyphs::PLAY }, Command::Toggle, true),
        (glyphs::NEXT, Command::Next, false),
    ] {
        x += margin;
        let b = R::new(x, row, rh, rh);
        x += rh + margin;
        let over = ui.hovered(b);
        let held = over && ui.held(b);
        let (bg, fg) = if main {
            (Some(if held { ui::ACCENT_BG } else if over { ui::ACCENT } else { ui::FG }), ui::BG)
        } else {
            (if held { Some(ui::HOVER) } else if over { Some(ui::SHADE) } else { None }, ui::FG)
        };
        if let Some(bg) = bg {
            ui.circle(b.center(), rh / 2.0, bg);
        }
        ui.icon(glyph, b, 1.25, fg);
        if ui.click(b) {
            app.act(Action::Mpd(command));
        }
    }
    // where the song is and its length, over the time's ends
    if clock.is_some() {
        ui.text(Style::Header, &library::clock(at), x0, times, f32::MAX);
        ui.text_right(Style::Header, &library::clock(length), x0 + cw, times);
    }

    // the right: repeat (the queue) and repeat one (the song on, over and
    // over), side by side, in the accent while on, repeat off from either
    let single = now.is_some_and(|n| n.status.single == SingleMode::Enabled);
    let repeat = now.is_some_and(|n| n.status.repeat);
    let repeats = x0 + cw - 2.0 * rh - rem(0.5);
    let mut x = repeats;
    for (glyph, on, command) in [(glyphs::REPEAT, repeat && !single, Command::RepeatAll), (glyphs::REPEAT_ONE, repeat && single, Command::RepeatOne)] {
        if ui.square_button(R::new(x, row, rh, rh), glyph, 1.25, if on { Look::Suggested } else { Look::Plain }, true) {
            app.act(Action::Mpd(if on { Command::RepeatOff } else { command }));
        }
        x += rh + rem(0.5);
    }

    // the left: the song's artist, that opens the page of the artist the
    // album is filed under, and the album or playlist the queue plays,
    // with its glyph, that opens its page
    let glyph = if app.master.playlist().is_some() { glyphs::PLAYLIST } else { glyphs::ALBUM };
    let place = match app.current().0 {
        Some(uri) => app.library.as_ref().and_then(|l| l.place(uri).map(|(a, t)| (l.clone(), a, t))),
        None => app.waiting(),
    };
    let artist = match &place {
        Some((library, a, _)) => Some(library.albums[*a].artist.clone()),
        None => now.and_then(|n| n.song.as_ref()).and_then(|s| s.song.artists().first().cloned()),
    };
    if now.is_some_and(|n| n.song.is_some()) || place.is_some() || app.master.playlist().is_some() {
        if ui.square_button(R::new(x0 + rh + rem(0.5), row, rh, rh), glyph, 1.25, Look::Plain, true) {
            open_source(app);
        }
    }
    if let Some(filed) = artist {
        if ui.square_button(R::new(x0, row, rh, rh), glyphs::ARTIST, 1.25, Look::Plain, true) {
            app.act(Action::OpenArtist(filed));
        }
    }

    // over the time, the song's title centred between where the song is
    // and its length, the pad from either, the ellipsis where it ends
    let title = match now.and_then(|n| n.song.as_ref()) {
        Some(entry) => entry.song.title().unwrap_or(&entry.song.url).to_owned(),
        // stopped at the end of the list: the list stays, its first track
        // ready, the way a paused one shows
        None => match &place {
            Some((library, a, t)) => library.albums[*a].tracks[*t].title.clone(),
            None => glyphs::NOTHING.to_owned(),
        },
    };
    let ends = match clock {
        Some(_) => ui.width(Style::Header, &library::clock(at)).max(ui.width(Style::Header, &library::clock(length))) + rem(PAD),
        None => 0.0,
    };
    let max = cw - 2.0 * ends;
    if max > 0.0 {
        let w = ui.width(Style::Header, &title).min(max);
        ui.text(Style::Header, &title, (x0 + (cw - w) / 2.0).round(), times, max);
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
