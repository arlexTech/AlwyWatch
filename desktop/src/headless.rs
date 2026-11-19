// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless mode for CI and benchmarks: renders the real UI with Slint's software renderer into
//! a buffer, runs the event loop (async tasks, timers), saves a PNG after a delay and exits.
//!
//! `ALWYWATCH_SCREENSHOT=out.png [ALWYWATCH_SCREENSHOT_AFTER_MS=3000] alwywatch`

use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::{PhysicalSize, PlatformError};

type Task = Box<dyn FnOnce() + Send>;

enum Msg {
    Run(Task),
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
    size: (u32, u32),
}

impl Headless {
    pub fn install(out: String, after: Duration) {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let size = (1280, 800);
        window.set_size(PhysicalSize::new(size.0, size.1));
        let (tx, rx) = mpsc::channel();
        slint::platform::set_platform(Box::new(Headless {
            window,
            start: Instant::now(),
            tx,
            rx,
            out,
            after,
            size,
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
        let (w, h) = self.size;
        let mut buf = vec![Rgb565Pixel::default(); (w * h) as usize];
        let deadline = Instant::now() + self.after;
        let mut first = true;
        loop {
            slint::platform::update_timers_and_animations();
            while let Ok(m) = self.rx.try_recv() {
                match m {
                    Msg::Run(f) => f(),
                    Msg::Quit => return Ok(()),
                }
            }
            self.window.draw_if_needed(|r| {
                r.render(&mut buf, w as usize);
            });
            if first {
                first = false;
                eprintln!("first frame after {} ms", self.start.elapsed().as_millis());
            }
            if Instant::now() >= deadline {
                // Final render of the whole frame, then save as PNG.
                self.window.request_redraw();
                self.window.draw_if_needed(|r| {
                    r.render(&mut buf, w as usize);
                });
                let rgb: Vec<u8> = buf
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
                image::save_buffer(&self.out, &rgb, w, h, image::ExtendedColorType::Rgb8)
                    .map_err(|e| PlatformError::Other(e.to_string()))?;
                eprintln!("saved {}", self.out);
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}
