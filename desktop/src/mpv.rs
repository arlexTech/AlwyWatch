// SPDX-License-Identifier: AGPL-3.0-or-later
//! Minimal libmpv binding, loaded at runtime (no build-time dependency on libmpv headers, and the
//! app still starts and browses when libmpv is missing). Only the dozen calls we need.

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

use libloading::Library;

#[repr(C)]
pub struct MpvHandle {
    _private: [u8; 0],
}

#[repr(C)]
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventEndFile {
    pub reason: c_int,
    pub error: c_int,
}

#[repr(C)]
struct MpvEventClientMessage {
    num_args: c_int,
    args: *const *const c_char,
}

pub const EVENT_SHUTDOWN: c_int = 1;
/// `script-message` sent by a key we bound (see `define-section` in the player).
pub const EVENT_CLIENT_MESSAGE: c_int = 16;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
/// Playback resumed after a seek (also once at the start).
pub const EVENT_PLAYBACK_RESTART: c_int = 21;
pub const END_FILE_EOF: c_int = 0;
pub const END_FILE_ERROR: c_int = 4;
const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;

struct Api {
    _lib: Library,
    create: unsafe extern "C" fn() -> *mut MpvHandle,
    initialize: unsafe extern "C" fn(*mut MpvHandle) -> c_int,
    set_option_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int,
    command: unsafe extern "C" fn(*mut MpvHandle, *mut *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(*mut MpvHandle, c_double) -> *mut MpvEvent,
    terminate_destroy: unsafe extern "C" fn(*mut MpvHandle),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    free: unsafe extern "C" fn(*mut c_void),
    /// The render API (video drawn inside AlwyWatch's window); None on a libmpv without it.
    render: Option<RenderApi>,
}

struct RenderApi {
    create: unsafe extern "C" fn(*mut *mut c_void, *mut MpvHandle, *mut RenderParam) -> c_int,
    set_update_callback:
        unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    render: unsafe extern "C" fn(*mut c_void, *mut RenderParam) -> c_int,
    report_swap: unsafe extern "C" fn(*mut c_void),
    free: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct OpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const RENDER_PARAM_OPENGL_FBO: c_int = 3;
const RENDER_PARAM_FLIP_Y: c_int = 4;

#[cfg(windows)]
const CANDIDATES: &[&str] = &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"];
#[cfg(not(windows))]
const CANDIDATES: &[&str] = &["libmpv.so.2", "libmpv.so", "libmpv.so.1"];

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let mut last = String::new();
        for name in CANDIDATES {
            // SAFETY: loading libmpv runs its initialisers; it is a well-behaved C library.
            match unsafe { Library::new(name) } {
                Ok(lib) => return unsafe { load(lib) },
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("libmpv not found ({last})"))
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// # Safety
/// `lib` must be libmpv (API version 2.x): the symbol signatures below match its C header.
unsafe fn load(lib: Library) -> Result<Api, String> {
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the type at each use site matches the libmpv C declaration.
            *unsafe { lib.get($name) }.map_err(|e| e.to_string())?
        };
    }
    macro_rules! opt {
        ($name:literal) => {
            // SAFETY: as above.
            unsafe { lib.get($name) }.ok().map(|s| *s)
        };
    }
    let render = (|| {
        Some(RenderApi {
            create: opt!(b"mpv_render_context_create\0")?,
            set_update_callback: opt!(b"mpv_render_context_set_update_callback\0")?,
            render: opt!(b"mpv_render_context_render\0")?,
            report_swap: opt!(b"mpv_render_context_report_swap\0")?,
            free: opt!(b"mpv_render_context_free\0")?,
        })
    })();
    Ok(Api {
        render,
        create: sym!(b"mpv_create\0"),
        initialize: sym!(b"mpv_initialize\0"),
        set_option_string: sym!(b"mpv_set_option_string\0"),
        get_property: sym!(b"mpv_get_property\0"),
        command: sym!(b"mpv_command\0"),
        wait_event: sym!(b"mpv_wait_event\0"),
        terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
        error_string: sym!(b"mpv_error_string\0"),
        free: sym!(b"mpv_free\0"),
        _lib: lib,
    })
}

pub fn available() -> bool {
    api().is_ok()
}

/// One mpv instance. Events (`wait_event`) are read by one thread only (the player's); commands
/// and properties may come from any thread (libmpv's client API is thread-safe for them).
pub struct Mpv {
    api: &'static Api,
    h: *mut MpvHandle,
}

// The handle may move to the player thread; libmpv handles are thread-safe for these calls.
unsafe impl Send for Mpv {}
// SAFETY: see the type's documentation: only the player thread calls `wait_event`.
unsafe impl Sync for Mpv {}

impl Mpv {
    pub fn new() -> Result<Self, String> {
        let api = api()?;
        // SAFETY: plain constructor; null means failure.
        let h = unsafe { (api.create)() };
        if h.is_null() {
            return Err("mpv_create failed".into());
        }
        Ok(Self { api, h })
    }

