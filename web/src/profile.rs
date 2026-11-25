// SPDX-License-Identifier: AGPL-3.0-or-later
//! The AlwyWatch account profile: the avatar drawn wherever a person is shown, the name and
//! avatar settings, the picture cropper, and the one-time welcome step.
//!
//! Built-in avatars and the default one (initial on a colour) are inline SVG, drawn from
//! `keep_service::profile`. Custom pictures need the signed-in headers (an `<img src>` can't
//! send them), so they're fetched once per (user, version) and shown from a blob URL.

use i18n::tr;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;

use keep_service::profile::{self as kp, AVATAR_MAX_BYTES, AVATAR_MAX_SIDE, AVATAR_TYPES};
use keep_service::{Avatar as AvatarSpec, KeepClient, Me};
use leptos::prelude::*;
use leptos::task::spawn_local;
use net::Error;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::state;
use crate::ui::ErrorText;

fn no_keep() -> String {
    tr!(
        "This only works with a login on one of this site's servers. Add one in Settings → Servers → Add a server."
    )
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

/// A coordinate as short SVG text ("0.95", "-1", "0").
fn num(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn points(pts: &[(f64, f64)]) -> String {
    pts.iter()
        .map(|(x, y)| format!("{} {}", num(*x), num(*y)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Points around (0, 0), the first straight up, at these radii in turn.
fn around(n: usize, radii: &[f64]) -> Vec<(f64, f64)> {
    (0..n)
        .map(|i| {
            let a = (-90.0 + 360.0 * i as f64 / n as f64).to_radians();
            let r = radii[i % radii.len()];
            (r * a.cos(), r * a.sin())
        })
        .collect()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    format!(
        r#"<rect x="{}" y="{}" width="{}" height="{}"/>"#,
        num(x0),
        num(y0),
        num(x1 - x0),
        num(y1 - y0)
    )
}

/// A built-in shape in the unit box of `docs/keep-protocol.md` (1 unit = a quarter of the
/// avatar's diameter), filled with `fg`. None for a shape this version doesn't know.
fn shape_markup(shape: &str, fg: &str) -> Option<String> {
    let plus = || {
        format!(
            "{}{}",
            rect(-1.0, -0.3, 1.0, 0.3),
            rect(-0.3, -1.0, 0.3, 1.0)
        )
    };
    let body = match shape {
        "circle" => r#"<circle r="1"/>"#.to_string(),
        "ring" => format!(r#"<circle r="0.75" fill="none" stroke="{fg}" stroke-width="0.5"/>"#),
        "square" => rect(-0.85, -0.85, 0.85, 0.85),
        "diamond" => format!(
            r#"<polygon points="{}"/>"#,
            points(&[(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)])
        ),
        "triangle" => format!(
            r#"<polygon points="{}"/>"#,
            points(&[(0.0, -1.0), (0.95, 0.7), (-0.95, 0.7)])
        ),
        "plus" => plus(),
        "cross" => format!(r#"<g transform="rotate(45)">{}</g>"#, plus()),
        "star" => format!(
            r#"<polygon points="{}"/>"#,
            points(&around(10, &[1.0, 0.45]))
        ),
        "hexagon" => format!(r#"<polygon points="{}"/>"#, points(&around(6, &[1.0]))),
        "bars" => format!(
            "{}{}{}",
            rect(-1.0, -0.8, 1.0, -0.4),
            rect(-1.0, -0.2, 1.0, 0.2),
            rect(-1.0, 0.4, 1.0, 0.8)
        ),
        "half" => r#"<path d="M-1 0.25A1 1 0 0 1 1 0.25Z"/>"#.to_string(),
        "dots" => [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]
            .iter()
            .map(|(x, y)| format!(r#"<circle cx="{}" cy="{}" r="0.35"/>"#, num(*x), num(*y)))
            .collect(),
        _ => return None,
    };
    Some(format!(r#"<g fill="{fg}">{body}</g>"#))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A built-in or default avatar as SVG (`preset` unknown or None: the name's initial on its
/// colour). The view box is the unit box doubled: the disc has radius 2.
pub fn avatar_svg(name: &str, preset: Option<&str>) -> String {
    let open = r#"<svg viewBox="-2 -2 4 4" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">"#;
    if let Some(p) = preset.and_then(|id| kp::avatar_presets().into_iter().find(|p| p.id == id))
        && let Some(shape) = shape_markup(&p.shape, &p.foreground)
    {
        return format!(
            r#"{open}<circle r="2" fill="{}"/>{shape}</svg>"#,
            p.background
        );
    }
    format!(
        r##"{open}<circle r="2" fill="{}"/><text y="0.06" text-anchor="middle" dominant-baseline="central" fill="#FFFFFF" font-size="1.8" font-weight="700" font-family="system-ui, sans-serif">{}</text></svg>"##,
        kp::default_avatar_color(name),
        escape(&kp::avatar_initial(name))
    )
}

thread_local! {
    /// The custom picture asked for each user (version), so each is fetched once.
    static ASKED: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

/// A blob URL for picture bytes.
fn blob_url(bytes: &[u8], mime: &str) -> Option<String> {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

/// A user's picture is this blob URL (the one it replaces is freed).
fn keep_picture(user_key: &str, version: u32, url: String) {
    let current = ASKED.with(|a| a.borrow().get(user_key).copied());
    if current.is_some_and(|v| v != version) {
        // A newer picture was asked meanwhile.
        let _ = web_sys::Url::revoke_object_url(&url);
        return;
    }
    let avatars = state::ctx().avatars;
    let old = avatars
        .try_update(|m| m.insert(user_key.to_owned(), (version, Some(url))))
        .flatten();
    if let Some((_, Some(old))) = old {
        let _ = web_sys::Url::revoke_object_url(&old);
    }
}

/// The picture just uploaded is the new one: no need to download it again.
fn seed_picture(user_key: &str, version: u32, url: String) {
    ASKED.with(|a| a.borrow_mut().insert(user_key.to_owned(), version));
    keep_picture(user_key, version, url);
}

async fn fetch_picture(k: &KeepClient, user_key: &str, version: u32) -> Option<String> {
    let mut req = state::ctx().http.get(k.avatar_url(user_key, version));
    for (name, value) in k.auth_headers() {
        req = req.header(name, value);
    }
    let res = req.send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let mime = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .filter(|t| AVATAR_TYPES.contains(t))
        .unwrap_or("image/jpeg")
        .to_owned();
    let bytes = res.bytes().await.ok()?;
    blob_url(&bytes, &mime)
}

/// Fetches a custom picture once per (user, version).
fn ensure_picture(user_key: &str, version: u32) {
    let asked = ASKED.with(|a| a.borrow().get(user_key).copied());
    if asked == Some(version) {
        return;
    }
    // The account session too: a profile with only an AlwyWatch account sees pictures.
    let Some(k) = state::ctx().person_client() else {
        return;
    };
    ASKED.with(|a| a.borrow_mut().insert(user_key.to_owned(), version));
    let key = user_key.to_owned();
    spawn_local(async move {
        match fetch_picture(&k, &key, version).await {
            Some(url) => keep_picture(&key, version, url),
            // Failed (offline, signed out): asked again by the next avatar shown.
            None => ASKED.with(|a| {
                let mut a = a.borrow_mut();
                if a.get(&key) == Some(&version) {
                    a.remove(&key);
                }
            }),
        }
    });
}

/// A round avatar: the custom picture (once loaded), else the built-in one, else the default.
/// `user_key` may be empty when unknown (no custom picture then).
#[component]
pub fn Avatar(
    #[prop(into)] user_key: String,
    #[prop(into)] name: String,
    avatar: AvatarSpec,
    #[prop(default = 32)] size: u32,
) -> impl IntoView {
    let avatars = state::ctx().avatars;
    let version = avatar.custom_version.filter(|_| !user_key.is_empty());
    if let Some(v) = version {
        ensure_picture(&user_key, v);
    }
    let svg = avatar_svg(&name, avatar.preset.as_deref());
    let style = format!("width:{size}px;height:{size}px");
    move || {
        let url = version.and_then(|v| {
            avatars.with(|m| {
                m.get(&user_key)
                    .filter(|(x, _)| *x == v)
                    .and_then(|(_, u)| u.clone())
            })
        });
        match url {
            Some(u) => view! { <img class="avatar" src=u alt="" style=style.clone() /> }.into_any(),
            None => view! { <span class="avatar" style=style.clone() inner_html=svg.clone() /> }
                .into_any(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Cropping
// ---------------------------------------------------------------------------------------------

/// The square of the picture under the round frame, in source pixels: always inside the
/// picture (the frame is always covered).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub w: f64,
    pub h: f64,
    /// 1 = the whole short side under the frame.
    pub zoom: f64,
    /// Top-left corner of the square.
    pub x: f64,
    pub y: f64,
}

impl Crop {
    /// Centred, zoomed out as far as possible.
    pub fn new(w: f64, h: f64) -> Self {
        let mut c = Self {
            w: w.max(1.0),
            h: h.max(1.0),
            zoom: 1.0,
            x: 0.0,
            y: 0.0,
        };
        let side = c.side();
        c.x = (c.w - side) / 2.0;
        c.y = (c.h - side) / 2.0;
        c
    }

    /// Zooming in stops when 64 source pixels fill the frame (or at 8×).
    pub fn max_zoom(&self) -> f64 {
        (self.w.min(self.h) / 64.0).clamp(1.0, 8.0)
    }

    pub fn side(&self) -> f64 {
        self.w.min(self.h) / self.zoom
    }

    fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, self.max_zoom());
        let side = self.side();
        self.x = self.x.clamp(0.0, self.w - side);
        self.y = self.y.clamp(0.0, self.h - side);
    }

    /// The picture moved by (dx, dy) on screen, with a frame `frame` pixels wide.
    pub fn pan(&mut self, dx: f64, dy: f64, frame: f64) {
        let k = self.side() / frame;
        self.x -= dx * k;
        self.y -= dy * k;
        self.clamp();
    }

    /// Zooms to `zoom` keeping the point under (ax, ay) (frame pixels) where it is.
    pub fn zoom_at(&mut self, zoom: f64, ax: f64, ay: f64, frame: f64) {
        let before = self.side();
        let (px, py) = (self.x + ax / frame * before, self.y + ay / frame * before);
        self.zoom = zoom.clamp(1.0, self.max_zoom());
        let after = self.side();
        self.x = px - ax / frame * after;
        self.y = py - ay / frame * after;
        self.clamp();
    }

    /// Side of the picture to upload: the square's own size, at most 512 (never upscaled).
    pub fn output_side(&self) -> u32 {
        (self.side().floor() as u32).clamp(1, AVATAR_MAX_SIDE)
    }
}

/// Frame (the round window) and the margin of picture shown around it, in CSS pixels.
const FRAME: f64 = 220.0;
const MARGIN: f64 = 30.0;
const STAGE: f64 = FRAME + 2.0 * MARGIN;

type Bitmap = StoredValue<Option<web_sys::ImageBitmap>, LocalStorage>;

fn context_2d(canvas: &web_sys::HtmlCanvasElement) -> Option<web_sys::CanvasRenderingContext2d> {
    canvas.get_context("2d").ok()??.dyn_into().ok()
}

/// The picture around and under the frame, at the screen's resolution.
fn draw_preview(canvas: &web_sys::HtmlCanvasElement, bitmap: Bitmap, k: &Crop) {
    let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
    let px = (STAGE * dpr).round();
    if canvas.width() != px as u32 {
        canvas.set_width(px as u32);
        canvas.set_height(px as u32);
    }
    let Some(ctx) = context_2d(canvas) else {
        return;
    };
    ctx.set_fill_style_str("#000");
    ctx.fill_rect(0.0, 0.0, px, px);
    let per = k.side() / FRAME;
    bitmap.with_value(|b| {
        if let Some(b) = b {
            let _ = ctx.draw_image_with_image_bitmap_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                b,
                k.x - MARGIN * per,
                k.y - MARGIN * per,
                STAGE * per,
                STAGE * per,
                0.0,
                0.0,
                px,
                px,
            );
        }
    });
}

/// The square under the frame as JPEG bytes.
async fn encode(bitmap: Bitmap, k: &Crop, quality: f64) -> Option<Vec<u8>> {
    let doc = web_sys::window()?.document()?;
    let canvas: web_sys::HtmlCanvasElement = doc.create_element("canvas").ok()?.dyn_into().ok()?;
    let out = k.output_side();
    canvas.set_width(out);
    canvas.set_height(out);
    let ctx = context_2d(&canvas)?;
    let _ = js_sys::Reflect::set(&ctx, &"imageSmoothingQuality".into(), &"high".into());
    // JPEG has no transparency: a transparent picture gets white behind it.
    ctx.set_fill_style_str("#fff");
    ctx.fill_rect(0.0, 0.0, out as f64, out as f64);
    let side = k.side();
    let drawn = bitmap.try_with_value(|b| {
        b.as_ref().map(|b| {
            ctx.draw_image_with_image_bitmap_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                b, k.x, k.y, side, side, 0.0, 0.0, out as f64, out as f64,
            )
        })
    });
    if !matches!(drawn, Some(Some(Ok(())))) {
        return None;
    }
    let p = js_sys::Promise::new(&mut |resolve, _| {
        let _ = canvas.to_blob_with_type_and_encoder_options(
            &resolve,
            "image/jpeg",
            &JsValue::from_f64(quality),
        );
    });
    let blob: web_sys::Blob = JsFuture::from(p).await.ok()?.dyn_into().ok()?;
    let buf = JsFuture::from(blob.array_buffer()).await.ok()?;
    Some(js_sys::Uint8Array::new(&buf).to_vec())
}

/// A picked file as a bitmap, turned upright as its EXIF says.
async fn read_picture(file: &web_sys::File) -> Option<web_sys::ImageBitmap> {
    let w = web_sys::window()?;
    let opts = web_sys::ImageBitmapOptions::new();
    opts.set_image_orientation(web_sys::ImageOrientation::FromImage);
    let p = w
        .create_image_bitmap_with_blob_and_image_bitmap_options(file, &opts)
        .ok()?;
    JsFuture::from(p).await.ok()?.dyn_into().ok()
}

/// The picture under a round frame: drag (mouse or touch) to move; wheel, pinch or the slider
/// to zoom; arrows and +/- from the keyboard, Enter saves, Escape cancels.
#[component]
fn Cropper(
    bitmap: Bitmap,
    w: f64,
    h: f64,
    on_done: impl Fn(Option<Vec<u8>>) + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let crop = RwSignal::new(Crop::new(w, h));
    let max_zoom = crop.get_untracked().max_zoom();
    let canvas = NodeRef::<leptos::html::Canvas>::new();
    let stage = NodeRef::<leptos::html::Div>::new();
    let dialog = NodeRef::<leptos::html::Div>::new();
    // Pointers down on the picture: id, last x, last y.
    let pointers = StoredValue::new(Vec::<(i32, f64, f64)>::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        if let Some(d) = dialog.get() {
            let _ = d.focus();
        }
    });
    Effect::new(move |_| {
        let k = crop.get();
        if let Some(c) = canvas.get() {
            draw_preview(&c, bitmap, &k);
        }
    });
    // A point on screen, in frame pixels.
    let in_frame = move |x: f64, y: f64| -> (f64, f64) {
        stage
            .get_untracked()
            .map_or((FRAME / 2.0, FRAME / 2.0), |s| {
                let r = s.get_bounding_client_rect();
                (x - r.left() - MARGIN, y - r.top() - MARGIN)
            })
    };
    let zoom_by = move |factor: f64, at: Option<(f64, f64)>| {
        let (ax, ay) = at.unwrap_or((FRAME / 2.0, FRAME / 2.0));
        crop.update(|c| c.zoom_at(c.zoom * factor, ax, ay, FRAME));
    };
    let down = move |ev: web_sys::PointerEvent| {
        ev.prevent_default();
        if let Some(s) = stage.get_untracked() {
            let _ = s.set_pointer_capture(ev.pointer_id());
        }
        let (id, x, y) = (ev.pointer_id(), ev.client_x() as f64, ev.client_y() as f64);
        pointers.update_value(|p| {
            p.retain(|q| q.0 != id);
            p.push((id, x, y));
        });
    };
    let moved = move |ev: web_sys::PointerEvent| {
        let (id, x, y) = (ev.pointer_id(), ev.client_x() as f64, ev.client_y() as f64);
        let before = pointers.get_value();
        let Some(i) = before.iter().position(|p| p.0 == id) else {
            return;
        };
        let mut after = before.clone();
        after[i] = (id, x, y);
        pointers.set_value(after.clone());
        match before.len() {
            1 => {
                let (dx, dy) = (x - before[i].1, y - before[i].2);
                crop.update(|c| c.pan(dx, dy, FRAME));
            }
            n if n >= 2 => {
                let dist = |p: &[(i32, f64, f64)]| (p[0].1 - p[1].1).hypot(p[0].2 - p[1].2);
                let (d0, d1) = (dist(&before), dist(&after));
                if d0 > 1.0 {
                    let mid = in_frame(
                        (after[0].1 + after[1].1) / 2.0,
                        (after[0].2 + after[1].2) / 2.0,
                    );
                    zoom_by(d1 / d0, Some(mid));
                }
            }
            _ => {}
        }
    };
    let up = move |ev: web_sys::PointerEvent| {
        let id = ev.pointer_id();
        pointers.update_value(|p| p.retain(|q| q.0 != id));
    };
    let wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let at = in_frame(ev.client_x() as f64, ev.client_y() as f64);
        zoom_by((-ev.delta_y() * 0.0015).exp(), Some(at));
    };
    let done = on_done.clone();
    let save = move || {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let k = crop.get_untracked();
        let done = done.clone();
        spawn_local(async move {
            // Under the server's limit: a lower quality when the first try is too big.
            let mut out = None;
            for q in [0.88, 0.75, 0.6] {
                match encode(bitmap, &k, q).await {
                    Some(b) if b.len() <= AVATAR_MAX_BYTES => {
                        out = Some(b);
                        break;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            busy.try_set(false);
            match out {
                Some(b) => done(Some(b)),
                None => {
                    error.try_set(Some(tr!(
                        "This picture couldn't be prepared. Try another one."
                    )));
                }
            }
        });
    };
    let save2 = save.clone();
    let cancel = on_done.clone();
    let cancel2 = on_done;
    let on_key = move |ev: web_sys::KeyboardEvent| {
        let tag = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .map(|e| e.tag_name().to_ascii_lowercase())
            .unwrap_or_default();
        let step = 10.0;
        match ev.key().as_str() {
            "Escape" => cancel(None),
            "Enter" if tag != "button" => save(),
            // The slider has the arrows for itself.
            _ if tag == "input" => return,
            "ArrowLeft" => crop.update(|c| c.pan(-step, 0.0, FRAME)),
            "ArrowRight" => crop.update(|c| c.pan(step, 0.0, FRAME)),
            "ArrowUp" => crop.update(|c| c.pan(0.0, -step, FRAME)),
            "ArrowDown" => crop.update(|c| c.pan(0.0, step, FRAME)),
            "+" | "=" => zoom_by(1.1, None),
            "-" | "_" => zoom_by(1.0 / 1.1, None),
            _ => return,
        }
        ev.prevent_default();
    };
    view! {
        <div class="dialog-back" on:keydown=on_key>
            <div class="dialog cropper" role="dialog" aria-modal="true" aria-labelledby="crop-title" tabindex="-1" node_ref=dialog>
                <h2 id="crop-title">{tr!("Move and zoom")}</h2>
                <div class="crop-stage" node_ref=stage style=format!("width:{STAGE}px;height:{STAGE}px")
                    on:pointerdown=down on:pointermove=moved on:pointerup=up on:pointercancel=up
                    on:wheel=wheel>
                    <canvas node_ref=canvas />
                    <div class="crop-ring" style=format!("--r:{}px", FRAME / 2.0) />
                </div>
                {(max_zoom > 1.0).then(|| view! {
                    <input class="crop-zoom" type="range" min="1" max=max_zoom.to_string() step="0.01" aria-label=tr!("Zoom")
                        prop:value=move || crop.get().zoom.to_string()
                        on:input=move |ev| {
                            if let Ok(z) = event_target_value(&ev).parse::<f64>() {
                                crop.update(|c| c.zoom_at(z, FRAME / 2.0, FRAME / 2.0, FRAME));
                            }
                        } />
                })}
                <p class="muted small">{tr!("Drag to move it, scroll or pinch to zoom.")}</p>
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                <div class="actions">
                    <button on:click=move |_| cancel2(None)>{tr!("Cancel")}</button>
                    <button class="primary" disabled=move || busy.get() on:click=move |_| save2()>
                        {move || if busy.get() { tr!("Saving…") } else { tr!("Save") }}

                    </button>
                </div>
            </div>
        </div>
    }
}

/// "Upload a picture": the file picker, then the cropper; `on_bytes` gets the JPEG.
#[component]
pub fn PictureUpload(
    on_bytes: impl Fn(Vec<u8>) + Clone + Send + Sync + 'static,
    #[prop(into, optional)] disabled: Signal<bool>,
) -> impl IntoView {
    let input = NodeRef::<leptos::html::Input>::new();
    let bitmap: Bitmap = StoredValue::new_local(None);
    // The picture's size while the cropper is open.
    let cropping = RwSignal::new(None::<(f64, f64)>);
    let error = RwSignal::new(None::<String>);
    let changed = move |_| {
        let Some(el) = input.get_untracked() else {
            return;
        };
        let file = el.files().and_then(|f| f.get(0));
        // The same file can be picked again.
        el.set_value("");
        let Some(file) = file else { return };
        error.set(None);
        spawn_local(async move {
            match read_picture(&file).await {
                Some(b) => {
                    let size = (b.width() as f64, b.height() as f64);
                    let _ = bitmap.try_update_value(|x| *x = Some(b));
                    cropping.try_set(Some(size));
                }
                None => {
                    error.try_set(Some(tr!(
                        "This picture can't be read. Try a JPEG or PNG file."
                    )));
                }
            }
        });
    };
    view! {
        <input type="file" accept="image/*" class="hidden-file" tabindex="-1" aria-hidden="true" node_ref=input on:change=changed />
        <button type="button" disabled=move || disabled.get()
            on:click=move |_| if let Some(i) = input.get_untracked() { i.click() }>{tr!("Upload a picture")}</button>
        {move || error.get().map(|e| view! { <span class="error small">{e}</span> })}
        {move || cropping.get().map(|(w, h)| {
            let on_bytes = on_bytes.clone();
            view! {
                <Cropper bitmap=bitmap w=w h=h on_done={move |b: Option<Vec<u8>>| {
                    cropping.set(None);
                    if let Some(old) = bitmap.try_update_value(Option::take).flatten() {
                        old.close();
                    }
                    if let Some(b) = b {
                        on_bytes(b);
                    }
                }} />
            }
        })}
    }
}

// ---------------------------------------------------------------------------------------------
// Settings and welcome
// ---------------------------------------------------------------------------------------------

/// Profile calls in plain words.
fn plain_profile(e: &Error) -> String {
    match e {
        Error::Status { status: 413 } => tr!("This picture is too big for the server."),
        Error::Status { status: 400 } => tr!("The server refused this name or picture."),
        other => state::plain_keep(other),
    }
}

/// The name's problem, as the server would say it (None: fine). The core's message is English:
/// its three cases are told apart here and said in the selected language.
pub fn name_problem(name: &str) -> Option<String> {
    kp::clean_display_name(name).err()?;
    Some(if name.chars().any(char::is_control) {
        tr!("The name can't contain line breaks or control characters.")
    } else if name.trim().is_empty() {
        tr!("The name can't be empty.")
    } else {
        tr!(
            "The name is too long ({max} characters at most).",
            max = kp::DISPLAY_NAME_MAX
        )
    })
}

/// Runs a profile call; the answer (my updated profile) is redrawn everywhere.
fn save_me<F, Fut>(
    busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
    call: F,
    then: impl FnOnce(&Me) + 'static,
) where
    F: FnOnce(KeepClient) -> Fut + 'static,
    Fut: Future<Output = Result<Me, Error>> + 'static,
{
    let c = state::ctx();
    let Some(k) = c.keep_client() else {
        message.set(Some(no_keep()));
        return;
    };
    busy.set(true);
    message.set(None);
    spawn_local(async move {
        match call(k).await {
            Ok(me) => {
                then(&me);
                c.set_me(me);
            }
            Err(e) => {
                message.try_set(Some(plain_profile(&e)));
            }
        }
        busy.try_set(false);
    });
}

/// Which avatar a grid shows as chosen: "default", "picture" or a preset id.
fn avatar_key(a: &AvatarSpec) -> String {
    match (&a.custom_version, &a.preset) {
        (Some(_), _) => "picture".into(),
        (None, Some(p)) => p.clone(),
        (None, None) => "default".into(),
    }
}

/// The built-in avatars to pick from (and the default one, when `default_tile`).
#[component]
fn PresetGrid(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] selected: Signal<String>,
    on_pick: impl Fn(Option<String>) + Clone + Send + Sync + 'static,
    #[prop(optional)] default_tile: bool,
    #[prop(into, optional)] disabled: Signal<bool>,
) -> impl IntoView {
    let presets = kp::avatar_presets();
    let pick_default = on_pick.clone();
    view! {
        <div class="avatar-grid" role="group" aria-label=tr!("Built-in avatars")>
            {default_tile.then(|| view! {
                <button type="button" class="avatar-tile" class:selected=move || selected.get() == "default"
                    aria-pressed=move || (selected.get() == "default").to_string() aria-label=tr!("Default: your initial")
                    disabled=move || disabled.get() on:click=move |_| pick_default(None)>
                    <span class="avatar" style="width:44px;height:44px" inner_html=move || avatar_svg(&name.get(), None) />
                </button>
            })}
            {presets.into_iter().map(|p| {
                let id = p.id.clone();
                let (i1, i2, i3) = (id.clone(), id.clone(), id.clone());
                let pick = on_pick.clone();
                view! {
                    <button type="button" class="avatar-tile" class:selected=move || selected.get() == i1
                        aria-pressed=move || (selected.get() == i2).to_string() aria-label=tr!("Avatar: {shape}", shape = p.shape)
                        disabled=move || disabled.get() on:click=move |_| pick(Some(i3.clone()))>
                        <span class="avatar" style="width:44px;height:44px" inner_html=avatar_svg("", Some(&id)) />
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

/// Settings → Account → How others see you: my picture and display name, and changing them.
#[component]
pub fn AccountProfile() -> impl IntoView {
    let c = state::ctx();
    let me = c.me;
    let editing = RwSignal::new(false);
    let changing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let message = RwSignal::new(None::<String>);
    let problem = move || name_problem(&draft.get());
    let save_name = move || {
        let Ok(name) = kp::clean_display_name(&draft.get_untracked()) else {
            return;
        };
        save_me(
            busy,
            message,
            move |k| async move { k.set_display_name(&name).await },
            move |_| editing.set(false),
        );
    };
    let pick = move |id: Option<String>| match id {
        Some(id) => save_me(
            busy,
            message,
            move |k| async move { k.set_avatar_preset(&id).await },
            |_| {},
        ),
        None => save_me(
            busy,
            message,
            |k| async move { k.remove_avatar().await },
            |_| {},
        ),
    };
    let upload = move |bytes: Vec<u8>| {
        let preview = blob_url(&bytes, "image/jpeg");
        save_me(
            busy,
            message,
            move |k| async move { k.upload_avatar(bytes, "image/jpeg").await },
            move |m| {
                if let (Some(v), Some(url)) = (m.avatar.custom_version, preview) {
                    seed_picture(&m.user_key, v, url);
                }
            },
        );
    };
    let selected = Signal::derive(move || {
        me.with(|m| {
            m.as_ref()
                .map(|m| avatar_key(&m.avatar))
                .unwrap_or_default()
        })
    });
    let name =
        Signal::derive(move || me.with(|m| m.as_ref().map(|m| m.name.clone()).unwrap_or_default()));
    view! {
        {move || me.get().map(|m| {
            let media = (m.name_is_custom || m.media_name != m.name) && !m.media_name.is_empty();
            let reset_to = m.media_name.clone();
            view! {
                <div class="profile-head">
                    <Avatar user_key=m.user_key.clone() name=m.name.clone() avatar=m.avatar.clone() size=64 />
                    <div>
                        <strong class="profile-name">{m.name.clone()}</strong>
                        {media.then(|| view! { <div class="muted small">{tr!("{name} on the media server", name = m.media_name)}</div> })}
                        {m.is_admin.then(|| view! { <div class="muted small">{tr!("Admin")}</div> })}
                    </div>
                </div>
                <div class="actions tight">
                    <button class="small" on:click={
                        let n = m.name.clone();
                        move |_| { draft.set(n.clone()); editing.update(|e| *e = !*e); }
                    }>{tr!("Change name")}</button>
                    {m.name_is_custom.then(|| view! {
                        <button class="small" disabled=move || busy.get() on:click=move |_| {
                            save_me(busy, message, |k| async move { k.reset_display_name().await }, move |_| editing.set(false));
                        }>{tr!("Reset to {name}", name = reset_to)}</button>
                    })}
                    <button class="small" on:click=move |_| changing.update(|x| *x = !*x)>{tr!("Change picture")}</button>
                </div>
            }
        })}
        {move || editing.get().then(|| {
            view! {
                <form class="name-form" on:submit=move |ev| { ev.prevent_default(); save_name(); }>
                    <input aria-label=tr!("Display name") maxlength="64" prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev)) />
                    <button class="primary small" type="submit" disabled=move || busy.get() || problem().is_some()>{tr!("Save")}</button>
                    <button class="small subtle" type="button" on:click=move |_| editing.set(false)>{tr!("Cancel")}</button>
                </form>
                {move || problem().map(|p| view! { <p class="error small">{p}</p> })}
            }
        })}
        {move || changing.get().then(|| {
            let has_avatar = move || selected.get() != "default";
            view! {
                <PresetGrid name=name selected=selected on_pick=pick disabled=busy />
                <div class="actions tight">
                    <PictureUpload on_bytes=upload disabled=busy />
                    {move || has_avatar().then(|| {
                        view! { <button class="subtle" disabled=move || busy.get() on:click=move |_| pick(None)>{tr!("Remove")}</button> }
                    })}
                </div>
            }
        })}
        {move || message.get().map(|m| view! { <ErrorText text=m /> })}
    }
}

/// An avatar chosen in the welcome step, not saved yet.
#[derive(Clone, PartialEq)]
enum Pick {
    Default,
    Preset(String),
    /// JPEG bytes, and a blob URL to show them.
    Picture(Vec<u8>, String),
}

/// Once per account: "This is how others see you", name and avatar. Continue saves what
/// changed, Skip saves nothing; both end the step (never blocking: a failed call just brings it
/// back at the next start).
#[component]
pub fn Welcome() -> impl IntoView {
    let c = state::ctx();
    let Some(me) = c.me.get_untracked() else {
        return ().into_any();
    };
    let name = RwSignal::new(me.name.clone());
    let pick = RwSignal::new(None::<Pick>);
    let current = avatar_key(&me.avatar);
    let selected = Signal::derive(move || {
        pick.with(|p| match p {
            None => current.clone(),
            Some(Pick::Default) => "default".into(),
            Some(Pick::Preset(id)) => id.clone(),
            Some(Pick::Picture(..)) => "picture".into(),
        })
    });
    let problem = move || name_problem(&name.get());
    let dialog = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        if let Some(d) = dialog.get() {
            let _ = d.focus();
        }
    });
    let close = move || state::ctx().welcome.set(false);
    let me2 = me.clone();
    let finish = move |save: bool| {
        let Some(k) = state::ctx().keep_client() else {
            return close();
        };
        let me = me2.clone();
        let new_name = kp::clean_display_name(&name.get_untracked())
            .ok()
            .filter(|n| save && *n != me.name);
        let choice = pick.get_untracked().filter(|_| save);
        close();
        spawn_local(async move {
            let c = state::ctx();
            let mut last = None;
            if let Some(n) = new_name
                && let Ok(m) = k.set_display_name(&n).await
            {
                last = Some(m);
            }
            let res = match choice {
                Some(Pick::Default) if avatar_key(&me.avatar) != "default" => {
                    Some(k.remove_avatar().await)
                }
                Some(Pick::Preset(id)) => Some(k.set_avatar_preset(&id).await),
                Some(Pick::Picture(bytes, url)) => {
                    let r = k.upload_avatar(bytes, "image/jpeg").await;
                    match &r {
                        Ok(m) if m.avatar.custom_version.is_some() => {
                            seed_picture(&m.user_key, m.avatar.custom_version.unwrap_or(0), url)
                        }
                        _ => {
                            let _ = web_sys::Url::revoke_object_url(&url);
                        }
                    }
                    Some(r)
                }
                _ => None,
            };
            if let Some(Ok(m)) = res {
                last = Some(m);
            }
            if let Ok(m) = k.mark_profile_set().await {
                last = Some(m);
            }
            if let Some(m) = last {
                c.set_me(m);
            }
        });
    };
    let finish2 = finish.clone();
    let user_key = me.user_key.clone();
    let avatar = me.avatar.clone();
    let on_pick = move |id: Option<String>| {
        pick.update(|p| {
            if let Some(Pick::Picture(_, url)) = p {
                let _ = web_sys::Url::revoke_object_url(url);
            }
            *p = Some(match id {
                Some(id) => Pick::Preset(id),
                None => Pick::Default,
            });
        })
    };
    let on_bytes = move |bytes: Vec<u8>| {
        let Some(url) = blob_url(&bytes, "image/jpeg") else {
            return;
        };
        pick.update(|p| {
            if let Some(Pick::Picture(_, old)) = p {
                let _ = web_sys::Url::revoke_object_url(old);
            }
            *p = Some(Pick::Picture(bytes, url));
        });
    };
    view! {
        <div class="dialog-back">
            <div class="dialog welcome" role="dialog" aria-modal="true" aria-labelledby="welcome-title" tabindex="-1" node_ref=dialog>
                <h2 id="welcome-title">{tr!("Welcome! This is how others see you")}</h2>
                <p class="muted small">{tr!("In watch together rooms and invitations. You can change it later in Settings.")}</p>
                <div class="profile-head">
                    {move || match pick.get() {
                        Some(Pick::Picture(_, url)) => view! { <img class="avatar" src=url alt="" style="width:64px;height:64px" /> }.into_any(),
                        Some(Pick::Preset(id)) => view! { <span class="avatar" style="width:64px;height:64px" inner_html=avatar_svg("", Some(&id)) /> }.into_any(),
                        Some(Pick::Default) => view! { <span class="avatar" style="width:64px;height:64px" inner_html=avatar_svg(&name.get(), None) /> }.into_any(),
                        None if avatar.custom_version.is_some() => view! {
                            <Avatar user_key=user_key.clone() name=name.get() avatar=avatar.clone() size=64 />
                        }.into_any(),
                        None => view! { <span class="avatar" style="width:64px;height:64px" inner_html=avatar_svg(&name.get(), avatar.preset.as_deref()) /> }.into_any(),
                    }}
                    <label class="welcome-name">{tr!("Your name")}
                        <input maxlength="64" prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev)) />
                    </label>
                </div>
                {move || problem().map(|p| view! { <p class="error small">{p}</p> })}
                <PresetGrid name=name selected=selected on_pick=on_pick default_tile=true />
                <div class="actions tight">
                    <PictureUpload on_bytes=on_bytes />
                </div>
                <div class="actions">
                    <button class="subtle" on:click=move |_| finish2(false)>{tr!("Skip")}</button>
                    <button class="primary" disabled=move || problem().is_some() on:click=move |_| finish(true)>{tr!("Continue")}</button>
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// A person's name, with their media-server name when it differs (names are not unique).
pub fn name_with_media(name: &str, media_name: &str) -> String {
    if media_name.is_empty() || media_name == name {
        name.to_owned()
    } else {
        format!("{name} ({media_name})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_starts_centred_and_covers_the_frame() {
        let c = Crop::new(1200.0, 800.0);
        assert_eq!((c.x, c.y, c.side()), (200.0, 0.0, 800.0));
        let mut m = c;
        // Dragging the picture right shows more of its left side, up to its edge.
        m.pan(60.0, 0.0, 240.0);
        assert_eq!(m.x, 0.0);
        m.pan(-10_000.0, -10_000.0, 240.0);
        assert_eq!((m.x, m.y), (400.0, 0.0));
    }

    #[test]
    fn zoom_keeps_the_anchor_and_stays_inside() {
        let mut c = Crop::new(1000.0, 1000.0);
        c.zoom_at(2.0, 120.0, 120.0, 240.0);
        assert_eq!((c.side(), c.x, c.y), (500.0, 250.0, 250.0));
        // Zooming at a corner keeps that corner of the picture under it.
        let mut c = Crop::new(1000.0, 1000.0);
        c.zoom_at(4.0, 0.0, 0.0, 240.0);
        assert_eq!((c.x, c.y), (0.0, 0.0));
        // Never zoomed out past the frame, nor in past 64 source pixels or 8×.
        c.zoom_at(0.1, 120.0, 120.0, 240.0);
        assert_eq!(c.zoom, 1.0);
        c.zoom_at(100.0, 120.0, 120.0, 240.0);
        assert_eq!(c.zoom, 8.0);
        let mut small = Crop::new(100.0, 150.0);
        small.zoom_at(3.0, 0.0, 0.0, 240.0);
        assert_eq!(small.zoom, 100.0 / 64.0);
    }

    #[test]
    fn output_is_never_upscaled() {
        assert_eq!(Crop::new(4000.0, 3000.0).output_side(), 512);
        assert_eq!(Crop::new(300.0, 200.0).output_side(), 200);
        let mut c = Crop::new(2000.0, 2000.0);
        c.zoom_at(8.0, 0.0, 0.0, 240.0);
        assert_eq!(c.output_side(), 250);
    }

    #[test]
    fn every_preset_draws() {
        for p in kp::avatar_presets() {
            let svg = avatar_svg("Ann", Some(&p.id));
            assert!(svg.contains(&p.background), "{}", p.id);
            assert!(!svg.contains("<text"), "{}", p.id);
        }
        // Unknown ids (newer server) and none: the initial on its colour.
        let d = avatar_svg("élodie", Some("p99"));
        assert!(d.contains(">É</text>"));
        assert!(d.contains(&kp::default_avatar_color("élodie")));
        assert!(avatar_svg("<b", None).contains(">B</text>"));
        assert!(avatar_svg("&", None).contains(">?</text>"));
    }

    #[test]
    fn shapes_follow_the_table() {
        let star = shape_markup("star", "#FFFFFF").unwrap();
        assert!(star.contains("0 -1 "), "first point straight up: {star}");
        let hex = around(6, &[1.0]);
        assert_eq!(hex.len(), 6);
        assert!((hex[0].0).abs() < 1e-9 && (hex[0].1 + 1.0).abs() < 1e-9);
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(0.95), "0.95");
        assert!(shape_markup("blob", "#FFFFFF").is_none());
    }

    #[test]
    fn names_read_plainly() {
        assert_eq!(name_problem("Ana"), None);
        assert_eq!(
            name_problem("  ").as_deref(),
            Some("The name can't be empty.")
        );
        assert!(name_problem(&"x".repeat(33)).unwrap().contains("32"));
        assert_eq!(name_with_media("Ana", "ana.p"), "Ana (ana.p)");
        assert_eq!(name_with_media("Ana", "Ana"), "Ana");
        assert_eq!(name_with_media("Ana", ""), "Ana");
        assert_eq!(
            avatar_key(&AvatarSpec {
                preset: Some("p03".into()),
                custom_version: None
            }),
            "p03"
        );
    }
}
