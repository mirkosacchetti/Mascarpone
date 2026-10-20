//! raylib, built from `vendor/raylib` (6.2, trimmed: no audio, no
//! models) with its own Makefile: GLFW for the desktop, Wayland and X11
//! both. Two changes to its source, in a copy: the window's app id
//! (Wayland) and class (X11), which sway's rules match and raylib has no
//! call for; and what GLFW's scroll keeps from Wayland: whether it came
//! from a wheel's notches or a touchpad's pixels, and the touchpad's
//! fingers lifted, for the scroll that goes on after them.
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

/// The app id set where raylib sets the window's hints; the scroll's
/// source and its stops kept where GLFW reads Wayland's pointer.
fn patch(src: &Path) {
    patch_app_id(src);
    patch_scroll(src);
}

/// Replaces `from` once in a file, which must have it.
fn replace(file: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(file).unwrap();
    assert!(text.contains(from), "raylib changed: no {from} in {}", file.display());
    std::fs::write(file, text.replacen(from, to, 1)).unwrap();
}

/// Two globals Mascarpone reads: `mascarpone_scroll_continuous`, set
/// before GLFW hands a scroll on (0 for a wheel's notches, 1 for a
/// touchpad's or a trackpoint's pixels); `mascarpone_scroll_stops`,
/// counting the vertical scrolls the compositor said ended (the fingers
/// lifted).
fn patch_scroll(src: &Path) {
    let wl = src.join("external").join("glfw").join("src").join("wl_window.c");
    replace(&wl, "static void pointerHandleAxis(", "int mascarpone_scroll_continuous = 0;\nunsigned int mascarpone_scroll_stops = 0;\n\nstatic void pointerHandleAxis(");
    replace(
        &wl,
        "        processPointerScroll(_glfw.wl.pending.discreteX, _glfw.wl.pending.discreteY);",
        "    {\n        mascarpone_scroll_continuous = 0;\n        processPointerScroll(_glfw.wl.pending.discreteX, _glfw.wl.pending.discreteY);\n    }",
    );
    replace(
        &wl,
        "        processPointerScroll(_glfw.wl.pending.scrollX, _glfw.wl.pending.scrollY);",
        "    {\n        mascarpone_scroll_continuous = 1;\n        processPointerScroll(_glfw.wl.pending.scrollX, _glfw.wl.pending.scrollY);\n    }",
    );
    replace(
        &wl,
        "                                  uint32_t axis)\n{\n}",
        "                                  uint32_t axis)\n{\n    if (axis == WL_POINTER_AXIS_VERTICAL_SCROLL)\n        mascarpone_scroll_stops++;\n}",
    );
}

fn patch_app_id(src: &Path) {
    let hints = "glfwDefaultWindowHints();";
    replace(
        &src.join("platforms").join("rcore_desktop_glfw.c"),
        hints,
        &format!("{hints} glfwWindowHintString(GLFW_WAYLAND_APP_ID, \"{APP_ID}\"); glfwWindowHintString(GLFW_X11_CLASS_NAME, \"{APP_ID}\"); glfwWindowHintString(GLFW_X11_INSTANCE_NAME, \"{APP_ID}\");"),
    );
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
