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

#[derive(Debug, Clone)]
pub enum Command {
    Toggle,
    RepeatOff,
    RepeatOne,
    RepeatAll,
    Next,
    Previous,
    Stop,
    Volume(i32),
    Forward(Duration),
    Backward(Duration),
    Random(bool),
    Play { uris: Vec<String>, start: usize },
    PlayPos(usize),
    Remove(usize),
    Move { from: usize, to: usize },
    Clear,
    ReloadPlaylists,
    Refresh,
    Queue { uris: Vec<String> },
    Cover { key: String, uri: String },
    Artist(String),
    ForgetArtist(String),
    Seek(Duration),
    Links(Vec<(String, Vec<String>)>),
}

#[derive(Debug, Clone)]
pub enum Event {
    Connected { address: String, version: String },
    Disconnected { reason: String },
    Status(Box<Now>),
    Library(Arc<Library>),
    Queue(Vec<(String, Option<Duration>)>),
    Playlists(Vec<crate::library::Playlist>),
    Cover { key: String, path: Option<PathBuf>, failed: bool },
    Artist { name: String, info: crate::net::Info },
    Vectors(Arc<crate::radio::Vectors>),
    Analyzing { done: usize, total: usize },
    Relations(Arc<crate::radio::Relations>),
    Linking { done: usize, total: usize },
}

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

fn address() -> String {
    let config = crate::config::get();
    if config.host.starts_with('/') {
        return config.host.clone();
    }
    format!("{}:{}", config.host, config.port)
}

pub fn start(radio: std::sync::mpsc::Sender<crate::radio::Job>, db: crate::db::Shared) -> (mpsc::UnboundedSender<Command>, std::sync::mpsc::Receiver<Event>, Out) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    let out = Out(ev_tx);
    let events = out.clone();
    std::thread::Builder::new()
        .name("mpd".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run(cmd_rx, events, radio, db));
        })
        .expect("mpd thread");
    (cmd_tx, ev_rx, out)
}

#[derive(Clone)]
pub struct Out(std::sync::mpsc::Sender<Event>);

impl Out {
    pub fn send(&self, event: Event) -> bool {
        let sent = self.0.send(event).is_ok();
        crate::rl::wake();
        sent
    }
}

async fn run(mut commands: mpsc::UnboundedReceiver<Command>, events: Out, radio: std::sync::mpsc::Sender<crate::radio::Job>, db: crate::db::Shared) {
    let address = address();
    let net = crate::net::Net::new();
    loop {
        match session(&address, &mut commands, &events, &net, &radio, &db).await {
            Ok(()) => return,
            Err(e) => {
                events.send(Event::Disconnected { reason: e.to_string() });
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn session(
    address: &str,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    events: &Out,
    net: &Arc<crate::net::Net>,
    radio: &std::sync::mpsc::Sender<crate::radio::Job>,
    db: &crate::db::Shared,
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
    let _ = client.command(cmd::SetBinaryLimit(1 << 20)).await;
    send_status(&client, events).await?;
    let mut library = send_library(&client, events).await?;
    send_queue(&client, events).await?;
    let slots = Arc::new(Semaphore::new(3));
    let lines = Arc::new(Semaphore::new(2));
    let mut reload_at: Option<tokio::time::Instant> = None;
    let mut asked = false;
    let mut linking: Option<tokio::task::JoinHandle<()>> = None;
    let ask_artist = |name: String| {
        let (net, events, lines, db) = (net.clone(), events.clone(), lines.clone(), db.clone());
        tokio::spawn(async move {
            let _line = lines.acquire().await;
            let info = crate::net::lookup(&net, &db, &name).await;
            events.send(Event::Artist { name, info });
        });
    };

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
                Some(Command::ForgetArtist(name)) => {
                    crate::net::forget(db, &name);
                    ask_artist(name);
                }
                Some(Command::Artist(name)) => ask_artist(name),
                Some(Command::Cover { key, uri }) => {
                    let (client, events, slots, net, library) = (client.clone(), events.clone(), slots.clone(), net.clone(), library.clone());
                    tokio::spawn(async move {
                        let _slot = slots.acquire().await;
                        let found = cover(&client, &net, &library, &key, &uri).await;
                        events.send(Event::Cover { key, failed: found.is_err(), path: found.ok().flatten() });
                    });
                }
                Some(Command::Links(names)) => {
                    if let Some(task) = linking.take() {
                        task.abort();
                    }
                    let (net, radio) = (net.clone(), radio.clone());
                    linking = Some(tokio::spawn(async move {
                        for (name, mbids) in names {
                            let job = match crate::net::links(&net, &name, &mbids).await {
                                Ok(l) => crate::radio::Job::Links { name, mbid: l.mbid, deezer: l.deezer, links: l.links, error: None },
                                Err(e) => {
                                    eprintln!("links of {name}: {e}");
                                    crate::radio::Job::Links { name, mbid: None, deezer: None, links: Vec::new(), error: Some(e.to_string()) }
                                }
                            };
                            if radio.send(job).is_err() {
                                return;
                            }
                        }
                    }));
                }
                Some(Command::Refresh) => {
                    if let Ok(files) = std::fs::read_dir(covers::cache_dir()) {
                        for f in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "none")) {
                            let _ = std::fs::remove_file(f);
                        }
                    }
                    asked = client.command(cmd::Update::new()).await.is_ok();
                    if !asked {
                        library = send_library(&client, events).await?;
                    }
                    send_queue(&client, events).await?;
                }
                Some(Command::ReloadPlaylists) => {
                    let lib = library.clone();
                    let playlists = tokio::task::spawn_blocking(move || lib.read_playlists()).await?;
                    events.send(Event::Playlists(playlists));
                }
                Some(c) => {
                    if let Err(e) = perform(&client, c).await {
                        eprintln!("mpd: {e}");
                    }
                }
            },
        }
    }
}