    fn check(&self, rc: c_int, what: &str) -> Result<(), String> {
        if rc >= 0 {
            return Ok(());
        }
        // SAFETY: mpv_error_string returns a static string for any code.
        let msg = unsafe { CStr::from_ptr((self.api.error_string)(rc)) }.to_string_lossy();
        Err(format!("{what}: {msg}"))
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name)?, cstr(value)?);
        // SAFETY: valid handle and NUL-terminated strings that outlive the call.
        let rc = unsafe { (self.api.set_option_string)(self.h, n.as_ptr(), v.as_ptr()) };
        self.check(rc, name)
    }

    pub fn initialize(&self) -> Result<(), String> {
        // SAFETY: valid handle, called once after options.
        self.check(unsafe { (self.api.initialize)(self.h) }, "mpv_initialize")
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        // SAFETY: NULL-terminated argv of valid C strings.
        self.check(
            unsafe { (self.api.command)(self.h, ptrs.as_mut_ptr()) },
            args[0],
        )
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let n = cstr(name).ok()?;
        let mut v: c_double = 0.0;
        // SAFETY: MPV_FORMAT_DOUBLE writes one f64 into `v`.
        let rc = unsafe {
            (self.api.get_property)(
                self.h,
                n.as_ptr(),
                FORMAT_DOUBLE,
                (&mut v as *mut c_double).cast(),
            )
        };
        (rc >= 0).then_some(v)
    }

    /// A property as text ("track-list/0/lang", "aid" → "1" or "no").
    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = cstr(name).ok()?;
        let mut v: *mut c_char = ptr::null_mut();
        // SAFETY: MPV_FORMAT_STRING writes a heap string we own and release with mpv_free.
        let rc = unsafe {
            (self.api.get_property)(
                self.h,
                n.as_ptr(),
                FORMAT_STRING,
                (&mut v as *mut *mut c_char).cast(),
            )
        };
        if rc < 0 || v.is_null() {
            return None;
        }
        // SAFETY: `v` is a valid NUL-terminated string until freed just below.
        let out = unsafe { CStr::from_ptr(v) }.to_string_lossy().into_owned();
        unsafe { (self.api.free)(v.cast()) };
        Some(out)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let n = cstr(name).ok()?;
        let mut v: c_int = 0;
        // SAFETY: MPV_FORMAT_FLAG writes one int into `v`.
        let rc = unsafe {
            (self.api.get_property)(
                self.h,
                n.as_ptr(),
                FORMAT_FLAG,
                (&mut v as *mut c_int).cast(),
            )
        };
        (rc >= 0).then_some(v != 0)
    }

    /// Next event. `timeout` in seconds.
    pub fn wait_event(&self, timeout: f64) -> Event {
        // SAFETY: the returned pointer and its data are valid until the next wait_event call;
        // everything is copied out before returning.
        unsafe {
            let ev = &*(self.api.wait_event)(self.h, timeout);
            let reason = (ev.event_id == EVENT_END_FILE && !ev.data.is_null())
                .then(|| (*(ev.data as *const MpvEventEndFile)).reason);
            let message = if ev.event_id == EVENT_CLIENT_MESSAGE && !ev.data.is_null() {
                let m = &*(ev.data as *const MpvEventClientMessage);
                (0..m.num_args.max(0) as usize)
                    .map(|i| *m.args.add(i))
                    .filter(|p| !p.is_null())
                    .map(|p| CStr::from_ptr(p).to_string_lossy().into_owned())
                    .collect()
            } else {
                Vec::new()
            };
            Event {
                id: ev.event_id,
                reason,
                message,
            }
        }
    }
}

