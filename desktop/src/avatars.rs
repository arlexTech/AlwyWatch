// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch avatars, the parts that need no UI: the built-in avatars drawn into pixels (exact
//! geometry of docs/keep-protocol.md § Built-in avatars, so every renderer shows the same), the
//! cropper's maths, and turning the chosen square of a picture into the JPEG that is uploaded.

use i18n::tr;
use image::{DynamicImage, ImageDecoder, ImageReader};

/// Pixels of a drawn built-in avatar (shown at 96 px at most, so crisp on 2× screens too… at
/// the sizes used; Slint scales it down elsewhere).
pub const PRESET_PX: u32 = 128;
/// Largest side of an uploaded picture (never upscaled).
pub const UPLOAD_MAX_SIDE: u32 = 512;
/// The server refuses more (413).
pub const UPLOAD_MAX_BYTES: usize = 512 * 1024;
/// JPEG quality of the upload (~0.88), and the retry when the result is still too big.
pub const JPEG_QUALITY: u8 = 88;
const JPEG_RETRY_QUALITY: u8 = 72;
/// Pictures larger than this are refused before decoding (a photo is a few MB).
pub const PICK_MAX_BYTES: u64 = 40 * 1024 * 1024;
/// Side of the picture shown in the cropper (the upload is rendered from the file itself).
pub const PREVIEW_MAX_SIDE: u32 = 1024;

