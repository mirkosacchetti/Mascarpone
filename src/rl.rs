#![allow(non_snake_case, dead_code)]

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector2 {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Image {
    pub data: *mut c_void,
    pub width: c_int,
    pub height: c_int,
    pub mipmaps: c_int,
    pub format: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Texture {
    pub id: c_uint,
    pub width: c_int,
    pub height: c_int,
    pub mipmaps: c_int,
    pub format: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    pub value: c_int,
    pub offsetX: c_int,
    pub offsetY: c_int,
    pub advanceX: c_int,
    pub image: Image,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Font {
    pub baseSize: c_int,
    pub glyphCount: c_int,
    pub glyphPadding: c_int,
    pub texture: Texture,
    pub recs: *mut Rectangle,
    pub glyphs: *mut GlyphInfo,
}

pub const FLAG_WINDOW_RESIZABLE: c_uint = 0x0000_0004;
pub const FLAG_WINDOW_HIGHDPI: c_uint = 0x0000_2000;
pub const FLAG_MSAA_4X_HINT: c_uint = 0x0000_0020;

pub const LOG_ERROR: c_int = 5;

pub const MOUSE_CURSOR_DEFAULT: c_int = 0;
pub const MOUSE_CURSOR_POINTING_HAND: c_int = 4;

pub const TEXTURE_FILTER_BILINEAR: c_int = 1;
pub const TEXTURE_FILTER_TRILINEAR: c_int = 2;

pub const PIXELFORMAT_UNCOMPRESSED_R8G8B8A8: c_int = 7;

pub const KEY_SPACE: i32 = 32;
pub const KEY_ESCAPE: i32 = 256;
pub const KEY_ENTER: i32 = 257;
pub const KEY_TAB: i32 = 258;
pub const KEY_BACKSPACE: i32 = 259;
pub const KEY_DELETE: i32 = 261;
pub const KEY_RIGHT: i32 = 262;
pub const KEY_LEFT: i32 = 263;
pub const KEY_DOWN: i32 = 264;
pub const KEY_UP: i32 = 265;
pub const KEY_PAGE_UP: i32 = 266;
pub const KEY_PAGE_DOWN: i32 = 267;
pub const KEY_HOME: i32 = 268;
pub const KEY_END: i32 = 269;
pub const KEY_KP_0: i32 = 320;
pub const KEY_KP_SUBTRACT: i32 = 333;
pub const KEY_KP_ADD: i32 = 334;
pub const KEY_KP_ENTER: i32 = 335;

pub const MOD_SHIFT: i32 = 0x1;
pub const MOD_CONTROL: i32 = 0x2;
pub const MOD_ALT: i32 = 0x4;

const GLFW_WAYLAND_LIBDECOR: c_int = 0x0005_3001;
const GLFW_WAYLAND_DISABLE_LIBDECOR: c_int = 0x0003_8002;
const GLFW_PLATFORM_WAYLAND: c_int = 0x0006_0003;
const GLFW_PRESS: c_int = 1;
const GLFW_REPEAT: c_int = 2;

extern "C" {
    pub fn SetConfigFlags(flags: c_uint);
    pub fn SetTraceLogLevel(level: c_int);
    pub fn InitWindow(width: c_int, height: c_int, title: *const c_char);
    pub fn CloseWindow();
    pub fn IsWindowFocused() -> bool;
    pub fn SetExitKey(key: c_int);
    pub fn SetTargetFPS(fps: c_int);
    pub fn SetWindowMinSize(width: c_int, height: c_int);
    pub fn SetWindowFocused();
    pub fn GetScreenWidth() -> c_int;
    pub fn GetScreenHeight() -> c_int;
    pub fn GetRenderWidth() -> c_int;
    pub fn GetRenderHeight() -> c_int;
    pub fn GetWindowScaleDPI() -> Vector2;
    pub fn GetMousePosition() -> Vector2;
    pub fn IsMouseButtonDown(button: c_int) -> bool;
    pub fn IsCursorOnScreen() -> bool;
    pub fn SetMouseCursor(cursor: c_int);
    pub fn GetClipboardText() -> *const c_char;

    pub fn BeginDrawing();
    pub fn EndDrawing();
    pub fn ClearBackground(color: Color);

    pub fn DrawRectangleRec(rec: Rectangle, color: Color);
    pub fn DrawRectangleRounded(rec: Rectangle, roundness: f32, segments: c_int, color: Color);
    pub fn DrawCircleV(center: Vector2, radius: f32, color: Color);
    pub fn DrawTriangleFan(points: *const Vector2, count: c_int, color: Color);
    pub fn DrawTexturePro(texture: Texture, source: Rectangle, dest: Rectangle, origin: Vector2, rotation: f32, tint: Color);

    pub fn LoadTextureFromImage(image: Image) -> Texture;
    pub fn UnloadTexture(texture: Texture);
    pub fn GenTextureMipmaps(texture: *mut Texture);
    pub fn SetTextureFilter(texture: Texture, filter: c_int);

    pub fn LoadFontFromMemory(file_type: *const c_char, data: *const u8, size: c_int, font_size: c_int, codepoints: *const c_int, count: c_int) -> Font;
    pub fn UnloadFont(font: Font);

    pub fn rlLoadIdentity();
    pub fn rlDrawRenderBatchActive();
    pub fn rlEnableScissorTest();
    pub fn rlDisableScissorTest();
    pub fn rlScissor(x: c_int, y: c_int, width: c_int, height: c_int);

    fn glfwInitHint(hint: c_int, value: c_int);
    fn glfwGetCurrentContext() -> *mut c_void;
    fn glfwGetPlatform() -> c_int;
    fn glfwGetWaylandWindow(window: *mut c_void) -> *mut c_void;
    fn glfwWaitEventsTimeout(timeout: f64);
    fn glfwPostEmptyEvent();
    fn glfwGetKeyName(key: c_int, scancode: c_int) -> *const c_char;
    fn glfwSetKeyCallback(window: *mut c_void, f: Option<KeyFn>) -> Option<KeyFn>;
    fn glfwSetCharCallback(window: *mut c_void, f: Option<CharFn>) -> Option<CharFn>;
    fn glfwSetMouseButtonCallback(window: *mut c_void, f: Option<ButtonFn>) -> Option<ButtonFn>;
    fn glfwSetScrollCallback(window: *mut c_void, f: Option<ScrollFn>) -> Option<ScrollFn>;
    fn glfwSetWindowCloseCallback(window: *mut c_void, f: Option<CloseFn>) -> Option<CloseFn>;

    static mascarpone_scroll_continuous: c_int;
    static mascarpone_scroll_stops: c_uint;
}

type KeyFn = extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int);
type CharFn = extern "C" fn(*mut c_void, c_uint);
type ButtonFn = extern "C" fn(*mut c_void, c_int, c_int, c_int);
type ScrollFn = extern "C" fn(*mut c_void, f64, f64);
type CloseFn = extern "C" fn(*mut c_void);

#[derive(Debug, Clone, Copy)]
pub enum Input {
    Key { code: i32, mods: i32 },
    Char(char),
    Press(i32),
    Release(i32),
    Scroll { y: f32, wheel: bool },
    ScrollStop,
}

struct Chain {
    key: Option<KeyFn>,
    char: Option<CharFn>,
    button: Option<ButtonFn>,
    scroll: Option<ScrollFn>,
    close: Option<CloseFn>,
}

static mut CHAIN: Chain = Chain { key: None, char: None, button: None, scroll: None, close: None };
static ALIVE: AtomicBool = AtomicBool::new(false);
static CLOSING: AtomicBool = AtomicBool::new(false);

thread_local! {
    static INPUT: RefCell<Vec<Input>> = const { RefCell::new(Vec::new()) };
    static STOPS: Cell<c_uint> = const { Cell::new(0) };
}

fn push(input: Input) {
    INPUT.with(|i| i.borrow_mut().push(input));
}

extern "C" fn on_key(window: *mut c_void, key: c_int, scancode: c_int, action: c_int, mods: c_int) {
    if action == GLFW_PRESS || action == GLFW_REPEAT {
        push(Input::Key { code: key, mods });
    }
    if let Some(f) = unsafe { CHAIN.key } {
        f(window, key, scancode, action, mods);
    }
}

extern "C" fn on_char(window: *mut c_void, codepoint: c_uint) {
    if let Some(c) = char::from_u32(codepoint) {
        push(Input::Char(c));
    }
    if let Some(f) = unsafe { CHAIN.char } {
        f(window, codepoint);
    }
}

extern "C" fn on_button(window: *mut c_void, button: c_int, action: c_int, mods: c_int) {
    push(if action == GLFW_PRESS { Input::Press(button) } else { Input::Release(button) });
    if let Some(f) = unsafe { CHAIN.button } {
        f(window, button, action, mods);
    }
}

extern "C" fn on_scroll(window: *mut c_void, x: f64, y: f64) {
    let wheel = unsafe { mascarpone_scroll_continuous } == 0;
    push(Input::Scroll { y: y as f32, wheel });
    if let Some(f) = unsafe { CHAIN.scroll } {
        f(window, x, y);
    }
}

/// The compositor asked the window to close. raylib's WindowShouldClose
/// learns of it only at the end of the next drawn frame, in
/// PollInputEvents, while an idle loop sleeping in `wait` is woken by the
/// request and would go back to sleep: the flag is kept here, as the
/// request arrives.
extern "C" fn on_close(window: *mut c_void) {
    CLOSING.store(true, Ordering::SeqCst);
    if let Some(f) = unsafe { CHAIN.close } {
        f(window);
    }
}

/// Whether the window was asked to close, by the compositor or a key.
pub fn closing() -> bool {
    CLOSING.load(Ordering::SeqCst)
}

pub fn open(width: i32, height: i32, title: &str) {
    let title = CString::new(title).unwrap();
    unsafe {
        SetTraceLogLevel(LOG_ERROR);
        glfwInitHint(GLFW_WAYLAND_LIBDECOR, GLFW_WAYLAND_DISABLE_LIBDECOR);
        SetConfigFlags(FLAG_WINDOW_RESIZABLE | FLAG_WINDOW_HIGHDPI | FLAG_MSAA_4X_HINT);
        InitWindow(width, height, title.as_ptr());
        SetExitKey(0);
        SetTargetFPS(60);
        let window = glfwGetCurrentContext();
        CHAIN = Chain {
            key: glfwSetKeyCallback(window, Some(on_key)),
            char: glfwSetCharCallback(window, Some(on_char)),
            button: glfwSetMouseButtonCallback(window, Some(on_button)),
            scroll: glfwSetScrollCallback(window, Some(on_scroll)),
            close: glfwSetWindowCloseCallback(window, Some(on_close)),
        };
        if glfwGetPlatform() == GLFW_PLATFORM_WAYLAND {
            WAYLAND.with(|w| *w.borrow_mut() = Wayland::new(glfwGetWaylandWindow(window)));
        }
    }
    ALIVE.store(true, Ordering::SeqCst);
}

struct Wayland {
    surface: *mut c_void,
    marshal: MarshalFn,
    version: VersionFn,
    add_listener: ListenerFn,
    destroy: DestroyFn,
    callback: *const c_void,
}

type MarshalFn = unsafe extern "C" fn(*mut c_void, u32, *const c_void, u32, u32, ...) -> *mut c_void;
type VersionFn = unsafe extern "C" fn(*mut c_void) -> u32;
type ListenerFn = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int;
type DestroyFn = unsafe extern "C" fn(*mut c_void);

impl Wayland {
    unsafe fn new(surface: *mut c_void) -> Option<Wayland> {
        if surface.is_null() {
            return None;
        }
        let lib = libc::dlopen(c"libwayland-client.so.0".as_ptr(), libc::RTLD_LAZY | libc::RTLD_NOLOAD);
        if lib.is_null() {
            return None;
        }
        let sym = |name: &CStr| {
            let p = libc::dlsym(lib, name.as_ptr());
            (!p.is_null()).then_some(p)
        };
        Some(Wayland {
            surface,
            marshal: std::mem::transmute::<*mut c_void, MarshalFn>(sym(c"wl_proxy_marshal_flags")?),
            version: std::mem::transmute::<*mut c_void, VersionFn>(sym(c"wl_proxy_get_version")?),
            add_listener: std::mem::transmute::<*mut c_void, ListenerFn>(sym(c"wl_proxy_add_listener")?),
            destroy: std::mem::transmute::<*mut c_void, DestroyFn>(sym(c"wl_proxy_destroy")?),
            callback: sym(c"wl_callback_interface")?,
        })
    }
}

thread_local! {
    static WAYLAND: RefCell<Option<Wayland>> = const { RefCell::new(None) };
    static UNSHOWN: Cell<Option<Instant>> = const { Cell::new(None) };
}

const WL_SURFACE_FRAME: u32 = 3;

#[repr(C)]
struct CallbackListener {
    done: extern "C" fn(*mut c_void, *mut c_void, u32),
}

static LISTENER: CallbackListener = CallbackListener { done: on_shown };

extern "C" fn on_shown(_data: *mut c_void, callback: *mut c_void, _time: u32) {
    UNSHOWN.with(|u| u.set(None));
    WAYLAND.with(|w| {
        if let Some(w) = &*w.borrow() {
            unsafe { (w.destroy)(callback) };
        }
    });
}

pub fn ask_shown() {
    if UNSHOWN.with(Cell::get).is_some() {
        return;
    }
    WAYLAND.with(|w| {
        let Some(w) = &*w.borrow() else { return };
        unsafe {
            let cb = (w.marshal)(w.surface, WL_SURFACE_FRAME, w.callback, (w.version)(w.surface), 0, std::ptr::null_mut::<c_void>());
            if cb.is_null() {
                return;
            }
            (w.add_listener)(cb, std::ptr::addr_of!(LISTENER).cast(), std::ptr::null_mut());
        }
        UNSHOWN.with(|u| u.set(Some(Instant::now())));
    });
}

pub fn may_draw() -> bool {
    match UNSHOWN.with(Cell::get) {
        None => true,
        Some(since) if since.elapsed() > Duration::from_secs(2) && INPUT.with(|i| !i.borrow().is_empty()) => {
            UNSHOWN.with(|u| u.set(None));
            true
        }
        Some(_) => false,
    }
}

pub fn close() {
    ALIVE.store(false, Ordering::SeqCst);
    unsafe { CloseWindow() };
}

pub fn input_waiting() -> bool {
    INPUT.with(|i| !i.borrow().is_empty())
}

pub fn take_input() -> Vec<Input> {
    let stops = unsafe { mascarpone_scroll_stops };
    if STOPS.with(|s| s.replace(stops)) != stops {
        push(Input::ScrollStop);
    }
    INPUT.with(|i| std::mem::take(&mut *i.borrow_mut()))
}

pub fn wait(seconds: f64) {
    unsafe { glfwWaitEventsTimeout(seconds.max(0.0)) };
}

pub fn wake() {
    if ALIVE.load(Ordering::SeqCst) {
        unsafe { glfwPostEmptyEvent() };
    }
}

pub fn key_char(code: i32) -> Option<char> {
    let name = unsafe { glfwGetKeyName(code, 0) };
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_str().ok()?;
    let mut chars = name.chars();
    let c = chars.next()?;
    chars.next().is_none().then(|| c.to_lowercase().next().unwrap_or(c))
}

pub fn clipboard() -> String {
    let text = unsafe { GetClipboardText() };
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

pub fn texture(rgba: &[u8], width: u32, height: u32) -> Texture {
    let image = Image { data: rgba.as_ptr() as *mut c_void, width: width as c_int, height: height as c_int, mipmaps: 1, format: PIXELFORMAT_UNCOMPRESSED_R8G8B8A8 };
    unsafe {
        let mut t = LoadTextureFromImage(image);
        GenTextureMipmaps(&mut t);
        SetTextureFilter(t, TEXTURE_FILTER_TRILINEAR);
        t
    }
}