const LIBRARY_STILL: Duration = Duration::from_secs(30 * 60);

async fn send_status(client: &Client, events: &Out) -> Result<()> {
    let (status, song) = client.command_list((cmd::Status, cmd::CurrentSong)).await?;
    events.send(Event::Status(Box::new(Now { status, song })));
    Ok(())
}

async fn send_library(client: &Client, events: &Out) -> Result<Arc<Library>> {
    let library = Arc::new(load_library(client).await?);
    events.send(Event::Library(library.clone()));
    Ok(library)
}

async fn load_library(client: &Client) -> Result<Library> {
    let top = client.raw_command(mpd_client::protocol::command::Command::new("lsinfo")).await?;
    let folders: Vec<String> = top.fields().filter(|(k, _)| *k == "directory").map(|(_, v)| v.to_owned()).collect();
    let mut songs = Vec::new();
    for batch in folders.chunks(50) {
        let lists = client.command_list(batch.iter().map(|f| cmd::ListAllIn::directory(f)).collect::<Vec<_>>()).await?;
        songs.extend(lists.into_iter().flatten());
    }
    if top.fields().any(|(k, _)| k == "file") {
        if let Ok(all) = client.command(cmd::ListAllIn::root()).await {
            songs = all;
        }
    }
    let mut albums = tokio::task::spawn_blocking(move || Library::gather(songs)).await?;
    split_credits(client, &mut albums).await;
    Ok(tokio::task::spawn_blocking(move || Library::assemble(albums)).await?)
}

/// The credits the tags may split: an album with several artist ids is
/// asked for its first file's own comments, which mpd does not index,
/// where a tagger that matched the album on MusicBrainz names the
/// artists one by one (`ALBUMARTISTS`, a value each; `ARTISTS` when
/// the file has no album artist). Whether the names are taken is the
/// library's rule.
async fn split_credits(client: &Client, albums: &mut [crate::library::Album]) {
    let mut split = 0;
    for album in albums.iter_mut().filter(|a| a.members.len() == 1) {
        // the first track that names them: a stray copy of one track,
        // tagged with the album but without the artists, is passed over
        let uris: Vec<String> = album.tracks.iter().map(|t| t.uri.clone()).collect();
        for uri in uris {
            let names = if Path::new(&uri).extension().is_some_and(|e| e.eq_ignore_ascii_case("mp3")) {
                // mpd (0.24) reads a many-valued ID3v2.4 frame as its first
                // value repeated, in the index and in the comments alike, so
                // an mp3's tags are read from the file: the ids too, which mpd
                // gave one of
                let path = crate::library::music_dir().join(&uri);
                let Ok(Some((names, ids))) = tokio::task::spawn_blocking(move || id3_credit(&path)).await else { continue };
                if ids.len() > 1 && album.members[0].mbids.len() < ids.len() {
                    album.members[0].mbids = ids;
                }
                names
            } else if album.members[0].mbids.len() > 1 || (album.members[0].mbids.is_empty() && looks_joint(&album.artist)) {
                // several ids, or none and a credit that reads as several
                // names: the names may be there, one by one
                let command = mpd_client::protocol::command::Command::new("readcomments").argument(uri.clone());
                let frame = match client.raw_command(command).await {
                    Ok(frame) => frame,
                    Err(e) => {
                        eprintln!("comments of {uri}: {e}");
                        continue;
                    }
                };
                let values = |tag: &str| frame.fields().filter(|(k, _)| k.eq_ignore_ascii_case(tag)).map(|(_, v)| v.to_owned()).collect::<Vec<String>>();
                let mut names = values("albumartists");
                if names.is_empty() {
                    names = values("artists");
                }
                names
            } else {
                break;
            };
            if names.is_empty() {
                continue;
            }
            if album.split(&names) {
                split += 1;
            }
            break;
        }
        if album.members.len() == 1 && album.split_joined() {
            split += 1;
        }
    }
    if split > 0 {
        eprintln!("library: {split} credits split into their artists");
    }
}

/// Whether a credit reads as several names joined: only to spare the
/// files that cannot be (what splits is the tags' own list of names).
fn looks_joint(credit: &str) -> bool {
    let lower = credit.to_lowercase();
    ["&", ",", "/", ";", " and ", " with ", " feat", " e ", " y ", " et ", " und "].iter().any(|j| lower.contains(j))
}