/// A copied-out mpv event.
pub struct Event {
    pub id: c_int,
    /// End-file reason, for `EVENT_END_FILE`.
    pub reason: Option<c_int>,
    /// Arguments of a client message (`script-message a b` gives `["a", "b"]`).
    pub message: Vec<String>,
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: handle created by mpv_create and not used after this.
        unsafe { (self.api.terminate_destroy)(self.h) }
    }
}

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "string contains NUL".to_string())
}

/// mpv drawing into AlwyWatch's OpenGL context (`vo=libmpv`). Lives on the UI thread, which owns
/// the context; holds the mpv instance so the core outlives it (mpv requires the render context
/// to be freed first: fields drop after `Drop::drop`).
pub struct RenderContext {
    ctx: *mut c_void,
    api: &'static Api,
    _mpv: std::sync::Arc<Mpv>,
}

impl RenderContext {
    /// `get_proc_address` resolves OpenGL functions in the current context (only used during
    /// this call). `on_update` is called from mpv's threads when a new frame is ready: it must
    /// not call into mpv, only schedule a redraw.
    pub fn new(
        mpv: std::sync::Arc<Mpv>,
        get_proc_address: &dyn Fn(&CStr) -> *const c_void,
        on_update: unsafe extern "C" fn(*mut c_void),
    ) -> Result<Self, String> {
        let api = mpv.api;
        let r = api.render.as_ref().ok_or("this libmpv has no render API")?;
        unsafe extern "C" fn resolve(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
            // SAFETY: `ctx` points at the `&dyn Fn` below, alive for the whole create call.
            let f = unsafe { &*(ctx as *const &dyn Fn(&CStr) -> *const c_void) };
            f(unsafe { CStr::from_ptr(name) }) as *mut c_void
        }
        let mut gpa: &dyn Fn(&CStr) -> *const c_void = get_proc_address;
        let mut init = OpenGlInitParams {
            get_proc_address: Some(resolve),
            get_proc_address_ctx: (&mut gpa as *mut &dyn Fn(&CStr) -> *const c_void).cast(),
        };
        let api_type = c"opengl";
        let mut params = [
            RenderParam {
                kind: RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            RenderParam {
                kind: RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&mut init as *mut OpenGlInitParams).cast(),
            },
            RenderParam {
                kind: 0,
                data: ptr::null_mut(),
            },
        ];
        let mut ctx: *mut c_void = ptr::null_mut();
        // SAFETY: valid handle; params terminated by kind 0; called on the GL thread with the
        // context current.
        let rc = unsafe { (r.create)(&mut ctx, mpv.h, params.as_mut_ptr()) };
        mpv.check(rc, "mpv_render_context_create")?;
        // SAFETY: valid context; the callback only schedules a redraw.
        unsafe { (r.set_update_callback)(ctx, Some(on_update), ptr::null_mut()) };
        Ok(Self {
            ctx,
            api,
            _mpv: mpv,
        })
    }

    /// Draws the current frame into framebuffer `fbo` of `w`×`h` pixels.
    pub fn render(&self, fbo: i32, w: i32, h: i32) {
        let Some(r) = self.api.render.as_ref() else {
            return;
        };
        let mut target = OpenGlFbo {
            fbo,
            w,
            h,
            internal_format: 0,
        };
        // The window's own framebuffer has its origin at the bottom.
        let mut flip: c_int = c_int::from(fbo == 0);
        let mut params = [
            RenderParam {
                kind: RENDER_PARAM_OPENGL_FBO,
                data: (&mut target as *mut OpenGlFbo).cast(),
            },
            RenderParam {
                kind: RENDER_PARAM_FLIP_Y,
                data: (&mut flip as *mut c_int).cast(),
            },
            RenderParam {
                kind: 0,
                data: ptr::null_mut(),
            },
        ];
        // SAFETY: valid context, called on the GL thread with the context current.
        unsafe { (r.render)(self.ctx, params.as_mut_ptr()) };
    }

    pub fn report_swap(&self) {
        if let Some(r) = self.api.render.as_ref() {
            // SAFETY: valid context.
            unsafe { (r.report_swap)(self.ctx) };
        }
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        if let Some(r) = self.api.render.as_ref() {
            // SAFETY: created by mpv_render_context_create, freed once, before the mpv core.
            unsafe { (r.free)(self.ctx) };
        }
    }
}
