//! The window: the header with the app and the tabs over every page,
//! the player under every page, the page between them (the root page's
//! tab, or a page opened over it), the word under the tabs, and a dialog
//! over all of it when one is up.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::{Action, App, Grid, Page, Tab, TABS};
use crate::glyphs;
use crate::library::Library;
use crate::master::Source;
use crate::pages::{self, count, length, ArtistPage, Tracks};
use crate::size::{self, rem};
use crate::ui::{self, Look, Style, Ui, R};

pub fn draw(app: &mut App, ui: &mut Ui) {
    let header = header_height(ui);
    let player = crate::player::height(ui);
    let content = R::new(0.0, header, ui.w, (ui.h - header - player).max(0.0));
    let modal = app.modal.is_some();
    ui.blocked = modal;

    let mut pages = std::mem::take(&mut app.pages);
    match pages.last_mut() {
        Some(Page::Album(t) | Page::Playlist(t)) => pages::draw_tracks(app, t, ui, content),
        Some(Page::Artist(a)) => pages::draw_artist(app, a, ui, content),
        None => root(app, ui, content),
    }
    app.pages = pages;

    draw_header(app, ui, header);
    crate::player::draw(app, ui, R::new(0.0, ui.h - player, ui.w, player));
    snack(app, ui, content);

    if let Some(mut m) = app.modal.take() {
        ui.blocked = false;
        ui.rect(R::new(0.0, 0.0, ui.w, ui.h), ui::DIM);
        let keep = m.draw(app, ui);
        if keep && app.modal.is_none() {
            app.modal = Some(m);
        }
    }
}

// the header

fn header_height(ui: &Ui) -> f32 {
    rem(0.5) * 2.0 + tab_height(ui)
}

fn tab_height(ui: &Ui) -> f32 {
    rem(0.25) * 2.0 + ui.line(Style::Title)
}

/// The app's mark and name on the left, written like a tab's; the tabs
/// in the middle of the window, each its key (1 to 4) and its glyph,
/// lit while on. A click, even on the tab already on, comes back to its
/// list, dealt again.
fn draw_header(app: &App, ui: &mut Ui, h: f32) {
    ui.rect(R::new(0.0, 0.0, ui.w, h), ui::BG);
    let th = tab_height(ui);
    let y = rem(0.5);
    let big = ui.face(Style::Title);
    let side = big.line;
    let mut x = rem(1.0);
    ui.icon(glyphs::APP, R::new(x, y + rem(0.25), side, side), 1.5, ui::FG);
    x += side + rem(0.5);
    ui.text(Style::Title, "Mascarpone", x, y + rem(0.25), f32::MAX);

    let tab_w = |i: usize| rem(0.75) * 2.0 + big.width(&(i + 1).to_string()) + rem(0.5) + side;
    let all: f32 = (0..TABS.len()).map(tab_w).sum::<f32>() + rem(0.25) * (TABS.len() as f32 - 1.0);
    let mut x = ((ui.w - all) / 2.0).round();
    for (i, tab) in TABS.iter().enumerate() {
        let w = tab_w(i);
        let r = R::new(x, y, w, th);
        let over = ui.hovered(r);
        if *tab == app.tab {
            ui.rounded(r, rem(0.5), if over { ui::HOVER } else { ui::SHADE });
        } else if over {
            ui.rounded(r, rem(0.5), ui::FLAT_HOVER);
        }
        let tx = x + rem(0.75);
        let nw = ui.text(Style::Title, &(i + 1).to_string(), tx, y + rem(0.25), f32::MAX);
        ui.icon(tab.glyph(), R::new(tx + nw + rem(0.5), y + rem(0.25), side, side), 1.5, ui::FG);
        if ui.click(r) {
            app.act(Action::Tab(*tab));
        }
        x += w + rem(0.25);
    }
}

/// The word under the tabs, top centre, over whatever page is up, about
/// what is going on: a refresh asked, the library read again, mpd lost.
fn snack(app: &App, ui: &mut Ui, content: R) {
    let Some((text, until)) = &app.snack else { return };
    if let Some(until) = until {
        ui.wake(until.saturating_duration_since(Instant::now()).as_secs_f64());
    }
    let w = ui.width(Style::Label, text);
    let (px, py) = (rem(1.25), rem(0.75));
    let r = R::new(((ui.w - w) / 2.0 - px).round(), content.y + rem(1.0), w + 2.0 * px, ui.line(Style::Label) + 2.0 * py);
    ui.rect(r, ui::SHADE);
    ui.text(Style::Label, text, r.x + px, r.y + py, w + 1.0);
}

