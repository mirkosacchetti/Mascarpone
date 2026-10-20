//! What the window knows and does: the library and the status mpd sent
//! last, the queue's pointer, the tabs and the pages open over them, the
//! dialog up. Everything on screen is drawn from here every frame; what a
//! click or a key asks is queued as an action while the frame draws and
//! done after it.

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
use crate::pages::{self, ArtistPage, Tracks};
use crate::picker::{self, Kind, Modal};
use crate::rl::{self, Input};
use crate::ui::{Scroll, Ui};

/// The root page's tabs, in Ctrl+Tab order.
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

/// A grid's cards: in the order dealt, the one selected, where it is
/// scrolled; and how many go across, as the last frame laid them.
#[derive(Default)]
pub struct Grid {
    pub order: Vec<usize>,
    pub selected: usize,
    pub scroll: Scroll,
    pub columns: usize,
    /// The selected card scrolled into view at the next frame.
    pub reveal: bool,
    /// The library the order was dealt from.
    pub dealt: Option<Arc<Library>>,
}

impl Grid {
    /// The keys of a grid (sway's by default: j and ; a card left and
    /// right, k and l a row down and up). True when the selection moved.
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

/// A page over the root.
pub enum Page {
    Album(Tracks),
    Playlist(Tracks),
    Artist(ArtistPage),
}

/// What a frame asks, done after it.
#[allow(clippy::large_enum_variant)]
pub enum Action {
    /// Back to the root page, on the tab, its content dealt again.
    Tab(Tab),
    Push(Page),
    /// An artist's page, unless it is the one up.
    OpenArtist(String),
    /// An album's page from the library as it is now, the track selected.
    OpenAlbum { album: usize, track: usize },
    OpenPlaylist(PathBuf),
    Back,
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
    /// A row selected on the tracks page up (the picker's Enter).
    SelectRow(usize),
    QueueRow(Option<usize>),
    Retry(String),
    /// A command picked in the commands' picker.
    Run(Shortcut),
    OpenUrl(String),
    Zoom(i32),
    Refresh,
}

/// The song's clock between two statuses: the moment the status came,
/// the song's elapsed and length.
#[derive(Clone, Copy)]
pub struct Clock {
    pub since: Instant,
    pub elapsed: Duration,
    pub length: Duration,
}

pub struct App {
    pub commands: UnboundedSender<Command>,
    events: Receiver<Event>,
    /// Taken off the channel before a wait, for the next frame.
    early: Vec<Event>,
    pub pictures: Pictures,
    pub library: Option<Arc<Library>>,
    /// The last status mpd sent.
    pub now: Option<Now>,
    pub master: Master,
    pub clock: Option<Clock>,
    /// The row selected on the queue's tab, kept over its rebuilds.
    pub queue_row: Option<usize>,
    /// The queue's tab, built again when it is shown or changes.
    pub queue: Option<Tracks>,
    /// Where the queue's tab was scrolled, kept over its rebuilds.
    pub queue_scroll: Option<crate::ui::Scroll>,
    /// mpd answers: the player can be used.
    pub connected: bool,
    lost: bool,
    /// The word under the tabs, and until when.
    pub snack: Option<(String, Option<Instant>)>,
    pub tab: Tab,
    pub albums: Grid,
    pub artists: Grid,
    pub playlists: Grid,
    /// The tab built again at this moment: hidden until its pictures are
    /// in, a quarter second at most, so no frame of placeholders comes
    /// first.
    pub shown_at: Instant,
    pub pages: Vec<Page>,
    /// The tab and its pages the queue was shown over (toggle queue),
    /// until it is hidden again or another tab comes up.
    under_queue: Option<(Tab, Vec<Page>)>,
    pub modal: Option<Modal>,
    pub actions: RefCell<Vec<Action>>,
    /// Asked by another launch to come up.
    pub present: bool,
    /// Asked to end.
    pub quit: bool,
}

impl App {
    pub fn new(commands: UnboundedSender<Command>, events: Receiver<Event>) -> App {
        App {
            pictures: Pictures::new(commands.clone()),
            commands,
            events,
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
            pages: Vec::new(),
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

    /// The uri of the song on, and whether it plays.
    pub fn current(&self) -> (Option<&str>, bool) {
        let now = self.now.as_ref();
        (now.and_then(|n| n.song.as_ref()).map(|s| s.song.url.as_str()), now.is_some_and(Now::playing))
    }

    /// The track the queue waits on when mpd has stopped at its end: its
    /// first, ready to play again; its album and place in it.
    pub fn waiting(&self) -> Option<(Arc<Library>, usize, usize)> {
        let library = self.library.clone()?;
        let first = self.master.uris.first()?;
        let (a, t) = library.place(first)?;
        Some((library, a, t))
    }

    /// Something to do before the next frame: no waiting for events.
    pub fn news(&mut self) -> bool {
        self.early.extend(self.events.try_iter());
        !self.early.is_empty() || self.pictures.news() || !self.actions.borrow().is_empty() || self.present
    }

    /// A word under the tabs, for a while or until the next one.
    pub fn notice(&mut self, text: String, for_: Option<Duration>) {
        self.snack = Some((text, for_.map(|d| Instant::now() + d)));
    }

    /// At the start of a frame: what mpd and the decoders sent. True when
    /// anything came.
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
            // mpd lost and found again, said under the tabs
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
                // the first load is the start, the next ones are news
                if self.library.is_some() {
                    self.notice(totals, Some(Duration::from_secs(4)));
                }
                self.library = Some(library);
                // the tab on screen dealt again; the others when shown
                self.load_tab();
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
                // stopped, the track waiting may be another
                self.set_clock();
                self.queue = None;
            }
            Event::Playlists(list) => {
                if let Some(library) = &self.library {
                    for p in &list {
                        ui.fonts.learn(&p.name);
                    }
                    library.set_playlists(list);
                }
            }
        }
    }

