use std::collections::HashSet;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::config::{self, Shortcut};
use crate::covers::Pictures;
use crate::glyphs;
use crate::library::Library;
use crate::master::{self, Master, Source};
use crate::mpd::{Command, Event, Now};
use crate::pages::{self, ArtistPage, Plays, Tracks};
use crate::picker::{self, Kind, Modal};
use crate::archive::Job;
use crate::radio::{self, Seed, Vectors};
use crate::rl::{self, Input};
use crate::ui::{Scroll, Ui};

pub const TABS: [Tab; 4] = [Tab::Albums, Tab::Artists, Tab::Playlists, Tab::Queue];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Albums,
    Artists,
    Playlists,
    Queue,
}

impl Tab {
    pub fn index(self) -> usize {
        TABS.iter().position(|t| *t == self).unwrap()
    }

    pub fn glyph(self) -> &'static str {
        [glyphs::ALBUM, glyphs::ARTIST, glyphs::PLAYLIST, glyphs::QUEUE][self.index()]
    }
}

#[derive(Default)]
pub struct Grid {
    pub order: Vec<usize>,
    pub selected: usize,
    pub scroll: Scroll,
    pub columns: usize,
    pub reveal: bool,
}

impl Grid {
    pub fn step(&mut self, input: &Input) -> bool {
        let n = self.order.len() as i64;
        if n == 0 {
            return false;
        }
        let columns = self.columns.max(1) as i64;
        let at = self.selected as i64;
        let to = match config::get().which(&[Shortcut::Left, Shortcut::Right, Shortcut::Down, Shortcut::Up], input) {
            Some(Shortcut::Left) => at - 1,
            Some(Shortcut::Right) => at + 1,
            Some(Shortcut::Down) => at + columns,
            Some(Shortcut::Up) => at - columns,
            _ => return false,
        };
        self.selected = to.clamp(0, n - 1) as usize;
        self.reveal = true;
        true
    }
}

pub enum Page {
    Album(Tracks),
    Playlist(Tracks),
    Artist(ArtistPage),
}

pub enum Action {
    Tab(Tab),
    OpenArtist(String),
    OpenAlbum { key: String, track: usize },
    OpenPlaylist(PathBuf),
    Back,
    Forward,
    Load { source: Source, uris: Vec<String>, start: usize },
    Append(Vec<String>),
    Enqueue(Vec<String>),
    AddToPlaylist(PathBuf, Vec<String>),
    SaveQueueAs(String),
    Delete(PathBuf),
    Shuffle(PathBuf, bool),
    Mpd(Command),
    Picker(Kind),
    Modal(Modal),
    SelectRow(usize),
    QueueRow(Option<usize>),
    Retry(String),
    Run(Shortcut),
    OpenUrl(String),
    Zoom(i32),
    Refresh,
    Radio(Seed),
}

#[derive(Clone, Copy)]
pub struct Clock {
    pub since: Instant,
    pub elapsed: Duration,
    pub length: Duration,
}

pub struct App {
    pub commands: UnboundedSender<Command>,
    events: Receiver<Event>,
    archive: std::sync::mpsc::Sender<Job>,
    pub vectors: Option<Arc<Vectors>>,
    pub analyzing: Option<(usize, usize)>,
    pub linking: Option<(usize, usize)>,
    early: Vec<Event>,
    pub pictures: Pictures,
    pub library: Option<Arc<Library>>,
    pub now: Option<Now>,
    pub master: Master,
    pub clock: Option<Clock>,
    pub queue_row: Option<usize>,
    pub queue: Option<Tracks>,
    pub queue_scroll: Option<crate::ui::Scroll>,
    pub connected: bool,
    lost: bool,
    pub snack: Option<(String, Option<Instant>)>,
    pub tab: Tab,
    pub albums: Grid,
    pub artists: Grid,
    pub playlists: Grid,
    pub shown_at: Instant,
    /// The grids are dealt anew with the next library: asked by Refresh.
    redeal: bool,
    /// mpd's crossfade before a radio set its own, to put back when the radio ends.
    fade_before: Option<Duration>,
    pub pages: Vec<Page>,
    forward: Vec<Page>,
    under_queue: Option<(Tab, Vec<Page>)>,
    pub modal: Option<Modal>,
    pub actions: RefCell<Vec<Action>>,
    pub present: bool,
    pub quit: bool,
}