// the root page

/// The tab on screen; built again a moment ago, hidden until its
/// pictures are in (a quarter second at most).
fn root(app: &mut App, ui: &mut Ui, area: R) {
    let t = app.shown_at.elapsed();
    let ready = (t >= Duration::from_millis(32) && app.pictures.waiting() == 0) || t >= Duration::from_millis(250);
    if !ready {
        ui.wake(0.016);
    }
    let blocked = ui.blocked;
    ui.blocked = blocked || !ready;
    ui.with_alpha(if ready { 1.0 } else { 0.0 }, |ui| match app.tab {
        Tab::Albums | Tab::Artists => {
            let Some(library) = app.library.clone() else { return };
            let albums = app.tab == Tab::Albums;
            let mut grid = std::mem::take(if albums { &mut app.albums } else { &mut app.artists });
            if !grid.dealt.as_ref().is_some_and(|d| Arc::ptr_eq(d, &library)) {
                let n = if albums { library.albums.len() } else { library.artists.len() };
                grid.order = (0..n).collect();
                fastrand::shuffle(&mut grid.order);
                grid.selected = grid.selected.min(n.saturating_sub(1));
                grid.dealt = Some(library.clone());
            }
            let cards = if albums { Cards::Albums(&library) } else { Cards::Artists(&library) };
            full_grid(app, ui, &mut grid, cards, area);
            *(if albums { &mut app.albums } else { &mut app.artists }) = grid;
        }
        Tab::Playlists => {
            let Some(library) = app.library.clone() else { return };
            let mut grid = std::mem::take(&mut app.playlists);
            playlists(app, ui, &mut grid, &library, area);
            app.playlists = grid;
        }
        Tab::Queue => {
            let mut queue = app.queue.take().unwrap_or_else(|| {
                let mut t = Tracks::queue(app);
                // the same place over a rebuild
                if let Some(old) = &app.queue_scroll {
                    t.scroll = *old;
                }
                t
            });
            pages::draw_tracks(app, &mut queue, ui, area);
            app.queue_scroll = Some(queue.scroll);
            if app.queue.is_none() {
                app.queue = Some(queue);
            }
        }
    });
    ui.blocked = blocked;
}

/// What a grid's cards are.
pub enum Cards<'a> {
    Albums(&'a Arc<Library>),
    Artists(&'a Arc<Library>),
}

/// A card's height: the cover, the name, the line under it.
pub fn card_height(ui: &Ui) -> f32 {
    2.0 * rem(size::PAD) + rem(size::COVER) + 2.0 * rem(0.25) + ui.line(Style::Header) + ui.line(Style::Subtitle)
}

/// The Albums and Artists tabs: as many cards across as fit the window,
/// twelve at most, each in the middle of its column; under the pointer
/// a card is selected.
fn full_grid(app: &App, ui: &mut Ui, grid: &mut Grid, cards: Cards, area: R) {
    pages::wheel(ui, area, &mut grid.scroll);
    let columns = ((area.w / (size::card() + size::gap())).floor() as usize).clamp(1, 12);
    ui.push_clip(area);
    let top = area.y - grid.scroll.offset;
    let h = draw_grid(app, ui, grid, cards, area.x, area.w, columns, top, area);
    grid.scroll.content = h + rem(4.0);
    if std::mem::take(&mut grid.reveal) {
        let rh = card_height(ui) + size::gap();
        let at = (grid.selected / columns) as f32 * rh;
        grid.scroll.reveal(at, at + rh);
        grid.scroll.clamp();
        ui.wake(0.0);
    }
    ui.pop_clip();
    pages::scrollbar(ui, area, &grid.scroll);
}

/// The cards of a grid, `columns` across `x..x + w` from `top`, the
/// ones in `clip` drawn; returns the grid's height.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid(app: &App, ui: &mut Ui, grid: &mut Grid, cards: Cards, x: f32, w: f32, columns: usize, top: f32, clip: R) -> f32 {
    grid.columns = columns;
    let n = grid.order.len();
    let col = w / columns as f32;
    let card_w = size::card();
    let card_h = card_height(ui);
    let rh = card_h + size::gap();
    let rows = n.div_ceil(columns);
    let first = (((clip.y - top) / rh).floor().max(0.0)) as usize;
    let last = ((((clip.bottom() - top) / rh).ceil().max(0.0)) as usize).min(rows);
    for row in first..last {
        for c in 0..columns {
            let i = row * columns + c;
            if i >= n {
                break;
            }
            let cell = R::new((x + c as f32 * col + (col - card_w) / 2.0).round(), top + row as f32 * rh + size::half_gap(), card_w, card_h);
            let over = match cards {
                Cards::Albums(l) => album_card(app, ui, cell, l, grid.order[i], i == grid.selected),
                Cards::Artists(l) => artist_card(app, ui, cell, l, grid.order[i], i == grid.selected),
            };
            if over && ui.moved {
                grid.selected = i;
            }
        }
    }
    rows as f32 * rh
}

