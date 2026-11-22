// SPDX-License-Identifier: AGPL-3.0-or-later
//! Video inside AlwyWatch's window (like Android and TV, no separate player window): mpv renders
//! each frame into the window's OpenGL framebuffer just before Slint draws, so the player's
//! controls (Slint) sit on top of the picture. Needs the OpenGL renderer; with the software
//! renderer (headless tests) `attach` fails and the player opens its own window instead.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, GraphicsAPI, RenderingState};

use crate::AppWindow;
use crate::mpv::{Mpv, RenderContext};

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
type GetIntegerv = unsafe extern "system" fn(u32, *mut i32);

/// Tells the waiting player whether the video is in the window.
type Answer = mpsc::Sender<Result<(), String>>;

#[derive(Default)]
struct Video {
    window: Option<slint::Weak<AppWindow>>,
    /// The notifier is installed (OpenGL renderer).
    ready: bool,
    /// A player waiting for its render context (made during the next frame).
    pending: Option<(Arc<Mpv>, Answer)>,
    ctx: Option<RenderContext>,
    get_integerv: Option<GetIntegerv>,
}

thread_local! {
    static VIDEO: RefCell<Video> = RefCell::default();
}

/// Sets up the window's rendering hook. `timing`: print the first frame's time (perf docs).
pub fn install(ui: &AppWindow, timing: Option<(Instant, bool)>) {
    let weak = ui.as_weak();
    VIDEO.with(|v| v.borrow_mut().window = Some(weak.clone()));
    let mut first = true;
    let result = ui
        .window()
        .set_rendering_notifier(move |state, api| match state {
            RenderingState::BeforeRendering => {
                if let GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                    before_rendering(&weak, get_proc_address);
                } else {
                    VIDEO.with(|v| {
                        if let Some((_, tx)) = v.borrow_mut().pending.take() {
                            let _ = tx.send(Err("no OpenGL".into()));
                        }
                    });
                }
            }
            RenderingState::AfterRendering => {
                if let GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                    gl_shot(&weak, get_proc_address);
                }
                VIDEO.with(|v| {
                    if let Some(c) = &v.borrow().ctx {
                        c.report_swap();
                    }
                });
                if first {
                    first = false;
                    if let Some((t0, exit)) = timing {
                        eprintln!("first frame after {} ms", t0.elapsed().as_millis());
                        if exit && let Some(w) = weak.upgrade() {
                            let _ = w.hide();
                        }
                    }
                }
            }
            RenderingState::RenderingTeardown => {
                VIDEO.with(|v| v.borrow_mut().ctx = None);
            }
            _ => {}
        });
    VIDEO.with(|v| v.borrow_mut().ready = result.is_ok());
}

fn before_rendering(
    weak: &slint::Weak<AppWindow>,
    get_proc_address: &dyn Fn(&std::ffi::CStr) -> *const c_void,
) {
    VIDEO.with(|v| {
        let mut v = v.borrow_mut();
        if v.get_integerv.is_none() {
            let f = get_proc_address(c"glGetIntegerv");
            if !f.is_null() {
                // SAFETY: glGetIntegerv has this signature on every OpenGL implementation.
                v.get_integerv =
                    Some(unsafe { std::mem::transmute::<*const c_void, GetIntegerv>(f) });
            }
        }
        if let Some((mpv, tx)) = v.pending.take() {
            let made = RenderContext::new(mpv, get_proc_address, on_update);
            let answer = made.as_ref().map(|_| ()).map_err(Clone::clone);
            v.ctx = made.ok();
            let _ = tx.send(answer);
        }
        let Some(ctx) = &v.ctx else { return };
        let Some(w) = weak.upgrade() else { return };
        let size = w.window().size();
        let mut fbo: i32 = 0;
        if let Some(f) = v.get_integerv {
            // SAFETY: writes one integer; the context is current during rendering.
            unsafe { f(GL_FRAMEBUFFER_BINDING, &mut fbo) };
        }
        ctx.render(fbo, size.width as i32, size.height as i32);
        // Give Slint's renderer the state it expects back (mpv leaves its own texture unit,
        // program and buffers bound; gradients drew nothing over the video without this).
        reset_gl(get_proc_address);
    });
}

/// mpv has a new frame (called on one of mpv's threads): ask for a redraw.
unsafe extern "C" fn on_update(_: *mut c_void) {
    let _ = slint::invoke_from_event_loop(|| {
        VIDEO.with(|v| {
            if let Some(w) = v.borrow().window.as_ref().and_then(slint::Weak::upgrade) {
                w.window().request_redraw();
            }
        });
    });
}