impl App {
    pub fn new(commands: UnboundedSender<Command>, events: Receiver<Event>, archive: std::sync::mpsc::Sender<Job>) -> App {
        App {
            pictures: Pictures::new(commands.clone()),
            commands,
            events,
            archive,
            vectors: None,
            analyzing: None,
            linking: None,
            early: Vec::new(),
            library: None,
            now: None,
            master: Master::load(),
            clock: None,
            queue_row: None,
            queue: None,
            queue_scroll: None,
            connected: true,
            lost: false,
            snack: None,
            tab: Tab::Albums,
            albums: Grid::default(),
            artists: Grid::default(),
            playlists: Grid::default(),
            shown_at: Instant::now(),
            redeal: false,
            fade_before: None,
            pages: Vec::new(),
            forward: Vec::new(),
            under_queue: None,
            modal: None,
            actions: RefCell::new(Vec::new()),
            present: false,
            quit: false,
        }
    }

    pub fn act(&self, action: Action) {
        self.actions.borrow_mut().push(action);
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn current(&self) -> (Option<&str>, bool) {
        let now = self.now.as_ref();
        (now.and_then(|n| n.song.as_ref()).map(|s| s.song.url.as_str()), now.is_some_and(Now::playing))
    }

    pub fn waiting(&self) -> Option<(Arc<Library>, usize, usize)> {
        let library = self.library.clone()?;
        let first = self.master.uris.first()?;
        let (a, t) = library.place(first)?;
        Some((library, a, t))
    }

    pub fn news(&mut self) -> bool {
        self.early.extend(self.events.try_iter());
        !self.early.is_empty() || self.pictures.news() || !self.actions.borrow().is_empty() || self.present
    }

    pub fn ask(&mut self, line: &str) {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match (word, rest) {
            ("tab", "albums") => self.act(Action::Tab(Tab::Albums)),
            ("tab", "artists") => self.act(Action::Tab(Tab::Artists)),
            ("tab", "playlists") => self.act(Action::Tab(Tab::Playlists)),
            ("tab", "queue") => self.act(Action::Tab(Tab::Queue)),
            ("artist", name) if !name.is_empty() => self.act(Action::OpenArtist(name.to_owned())),
            ("album", key) if !key.is_empty() => self.act(Action::OpenAlbum { key: key.to_owned(), track: 0 }),
            ("search", "") => self.act(Action::Picker(Kind::All)),
            ("commands", "") => self.act(Action::Picker(Kind::Commands)),
            ("escape", "") => {
                if self.modal.take().is_none() {
                    self.act(Action::Back);
                }
            }
            _ => eprintln!("asked: {line}: not understood"),
        }
    }

    pub fn notice(&mut self, text: String, for_: Option<Duration>) {
        self.snack = Some((text, for_.map(|d| Instant::now() + d)));
    }

    pub fn begin_frame(&mut self, ui: &Ui) -> bool {
        let mut any = self.pictures.begin_frame();
        let events: Vec<Event> = std::mem::take(&mut self.early).into_iter().chain(self.events.try_iter()).collect();
        for event in events {
            any = true;
            self.event(event, ui);
        }
        if let Some((_, Some(until))) = &self.snack {
            if Instant::now() >= *until {
                self.snack = None;
                any = true;
            }
        }
        if std::mem::take(&mut self.present) {
            unsafe { rl::SetWindowFocused() };
            focus_in_sway();
        }
        any
    }

    fn event(&mut self, event: Event, ui: &Ui) {
        match event {
            Event::Connected { address, version } => {
                eprintln!("mpd {version} at {address}");
                if std::mem::replace(&mut self.lost, false) {
                    self.notice(format!("{}  MPD", glyphs::CONNECTED), Some(Duration::from_secs(4)));
                }
            }
            Event::Disconnected { reason } => {
                if !std::mem::replace(&mut self.lost, true) {
                    eprintln!("mpd unreachable: {reason}");
                    self.notice(format!("{}  MPD", glyphs::DISCONNECTED), None);
                }
                self.connected = false;
            }
            Event::Status(now) => {
                self.connected = true;
                if let Some(song) = &now.song {
                    ui.fonts.learn(song.song.title().unwrap_or(&song.song.url));
                }
                self.now = Some(*now);
                self.set_clock();
                self.radio_tick();
            }
            Event::Library(library) => {
                let totals = [
                    pages::count(library.albums.len(), glyphs::ALBUM),
                    pages::count(library.artists.len(), glyphs::ARTIST),
                    pages::count(library.tracks(), glyphs::TRACK),
                    pages::count(library.playlists().len(), glyphs::PLAYLIST),
                ]
                .join("   ");
                learn(ui, &library);
                if self.library.is_some() {
                    self.notice(totals, Some(Duration::from_secs(4)));
                }
                let _ = self.archive.send(Job::Library(library.clone()));
                // the grids: dealt at random once, with the first library,
                // and again when Refresh asks; a library that comes on its
                // own (mpd's database changed) keeps the order, the albums
                // and artists gone dropped, the new ones shuffled in at the end
                let old = self.library.replace(library.clone());
                match old {
                    Some(old) if !std::mem::take(&mut self.redeal) => {
                        carry(&mut self.albums, library.albums.len(), |i| library.album_index(&old.albums[i].key));
                        carry(&mut self.artists, library.artists.len(), |i| library.artist(&old.artists[i].name));
                    }
                    _ => {
                        deal(&mut self.albums, library.albums.len());
                        deal(&mut self.artists, library.artists.len());
                    }
                }
                self.load_tab();
                self.refresh_pages();
                self.set_clock();
            }
            Event::Cover { key, path, failed } => self.pictures.cover_arrived(&key, path.as_deref(), failed),
            Event::Artist { name, info } => {
                if let Some(bio) = &info.bio {
                    ui.fonts.learn(bio);
                }
                self.pictures.artist_arrived(&name, &info);
            }
            Event::Queue(songs) => {
                let uris: Vec<String> = songs.iter().map(|(u, _)| u.clone()).collect();
                self.master.reconcile(&uris);
                self.set_clock();
                self.queue = None;
                self.radio_tick();
            }
            Event::Vectors(vectors) => {
                self.vectors = Some(vectors);
                self.radio_tick();
            }
            Event::Analyzing { done, total } => {
                if done >= total && self.analyzing.is_some() {
                    self.notice(format!("{}  {}", glyphs::RADIO, pages::count(total, glyphs::TRACK)), Some(Duration::from_secs(4)));
                }
                self.analyzing = (done < total).then_some((done, total));
            }
            Event::Linking { done, total } => self.linking = (done < total).then_some((done, total)),
            Event::Playlists(list) => {
                if let Some(library) = &self.library {
                    for p in &list {
                        ui.fonts.learn(&p.name);
                    }
                    let _ = self.archive.send(Job::Playlists(list.clone()));
                    library.set_playlists(list);
                }
            }
        }
    }

    fn set_clock(&mut self) {
        let Some(now) = &self.now else { return };
        self.clock = match (&now.song, now.status.elapsed) {
            (Some(entry), Some(elapsed)) => {
                let length = now.status.duration.or(entry.song.duration).unwrap_or_default();
                Some(Clock { since: Instant::now(), elapsed, length })
            }
            _ => self.waiting().map(|(library, a, t)| Clock {
                since: Instant::now(),
                elapsed: Duration::ZERO,
                length: library.albums[a].tracks[t].duration.unwrap_or_default(),
            }),
        };
    }

    pub fn load_tab(&mut self) {
        let Some(library) = self.library.clone() else { return };
        match self.tab {
            Tab::Albums | Tab::Artists => {}
            Tab::Playlists => {
                self.playlists.order = (0..library.playlists().len()).collect();
                self.playlists.selected = 0;
                self.playlists.scroll.offset = 0.0;
            }
            Tab::Queue => {
                self.queue = None;
                self.queue_scroll = None;
                self.queue_row = None;
            }
        }
        self.shown_at = Instant::now();
    }

    fn toggle_queue(&mut self) {
        self.forward.clear();
        match self.under_queue.take() {
            Some((tab, pages)) if self.tab == Tab::Queue => {
                self.tab = tab;
                self.pages = pages;
            }
            _ if self.tab == Tab::Queue => {}
            _ => {
                self.under_queue = Some((self.tab, std::mem::take(&mut self.pages)));
                self.tab = Tab::Queue;
                self.load_tab();
            }
        }
    }

    fn refresh_pages(&mut self) {
        let Some(library) = self.library.clone() else { return };
        let reopen = |pages: &mut Vec<Page>| {
            *pages = std::mem::take(pages).iter().filter_map(|p| reopen(p, &library)).collect();
        };
        reopen(&mut self.pages);
        reopen(&mut self.forward);
        if let Some((_, pages)) = &mut self.under_queue {
            reopen(pages);
        }
    }

    pub fn show_tab(&mut self, tab: Tab) {
        self.under_queue = None;
        self.pages.clear();
        self.forward.clear();
        self.tab = tab;
        self.load_tab();
    }

    /// Repeating one song ends with the queue it was set on: a new or
    /// grown queue puts the repeat off.
    fn unrepeat_one(&mut self) {
        if self.now.as_ref().is_some_and(|n| n.status.single == mpd_client::commands::SingleMode::Enabled) {
            self.send(Command::RepeatOff);
        }
    }

    pub fn load(&mut self, source: Source, uris: Vec<String>, start: usize) {
        self.unrepeat_one();
        let random = match &source {
            Source::Playlist { path } => master::shuffled(path),
            _ => false,
        };
        let start = if random && start == 0 && !uris.is_empty() { fastrand::usize(..uris.len()) } else { start };
        self.send(Command::Random(random));
        self.send(Command::Play { uris: uris.clone(), start });
        self.master.point(source, uris);
    }

    pub fn append(&mut self, uris: Vec<String>) {
        self.unrepeat_one();
        self.master.append(&uris);
        self.send(Command::Queue { uris });
    }

    pub fn add_to_playlist(&mut self, path: &Path, uris: &[String]) {
        for uri in uris {
            if let Err(e) = master::append_to_playlist(path, uri) {
                eprintln!("{}: {e}", path.display());
                return;
            }
        }
        if self.master.source == (Source::Playlist { path: path.to_path_buf() }) {
            self.master.uris.extend_from_slice(uris);
            self.master.save();
            self.send(Command::Queue { uris: uris.to_vec() });
        }
        self.send(Command::ReloadPlaylists);
    }

    pub fn enqueue(&mut self, uris: Vec<String>) {
        match self.master.source.clone() {
            Source::Playlist { path } => self.add_to_playlist(&path, &uris),
            _ => self.append(uris),
        }
    }

    pub fn save_queue_as(&mut self, name: &str) {
        let path = master::path_for(name);
        let uris = self.master.uris.clone();
        if let Err(e) = master::write_playlist(&path, &uris) {
            eprintln!("{}: {e}", path.display());
            return;
        }
        let (origin, url) = match &self.master.source {
            Source::Radio { seed: Seed::Track(uri) } => ("radio:track", Some(uri.clone())),
            Source::Radio { seed: Seed::Album(key) } => ("radio:album", Some(key.clone())),
            Source::Radio { seed: Seed::Artist(artist) } => ("radio:artist", Some(artist.clone())),
            _ => ("queue", None),
        };
        let _ = self.archive.send(Job::Origin { path: path.clone(), name: name.to_owned(), origin: origin.to_owned(), url });
        if matches!(self.master.source, Source::Radio { .. }) {
            self.notice(format!("{}  {name}", glyphs::PLAYLIST), Some(Duration::from_secs(4)));
        } else {
            self.master.point(Source::Playlist { path }, uris);
            self.queue = None;
        }
        self.send(Command::ReloadPlaylists);
    }

    fn delete(&mut self, path: &Path) {
        if let Err(e) = master::delete_playlist(path) {
            eprintln!("{}: {e}", path.display());
            return;
        }
        if self.master.source == (Source::Playlist { path: path.to_path_buf() }) {
            self.master.source = Source::Anon;
            self.master.save();
        }
        self.pages.pop();
        self.forward.clear();
        self.queue = None;
        self.send(Command::ReloadPlaylists);
    }

    pub fn keys(&mut self, ui: &mut Ui) {
        let inputs = std::mem::take(&mut ui.inputs);
        for input in &inputs {
            if let Input::Press(button) = *input {
                // the mouse's side buttons: back as Escape (a dialog closed,
                // else a page), forward to the page Back left
                if button == rl::MOUSE_BACK {
                    if self.modal.take().is_none() {
                        self.act(Action::Back);
                    }
                } else if button == rl::MOUSE_FORWARD && self.modal.is_none() {
                    self.act(Action::Forward);
                }
                continue;
            }
            let zoom = match config::get().which(&[Shortcut::ZoomIn, Shortcut::ZoomOut, Shortcut::ZoomReset], input) {
                Some(Shortcut::ZoomIn) => Some(1),
                Some(Shortcut::ZoomOut) => Some(-1),
                Some(_) => Some(0),
                None => None,
            };
            if let Some(step) = zoom {
                self.act(Action::Zoom(step));
                continue;
            }
            if let Some(mut modal) = self.modal.take() {
                let keep = modal.key(self, input);
                if keep && self.modal.is_none() {
                    self.modal = Some(modal);
                }
                continue;
            }
            if !self.window_key(input) {
                self.page_key(input);
            }
            let asked: Vec<Action> = {
                let mut actions = self.actions.borrow_mut();
                let (dialogs, rest) = std::mem::take(&mut *actions).into_iter().partition(|a| matches!(a, Action::Picker(_) | Action::Modal(_)));
                *actions = rest;
                dialogs
            };
            for action in asked {
                self.perform(action);
            }
        }
    }

    fn window_key(&mut self, input: &Input) -> bool {
        let global: Vec<Shortcut> = std::iter::once(Shortcut::Commands).chain(config::commands().into_iter().map(|(s, _, _)| s)).collect();
        match config::get().which(&global, input) {
            Some(shortcut) => {
                self.run(shortcut);
                true
            }
            None => matches!(input, Input::Key { code: rl::KEY_TAB, .. }),
        }
    }

    pub fn run(&mut self, shortcut: Shortcut) {
        use Shortcut as S;
        let now = self.now.as_ref().map(|n| (n.status.repeat, n.status.single == mpd_client::commands::SingleMode::Enabled));
        match shortcut {
            S::Commands => self.act(Action::Picker(Kind::Commands)),
            S::SearchAll => self.act(Action::Picker(Kind::All)),
            S::Search => {
                let kind = self.search_kind();
                self.act(Action::Picker(kind));
            }
            S::Albums => self.act(Action::Tab(Tab::Albums)),
            S::Artists => self.act(Action::Tab(Tab::Artists)),
            S::Playlists => self.act(Action::Tab(Tab::Playlists)),
            S::Queue => self.act(Action::Tab(Tab::Queue)),
            S::ToggleQueue => self.toggle_queue(),
            S::NextTab => self.act(Action::Tab(TABS[(self.tab.index() + 1) % TABS.len()])),
            S::PreviousTab => self.act(Action::Tab(TABS[(self.tab.index() + TABS.len() - 1) % TABS.len()])),
            S::Back => self.act(Action::Back),
            S::PlayPause => self.send(Command::Toggle),
            S::Stop => self.send(Command::Stop),
            S::Previous => self.send(Command::Previous),
            S::Next => self.send(Command::Next),
            S::SeekForward => self.send(Command::Forward(Duration::from_secs(10))),
            S::SeekBackward => self.send(Command::Backward(Duration::from_secs(10))),
            S::VolumeUp => self.send(Command::Volume(5)),
            S::VolumeDown => self.send(Command::Volume(-5)),
            S::Repeat => self.send(if now.is_some_and(|(r, single)| r && !single) { Command::RepeatOff } else { Command::RepeatAll }),
            S::RepeatOne => self.send(if now.is_some_and(|(r, single)| r && single) { Command::RepeatOff } else { Command::RepeatOne }),
            S::GoToPlaying => crate::player::open_source(self),
            S::Radio => {
                if let Some(seed) = self.seed_here() {
                    self.act(Action::Radio(seed));
                }
            }
            S::SaveQueue => {
                if !self.master.uris.is_empty() {
                    let proposed = self.library.as_ref().and_then(|l| self.master.radio_name(l));
                    self.act(Action::Modal(Modal::name(proposed)));
                }
            }
            S::ClearQueue => {
                self.act(Action::QueueRow(None));
                self.send(Command::Clear);
            }
            S::Refresh => self.act(Action::Refresh),
            S::ZoomIn => self.act(Action::Zoom(1)),
            S::ZoomOut => self.act(Action::Zoom(-1)),
            S::ZoomReset => self.act(Action::Zoom(0)),
            S::Quit => self.quit = true,
            _ => {}
        }
    }

    fn seed_here(&self) -> Option<Seed> {
        let library = self.library.as_ref()?;
        match self.pages.last() {
            Some(Page::Album(t) | Page::Playlist(t)) => t.selected_uri().map(Seed::Track),
            Some(Page::Artist(a)) => Some(Seed::Artist(a.library.artists[a.index].name.clone())),
            None => match self.tab {
                Tab::Queue => self.queue.as_ref().and_then(|q| q.selected_uri()).map(Seed::Track),
                Tab::Albums => self.albums.order.get(self.albums.selected).and_then(|&i| library.albums.get(i)).map(|a| Seed::Album(a.key.clone())),
                Tab::Artists => self.artists.order.get(self.artists.selected).and_then(|&i| library.artists.get(i)).map(|a| Seed::Artist(a.name.clone())),
                Tab::Playlists => None,
            },
        }
    }

    fn start_radio(&mut self, seed: Seed) {
        let Some(library) = self.library.clone() else { return };
        let mut uris = radio::opening(&library, &seed);
        let empty = Vectors::default();
        let vectors = self.vectors.as_deref().unwrap_or(&empty);
        let more = radio::path(&library, vectors, &seed, &uris, radio::ARCS * radio::STEP);
        uris.extend(more);
        if uris.is_empty() {
            self.notice(format!("{}  {}", glyphs::RADIO, glyphs::NOTHING), Some(Duration::from_secs(4)));
            return;
        }
        self.unrepeat_one();
        self.send(Command::Random(false));
        self.send(Command::Play { uris: uris.clone(), start: 0 });
        self.master.point(Source::Radio { seed }, uris);
        self.queue = None;
        if self.tab != Tab::Queue {
            self.toggle_queue();
        }
    }

    /// A radio has its crossfade (`[radio] crossfade`), the queue
    /// otherwise what mpd had before the radio.
    fn fade_tick(&mut self) {
        let Some(now) = &self.now else { return };
        let wanted = crate::config::get().crossfade;
        match (matches!(self.master.source, Source::Radio { .. }), self.fade_before) {
            (true, None) => {
                // a radio found already on at the start (the app restarted into it) has the fade set by this app
                self.fade_before = Some(if now.status.crossfade == wanted { Duration::ZERO } else { now.status.crossfade });
                if now.status.crossfade != wanted {
                    self.send(Command::Crossfade(wanted));
                }
            }
            (false, Some(before)) => {
                self.fade_before = None;
                if now.status.crossfade != before {
                    self.send(Command::Crossfade(before));
                }
            }
            _ => {}
        }
    }

    fn radio_tick(&mut self) {
        self.fade_tick();
        let Source::Radio { seed } = &self.master.source else { return };
        let (Some(library), Some(now), Some(vectors)) = (&self.library, &self.now, &self.vectors) else { return };
        if now.status.state == mpd_client::responses::PlayState::Stopped {
            return;
        }
        let Some((pos, _)) = now.status.current_song else { return };
        // the next arc comes as the last one starts
        let left = self.master.uris.len().saturating_sub(pos.0 + 1);
        if left >= radio::STEP {
            return;
        }
        let more = radio::path(library, vectors, seed, &self.master.uris, radio::STEP);
        if more.is_empty() {
            return;
        }
        self.master.grow(&more);
        self.send(Command::Queue { uris: more });
    }

    fn search_kind(&self) -> Kind {
        match self.pages.last() {
            Some(Page::Album(_) | Page::Playlist(_)) => Kind::Tracks,
            Some(Page::Artist(a)) => Kind::Artist(a.index),
            None => match self.tab {
                Tab::Albums => Kind::Albums,
                Tab::Artists => Kind::Artists,
                Tab::Playlists => Kind::Playlists,
                Tab::Queue => Kind::ToQueue,
            },
        }
    }

    fn page_key(&mut self, input: &Input) {
        let enter = config::get().is(Shortcut::Open, input);
        let mut pages = std::mem::take(&mut self.pages);
        match pages.last_mut() {
            Some(Page::Album(t) | Page::Playlist(t)) => pages::tracks_key(self, t, input),
            Some(Page::Artist(a)) => {
                if !a.grid.step(input) && enter {
                    if let Some(&album) = a.grid.order.get(a.grid.selected) {
                        self.act(Action::OpenAlbum { key: a.library.albums[album].key.clone(), track: 0 });
                    }
                }
            }
            None => {}
        }
        self.pages = pages;
        if self.pages.is_empty() {
            self.root_key(input, enter);
        }
    }

    fn root_key(&mut self, input: &Input, enter: bool) {
        if self.tab == Tab::Queue {
            if let Some(mut queue) = self.queue.take() {
                pages::tracks_key(self, &mut queue, input);
                self.queue = Some(queue);
            }
            return;
        }
        let library = self.library.clone();
        let grid = match self.tab {
            Tab::Albums => &mut self.albums,
            Tab::Artists => &mut self.artists,
            _ => &mut self.playlists,
        };
        if grid.step(input) || !enter {
            return;
        }
        let Some(&i) = grid.order.get(grid.selected) else { return };
        let Some(library) = library else { return };
        let action = match self.tab {
            Tab::Albums => library.albums.get(i).map(|a| Action::OpenAlbum { key: a.key.clone(), track: 0 }),
            Tab::Artists => library.artists.get(i).map(|a| Action::OpenArtist(a.name.clone())),
            _ => library.playlists().get(i).map(|p| Action::OpenPlaylist(p.path.clone())),
        };
        if let Some(action) = action {
            self.act(action);
        }
    }

    pub fn apply(&mut self) -> bool {
        let actions = std::mem::take(&mut *self.actions.borrow_mut());
        let any = !actions.is_empty();
        for action in actions {
            self.perform(action);
        }
        any
    }

    /// A page opened over the stack: what was ahead, left by Back, is
    /// forgotten, as a browser's forward is.
    fn open(&mut self, page: Page) {
        self.forward.clear();
        self.pages.push(page);
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::Tab(tab) => self.show_tab(tab),
            Action::OpenArtist(name) => {
                let Some(library) = self.library.clone() else { return };
                let Some(index) = library.artist(&name) else { return };
                if !matches!(self.pages.last(), Some(Page::Artist(a)) if a.name() == name) {
                    self.open(Page::Artist(ArtistPage::new(&library, index)));
                }
            }
            Action::OpenAlbum { key, track } => {
                let Some(library) = self.library.clone() else { return };
                if let Some(Page::Album(t)) = self.pages.last_mut() {
                    if t.plays == Plays::Load(Source::Album { key: key.clone() }) {
                        t.select(track);
                        return;
                    }
                }
                if let Some(album) = library.album_index(&key) {
                    self.open(Page::Album(Tracks::album(&library, album, track)));
                }
            }
            Action::OpenPlaylist(path) => {
                let Some(library) = self.library.clone() else { return };
                if matches!(self.pages.last(), Some(Page::Playlist(t)) if t.plays == Plays::Load(Source::Playlist { path: path.clone() })) {
                    return;
                }
                let page = library.playlists().iter().find(|p| p.path == path).map(|p| Tracks::playlist(&library, p));
                if let Some(page) = page {
                    self.open(Page::Playlist(page));
                }
            }
            Action::Back => {
                if let Some(page) = self.pages.pop() {
                    self.forward.push(page);
                } else if self.tab == Tab::Queue && self.under_queue.is_some() {
                    self.toggle_queue();
                }
            }
            Action::Forward => {
                if let Some(page) = self.forward.pop() {
                    self.pages.push(page);
                }
            }
            Action::Load { source, uris, start } => self.load(source, uris, start),
            Action::Append(uris) => self.append(uris),
            Action::Enqueue(uris) => self.enqueue(uris),
            Action::AddToPlaylist(path, uris) => self.add_to_playlist(&path, &uris),
            Action::SaveQueueAs(name) => self.save_queue_as(&name),
            Action::Delete(path) => self.delete(&path),
            Action::Shuffle(path, on) => {
                if let Err(e) = master::set_shuffle(&path, on) {
                    eprintln!("{}: {e}", path.display());
                    return;
                }
                let playlists = self.pages.iter_mut().filter_map(|page| match page {
                    Page::Playlist(t) => Some(t),
                    _ => None,
                });
                for t in playlists.chain(self.queue.as_mut()) {
                    if let Some((p, s)) = &mut t.shuffle {
                        if *p == path {
                            *s = on;
                        }
                    }
                }
                if self.master.source == (Source::Playlist { path }) {
                    self.send(Command::Random(on));
                }
                if let Some(library) = &self.library {
                    let _ = self.archive.send(Job::Playlists(library.playlists().clone()));
                }
            }
            Action::Mpd(Command::Seek(to)) => {
                if let Some(clock) = &mut self.clock {
                    clock.elapsed = to.min(clock.length);
                    clock.since = Instant::now();
                }
                self.send(Command::Seek(to));
            }
            Action::Mpd(c) => self.send(c),
            Action::Picker(kind) => {
                if let Some(p) = picker::Picker::open(self, kind) {
                    self.modal = Some(Modal::Picker(Box::new(p)));
                }
            }
            Action::Modal(m) => self.modal = Some(m),
            Action::SelectRow(i) => {
                if let Some(Page::Album(t) | Page::Playlist(t)) = self.pages.last_mut() {
                    t.select(i);
                }
            }
            Action::QueueRow(row) => self.queue_row = row,
            Action::Retry(name) => self.pictures.retry(&name),
            Action::Run(shortcut) => self.run(shortcut),
            Action::OpenUrl(url) => {
                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
            }
            Action::Zoom(0) => crate::size::reset(),
            Action::Zoom(step) => crate::size::zoom(step),
            Action::Refresh => {
                self.notice(format!("{}  MPD", glyphs::REFRESH), None);
                self.redeal = true;
                self.send(Command::Refresh);
            }
            Action::Radio(seed) => self.start_radio(seed),
        }
    }
}