/// The round play button in the bottom right corner of a card's
/// picture, plain as the player's square buttons; true on a click.
fn play_button(ui: &mut Ui, picture: R) -> bool {
    let b = rem(2.75);
    let r = R::new(picture.right() - rem(0.5) - b, picture.bottom() - rem(0.5) - b, b, b);
    ui.round_button(r, glyphs::PLAY, 1.0, Look::Plain, true)
}

fn card_background(ui: &Ui, cell: R, over: bool, selected: bool) {
    if selected {
        ui.rect(cell, ui::SHADE);
    } else if over {
        ui.rect(cell, ui::CARD);
    }
}

/// An album's card: the cover, a play button on it (on the card under
/// the pointer and on the selected one), the title, the artist (a link to
/// their page) and the year. A click opens the album. True under the
/// pointer.
fn album_card(app: &App, ui: &mut Ui, cell: R, library: &Arc<Library>, a: usize, selected: bool) -> bool {
    let album = &library.albums[a];
    let over = ui.hovered(cell);
    card_background(ui, cell, over, selected);
    let pad = rem(size::PAD);
    let side = rem(size::COVER);
    let cover = R::new(cell.x + pad, cell.y + pad, side, side);
    match app.pictures.cover(&album.key, album.cover_uri()) {
        Some(t) => ui.picture(t, cover),
        None => ui.placeholder(glyphs::ALBUM, cover, false),
    }
    if (over || selected) && play_button(ui, cover) {
        app.act(Action::Load { source: Source::Album { key: album.key.clone() }, uris: album.uris(), start: 0 });
    }
    let y = cover.bottom() + rem(0.25);
    ui.text(Style::Header, &album.title, cover.x, y, side);
    let y = y + ui.line(Style::Header) + rem(0.25);
    let year = album.year.as_deref().unwrap_or_default();
    let yw = if year.is_empty() { 0.0 } else { ui.text_right(Style::Subtitle, year, cover.right(), y) + rem(0.5) };
    if ui.link(Style::Subtitle, &album.artist, cover.x, y, side - yw).0 {
        app.act(Action::OpenArtist(album.artist.clone()));
    }
    if ui.click(cell) {
        app.act(Action::Push(Page::Album(Tracks::album(library, a, 0))));
    }
    over
}

/// An artist's card: a round portrait as big as a cover (a placeholder
/// until the picture comes), the name, the number of albums. A click
/// opens the artist.
fn artist_card(app: &App, ui: &mut Ui, cell: R, library: &Arc<Library>, i: usize, selected: bool) -> bool {
    let artist = &library.artists[i];
    let over = ui.hovered(cell);
    card_background(ui, cell, over, selected);
    let pad = rem(size::PAD);
    let side = rem(size::COVER);
    let portrait = R::new(cell.x + pad, cell.y + pad, side, side);
    match app.pictures.face(&artist.name) {
        Some(t) => {
            ui.circle(portrait.center(), side / 2.0, ui::SHADE);
            ui.picture(t, portrait);
        }
        None => ui.placeholder(glyphs::ARTIST, portrait, true),
    }
    let mut y = portrait.bottom() + rem(0.25);
    for (style, text) in [(Style::Header, artist.name.clone()), (Style::Subtitle, count(artist.albums.len(), glyphs::ALBUM))] {
        let face = ui.face(style);
        let shown = face.ellipsize(&text, side).into_owned();
        let w = face.width(&shown);
        ui.text(style, &shown, (portrait.x + (side - w) / 2.0).round(), y, side + 1.0);
        y += face.line + rem(0.25);
    }
    if ui.click(cell) {
        app.act(Action::Push(Page::Artist(ArtistPage::new(library, i))));
    }
    over
}

