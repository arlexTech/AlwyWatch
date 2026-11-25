// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless mode for CI and benchmarks: renders the real UI with Slint's software renderer into
//! a buffer, runs the event loop (async tasks, timers), saves a PNG after a delay and exits.
//!
//! `ALWYWATCH_SCREENSHOT=out.png [ALWYWATCH_SCREENSHOT_AFTER_MS=3000] alwywatch`
//!
//! Scripted mode (`ALWYWATCH_DRIVE=1`, `ALWYWATCH_SCREENSHOT` ignored): one command per line on
//! stdin, to drive the real UI end to end without a display:
//! `click X Y`, `rclick X Y`, `press X Y` / `release X Y` (held between), `scroll X Y DY`, `key NAME` (Tab, Return, Escape, Up, Down, Left, Right, Space,
//! Backspace or one character), `type TEXT`, `shot FILE.png`, `size W H`, `quit`.

use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use std::cell::Cell;
use std::io::BufRead;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{LogicalPosition, PhysicalSize, PlatformError, SharedString};

type Task = Box<dyn FnOnce() + Send>;

enum Msg {
    Run(Task),
    Cmd(String),
    Quit,
}

struct Proxy(mpsc::Sender<Msg>);

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        self.0
            .send(Msg::Quit)
            .map_err(|_| slint::EventLoopError::EventLoopTerminated)
    }
    fn invoke_from_event_loop(&self, event: Task) -> Result<(), slint::EventLoopError> {
        self.0
            .send(Msg::Run(event))
            .map_err(|_| slint::EventLoopError::EventLoopTerminated)
    }
}

pub struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    out: String,
    after: Duration,
    size: Cell<(u32, u32)>,
    driven: bool,
}

impl Headless {
    pub fn install(out: String, after: Duration, driven: bool) {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let size = (1280, 800);
        window.set_size(PhysicalSize::new(size.0, size.1));
        let (tx, rx) = mpsc::channel();
        if driven {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    if tx.send(Msg::Cmd(line)).is_err() {
                        break;
                    }
                }
                let _ = tx.send(Msg::Quit);
            });
        }
        slint::platform::set_platform(Box::new(Headless {
            window,
            start: Instant::now(),
            tx,
            rx,
            out,
            after,
            size: Cell::new(size),
            driven,
        }))
        .expect("platform set once");
    }
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy(self.tx.clone())))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let deadline = Instant::now() + self.after;
        let mut first = true;
        loop {
            slint::platform::update_timers_and_animations();
            while let Ok(m) = self.rx.try_recv() {
                match m {
                    Msg::Run(f) => f(),
                    Msg::Cmd(c) => {
                        if !self.command(&c)? {
                            return Ok(());
                        }
                    }
                    Msg::Quit => return Ok(()),
                }
            }
            if first {
                first = false;
                self.render();
                eprintln!("first frame after {} ms", self.start.elapsed().as_millis());
            }
            if !self.driven && Instant::now() >= deadline {
                self.save(&self.out)?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

impl Headless {
    fn render(&self) -> Vec<Rgb565Pixel> {
        let (w, h) = self.size.get();
        let mut buf = vec![Rgb565Pixel::default(); (w * h) as usize];
        self.window.request_redraw();
        self.window.draw_if_needed(|r| {
            r.render(&mut buf, w as usize);
        });
        buf
    }

    /// Renders the whole frame and saves it as PNG.
    fn save(&self, out: &str) -> Result<(), PlatformError> {
        let (w, h) = self.size.get();
        let rgb: Vec<u8> = self
            .render()
            .iter()
            .flat_map(|p| {
                let v = p.0;
                let (r, g, b) = ((v >> 11) & 0x1f, (v >> 5) & 0x3f, v & 0x1f);
                [
                    (r << 3 | r >> 2) as u8,
                    (g << 2 | g >> 4) as u8,
                    (b << 3 | b >> 2) as u8,
                ]
            })
            .collect();
        image::save_buffer(out, &rgb, w, h, image::ExtendedColorType::Rgb8)
            .map_err(|e| PlatformError::Other(e.to_string()))?;
        eprintln!("saved {out}");
        Ok(())
    }

    /// Runs one scripted command; false means quit.
    fn command(&self, line: &str) -> Result<bool, PlatformError> {
        let (cmd, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        let nums: Vec<f32> = rest
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        let w = &self.window;
        match (cmd, nums.as_slice()) {
            ("click", [x, y]) => {
                let position = LogicalPosition::new(*x, *y);
                let button = PointerEventButton::Left;
                w.dispatch_event(WindowEvent::PointerMoved { position });
                w.dispatch_event(WindowEvent::PointerPressed { position, button });
                w.dispatch_event(WindowEvent::PointerReleased { position, button });
            }
            ("rclick", [x, y]) => {
                let position = LogicalPosition::new(*x, *y);
                let button = PointerEventButton::Right;
                w.dispatch_event(WindowEvent::PointerMoved { position });
                w.dispatch_event(WindowEvent::PointerPressed { position, button });
                w.dispatch_event(WindowEvent::PointerReleased { position, button });
            }
            // A press held across lines (long press): `press X Y`, … , `release X Y`.
            ("press" | "release", [x, y]) => {
                let position = LogicalPosition::new(*x, *y);
                let button = PointerEventButton::Left;
                w.dispatch_event(WindowEvent::PointerMoved { position });
                w.dispatch_event(if cmd == "press" {
                    WindowEvent::PointerPressed { position, button }
                } else {
                    WindowEvent::PointerReleased { position, button }
                });
            }
            ("scroll", [x, y, dy]) => {
                let position = LogicalPosition::new(*x, *y);
                w.dispatch_event(WindowEvent::PointerMoved { position });
                w.dispatch_event(WindowEvent::PointerScrolled {
                    position,
                    delta_x: 0.0,
                    delta_y: *dy,
                });
            }
            ("size", [width, height]) => {
                self.size.set((*width as u32, *height as u32));
                w.set_size(PhysicalSize::new(*width as u32, *height as u32));
            }
            ("key", _) => {
                let text: SharedString = match rest {
                    "Tab" => Key::Tab.into(),
                    "Return" => Key::Return.into(),
                    "Escape" => Key::Escape.into(),
                    "Up" => Key::UpArrow.into(),
                    "Down" => Key::DownArrow.into(),
                    "Left" => Key::LeftArrow.into(),
                    "Right" => Key::RightArrow.into(),
                    "Space" => Key::Space.into(),
                    "Backspace" => Key::Backspace.into(),
                    c => c.into(),
                };
                w.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
                w.dispatch_event(WindowEvent::KeyReleased { text });
            }
            ("type", _) => {
                for c in rest.chars() {
                    let text: SharedString = c.to_string().into();
                    w.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
                    w.dispatch_event(WindowEvent::KeyReleased { text });
                }
            }
            ("shot", _) => self.save(rest)?,
            ("quit", _) => return Ok(false),
            _ => eprintln!("unknown command: {line}"),
        }
        Ok(true)
    }
}
