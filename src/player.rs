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

const EDGE: f32 = 0.5;
const BOTTOM: f32 = 2.0;
const BIG: f32 = 3.25;
const PAD: f32 = 1.0;

pub fn height(ui: &Ui) -> f32 {
    rem(EDGE) + ui.line(Style::Header) + ui.line(Style::Subtitle) + rem(0.25) + rem(0.5) + rem(EDGE) + rem(BIG).max(ui.square_side()) + rem(BOTTOM)
}

pub fn draw(app: &App, ui: &mut Ui, r: R) {
    ui.rect(r, ui::BG);
    let blocked = ui.blocked;
    ui.blocked = blocked || !app.connected;
    ui.with_alpha(if app.connected { 1.0 } else { 0.5 }, |ui| bar(app, ui, r));
    ui.blocked = blocked;
}

fn bar(app: &App, ui: &mut Ui, r: R) {
    let (x0, cw) = crate::pages::column(r);
    let now = app.now.as_ref();
    let playing = now.is_some_and(|n| n.playing());

    let clock = app.clock;
    let length = clock.map(|c| c.length).unwrap_or_default();
    let mut at = clock.map(|c| if playing { c.elapsed + c.since.elapsed() } else { c.elapsed }.min(c.length)).unwrap_or_default();
    let lh = ui.line(Style::Header);
    let times = r.y + rem(EDGE);
    let bar = R::new(x0, times + lh + ui.line(Style::Subtitle) + rem(0.25), cw, rem(0.5));
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

    let rh = ui.square_side();
    let (small, big, margin) = (rem(2.25), rem(BIG), rem(0.5));
    let mid = bar.bottom() + rem(EDGE) + big.max(rh) / 2.0;
    let row = (mid - rh / 2.0).round();
    let all = 2.0 * small + big + 6.0 * margin;
    let mut x = (x0 + (cw - all) / 2.0).round();
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
    if clock.is_some() {
        ui.text(Style::Header, &library::clock(at), x0, times, f32::MAX);
        ui.text_right(Style::Header, &library::clock(length), x0 + cw, times);
    }

    let single = now.is_some_and(|n| n.status.single == SingleMode::Enabled);
    let repeat = now.is_some_and(|n| n.status.repeat);
    let repeats = x0 + cw - 2.0 * rh - rem(0.5);
    let mut x = repeats;
    for (glyph, on, command) in [(glyphs::REPEAT, repeat && !single, Command::RepeatAll), (glyphs::REPEAT_ONE, repeat && single, Command::RepeatOne)] {
        if ui.square(x, row, glyph, if on { Look::Suggested } else { Look::Plain }, true) {
            app.act(Action::Mpd(if on { Command::RepeatOff } else { command }));
        }
        x += rh + rem(0.5);
    }

    let place = match app.current().0 {
        Some(uri) => app.library.as_ref().and_then(|l| l.place(uri).map(|(a, t)| (l.clone(), a, t))),
        None => app.waiting(),
    };
    // the playlist or the radio the queue plays, next to the album; nothing for an album or a loose queue
    if matches!(app.master.source, Source::Playlist { .. } | Source::Radio { .. }) && ui.square(x0 + rh + rem(0.5), row, app.master.glyph(), Look::Plain, true) {
        open_source(app);
    }
    // the album of the track playing, the track selected on its page
    if let Some((library, a, t)) = &place {
        if ui.square(x0, row, glyphs::ALBUM, Look::Plain, true) {
            app.act(Action::OpenAlbum { key: library.albums[*a].key.clone(), track: *t });
        }
    }

    let title = match now.and_then(|n| n.song.as_ref()) {
        Some(entry) => entry.song.title().unwrap_or(&entry.song.url).to_owned(),
        None => match &place {
            Some((library, a, t)) => library.albums[*a].tracks[*t].title.clone(),
            None => glyphs::NOTHING.to_owned(),
        },
    };
    // the track's artists, under the title, a link each: the album's
    // members when the track is credited as the album, else the track's
    // own tag; mpd's artists, plain, for a track the library has not
    let by: Vec<String> = match &place {
        Some((library, a, t)) => {
            let album = &library.albums[*a];
            let track = &album.tracks[*t];
            if track.artist == album.artist { album.names().map(str::to_owned).collect() } else { vec![track.artist.clone()] }
        }
        None => now.and_then(|n| n.song.as_ref()).map(|s| s.song.artists().to_vec()).unwrap_or_default(),
    };
    let ends = match clock {
        Some(_) => ui.width(Style::Header, &library::clock(at)).max(ui.width(Style::Header, &library::clock(length))) + rem(PAD),
        None => 0.0,
    };
    let max = cw - 2.0 * ends;
    if max > 0.0 {
        let w = ui.width(Style::Header, &title).min(max);
        let tx = (x0 + (cw - w) / 2.0).round();
        match &place {
            Some((library, a, t)) => {
                if ui.link(Style::Header, &title, tx, times, max).0 {
                    app.act(Action::OpenAlbum { key: library.albums[*a].key.clone(), track: *t });
                }
            }
            None => {
                ui.text(Style::Header, &title, tx, times, max);
            }
        }
        if !by.is_empty() {
            let line = by.join(", ");
            let w = ui.width(Style::Subtitle, &line).min(max);
            let bx = (x0 + (cw - w) / 2.0).round();
            if place.is_some() {
                if let Some(i) = ui.links(Style::Subtitle, &by, bx, times + lh, max) {
                    app.act(Action::OpenArtist(by[i].clone()));
                }
            } else {
                ui.text(Style::Subtitle, &line, bx, times + lh, max);
            }
        }
    }
}

pub fn open_source(app: &App) {
    match &app.master.source {
        Source::Playlist { path } => app.act(Action::OpenPlaylist(path.clone())),
        Source::Anon | Source::Radio { .. } => app.act(Action::Tab(Tab::Queue)),
        Source::Album { .. } | Source::None => {
            let (Some(uri), _) = app.current() else { return };
            if let Some(library) = &app.library {
                if let Some((album, track)) = library.place(uri) {
                    app.act(Action::OpenAlbum { key: library.albums[album].key.clone(), track });
                }
            }
        }
    }
}
