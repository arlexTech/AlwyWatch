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
}

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
    Ok(Api {
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

/// One mpv instance. Not `Sync`: drive it from a single thread.
pub struct Mpv {
    api: &'static Api,
    h: *mut MpvHandle,
}

// The handle may move to the player thread; libmpv handles are thread-safe for these calls.
unsafe impl Send for Mpv {}

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