/// `#RRGGBB` → (r, g, b); anything else is grey.
pub fn parse_hex(c: &str) -> (u8, u8, u8) {
    let h = c.trim_start_matches('#');
    let v = (h.len() == 6)
        .then(|| u32::from_str_radix(h, 16).ok())
        .flatten()
        .unwrap_or(0x80_80_80);
    ((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn in_polygon(x: f32, y: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Regular polygon / star points: `radii` repeat around, first point straight up.
fn ring_points(count: usize, radii: &[f32]) -> Vec<(f32, f32)> {
    (0..count)
        .map(|k| {
            let a = (-90.0 + k as f32 * 360.0 / count as f32).to_radians();
            let r = radii[k % radii.len()];
            (r * a.cos(), r * a.sin())
        })
        .collect()
}

fn plus(x: f32, y: f32) -> bool {
    (x.abs() <= 1.0 && y.abs() <= 0.3) || (x.abs() <= 0.3 && y.abs() <= 1.0)
}

/// Whether the point (unit box [-1, 1]², y down) is inside the shape.
pub fn inside_shape(shape: &str, x: f32, y: f32) -> bool {
    match shape {
        "circle" => x * x + y * y <= 1.0,
        "ring" => ((x * x + y * y).sqrt() - 0.75).abs() <= 0.25,
        "square" => x.abs() <= 0.85 && y.abs() <= 0.85,
        "diamond" => x.abs() + y.abs() <= 1.0,
        "triangle" => in_polygon(x, y, &[(0.0, -1.0), (0.95, 0.7), (-0.95, 0.7)]),
        "plus" => plus(x, y),
        "cross" => {
            let s = std::f32::consts::FRAC_1_SQRT_2;
            plus((x + y) * s, (y - x) * s)
        }
        "star" => in_polygon(x, y, &ring_points(10, &[1.0, 0.45])),
        "hexagon" => in_polygon(x, y, &ring_points(6, &[1.0])),
        "bars" => {
            x.abs() <= 1.0
                && [(-0.8, -0.4), (-0.2, 0.2), (0.4, 0.8)]
                    .iter()
                    .any(|&(a, b)| y >= a && y <= b)
        }
        "half" => y <= 0.25 && x * x + (y - 0.25) * (y - 0.25) <= 1.0,
        "dots" => [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]
            .iter()
            .any(|&(cx, cy)| (x - cx) * (x - cx) + (y - cy) * (y - cy) <= 0.35 * 0.35),
        _ => false,
    }
}

/// A built-in avatar as straight-alpha RGBA, `px` × `px`: a disc of `background` with the shape
/// in `foreground` clipped to it (1 unit = 25 % of the diameter), 4×4 samples per pixel.
pub fn draw_preset(shape: &str, background: &str, foreground: &str, px: u32) -> Vec<u8> {
    const N: u32 = 4;
    let (bg, fg) = (parse_hex(background), parse_hex(foreground));
    let unit = px as f32 / 4.0;
    let c = px as f32 / 2.0;
    let mut out = vec![0u8; (px * px * 4) as usize];
    for py in 0..px {
        for pxl in 0..px {
            let (mut disc, mut fill) = (0u32, 0u32);
            for sy in 0..N {
                for sx in 0..N {
                    let x = (pxl as f32 + (sx as f32 + 0.5) / N as f32 - c) / unit;
                    let y = (py as f32 + (sy as f32 + 0.5) / N as f32 - c) / unit;
                    if x * x + y * y <= 4.0 {
                        disc += 1;
                        if inside_shape(shape, x, y) {
                            fill += 1;
                        }
                    }
                }
            }
            if disc == 0 {
                continue;
            }
            let mix =
                |b: u8, f: u8| ((u32::from(b) * (disc - fill) + u32::from(f) * fill) / disc) as u8;
            let i = ((py * px + pxl) * 4) as usize;
            out[i] = mix(bg.0, fg.0);
            out[i + 1] = mix(bg.1, fg.1);
            out[i + 2] = mix(bg.2, fg.2);
            out[i + 3] = (disc * 255 / (N * N)) as u8;
        }
    }
    out
}

// ----- cropper --------------------------------------------------------------------------------

/// The cropper's state, in source pixels of the (orientation-corrected) picture: the square
/// under the round frame is centred on (`cx`, `cy`) and `side` = shorter side / `zoom` wide. The
/// frame is always covered by the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub w: f32,
    pub h: f32,
    pub zoom: f32,
    pub cx: f32,
    pub cy: f32,
}

/// Zoom stops at 4× or when the square would be under this many source pixels.
const MIN_SIDE: f32 = 32.0;

impl Crop {
    /// Centred, zoomed out as far as the frame allows (the shorter side fills it).
    pub fn new(w: u32, h: u32) -> Self {
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        Crop {
            w,
            h,
            zoom: 1.0,
            cx: w / 2.0,
            cy: h / 2.0,
        }
    }

    pub fn max_zoom(&self) -> f32 {
        (self.w.min(self.h) / MIN_SIDE).clamp(1.0, 4.0)
    }

    /// Side of the square under the frame, in source pixels.
    pub fn side(&self) -> f32 {
        self.w.min(self.h) / self.zoom
    }

    fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, self.max_zoom());
        let half = self.side() / 2.0;
        self.cx = self.cx.clamp(half, self.w - half);
        self.cy = self.cy.clamp(half, self.h - half);
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = if zoom.is_finite() { zoom } else { 1.0 };
        self.clamp();
    }

    /// The picture dragged by (`dx`, `dy`) screen pixels with a frame `frame` pixels wide.
    pub fn drag(&mut self, dx: f32, dy: f32, frame: f32) {
        let k = frame / self.side();
        self.cx -= dx / k;
        self.cy -= dy / k;
        self.clamp();
    }

    /// Where to draw the whole picture in a square viewport of `viewport` pixels whose centred
    /// round frame is `frame` pixels wide: (x, y, width, height).
    pub fn view(&self, viewport: f32, frame: f32) -> (f32, f32, f32, f32) {
        let k = frame / self.side();
        (
            viewport / 2.0 - self.cx * k,
            viewport / 2.0 - self.cy * k,
            self.w * k,
            self.h * k,
        )
    }

    /// The square to cut, in whole source pixels and inside the picture: (x, y, side).
    pub fn source_rect(&self) -> (u32, u32, u32) {
        let (w, h) = (self.w as u32, self.h as u32);
        let side = (self.side().round() as u32).clamp(1, w.min(h));
        let x = ((self.cx - side as f32 / 2.0).round().max(0.0) as u32).min(w - side);
        let y = ((self.cy - side as f32 / 2.0).round().max(0.0) as u32).min(h - side);
        (x, y, side)
    }

    /// Side of the uploaded square: the cut square, at most 512 (never upscaled).
    pub fn output_side(&self) -> u32 {
        self.source_rect().2.min(UPLOAD_MAX_SIDE)
    }
}

/// The cropper's veil: `size` × `size` RGBA, dark outside the centred disc of diameter
/// `frame`, clear inside (anti-aliased edge).
pub fn frame_mask(size: u32, frame: f32) -> Vec<u8> {
    let (c, r) = (size as f32 / 2.0, frame / 2.0);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let a = ((d - r).clamp(0.0, 1.0) * 168.0) as u8;
            out[((y * size + x) * 4 + 3) as usize] = a;
        }
    }
    out
}

// ----- the picture ------------------------------------------------------------------------------