/// From the player thread, after `mpv_initialize` and before loading the file: draw this mpv in
/// the window. Err: no OpenGL here (the player uses its own window).
pub fn attach(mpv: Arc<Mpv>) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    slint::invoke_from_event_loop(move || {
        VIDEO.with(|v| {
            let mut v = v.borrow_mut();
            if !v.ready {
                let _ = tx.send(Err("no OpenGL".into()));
                return;
            }
            // A previous player's context goes first (it holds that mpv).
            v.ctx = None;
            v.pending = Some((mpv, tx));
            if let Some(w) = v.window.as_ref().and_then(slint::Weak::upgrade) {
                w.window().request_redraw();
            }
        });
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the window did not draw".to_string())?
}

/// On the UI thread when playback ended: frees the render context (and with it, once the player
/// thread is done, the mpv instance).
pub fn detach() {
    VIDEO.with(|v| {
        let mut v = v.borrow_mut();
        v.ctx = None;
        v.pending = None;
        if let Some(w) = v.window.as_ref().and_then(slint::Weak::upgrade) {
            w.window().request_redraw();
        }
    });
}

/// Test hook: `ALWYWATCH_GL_SHOT=out.png` saves what the window drew (video and controls) once,
/// `ALWYWATCH_GL_SHOT_AFTER_MS` (default 6000) after the first frame with a video.
fn gl_shot(
    weak: &slint::Weak<AppWindow>,
    get_proc_address: &dyn Fn(&std::ffi::CStr) -> *const c_void,
) {
    use std::cell::Cell;
    thread_local! { static SINCE: Cell<Option<Instant>> = const { Cell::new(None) }; }
    let Some(out) = std::env::var_os("ALWYWATCH_GL_SHOT") else {
        return;
    };
    if VIDEO.with(|v| v.borrow().ctx.is_none()) {
        return;
    }
    let since = SINCE.with(|s| {
        *s.get().get_or_insert_with(|| {
            let n = Instant::now();
            s.set(Some(n));
            n
        })
    });
    let after = std::env::var("ALWYWATCH_GL_SHOT_AFTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6000);
    if since.elapsed() < Duration::from_millis(after) {
        if let Some(w) = weak.upgrade() {
            w.window().request_redraw();
        }
        return;
    }
    // SAFETY: test-only; read once, the env var is removed after.
    unsafe { std::env::remove_var("ALWYWATCH_GL_SHOT") };
    let Some(w) = weak.upgrade() else { return };
    let size = w.window().size();
    let f = get_proc_address(c"glReadPixels");
    if f.is_null() {
        return;
    }
    type ReadPixels = unsafe extern "system" fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    // SAFETY: glReadPixels' signature; the buffer holds w*h RGBA pixels.
    let read = unsafe { std::mem::transmute::<*const c_void, ReadPixels>(f) };
    let (wd, ht) = (size.width as usize, size.height as usize);
    let mut px = vec![0u8; wd * ht * 4];
    unsafe {
        read(
            0,
            0,
            wd as i32,
            ht as i32,
            0x1908,
            0x1401,
            px.as_mut_ptr().cast(),
        )
    };
    let mut flipped = Vec::with_capacity(px.len());
    for row in px.chunks(wd * 4).rev() {
        flipped.extend_from_slice(row);
    }
    if let Some(img) = image::RgbaImage::from_raw(wd as u32, ht as u32, flipped) {
        let _ = img.save(&out);
        eprintln!("saved {}", out.to_string_lossy());
    }
}

fn reset_gl(get_proc_address: &dyn Fn(&std::ffi::CStr) -> *const c_void) {
    type U = unsafe extern "system" fn(u32);
    type UU = unsafe extern "system" fn(u32, u32);
    let f = |n: &std::ffi::CStr| {
        let p = get_proc_address(n);
        (!p.is_null()).then_some(p)
    };
    // SAFETY: standard OpenGL entry points with these signatures; the context is current.
    unsafe {
        if let Some(p) = f(c"glActiveTexture") {
            std::mem::transmute::<*const c_void, U>(p)(0x84C0);
        }
        if let Some(p) = f(c"glBindTexture") {
            std::mem::transmute::<*const c_void, UU>(p)(0x0DE1, 0);
        }
        if let Some(p) = f(c"glUseProgram") {
            std::mem::transmute::<*const c_void, U>(p)(0);
        }
        if let Some(p) = f(c"glBindVertexArray") {
            std::mem::transmute::<*const c_void, U>(p)(0);
        }
        if let Some(p) = f(c"glBindBuffer") {
            std::mem::transmute::<*const c_void, UU>(p)(0x8892, 0);
        }
    }
}