fn reopen(page: &Page, library: &Arc<Library>) -> Option<Page> {
    match page {
        Page::Album(t) => {
            let Plays::Load(Source::Album { key }) = &t.plays else { return None };
            let mut new = Tracks::album(library, library.album_index(key)?, t.selected.unwrap_or(0));
            new.scroll = t.scroll;
            new.reveal = false;
            Some(Page::Album(new))
        }
        Page::Playlist(t) => {
            let Plays::Load(Source::Playlist { path }) = &t.plays else { return None };
            let playlists = library.playlists();
            let mut new = Tracks::playlist(library, playlists.iter().find(|p| p.path == *path)?);
            new.selected = t.selected.filter(|&i| new.rows.get(i).is_some_and(|r| r.start.is_some()));
            new.scroll = t.scroll;
            new.reveal = false;
            Some(Page::Playlist(new))
        }
        Page::Artist(a) => {
            let mut new = ArtistPage::new(library, library.artist(a.name())?);
            new.grid.selected = a.grid.selected.min(new.grid.order.len().saturating_sub(1));
            new.grid.scroll = a.grid.scroll;
            new.grid.reveal = false;
            new.words = a.words;
            Some(Page::Artist(new))
        }
    }
}

/// A grid dealt at random, from the top.
fn deal(grid: &mut Grid, n: usize) {
    grid.order = (0..n).collect();
    fastrand::shuffle(&mut grid.order);
    grid.selected = grid.selected.min(n.saturating_sub(1));
    grid.scroll.offset = 0.0;
    grid.reveal = true;
}

