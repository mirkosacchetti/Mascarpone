use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use crate::rl::{self, Input};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shortcut {
    Commands,
    SearchAll,
    Search,
    Albums,
    Artists,
    Playlists,
    Queue,
    ToggleQueue,
    NextTab,
    PreviousTab,
    Back,
    PlayPause,
    Stop,
    Previous,
    Next,
    SeekForward,
    SeekBackward,
    VolumeUp,
    VolumeDown,
    Repeat,
    RepeatOne,
    GoToPlaying,
    Radio,
    SaveQueue,
    ClearQueue,
    Refresh,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Quit,
    Left,
    Right,
    Down,
    Up,
    Open,
    AddToPlaylist,
    Remove,
    MoveDown,
    MoveUp,
    PickerNext,
    PickerPrevious,
    PickerPageDown,
    PickerPageUp,
    PickerOpen,
    PickerClose,
}

const ACTIONS: &[(Shortcut, &str, &str, Option<&str>)] = &[
    (Shortcut::Commands, "commands", "Ctrl+k", None),
    (Shortcut::SearchAll, "search_all", "/", Some("Search everything")),
    (Shortcut::Search, "search", "Ctrl+f", Some("Search what is on screen")),
    (Shortcut::Albums, "albums", "1", Some("Albums")),
    (Shortcut::Artists, "artists", "2", Some("Artists")),
    (Shortcut::Playlists, "playlists", "3", Some("Playlists")),
    (Shortcut::Queue, "queue", "4", Some("Queue")),
    (Shortcut::ToggleQueue, "toggle_queue", "Tab", Some("Show or hide the queue")),
    (Shortcut::NextTab, "next_tab", "Ctrl+Tab", Some("Next tab")),
    (Shortcut::PreviousTab, "previous_tab", "Ctrl+Shift+Tab", Some("Previous tab")),
    (Shortcut::Back, "back", "Escape", Some("Back a page")),
    (Shortcut::PlayPause, "play_pause", "Space c", Some("Play or pause")),
    (Shortcut::Stop, "stop", "x", Some("Stop")),
    (Shortcut::Previous, "previous", "z", Some("Previous song")),
    (Shortcut::Next, "next", "v", Some("Next song")),
    (Shortcut::SeekForward, "seek_forward", "]", Some("Ten seconds forward")),
    (Shortcut::SeekBackward, "seek_backward", "[", Some("Ten seconds back")),
    (Shortcut::VolumeUp, "volume_up", "+ =", Some("Volume up")),
    (Shortcut::VolumeDown, "volume_down", "-", Some("Volume down")),
    (Shortcut::Repeat, "repeat", "r", Some("Repeat the queue, on or off")),
    (Shortcut::RepeatOne, "repeat_one", "R", Some("Repeat the song, on or off")),
    (Shortcut::GoToPlaying, "go_to_playing", "g", Some("Go to what plays: its album or playlist")),
    (Shortcut::Radio, "radio", "s", Some("Radio from what is selected: a track, an album, an artist")),
    (Shortcut::SaveQueue, "save_queue", "", Some("Save the queue as a playlist")),
    (Shortcut::ClearQueue, "clear_queue", "", Some("Clear the queue")),
    (Shortcut::Refresh, "refresh", "Ctrl+r", Some("Refresh: mpd's database, the library, the grids dealt anew")),
    (Shortcut::ZoomIn, "zoom_in", "Ctrl+= Ctrl++ Ctrl+KP_Add", Some("Zoom in")),
    (Shortcut::ZoomOut, "zoom_out", "Ctrl+- Ctrl+KP_Subtract", Some("Zoom out")),
    (Shortcut::ZoomReset, "zoom_reset", "Ctrl+0 Ctrl+KP_0", Some("Zoom back to 100%")),
    (Shortcut::Quit, "quit", "Ctrl+q", Some("Quit")),
    (Shortcut::Left, "left", "j Left", None),
    (Shortcut::Right, "right", "; Right", None),
    (Shortcut::Down, "down", "k Down", None),
    (Shortcut::Up, "up", "l Up", None),
    (Shortcut::Open, "open", "Enter", None),
    (Shortcut::AddToPlaylist, "add_to_playlist", "a", None),
    (Shortcut::Remove, "remove", "d Delete", None),
    (Shortcut::MoveDown, "move_down", "K", None),
    (Shortcut::MoveUp, "move_up", "L", None),
    (Shortcut::PickerNext, "picker_next", "Down Tab Ctrl+n Ctrl+k", None),
    (Shortcut::PickerPrevious, "picker_previous", "Up Shift+Tab Ctrl+p Ctrl+l", None),
    (Shortcut::PickerPageDown, "picker_page_down", "PageDown", None),
    (Shortcut::PickerPageUp, "picker_page_up", "PageUp", None),
    (Shortcut::PickerOpen, "picker_open", "Enter", None),
    (Shortcut::PickerClose, "picker_close", "Escape Ctrl+g", None),
];