/// The playlists as cards, two across the column of three album cards:
/// the name and the facts on the left, on the right the covers of the
/// first sixteen albums its tracks come from, a play button on them (on
/// the card under the pointer and on the selected one). A click opens
/// one; the same keys as the grids.
fn playlists(app: &App, ui: &mut Ui, grid: &mut Grid, library: &Arc<Library>, area: R) {
    let playlists = library.playlists();
    grid.order = (0..playlists.len()).collect();
    grid.columns = 2;
    pages::wheel(ui, area, &mut grid.scroll);
    let (x0, cw) = pages::column(area);
    let gap = size::gap();
    let card_w = ((cw - gap) / 2.0).floor();
    let pad = rem(size::PAD);
    const SIDE: usize = 4;
    // four tiles and three spaces, a quarter short of the cover
    let (tile, space) = (rem(2.75), rem(0.25));
    let mosaic = SIDE as f32 * tile + (SIDE as f32 - 1.0) * space;
    let left_w = (card_w - 2.0 * pad - rem(0.75) - mosaic).max(rem(2.0));
    let (lh, ls) = (ui.line(Style::Header), ui.line(Style::Subtitle));
    let header = ui.face(Style::Header);
    let facts: Vec<Vec<String>> = playlists
        .iter()
        .map(|p| {
            let mut f = vec![count(p.entries.len(), glyphs::TRACK), length(p.duration())];
            if p.missing() > 0 {
                f.push(count(p.missing(), glyphs::MISSING));
            }
            f
        })
        .collect();
    let names: Vec<Vec<String>> = playlists.iter().map(|p| header.wrap(&p.name, left_w, 3)).collect();
    // all as tall as the tallest
    let left_h = names.iter().zip(&facts).map(|(n, f)| n.len() as f32 * lh + rem(0.5) + f.len() as f32 * ls).fold(0.0, f32::max);
    let card_h = 2.0 * pad + left_h.max(mosaic);
    let rh = card_h + gap;
    ui.push_clip(area);
    let top = area.y - grid.scroll.offset + size::half_gap();
    for (i, p) in playlists.iter().enumerate() {
        let (row, col) = (i / 2, i % 2);
        let cell = R::new(x0 + col as f32 * (card_w + gap), top + row as f32 * rh, card_w, card_h);
        if cell.bottom() < area.y || cell.y > area.bottom() {
            continue;
        }
        let over = ui.hovered(cell);
        if i == grid.selected {
            ui.rect(cell, if over { ui::HOVER } else { ui::SHADE });
        } else if over {
            ui.rect(cell, ui::CARD);
        }
        let mut y = cell.y + pad;
        for line in &names[i] {
            ui.text(Style::Header, line, cell.x + pad, y, left_w);
            y += lh;
        }
        y += rem(0.5);
        for f in &facts[i] {
            ui.text(Style::Subtitle, f, cell.x + pad, y, left_w);
            y += ls;
        }
        // the first sixteen albums, each once, in a 4x4 mosaic
        let mut albums: Vec<usize> = Vec::new();
        for entry in &p.entries {
            if let Some(a) = entry.album {
                if !albums.contains(&a) {
                    albums.push(a);
                    if albums.len() == SIDE * SIDE {
                        break;
                    }
                }
            }
        }
        let mx = cell.right() - pad - mosaic;
        for t in 0..SIDE * SIDE {
            let r = R::new(mx + (t % SIDE) as f32 * (tile + space), cell.y + pad + (t / SIDE) as f32 * (tile + space), tile, tile);
            let cover = albums.get(t).and_then(|&a| library.albums.get(a)).and_then(|a| app.pictures.cover(&a.key, a.cover_uri()));
            match cover {
                Some(tex) => ui.picture(tex, r),
                None => ui.placeholder(glyphs::ALBUM, r, false),
            }
        }
        // on the mosaic, as on an album's cover, the play button
        if (over || i == grid.selected) && p.missing() < p.entries.len() && play_button(ui, R::new(mx, cell.y + pad, mosaic, mosaic)) {
            app.act(Action::Load { source: Source::Playlist { path: p.path.clone() }, uris: p.uris(), start: 0 });
        }
        if ui.click(cell) {
            grid.selected = i;
            app.act(Action::OpenPlaylist(p.path.clone()));
        }
    }
    let rows = playlists.len().div_ceil(2);
    grid.scroll.content = size::half_gap() + rows as f32 * rh + rem(4.0) - gap;
    if std::mem::take(&mut grid.reveal) {
        let at = (grid.selected / 2) as f32 * rh;
        grid.scroll.reveal(at, at + rh + size::half_gap());
        grid.scroll.clamp();
        ui.wake(0.0);
    }
    ui.pop_clip();
    pages::scrollbar(ui, area, &grid.scroll);
}