/// A grid carried over to a new library: the order kept where its
/// cards are still there (`place` gives the new index of an old one),
/// the newcomers shuffled in at the end, the scroll where it was.
fn carry(grid: &mut Grid, n: usize, place: impl Fn(usize) -> Option<usize>) {
    let mut order: Vec<usize> = grid.order.iter().filter_map(|&i| place(i)).collect();
    let present: HashSet<usize> = order.iter().copied().collect();
    let mut new: Vec<usize> = (0..n).filter(|i| !present.contains(i)).collect();
    fastrand::shuffle(&mut new);
    order.extend(new);
    grid.order = order;
    grid.selected = grid.selected.min(n.saturating_sub(1));
}

fn learn(ui: &Ui, library: &Library) {
    let mut all = String::new();
    for album in &library.albums {
        all.push_str(&album.title);
        all.push_str(&album.artist);
        for name in album.names() {
            all.push_str(name);
        }
        for t in &album.tracks {
            all.push_str(&t.title);
            all.push_str(&t.artist);
        }
    }
    for p in library.playlists().iter() {
        all.push_str(&p.name);
        for e in &p.entries {
            all.push_str(&e.title);
        }
    }
    ui.fonts.learn(&all);
}

fn focus_in_sway() {
    if std::env::var_os("SWAYSOCK").is_some() {
        let _ = std::process::Command::new("swaymsg").arg("[app_id=\"^cards\\.random\\.Mascarpone$\"] focus").stdout(std::process::Stdio::null()).spawn();
    }
}
