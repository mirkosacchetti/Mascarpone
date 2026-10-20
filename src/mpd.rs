//! The connection to mpd, on a tokio thread of its own: the window keeps
//! the main thread. Commands come in on a channel, events (the status
//! after every change mpd reports, the library, covers, connection lost
//! and regained) go out on another, each waking the window, which
//! redraws on them. A lost connection is retried every two seconds, like
//! mpc would be run again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use mpd_client::client::{CommandError, ConnectionEvent, Subsystem};
use mpd_client::commands::{self as cmd, SongPosition};
use mpd_client::responses::{PlayState, SongInQueue, Status};
use mpd_client::Client;
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::{mpsc, Semaphore};

use crate::covers;
use crate::library::Library;

/// What the window asks of mpd.
#[derive(Debug, Clone)]
pub enum Command {
    Toggle,
    /// No repeat: single and repeat off.
    RepeatOff,
    /// The song on, over and over: single and repeat on.
    RepeatOne,
    /// The queue over and over: repeat on, single off.
    RepeatAll,
    Next,
    Previous,
    Stop,
    /// The volume up or down by this much, within 0 to 100.
    Volume(i32),
    /// Forward or back in the song on.
    Forward(Duration),
    Backward(Duration),
    /// mpd's random mode: the queue played in a random order.
    Random(bool),
    /// Replace the queue with these and play from the one at `start`.
    Play { uris: Vec<String>, start: usize },
    /// Play the queue from this position.
    PlayPos(usize),
    /// Edits of the queue, by position.
    Remove(usize),
    Move { from: usize, to: usize },
    Clear,
    /// The playlist files changed: read them again.
    ReloadPlaylists,
    /// Refresh: mpd updates its database (the library follows on its
    /// `database` event), the library and the playlists are read again now.
    Refresh,
    /// Append these to the queue.
    Queue { uris: Vec<String> },
    /// The cover of the album `key`, from the file `uri`.
    Cover { key: String, uri: String },
    /// The artist's portrait and biography, from the net or the cache.
    Artist(String),
    /// Jump to this point of the song on.
    Seek(Duration),
}

/// What mpd tells the window.
#[derive(Debug, Clone)]
pub enum Event {
    Connected { address: String, version: String },
    Disconnected { reason: String },
    Status(Box<Now>),
    Library(Arc<Library>),
    /// The queue, each song's uri and length, after every change.
    Queue(Vec<(String, Option<Duration>)>),
    /// The playlists read again, for the library sent before.
    Playlists(Vec<crate::library::Playlist>),
    /// The thumbnail of an album's cover, none when the album has none;
    /// `failed` when it could not be had this time (mpd or the net
    /// erred), to be asked again later.
    Cover { key: String, path: Option<PathBuf>, failed: bool },
    /// What was found about an artist.
    Artist { name: String, info: crate::net::Info },
}

/// The player as it is now: status and the song it is on.
#[derive(Debug, Clone)]
pub struct Now {
    pub status: Status,
    pub song: Option<SongInQueue>,
}

impl Now {
    pub fn playing(&self) -> bool {
        self.status.state == PlayState::Playing
    }
}

/// Where mpd listens: the configuration's host (a path means a Unix
/// socket) and port, MPD_HOST and MPD_PORT winning as mpc reads them.
fn address() -> String {
    let config = crate::config::get();
    if config.host.starts_with('/') {
        return config.host.clone();
    }
    format!("{}:{}", config.host, config.port)
}

/// Starts the mpd thread; returns the end of the pipes the window holds.
pub fn start() -> (mpsc::UnboundedSender<Command>, std::sync::mpsc::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("mpd".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run(cmd_rx, Out(ev_tx)));
        })
        .expect("mpd thread");
    (cmd_tx, ev_rx)
}

/// The events' end on this side: each one sent wakes the window.
#[derive(Clone)]
pub struct Out(std::sync::mpsc::Sender<Event>);

impl Out {
    /// False when the window is gone.
    fn send(&self, event: Event) -> bool {
        let sent = self.0.send(event).is_ok();
        crate::rl::wake();
        sent
    }
}