/// Opens a picture the way it should look (EXIF orientation applied).
pub fn open_oriented(path: &std::path::Path) -> Result<DynamicImage, String> {
    let too_big = std::fs::metadata(path)
        .map(|m| m.len() > PICK_MAX_BYTES)
        .unwrap_or(false);
    if too_big {
        return Err(tr!("This picture is too large (40 MB at most)."));
    }
    let unreadable = |_| tr!("This file isn't a picture AlwyWatch can read (JPEG, PNG or WebP).");
    let mut decoder = ImageReader::open(path)
        .map_err(|e| tr!("Can't open this file: {error}", error = e))?
        .with_guessed_format()
        .map_err(|e| tr!("Can't open this file: {error}", error = e))?
        .into_decoder()
        .map_err(unreadable)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// The cropper's picture: smaller than the file when that is big (the upload uses the file).
pub fn preview(img: &DynamicImage) -> image::RgbaImage {
    if img.width().max(img.height()) > PREVIEW_MAX_SIDE {
        img.thumbnail(PREVIEW_MAX_SIDE, PREVIEW_MAX_SIDE)
            .into_rgba8()
    } else {
        img.to_rgba8()
    }
}

/// Cuts the chosen square, scales it to `crop.output_side()` and encodes it as JPEG (quality
/// 88, lower once if still over the server's limit).
pub fn render_upload(img: &DynamicImage, crop: &Crop) -> Result<Vec<u8>, String> {
    let (x, y, side) = crop.source_rect();
    let out = crop.output_side();
    let square = img.crop_imm(x, y, side, side);
    let square = if out < side {
        square.resize_exact(out, out, image::imageops::FilterType::Lanczos3)
    } else {
        square
    };
    let rgb = square.to_rgb8();
    for quality in [JPEG_QUALITY, JPEG_RETRY_QUALITY] {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&rgb)
            .map_err(|e| tr!("Can't prepare the picture: {error}", error = e))?;
        if bytes.len() <= UPLOAD_MAX_BYTES {
            return Ok(bytes);
        }
    }
    Err(tr!("This picture stays too large once cropped."))
}

/// Decodes a custom avatar fetched from the server.
pub fn decode(bytes: &[u8]) -> Option<image::RgbaImage> {
    image::load_from_memory(bytes).ok().map(|i| i.into_rgba8())
}

