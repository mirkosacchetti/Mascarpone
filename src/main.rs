mod app;
mod archive;
mod bars;
mod config;
mod covers;
mod db;
mod dirs;
mod glyphs;
mod library;
mod master;
mod mpd;
mod net;
mod pages;
mod picker;
mod player;
mod radio;
mod shape;
mod rl;
mod size;
mod text;
mod ui;
mod view;
mod voice;

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const APP_ID: &str = "cards.random.Mascarpone";

static PRESENT: AtomicBool = AtomicBool::new(false);
static ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn main() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = dirs::cache().join("mascarpone").join("panic.log");
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {info}", dirs::now("%Y-%m-%d %H:%M:%S"));
        }
        hook(info);
    }));
    let words: Vec<String> = std::env::args().skip(1).collect();
    if !words.is_empty() {
        match UnixStream::connect(socket_path()) {
            Ok(mut stream) => {
                let _ = writeln!(stream, "{}", words.join(" "));
            }
            Err(e) => eprintln!("no Mascarpone running: {e}"),
        }
        return;
    }
    let Some(listener) = single() else { return };

    size::init();
    rl::open(size::rem(72.0) as i32, size::rem(48.0) as i32, "Mascarpone");
    if let Some(listener) = listener {
        std::thread::Builder::new()
            .name("present".into())
            .spawn(move || {
                for mut stream in listener.incoming().flatten() {
                    let mut line = String::new();
                    let _ = stream.read_to_string(&mut line);
                    let line = line.trim();
                    if line == "present" {
                        PRESENT.store(true, Ordering::SeqCst);
                    } else {
                        ASKED.lock().unwrap().push(line.to_owned());
                    }
                    rl::wake();
                }
            })
            .expect("present thread");
    }

    let db = db::open();
    let (archive, jobs) = std::sync::mpsc::channel();
    let (commands, events, out) = mpd::start(archive.clone(), db.clone());
    archive::start(jobs, out, commands.clone(), db);
    let mut app = app::App::new(commands, events, archive);
    let mut ui = ui::Ui::new();
    let mut more: u8 = 2;
    let mut held = false;
    let mut due: Option<Instant> = None;
    while !rl::closing() && !app.quit {
        if PRESENT.swap(false, Ordering::SeqCst) {
            app.present = true;
        }
        for line in std::mem::take(&mut *ASKED.lock().unwrap()) {
            app.ask(&line);
        }
        if !rl::may_draw() {
            held |= app.begin_frame(&ui);
            rl::wait(60.0);
            continue;
        }
        let now = Instant::now();
        if more == 0 && !held && !app.present && !ui.stirred() && !app.news() && ASKED.lock().unwrap().is_empty() && due.is_none_or(|d| d > now) {
            rl::wait(due.map_or(60.0, |d| (d - now).as_secs_f64()));
            continue;
        }
        let input = ui.begin_frame();
        let news = app.begin_frame(&ui) || std::mem::take(&mut held);
        app.keys(&mut ui);
        unsafe {
            rl::BeginDrawing();
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

fn single() -> Option<Option<UnixListener>> {
    let path = socket_path();
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let _ = stream.write_all(b"present\n");
        return None;
    }
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(l) => Some(Some(l)),
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            Some(None)
        }
    }
}