async fn run(mut commands: mpsc::UnboundedReceiver<Command>, events: Out) {
    let address = address();
    let net = crate::net::Net::new();
    loop {
        match session(&address, &mut commands, &events, &net).await {
            Ok(()) => return, // the window is gone
            Err(e) => {
                events.send(Event::Disconnected { reason: e.to_string() });
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// One connection, until it drops. Ok means the window closed its end.
async fn session(
    address: &str,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    events: &Out,
    net: &Arc<crate::net::Net>,
) -> Result<()> {
    let password = crate::config::get().password.as_deref();
    let (client, mut changes) = if address.starts_with('/') {
        Client::connect_with_password_opt(UnixStream::connect(address).await?, password).await?
    } else {
        Client::connect_with_password_opt(TcpStream::connect(address).await?, password).await?
    };
    let version = client.protocol_version().to_string();
    if !events.send(Event::Connected { address: address.into(), version }) {
        return Ok(());
    }
    // covers come in big chunks rather than mpd's 8 KiB default
    let _ = client.command(cmd::SetBinaryLimit(1 << 20)).await;
    send_status(&client, events).await?;
    let mut library = send_library(&client, events).await?;
    send_queue(&client, events).await?;
    // a few covers at a time, not the whole grid at once; artists from
    // the net two at a time
    let slots = Arc::new(Semaphore::new(3));
    let lines = Arc::new(Semaphore::new(2));
    // mpd's database changing by itself (files synced in, its own update)
    // reads the library again only when it has been still for a while: a
    // sync brings hundreds of changes. One asked for (Ctrl+R) at once.
    let mut reload_at: Option<tokio::time::Instant> = None;
    let mut asked = false;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(reload_at.unwrap_or_else(tokio::time::Instant::now)), if reload_at.is_some() => {
                reload_at = None;
                library = send_library(&client, events).await?;
            }
            change = changes.next() => match change {
                Some(ConnectionEvent::SubsystemChange(sub)) => match sub {
                    Subsystem::Player | Subsystem::Mixer | Subsystem::Options => {
                        send_status(&client, events).await?;
                    }
                    Subsystem::Queue => {
                        send_status(&client, events).await?;
                        send_queue(&client, events).await?;
                    }
                    Subsystem::Database if asked => {
                        asked = false;
                        reload_at = None;
                        library = send_library(&client, events).await?;
                    }
                    Subsystem::Database => reload_at = Some(tokio::time::Instant::now() + LIBRARY_STILL),
                    // the end of an update asked for, whatever came first:
                    // read at once (an update that changed nothing sends
                    // no database event); the status, awaited, cannot go in
                    // the guard
                    #[allow(clippy::collapsible_match)]
                    Subsystem::Update if asked => {
                        if client.command(cmd::Status).await.is_ok_and(|s| s.update_job.is_none()) {
                            asked = false;
                            reload_at = None;
                            library = send_library(&client, events).await?;
                        }
                    }
                    _ => {}
                },
                Some(ConnectionEvent::ConnectionClosed(e)) => anyhow::bail!("connection closed: {e}"),
                None => anyhow::bail!("connection closed"),
            },
            command = commands.recv() => match command {
                None => return Ok(()),
                Some(Command::Cover { key, uri }) => {
                    let (client, events, slots, net, library) = (client.clone(), events.clone(), slots.clone(), net.clone(), library.clone());
                    tokio::spawn(async move {
                        let _slot = slots.acquire().await;
                        let found = cover(&client, &net, &library, &key, &uri).await;
                        events.send(Event::Cover { key, failed: found.is_err(), path: found.ok().flatten() });
                    });
                }
                Some(Command::Artist(name)) => {
                    let (net, events, lines) = (net.clone(), events.clone(), lines.clone());
                    tokio::spawn(async move {
                        let _line = lines.acquire().await;
                        let info = crate::net::lookup(&net, &name).await;
                        events.send(Event::Artist { name, info });
                    });
                }
                Some(Command::Refresh) => {
                    // the albums the net had no cover for are asked again
                    if let Ok(files) = std::fs::read_dir(covers::cache_dir()) {
                        for f in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "none")) {
                            let _ = std::fs::remove_file(f);
                        }
                    }
                    // read now, and again when mpd's update ends (its
                    // database event, taken at once)
                    asked = client.command(cmd::Update::new()).await.is_ok();
                    library = send_library(&client, events).await?;
                    send_queue(&client, events).await?;
                }
                Some(Command::ReloadPlaylists) => {
                    let lib = library.clone();
                    let playlists = tokio::task::spawn_blocking(move || lib.read_playlists()).await?;
                    events.send(Event::Playlists(playlists));
                }
                // a refused command (play on an empty queue) is not a
                // lost connection: logged, the session goes on
                Some(c) => {
                    if let Err(e) = perform(&client, c).await {
                        eprintln!("mpd: {e}");
                    }
                }
            },
        }
    }
}