pub fn commands() -> Vec<(Shortcut, &'static str, String)> {
    ACTIONS.iter().filter_map(|(a, _, _, title)| Some((*a, (*title)?, get().labels.get(a).cloned().unwrap_or_default()))).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Chord {
    ctrl: bool,
    alt: bool,
    shift: bool,
    key: Key,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Key {
    Char(char),
    Code(i32),
}

const NAMES: &[(&str, i32)] = &[
    ("Escape", rl::KEY_ESCAPE),
    ("Esc", rl::KEY_ESCAPE),
    ("Enter", rl::KEY_ENTER),
    ("Return", rl::KEY_ENTER),
    ("Tab", rl::KEY_TAB),
    ("Backspace", rl::KEY_BACKSPACE),
    ("Delete", rl::KEY_DELETE),
    ("Up", rl::KEY_UP),
    ("Down", rl::KEY_DOWN),
    ("Left", rl::KEY_LEFT),
    ("Right", rl::KEY_RIGHT),
    ("PageUp", rl::KEY_PAGE_UP),
    ("PageDown", rl::KEY_PAGE_DOWN),
    ("Home", rl::KEY_HOME),
    ("End", rl::KEY_END),
    ("KP_Add", rl::KEY_KP_ADD),
    ("KP_Subtract", rl::KEY_KP_SUBTRACT),
    ("KP_0", rl::KEY_KP_0),
    ("KP_Enter", rl::KEY_KP_ENTER),
];

impl Chord {
    fn parse(text: &str) -> Option<Chord> {
        let mut chord = Chord { ctrl: false, alt: false, shift: false, key: Key::Char(' ') };
        let mut rest = text;
        loop {
            let lower = rest.to_ascii_lowercase();
            let take = |prefix: &str| lower.starts_with(prefix) && rest.len() > prefix.len();
            if take("ctrl+") {
                chord.ctrl = true;
                rest = &rest[5..];
            } else if take("alt+") {
                chord.alt = true;
                rest = &rest[4..];
            } else if take("shift+") {
                chord.shift = true;
                rest = &rest[6..];
            } else {
                break;
            }
        }
        let mut chars = rest.chars();
        chord.key = match (chars.next(), chars.next()) {
            (Some(c), None) => Key::Char(c),
            _ if rest.eq_ignore_ascii_case("space") => Key::Char(' '),
            _ => Key::Code(NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(rest))?.1),
        };
        Some(chord)
    }

    fn matches(&self, input: &Input) -> bool {
        match *input {
            Input::Char(c) => !self.ctrl && !self.alt && self.key == Key::Char(c),
            Input::Key { code, mods } => {
                let ctrl = mods & rl::MOD_CONTROL != 0;
                let alt = mods & rl::MOD_ALT != 0;
                let printable = rl::key_char(code);
                if printable.is_some() && !ctrl && !alt && !matches!(self.key, Key::Code(_)) {
                    return false;
                }
                if self.ctrl != ctrl || self.alt != alt {
                    return false;
                }
                match self.key {
                    Key::Char(c) => printable.is_some_and(|p| c.to_lowercase().eq(p.to_lowercase())),
                    Key::Code(k) => {
                        self.shift == (mods & rl::MOD_SHIFT != 0) && (k == code || (k == rl::KEY_ENTER && code == rl::KEY_KP_ENTER))
                    }
                }
            }
            _ => false,
        }
    }
}

pub struct Config {
    pub host: String,
    pub port: String,
    pub password: Option<String>,
    pub music: PathBuf,
    pub playlists: PathBuf,
    pub threads: usize,
    /// The crossfade between a radio's tracks; nothing for the queue otherwise.
    pub crossfade: Duration,
    keys: HashMap<Shortcut, Vec<Chord>>,
    labels: HashMap<Shortcut, String>,
}

impl Config {
    pub fn is(&self, action: Shortcut, input: &Input) -> bool {
        self.keys.get(&action).is_some_and(|chords| chords.iter().any(|c| c.matches(input)))
    }

    pub fn which(&self, actions: &[Shortcut], input: &Input) -> Option<Shortcut> {
        actions.iter().copied().find(|a| self.is(*a, input))
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn path() -> PathBuf {
    crate::dirs::config().join("mascarpone").join("mascarpone.conf")
}

pub fn get() -> &'static Config {
    CONFIG.get_or_init(load)
}

fn load() -> Config {
    let path = path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut values: HashMap<(String, String), String> = HashMap::new();
    let mut section = String::new();
    for (n, line) in text.lines().enumerate() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) => {
                values.insert((section.clone(), k.trim().to_owned()), v.trim().to_owned());
            }
            None => eprintln!("{}:{}: not a key = value line", path.display(), n + 1),
        }
    }
    let mut take = |section: &str, key: &str| values.remove(&(section.to_owned(), key.to_owned())).filter(|v| !v.is_empty());

    let home = |p: String| match p.strip_prefix("~/") {
        Some(rest) => crate::dirs::home().join(rest),
        None => PathBuf::from(p),
    };
    let env_host = std::env::var("MPD_HOST").ok().filter(|h| !h.is_empty());
    let (env_password, env_host) = match env_host {
        Some(h) if !h.starts_with('/') && h.contains('@') => {
            let (p, h) = h.split_once('@').unwrap();
            (Some(p.to_owned()), Some(h.to_owned()))
        }
        h => (None, h),
    };
    let host = env_host.or_else(|| take("mpd", "host")).unwrap_or_else(|| "localhost".into());
    let port = std::env::var("MPD_PORT").ok().filter(|p| !p.is_empty()).or_else(|| take("mpd", "port")).unwrap_or_else(|| "6600".into());
    let password = env_password.or_else(|| take("mpd", "password"));
    let music = take("library", "music").map(home).unwrap_or_else(crate::dirs::music);
    let playlists = take("library", "playlists").map(home).unwrap_or_else(|| music.join("Playlists"));
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let threads = take("radio", "threads").and_then(|t| t.parse().ok()).filter(|&n| n > 0).unwrap_or((cores / 4).max(1));
    let crossfade = Duration::from_secs(take("radio", "crossfade").and_then(|t| t.parse().ok()).unwrap_or(1));

    let mut keys = HashMap::new();
    let mut labels = HashMap::new();
    for (action, name, default, _) in ACTIONS {
        let text = match values.remove(&("keys".to_owned(), (*name).to_owned())) {
            Some(v) => v,
            None => (*default).to_owned(),
        };
        labels.insert(*action, text.split_whitespace().collect::<Vec<_>>().join("  "));
        let chords = text
            .split_whitespace()
            .filter_map(|k| {
                let chord = Chord::parse(k);
                if chord.is_none() {
                    eprintln!("{}: [keys] {name}: no key {k}", path.display());
                }
                chord
            })
            .collect();
        keys.insert(*action, chords);
    }
    for (section, key) in values.keys() {
        eprintln!("{}: [{section}] {key}: not a setting", path.display());
    }
    Config { host, port, password, music, playlists, threads, crossfade, keys, labels }
}

fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && i > 0 && bytes[i - 1].is_ascii_whitespace() {
            let before = line[..i].trim_end();
            if before.ends_with('=') {
                continue;
            }
            return &line[..i];
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_read() {
        assert_eq!(Chord::parse("Ctrl++"), Some(Chord { ctrl: true, alt: false, shift: false, key: Key::Char('+') }));
        assert_eq!(Chord::parse("Ctrl+Shift+Tab"), Some(Chord { ctrl: true, alt: false, shift: true, key: Key::Code(rl::KEY_TAB) }));
        assert_eq!(Chord::parse("K"), Some(Chord { ctrl: false, alt: false, shift: false, key: Key::Char('K') }));
        assert_eq!(Chord::parse("Space"), Some(Chord { ctrl: false, alt: false, shift: false, key: Key::Char(' ') }));
        assert_eq!(Chord::parse("Nope"), None);
    }

    #[test]
    fn comments_go() {
        assert_eq!(strip_comment("# all"), "");
        assert_eq!(strip_comment("host = here # there").trim(), "host = here");
        assert_eq!(strip_comment("search_all = #"), "search_all = #");
        assert_eq!(strip_comment("left = j;#x"), "left = j;#x");
    }

    #[test]
    fn characters_match() {
        let k = Chord::parse("K").unwrap();
        assert!(k.matches(&Input::Char('K')));
        assert!(!k.matches(&Input::Char('k')));
        let space = Chord::parse("Space").unwrap();
        assert!(space.matches(&Input::Char(' ')));
    }
}