    /// The clock set again from the last status and the queue.
    fn set_clock(&mut self) {
        let Some(now) = &self.now else { return };
        self.clock = match (&now.song, now.status.elapsed) {
            (Some(entry), Some(elapsed)) => {
                let length = now.status.duration.or(entry.song.duration).unwrap_or_default();
                Some(Clock { since: Instant::now(), elapsed, length })
            }
            // the waiting first track: at its start
            _ => self.waiting().map(|(library, a, t)| Clock {
                since: Instant::now(),
                elapsed: Duration::ZERO,
                length: library.albums[a].tracks[t].duration.unwrap_or_default(),
            }),
        };
    }

    /// The tab on screen built again: albums and artists dealt in a
    /// random order (the picker keeps the alphabet), the queue as mpd has
    /// it; hidden until its pictures are in.
    pub fn load_tab(&mut self) {
        let Some(library) = self.library.clone() else { return };
        match self.tab {
            Tab::Albums => deal(&mut self.albums, &library, library.albums.len()),
            Tab::Artists => deal(&mut self.artists, &library, library.artists.len()),
            Tab::Playlists => {
                self.playlists.order = (0..library.playlists().len()).collect();
                self.playlists.selected = 0;
                self.playlists.scroll.offset = 0.0;
            }
            // shown anew: on the song on, in the middle
            Tab::Queue => {
                self.queue = None;
                self.queue_scroll = None;
                self.queue_row = None;
            }
        }
        self.shown_at = Instant::now();
    }

    /// The queue shown over the tab and the pages up, or, shown so,
    /// hidden: the tab and its pages back as they were, not dealt again.
    fn toggle_queue(&mut self) {
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

    /// The tab shown, back on the root page, its content built again.
    pub fn show_tab(&mut self, tab: Tab) {
        self.under_queue = None;
        self.pages.clear();
        self.tab = tab;
        self.load_tab();
    }

    /// Loads a list on the queue and plays it from `start`: the master
    /// points to it. A playlist marked shuffle plays in a random order
    /// (mpd's random mode, the queue in the file's order), anything else
    /// in order.
    pub fn load(&mut self, source: Source, uris: Vec<String>, start: usize) {
        let random = match &source {
            Source::Playlist { path } => master::shuffled(path),
            _ => false,
        };
        // shuffled from the top: any track first, not always the first
        let start = if random && start == 0 && !uris.is_empty() { fastrand::usize(..uris.len()) } else { start };
        self.send(Command::Random(random));
        self.send(Command::Play { uris: uris.clone(), start });
        self.master.point(source, uris);
    }

    /// Appends to the queue: the master's pointer drops.
    pub fn append(&mut self, uris: Vec<String>) {
        self.master.append(&uris);
        self.send(Command::Queue { uris });
    }

    /// Tracks into a playlist file; into the queue too when the master
    /// points to that playlist.
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

    /// Tracks onto the queue from its tab: a playlist file the master
    /// points to grows with it, anything else is detached.
    pub fn enqueue(&mut self, uris: Vec<String>) {
        match self.master.source.clone() {
            Source::Playlist { path } => self.add_to_playlist(&path, &uris),
            _ => self.append(uris),
        }
    }

    /// The queue saved as a playlist file, which the master then points to.
    pub fn save_queue_as(&mut self, name: &str) {
        let path = master::path_for(name);
        let uris = self.master.uris.clone();
        if let Err(e) = master::write_playlist(&path, &uris) {
            eprintln!("{}: {e}", path.display());
            return;
        }
        self.master.point(Source::Playlist { path }, uris);
        self.queue = None;
        self.send(Command::ReloadPlaylists);
    }

    /// A playlist deleted (see master::delete_playlist): its page closes,
    /// and a queue it was the origin of stays as it is, the Anon Playlist.
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
        self.queue = None;
        self.send(Command::ReloadPlaylists);
    }

