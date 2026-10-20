//! Mascarpone: an mpd client, drawn with raylib. The window and its loop
//! here: a frame is drawn when something changed (a key, the pointer,
//! mpd, a picture decoded, a clock), and in between the window sleeps.
//! What it knows and does in `app.rs`; the window's parts in `view.rs`,
//! `pages.rs`, `player.rs` and `picker.rs`; the talk with mpd in
//! `mpd.rs`; the library, the queue's origin, the covers and the net in
//! their files.

mod app;
mod bars;
mod config;
mod covers;
mod dirs;
mod glyphs;
mod library;
mod master;
mod mpd;
mod net;
mod pages;
mod picker;
mod player;
mod rl;
mod size;
mod text;
mod ui;
mod view;

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The app's id: the window's (sway's rules match it), the socket's name.
const APP_ID: &str = "cards.random.Mascarpone";

/// Another launch asked the window to come up.
static PRESENT: AtomicBool = AtomicBool::new(false);

fn main() {
    // launched from a launcher, stderr goes nowhere: a panic is kept in
    // the cache folder too
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = dirs::cache().join("mascarpone").join("panic.log");
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {info}", dirs::now("%Y-%m-%d %H:%M:%S"));
        }
        hook(info);
    }));
    // one instance, one window: launched again, the running one is asked
    // to come up, and this one ends
    let Some(listener) = single() else { return };

    size::init();
    rl::open(size::rem(72.0) as i32, size::rem(48.0) as i32, "Mascarpone");
    if let Some(listener) = listener {
        std::thread::Builder::new()
            .name("present".into())
            .spawn(move || {
                for mut stream in listener.incoming().flatten() {
                    let mut word = String::new();
                    let _ = stream.read_to_string(&mut word);
                    PRESENT.store(true, Ordering::SeqCst);
                    rl::wake();
                }
            })
            .expect("present thread");
    }

    let (commands, events) = mpd::start();
    let mut app = app::App::new(commands, events);
    let mut ui = ui::Ui::new();
    // frames still to draw before sleeping: one more after any change,
    // for what the change moved under the pointer
    let mut more: u8 = 2;
    // what came while no frame could be drawn
    let mut held = false;
    // the next frame a clock or an animation wants
    let mut due: Option<Instant> = None;
    while !unsafe { rl::WindowShouldClose() } && !app.quit {
        if PRESENT.swap(false, Ordering::SeqCst) {
            app.present = true;
        }
        // the last frame not shown yet, or out of sight: mpd's news and the
        // pictures taken in, nothing drawn until the compositor wants it
        if !rl::may_draw() {
            held |= app.begin_frame(&ui);
            rl::wait(60.0);
            continue;
        }
        let now = Instant::now();
        if more == 0 && !held && !app.present && !ui.stirred() && !app.news() && due.is_none_or(|d| d > now) {
            rl::wait(due.map_or(60.0, |d| (d - now).as_secs_f64()));
            continue;
        }
        let input = ui.begin_frame();
        let news = app.begin_frame(&ui) || std::mem::take(&mut held);
        app.keys(&mut ui);
        unsafe {
            rl::BeginDrawing();
            // raylib scales to the screen's points; the sizes here are its
            // pixels already
            rl::rlLoadIdentity();
            rl::ClearBackground(ui::BG);
        }
        view::draw(&mut app, &mut ui);
        rl::ask_shown();
        unsafe { rl::EndDrawing() };
        ui.end_frame();
        let acted = app.apply();
        more = if input || news || acted { 1 } else { more.saturating_sub(1) };
        due = ui.wake_in.map(|t| Instant::now() + Duration::from_secs_f64(t));
    }
    rl::close();
    let _ = std::fs::remove_file(socket_path());
}

fn socket_path() -> std::path::PathBuf {
    dirs::runtime().join(format!("{APP_ID}.sock"))
}

/// The socket only the first instance listens on; none when another one
/// is running (and has been asked to come up). When it cannot be made
/// (a runtime folder too deep for a socket's name), no uniqueness, but
/// a window all the same.
fn single() -> Option<Option<UnixListener>> {
    let path = socket_path();
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let _ = stream.write_all(b"present\n");
        return None;
    }
    // nobody there: what is left of one that ended badly goes
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(l) => Some(Some(l)),
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            Some(None)
        }
    }
}
