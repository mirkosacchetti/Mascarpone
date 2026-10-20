//! raylib, built from `vendor/raylib` (6.2, trimmed: no audio, no
//! models) with its own Makefile: GLFW for the desktop, Wayland and X11
//! both. One change to its source, in a copy: the window's app id
//! (Wayland) and class (X11), which sway's rules match and raylib has no
//! call for.
//!
//! Built once in `target/raylib`, for every profile and for `cargo
//! check` alike, and again only when the vendored source or this file
//! changes: a stamp says what the library there was built from. Nothing
//! is written in `OUT_DIR`, which rust-analyzer reads as the crate's
//! own: files changing there start its checks over.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

const APP_ID: &str = "cards.random.Mascarpone";

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("vendor").join("raylib").join("src");
    println!("cargo:rerun-if-changed=vendor/raylib/src");
    println!("cargo:rerun-if-changed=build.rs");

    let dir = build_dir();
    std::fs::create_dir_all(&dir).unwrap();
    // one build at a time: cargo build and rust-analyzer's check may run
    // together
    let lock = File::create(dir.join("lock")).unwrap();
    lock.lock().unwrap();

    let src = dir.join("src");
    let stamp = dir.join("stamp");
    let wanted = format!("{}\n{}", fingerprint(&vendor), fingerprint(&root.join("build.rs")));
    let built = src.join("libraylib.a").exists() && std::fs::read_to_string(&stamp).is_ok_and(|s| s == wanted);
    if !built {
        let _ = std::fs::remove_file(&stamp);
        let _ = std::fs::remove_dir_all(&src);
        copy(&vendor, &src);
        patch(&src);
        make(&src);
        std::fs::write(&stamp, wanted).unwrap();
    }

    println!("cargo:rustc-link-search=native={}", src.display());
    println!("cargo:rustc-link-lib=static=raylib");
    // GLFW opens Wayland, X11 and EGL itself, at run time
    for lib in ["GL", "m", "dl", "pthread", "rt"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}

/// `target/raylib`: `OUT_DIR` is `<target>/<profile>/build/<crate>/out`.
fn build_dir() -> PathBuf {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let inside_build = out.ancestors().nth(2).is_some_and(|b| b.ends_with("build"));
    match out.ancestors().nth(4).filter(|_| inside_build) {
        Some(target) => target.join("raylib"),
        None => out.join("raylib"),
    }
}

/// What a file or a tree is: each file's path, size and time.
fn fingerprint(path: &Path) -> String {
    let mut lines = Vec::new();
    walk(path, &mut |p| {
        let meta = std::fs::metadata(p).unwrap();
        let time = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        lines.push(format!("{} {} {time}", p.display(), meta.len()));
    });
    lines.sort();
    lines.join("\n")
}

fn walk(path: &Path, f: &mut impl FnMut(&Path)) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            walk(&entry.path(), f);
        }
    } else {
        f(path);
    }
}

/// The app id set where raylib sets the window's hints.
fn patch(src: &Path) {
    let glfw = src.join("platforms").join("rcore_desktop_glfw.c");
    let text = std::fs::read_to_string(&glfw).unwrap();
    let hints = "glfwDefaultWindowHints();";
    assert!(text.contains(hints), "raylib changed: no {hints} in rcore_desktop_glfw.c");
    let text = text.replacen(
        hints,
        &format!("{hints} glfwWindowHintString(GLFW_WAYLAND_APP_ID, \"{APP_ID}\"); glfwWindowHintString(GLFW_X11_CLASS_NAME, \"{APP_ID}\"); glfwWindowHintString(GLFW_X11_INSTANCE_NAME, \"{APP_ID}\");"),
        1,
    );
    std::fs::write(&glfw, text).unwrap();
}

fn make(src: &Path) {
    let status = Command::new("make")
        .current_dir(src)
        .arg(format!("-j{}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)))
        .args([
            "PLATFORM=PLATFORM_DESKTOP_GLFW",
            "GLFW_LINUX_ENABLE_WAYLAND=TRUE",
            "GLFW_LINUX_ENABLE_X11=TRUE",
            "RAYLIB_MODULE_AUDIO=FALSE",
            "RAYLIB_MODULE_MODELS=FALSE",
            "RAYLIB_LIBTYPE=STATIC",
            "RAYLIB_BUILD_MODE=RELEASE",
            // no images from the clipboard: they need X11 linked, not
            // opened at run time as GLFW does
            "CUSTOM_CFLAGS=-DSUPPORT_CLIPBOARD_IMAGE=0",
        ])
        .arg(format!("RAYLIB_SRC_PATH={}", src.display()))
        .arg(format!("RAYLIB_RELEASE_PATH={}", src.display()))
        .status()
        .expect("make: raylib needs make and a C compiler");
    assert!(status.success(), "raylib did not build");
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}