/// File name of a cached custom avatar: (server, user, version) never changes its picture.
pub fn cache_name(server: &str, user_key: &str, version: u32) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in server.bytes().chain([0u8]).chain(user_key.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}-{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#C62828"), (0xc6, 0x28, 0x28));
        assert_eq!(parse_hex("#ffffff"), (255, 255, 255));
        assert_eq!(parse_hex("nope"), (0x80, 0x80, 0x80));
    }

    #[test]
    fn shapes_follow_the_table() {
        assert!(inside_shape("circle", 0.0, 0.9) && !inside_shape("circle", 0.8, 0.8));
        assert!(inside_shape("ring", 0.75, 0.0) && !inside_shape("ring", 0.0, 0.0));
        assert!(inside_shape("square", 0.84, -0.84) && !inside_shape("square", 0.9, 0.0));
        assert!(inside_shape("diamond", 0.0, 0.99) && !inside_shape("diamond", 0.6, 0.6));
        // Triangle: point up, flat side at y = 0.7.
        assert!(inside_shape("triangle", 0.0, 0.6) && !inside_shape("triangle", 0.0, 0.75));
        assert!(!inside_shape("triangle", 0.5, -0.5));
        assert!(inside_shape("plus", 0.9, 0.0) && !inside_shape("plus", 0.5, 0.5));
        // Cross: the plus turned 45°.
        assert!(inside_shape("cross", 0.6, 0.6) && !inside_shape("cross", 0.9, 0.0));
        // Star: first point straight up, inner radius 0.45.
        assert!(inside_shape("star", 0.0, -0.95) && !inside_shape("star", 0.0, 0.95));
        assert!(inside_shape("star", 0.0, 0.0));
        assert!(inside_shape("hexagon", 0.0, -0.99) && !inside_shape("hexagon", 0.95, 0.0));
        assert!(inside_shape("bars", 0.0, 0.0) && !inside_shape("bars", 0.0, 0.3));
        assert!(inside_shape("bars", -0.9, -0.6) && !inside_shape("bars", 0.0, 0.9));
        // Half: the upper half of the disc centred (0, 0.25), flat side down.
        assert!(inside_shape("half", 0.0, -0.7) && !inside_shape("half", 0.0, 0.3));
        assert!(inside_shape("dots", 0.5, 0.5) && !inside_shape("dots", 0.0, 0.0));
        assert!(!inside_shape("unknown", 0.0, 0.0));
    }

    #[test]
    fn preset_is_a_disc_with_the_shape() {
        let px = 64;
        let p = draw_preset("circle", "#C62828", "#FFFFFF", px);
        let at = |x: u32, y: u32| {
            let i = ((y * px + x) * 4) as usize;
            (p[i], p[i + 1], p[i + 2], p[i + 3])
        };
        assert_eq!(at(0, 0).3, 0, "corner outside the disc");
        assert_eq!(at(32, 32), (255, 255, 255, 255), "centre: the shape");
        assert_eq!(at(32, 4), (0xc6, 0x28, 0x28, 255), "edge: the background");
    }

    #[test]
    fn crop_starts_centred_and_covers_the_frame() {
        let c = Crop::new(4000, 3000);
        assert_eq!(c.side(), 3000.0);
        assert_eq!(c.source_rect(), (500, 0, 3000));
        assert_eq!(c.output_side(), 512);
        let (x, y, w, h) = c.view(360.0, 300.0);
        // Frame (30..330) covered vertically, picture centred.
        assert_eq!((y, h), (30.0, 300.0));
        assert!((x + w / 2.0 - 180.0).abs() < 0.01);
    }

    #[test]
    fn crop_drag_stops_at_the_edges() {
        let mut c = Crop::new(4000, 3000);
        c.drag(10_000.0, 0.0, 300.0);
        assert_eq!(c.source_rect(), (0, 0, 3000));
        c.drag(-10_000.0, -50.0, 300.0);
        assert_eq!(c.source_rect(), (1000, 0, 3000));
        // Zoomed 2×: the square is 1500 px; one screen pixel moves 5 source pixels.
        c.set_zoom(2.0);
        c.drag(-10_000.0, 0.0, 300.0);
        assert_eq!(c.source_rect(), (2500, 750, 1500));
        let before = c.cx;
        c.drag(-10.0, 0.0, 300.0);
        assert!((c.cx - before).abs() < 0.01, "already at the right edge");
        c.drag(10.0, 0.0, 300.0);
        assert!((before - c.cx - 50.0).abs() < 0.01);
    }

    #[test]
    fn crop_zoom_limits() {
        let mut c = Crop::new(800, 600);
        c.set_zoom(10.0);
        assert_eq!(c.zoom, 4.0);
        assert_eq!(c.source_rect().2, 150);
        assert_eq!(c.output_side(), 150, "never upscaled");
        c.set_zoom(0.2);
        assert_eq!(c.zoom, 1.0);
        let mut tiny = Crop::new(40, 40);
        tiny.set_zoom(3.0);
        assert_eq!(tiny.max_zoom(), 1.25);
        assert_eq!(tiny.zoom, 1.25);
        assert_eq!(tiny.source_rect(), (4, 4, 32));
    }

    #[test]
    fn upload_is_a_small_jpeg_square() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(1200, 900, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        }));
        let mut c = Crop::new(1200, 900);
        c.set_zoom(1.5);
        let bytes = render_upload(&img, &c).unwrap();
        assert_eq!(&bytes[..3], &[0xff, 0xd8, 0xff]);
        assert!(bytes.len() <= UPLOAD_MAX_BYTES);
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (512, 512));
        // A small picture stays its size.
        let small = DynamicImage::ImageRgb8(image::RgbImage::new(300, 200));
        let bytes = render_upload(&small, &Crop::new(300, 200)).unwrap();
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (200, 200));
    }

    #[test]
    fn mask_is_clear_inside_the_frame() {
        let m = frame_mask(360, 300.0);
        let alpha = |x: u32, y: u32| m[((y * 360 + x) * 4 + 3) as usize];
        assert_eq!(alpha(180, 180), 0);
        assert_eq!(alpha(180, 35), 0);
        assert_eq!(alpha(5, 5), 168);
        assert_eq!(alpha(180, 25), 168);
    }

    #[test]
    fn cache_names_differ_by_version_and_user() {
        let a = cache_name("https://w", "jellyfin:1", 1);
        assert_ne!(a, cache_name("https://w", "jellyfin:1", 2));
        assert_ne!(a, cache_name("https://w", "jellyfin:2", 1));
        assert!(a.ends_with("-1"));
    }
}