/// How long mpd's database has to be still before the library is read
/// again by itself.
const LIBRARY_STILL: Duration = Duration::from_secs(30 * 60);

async fn send_status(client: &Client, events: &Out) -> Result<()> {
    let (status, song) = client.command_list((cmd::Status, cmd::CurrentSong)).await?;
    events.send(Event::Status(Box::new(Now { status, song })));
    Ok(())
}

/// The library read a top folder at a time: a single `listallinfo` of
/// everything overflows mpd's output buffer (8 MB unless its config says
/// more) on a library of ten thousand songs, and mpd drops the client.
async fn send_library(client: &Client, events: &Out) -> Result<Arc<Library>> {
    let top = client.raw_command(mpd_client::protocol::command::Command::new("lsinfo")).await?;
    let folders: Vec<String> = top.fields().filter(|(k, _)| *k == "directory").map(|(_, v)| v.to_owned()).collect();
    // fifty folders a command list: one at a time costs a round trip
    // each, seconds on four hundred folders
    let mut songs = Vec::new();
    for batch in folders.chunks(50) {
        let lists = client.command_list(batch.iter().map(|f| cmd::ListAllIn::directory(f)).collect::<Vec<_>>()).await?;
        songs.extend(lists.into_iter().flatten());
    }
    // songs right in the music folder, outside any folder (rare): only
    // the whole listing has them, taken when mpd manages to send it
    if top.fields().any(|(k, _)| k == "file") {
        if let Ok(all) = client.command(cmd::ListAllIn::root()).await {
            songs = all;
        }
    }
    let library = Arc::new(tokio::task::spawn_blocking(move || Library::build(songs)).await?);
    events.send(Event::Library(library.clone()));
    Ok(library)
}

async fn send_queue(client: &Client, events: &Out) -> Result<()> {
    let queue = client.command(cmd::Queue).await?;
    events.send(Event::Queue(queue.into_iter().map(|s| (s.song.url, s.song.duration)).collect()));
    Ok(())
}

/// The album's thumbnail: from the cache, else from mpd and into the
/// cache. When mpd has no picture, one from the net, written as
/// `cover.jpg` in the album's folder (the library's convention, and
/// where mpd then finds it); an album the net has nothing for is
/// marked, and not asked again. The thumbnail's path, none when the album
/// has no cover; an error (logged) when it could not be had this time.
async fn cover(client: &Client, net: &Arc<crate::net::Net>, library: &Arc<Library>, key: &str, uri: &str) -> Result<Option<PathBuf>, ()> {
    let path = covers::path_for(key);
    if path.exists() {
        return Ok(Some(path));
    }
    let none = path.with_extension("none");
    let bytes = match client.album_art(uri).await {
        Ok(Some((bytes, _mime))) => Some(bytes.to_vec()),
        // nothing in the file, nothing next to it ("No file exists")
        Ok(None) => fetch_cover(net, library, key, uri, &none).await?,
        Err(CommandError::ErrorResponse { error, .. }) if error.code == 50 => fetch_cover(net, library, key, uri, &none).await?,
        Err(e) => {
            eprintln!("cover of {uri}: {e}");
            return Err(());
        }
    };
    let Some(bytes) = bytes else { return Ok(None) };
    let saved = path.clone();
    match tokio::task::spawn_blocking(move || covers::thumbnail(&bytes, &saved)).await {
        Ok(Ok(())) => Ok(Some(path)),
        Ok(Err(e)) => {
            eprintln!("cover of {uri}: {e}");
            Err(())
        }
        Err(e) => {
            eprintln!("cover of {uri}: {e}");
            Err(())
        }
    }
}