/// The artists an mp3's tags credit its album to, one by one, with
/// their ids: the `ALBUMARTISTS` and `MusicBrainz Album Artist Id`
/// user frames (`ARTISTS` and `MusicBrainz Artist Id` when the file
/// has no album artist), their values null-separated in ID3v2.4.
/// None: no tag, or none of those frames.
fn id3_credit(path: &Path) -> Option<(Vec<String>, Vec<String>)> {
    let tag = match id3::Tag::read_from_path(path) {
        Ok(tag) => tag,
        Err(id3::Error { kind: id3::ErrorKind::NoTag, .. }) => return None,
        Err(e) => {
            eprintln!("tags of {}: {e}", path.display());
            return None;
        }
    };
    let values = |description: &str| -> Vec<String> {
        tag.extended_texts()
            .filter(|t| t.description.eq_ignore_ascii_case(description))
            .flat_map(|t| t.value.split('\0'))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (names, ids) = match values("ALBUMARTISTS") {
        names if !names.is_empty() => (names, values("MusicBrainz Album Artist Id")),
        _ => (values("ARTISTS"), values("MusicBrainz Artist Id")),
    };
    if names.is_empty() && ids.is_empty() {
        return None;
    }
    Some((names, ids))
}

async fn send_queue(client: &Client, events: &Out) -> Result<()> {
    let queue = client.command(cmd::Queue).await?;
    events.send(Event::Queue(queue.into_iter().map(|s| (s.song.url, s.song.duration)).collect()));
    Ok(())
}

async fn cover(client: &Client, net: &Arc<crate::net::Net>, library: &Arc<Library>, key: &str, uri: &str) -> Result<Option<PathBuf>, ()> {
    let path = covers::path_for(key);
    if path.exists() {
        return Ok(Some(path));
    }
    let none = path.with_extension("none");
    let bytes = match client.album_art(uri).await {
        Ok(Some((bytes, _mime))) => Some(bytes.to_vec()),
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

async fn fetch_cover(net: &crate::net::Net, library: &Library, key: &str, uri: &str, none: &Path) -> Result<Option<Vec<u8>>, ()> {
    if none.exists() {
        return Ok(None);
    }
    let Some((artist, album)) = key.split_once('\u{1f}') else { return Ok(None) };
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
        Command::ReloadPlaylists | Command::Refresh | Command::Cover { .. } | Command::Artist(_) | Command::ForgetArtist(_) | Command::Links(_) => unreachable!("handled in session"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_mp3_s_credit_is_read_from_its_frames() {
        use id3::frame::ExtendedText;
        use id3::TagLike;
        let dir = std::env::temp_dir().join(format!("mascarpone-id3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1.mp3");
        std::fs::write(&path, b"").unwrap();
        let mut tag = id3::Tag::new();
        tag.set_album_artist("Max Richter / Stéphane Moucha");
        tag.add_frame(ExtendedText { description: "ALBUMARTISTS".into(), value: "Max Richter\0Stéphane Moucha".into() });
        tag.add_frame(ExtendedText { description: "MusicBrainz Album Artist Id".into(), value: "richter\0moucha".into() });
        tag.write_to_path(&path, id3::Version::Id3v24).unwrap();
        let (names, ids) = id3_credit(&path).unwrap();
        assert_eq!(names, ["Max Richter", "Stéphane Moucha"]);
        assert_eq!(ids, ["richter", "moucha"]);
        let bare = dir.join("2.mp3");
        std::fs::write(&bare, b"\xff\xfb\x90\x00").unwrap();
        assert_eq!(id3_credit(&bare), None, "no tag at all");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `cargo test --release mpd::tests::credits_probe -- --ignored --nocapture`
    /// loads the library from the configured mpd and prints the credits
    /// the tags split, each with its members, and the artists' count.
    #[tokio::test]
    #[ignore]
    async fn credits_probe() {
        let address = address();
        let password = crate::config::get().password.as_deref();
        let (client, _changes) = if address.starts_with('/') {
            Client::connect_with_password_opt(UnixStream::connect(&address).await.unwrap(), password).await.unwrap()
        } else {
            Client::connect_with_password_opt(TcpStream::connect(&address).await.unwrap(), password).await.unwrap()
        };
        let library = load_library(&client).await.unwrap();
        let mut split = 0;
        for album in library.albums.iter().filter(|a| a.members.len() > 1) {
            split += 1;
            println!("{} | {} | {}", album.artist, album.title, album.names().collect::<Vec<_>>().join(", "));
        }
        println!("{split} credits split, {} albums, {} artists", library.albums.len(), library.artists.len());
        // with DB_COPY=<path>, the library is written to that copy of a database too
        if let Ok(path) = std::env::var("DB_COPY") {
            let mut db = crate::db::Db::open(Path::new(&path)).unwrap();
            db.write_library(&library).unwrap();
            println!("written to {path}: {} artists linked", db.linked().unwrap().len());
        }
    }
}