    /// The keys of this frame: a dialog's own when one is up; else the
    /// window's, then the page's.
    pub fn keys(&mut self, ui: &mut Ui) {
        let inputs = std::mem::take(&mut ui.inputs);
        for input in &inputs {
            // the zoom anywhere, under a dialog too
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
            // a dialog asked for is up at once: the keys after this one,
            // typed before the frame is drawn, are its own
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

    /// The window's keys: everything the commands' picker lists, and
    /// the picker itself.
    fn window_key(&mut self, input: &Input) -> bool {
        let global: Vec<Shortcut> = std::iter::once(Shortcut::Commands).chain(config::commands().into_iter().map(|(s, _, _)| s)).collect();
        match config::get().which(&global, input) {
            Some(shortcut) => {
                self.run(shortcut);
                true
            }
            // Tab, when no action has it (by default toggle queue): nothing
            None => matches!(input, Input::Key { code: rl::KEY_TAB, .. }),
        }
    }

    /// A command, from its key or from the commands' picker: the pickers,
    /// the tabs (from any page, dealt again like a click; the next and the
    /// one before, back on the root page first), back a page, the player,
    /// the queue, the zoom.
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
            // repeat one off: no repeat at all; repeat off from repeat one too
            S::Repeat => self.send(if now.is_some_and(|(r, single)| r && !single) { Command::RepeatOff } else { Command::RepeatAll }),
            S::RepeatOne => self.send(if now.is_some_and(|(r, single)| r && single) { Command::RepeatOff } else { Command::RepeatOne }),
            S::GoToPlaying => crate::player::open_source(self),
            S::SaveQueue => {
                if !self.master.uris.is_empty() {
                    self.act(Action::Modal(Modal::name()));
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
            // a page's own keys, and the picker's
            _ => {}
        }
    }

    /// What Ctrl+F searches: a tracks page its rows, an artist's page
    /// that artist; the root page its tab, the queue's albums and tracks
    /// to append to it.
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
        // the page out while it takes the key: what it asks is queued, the
        // pages stay as they are until the frame is drawn
        let mut pages = std::mem::take(&mut self.pages);
        match pages.last_mut() {
            Some(Page::Album(t) | Page::Playlist(t)) => pages::tracks_key(self, t, input),
            Some(Page::Artist(a)) => {
                if !a.grid.step(input) && enter {
                    if let Some(&album) = a.grid.order.get(a.grid.selected) {
                        self.act(Action::Push(Page::Album(Tracks::album(&a.library, album, 0))));
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

    /// The root page's keys: the queue's rows, or a grid's cards.
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
        let action = match self.tab {
            Tab::Albums => Some(Action::OpenAlbum { album: i, track: 0 }),
            Tab::Artists => library.map(|l| Action::Push(Page::Artist(ArtistPage::new(&l, i)))),
            _ => library.and_then(|l| l.playlists().get(i).map(|p| Action::OpenPlaylist(p.path.clone()))),
        };
        if let Some(action) = action {
            self.act(action);
        }
    }

    /// What the frame asked, done.
    pub fn apply(&mut self) -> bool {
        let actions = std::mem::take(&mut *self.actions.borrow_mut());
        let any = !actions.is_empty();
        for action in actions {
            self.perform(action);
        }
        any
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::Tab(tab) => self.show_tab(tab),
            Action::Push(page) => self.pages.push(page),
            Action::OpenArtist(name) => {
                let Some(library) = self.library.clone() else { return };
                let Some(index) = library.artist(&name) else { return };
                if !matches!(self.pages.last(), Some(Page::Artist(a)) if a.index == index && Arc::ptr_eq(&a.library, &library)) {
                    self.pages.push(Page::Artist(ArtistPage::new(&library, index)));
                }
            }
            Action::OpenAlbum { album, track } => {
                if let Some(library) = self.library.clone() {
                    if album < library.albums.len() {
                        self.pages.push(Page::Album(Tracks::album(&library, album, track)));
                    }
                }
            }
            Action::OpenPlaylist(path) => {
                let Some(library) = self.library.clone() else { return };
                let page = library.playlists().iter().find(|p| p.path == path).map(|p| Tracks::playlist(&library, p));
                if let Some(page) = page {
                    self.pages.push(Page::Playlist(page));
                }
            }
            Action::Back => {
                self.pages.pop();
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
                // on the queue at once when it plays this
                if self.master.source == (Source::Playlist { path }) {
                    self.send(Command::Random(on));
                }
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
                self.send(Command::Refresh);
            }
        }
    }
}

/// A grid dealt again in a random order, the selection kept in its
/// place.
fn deal(grid: &mut Grid, library: &Arc<Library>, n: usize) {
    grid.order = (0..n).collect();
    fastrand::shuffle(&mut grid.order);
    grid.selected = grid.selected.min(n.saturating_sub(1));
    grid.scroll.offset = 0.0;
    grid.reveal = true;
    grid.dealt = Some(library.clone());
}

/// Every name of the library noted, for the faces to draw.
fn learn(ui: &Ui, library: &Library) {
    let mut all = String::new();
    for album in &library.albums {
        all.push_str(&album.title);
        all.push_str(&album.artist);
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

/// Launched again: sway shows the window, even from the scratchpad.
fn focus_in_sway() {
    if std::env::var_os("SWAYSOCK").is_some() {
        let _ = std::process::Command::new("swaymsg").arg("[app_id=\"^cards\\.random\\.Mascarpone$\"] focus").stdout(std::process::Stdio::null()).spawn();
    }
}