/// The net's cover for the album of `key`, saved as `cover.jpg` in the
/// album's folder when the folder is the album's alone (a folder of
/// loose tracks would lend it to every song there); the marker written
/// when there is none; an error when the net erred.
async fn fetch_cover(net: &crate::net::Net, library: &Library, key: &str, uri: &str, none: &Path) -> Result<Option<Vec<u8>>, ()> {
    if none.exists() {
        return Ok(None);
    }
    let Some((artist, album)) = key.split_once('\u{1f}') else { return Ok(None) };
    // a folder of loose tracks is no album anyone can name
    if artist.is_empty() {
        mark_none(none);
        return Ok(None);
    }
    match crate::net::album_cover(net, artist, album).await {
        Ok(Some(bytes)) => {
            if let Some(folder) = library.own_folder(key) {
                let file = crate::library::music_dir().join(folder).join("cover.jpg");
                if let Err(e) = std::fs::write(&file, &bytes) {
                    eprintln!("cover of {uri}: writing {}: {e}", file.display());
                }
            }
            Ok(Some(bytes))
        }
        Ok(None) => {
            mark_none(none);
            Ok(None)
        }
        Err(e) => {
            eprintln!("cover of {uri}: {e}");
            Err(())
        }
    }
}

/// The marker of an album without a cover, so the net is not asked again.
fn mark_none(none: &Path) {
    if let Some(dir) = none.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(none, b"");
}

async fn perform(client: &Client, command: Command) -> Result<()> {
    match command {
        Command::Toggle => {
            let status = client.command(cmd::Status).await?;
            match status.state {
                PlayState::Playing => client.command(cmd::SetPause(true)).await?,
                PlayState::Paused => client.command(cmd::SetPause(false)).await?,
                PlayState::Stopped => client.command(cmd::Play::current()).await?,
            }
        }
        Command::RepeatOne => client.command_list((cmd::SetSingle(cmd::SingleMode::Enabled), cmd::SetRepeat(true))).await.map(|_| ())?,
        Command::RepeatAll => client.command_list((cmd::SetSingle(cmd::SingleMode::Disabled), cmd::SetRepeat(true))).await.map(|_| ())?,
        Command::RepeatOff => client.command_list((cmd::SetSingle(cmd::SingleMode::Disabled), cmd::SetRepeat(false))).await.map(|_| ())?,
        Command::Next => client.command(cmd::Next).await?,
        Command::Previous => client.command(cmd::Previous).await?,
        Command::Stop => client.command(cmd::Stop).await?,
        Command::Volume(by) => {
            let status = client.command(cmd::Status).await?;
            let volume = (i32::from(status.volume) + by).clamp(0, 100) as u8;
            client.command(cmd::SetVolume(volume)).await?
        }
        Command::Forward(by) => client.command(cmd::Seek(cmd::SeekMode::Forward(by))).await?,
        Command::Backward(by) => client.command(cmd::Seek(cmd::SeekMode::Backward(by))).await?,
        // the adds go as one command list: one round trip, not one a song
        Command::Play { uris, start } => {
            client.command(cmd::ClearQueue).await?;
            if !uris.is_empty() {
                client.command_list(uris.iter().map(|u| cmd::Add::uri(u)).collect::<Vec<_>>()).await?;
            }
            client.command(cmd::Play::song(SongPosition(start))).await?;
        }
        Command::Queue { uris } => {
            if !uris.is_empty() {
                client.command_list(uris.iter().map(|u| cmd::Add::uri(u)).collect::<Vec<_>>()).await?;
            }
        }
        Command::PlayPos(pos) => client.command(cmd::Play::song(SongPosition(pos))).await?,
        Command::Remove(pos) => client.command(cmd::Delete::position(SongPosition(pos))).await?,
        Command::Move { from, to } => client.command(cmd::Move::position(SongPosition(from)).to_position(SongPosition(to))).await?,
        Command::Clear => client.command(cmd::ClearQueue).await?,
        Command::Random(on) => client.command(cmd::SetRandom(on)).await?,
        Command::Seek(to) => client.command(cmd::Seek(cmd::SeekMode::Absolute(to))).await?,
        Command::ReloadPlaylists | Command::Refresh | Command::Cover { .. } | Command::Artist(_) => unreachable!("handled in session"),
    }
    Ok(())
}
