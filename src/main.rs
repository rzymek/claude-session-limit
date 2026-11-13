mod render;
mod usage;

use chrono::{Local, TimeDelta};
use render::{Canvas, Font};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use usage::Usage;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::x11::{WindowAttributesExtX11, WindowType};
use winit::window::{Window, WindowId, WindowLevel};

const POLL_INTERVAL: Duration = Duration::from_secs(60);
const SESSION_LEN_SECS: f64 = 5.0 * 3600.0;

// Layout in logical pixels.
const WIN_W: f64 = 290.0;
const WIN_H: f64 = 56.0;
const FONT_SIZE: f32 = 12.0;

const BG: u32 = 0x1e1e1e;
const BORDER: u32 = 0x444444;
const TRACK: u32 = 0x3a3a3a;
const TEXT: u32 = 0xdddddd;
const DIM: u32 = 0x999999;
const BLUE: u32 = 0x5b8def;
const GREEN: u32 = 0x4caf50;
const AMBER: u32 = 0xffb300;
const RED: u32 = 0xe53935;

type FetchResult = Result<Usage, String>;

struct App {
    font: Font,
    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    usage: Option<Usage>,
    error: Option<String>,
    refresh_tx: mpsc::Sender<()>,
    next_tick: Instant,
    position: Option<PhysicalPosition<i32>>,
    position_dirty: bool,
    refreshed_after_reset: bool,
}

fn pos_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("claude-session-limit").join("pos"))
}

fn load_position() -> Option<PhysicalPosition<i32>> {
    let s = std::fs::read_to_string(pos_file()?).ok()?;
    let mut it = s.split_whitespace().map(|v| v.parse::<i32>().ok());
    Some(PhysicalPosition::new(it.next()??, it.next()??))
}

fn save_position(p: PhysicalPosition<i32>) {
    if let Some(f) = pos_file() {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, format!("{} {}\n", p.x, p.y));
    }
}

fn usage_color(pct: f64) -> u32 {
    if pct >= 90.0 {
        RED
    } else if pct >= 70.0 {
        AMBER
    } else {
        GREEN
    }
}

impl App {
    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else {
            return;
        };
        let s = window.scale_factor();
        let px = |v: f64| (v * s).round() as i32;
        let fsize = FONT_SIZE * s as f32;
        let font = &self.font;

        let mut c = Canvas { buf: &mut buf, w: w.get() as usize, h: h.get() as usize };
        c.clear(BG);
        c.border(BORDER);

        let label_x = px(10.0);
        let bar_x = px(64.0);
        let bar_w = px(120.0);
        let bar_h = px(12.0);
        let value_x = bar_x + bar_w + px(8.0);
        let row_y = [px(12.0), px(32.0)];
        let baseline = |y: i32| y + bar_h - px(2.0);

        match (&self.usage, &self.error) {
            (None, Some(err)) => {
                c.text(font, fsize, label_x, px(WIN_H / 2.0 + 4.0), err, TEXT);
            }
            (None, None) => {
                c.text(font, fsize, label_x, px(WIN_H / 2.0 + 4.0), "Loading…", DIM);
            }
            (Some(u), err) => {
                // Row 1: session usage.
                c.text(font, fsize, label_x, baseline(row_y[0]), "Session", DIM);
                let pct = u.utilization.clamp(0.0, 100.0);
                c.bar(bar_x, row_y[0], bar_w, bar_h, pct / 100.0, usage_color(pct), TRACK);
                c.text(font, fsize, value_x, baseline(row_y[0]), &format!("{:.0}%", u.utilization), TEXT);

                // Row 2: time until reset.
                c.text(font, fsize, label_x, baseline(row_y[1]), "Reset", DIM);
                match u.resets_at {
                    Some(reset) => {
                        let remaining = (reset - Local::now()).max(TimeDelta::zero());
                        let secs = remaining.num_seconds() as f64;
                        let elapsed = 1.0 - secs / SESSION_LEN_SECS;
                        c.bar(bar_x, row_y[1], bar_w, bar_h, elapsed, BLUE, TRACK);
                        let mins = remaining.num_minutes();
                        let txt = format!("{}:{:02}", mins / 60, mins % 60);
                        let adv = c.text(font, fsize, value_x, baseline(row_y[1]), &txt, TEXT);
                        let at = format!(" @ {}", reset.format("%H:%M"));
                        c.text(font, fsize, value_x + adv, baseline(row_y[1]), &at, DIM);
                    }
                    None => {
                        c.bar(bar_x, row_y[1], bar_w, bar_h, 0.0, BLUE, TRACK);
                        c.text(font, fsize, value_x, baseline(row_y[1]), "no session", DIM);
                    }
                }

                // Stale-data indicator: the last refresh failed.
                if err.is_some() {
                    let d = px(5.0);
                    c.rect(c.w as i32 - d - px(3.0), px(3.0), d, d, RED);
                }
            }
        }
        let _ = buf.present();
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(p) = self.position {
            save_position(p);
        }
        event_loop.exit();
    }
}

impl ApplicationHandler<FetchResult> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("Claude session")
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size(LogicalSize::new(WIN_W, WIN_H))
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_name("claude-session-limit", "claude-session-limit")
            .with_x11_window_type(vec![WindowType::Utility]);
        if let Some(p) = load_position() {
            attrs = attrs.with_position(p);
            self.position = Some(p);
        }
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));
        let context = Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(Surface::new(&context, window.clone()).expect("softbuffer surface"));
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::Moved(p) => {
                self.position = Some(p);
                self.position_dirty = true;
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => match button {
                MouseButton::Left => {
                    if let Some(w) = &self.window {
                        let _ = w.drag_window();
                    }
                }
                MouseButton::Right => {
                    let _ = self.refresh_tx.send(());
                }
                MouseButton::Middle => self.quit(event_loop),
                _ => {}
            },
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, .. },
                ..
            } => match logical_key.as_ref() {
                Key::Named(NamedKey::Escape) | Key::Character("q") => self.quit(event_loop),
                Key::Character("r") | Key::Named(NamedKey::F5) => {
                    let _ = self.refresh_tx.send(());
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, result: FetchResult) {
        match result {
            Ok(u) => {
                self.refreshed_after_reset = false;
                self.usage = Some(u);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.next_tick = now + Duration::from_secs(1);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            if self.position_dirty {
                self.position_dirty = false;
                if let Some(p) = self.position {
                    save_position(p);
                }
            }
            // Fetch fresh data shortly after the window resets.
            if let Some(reset) = self.usage.as_ref().and_then(|u| u.resets_at) {
                if !self.refreshed_after_reset && Local::now() > reset + TimeDelta::seconds(5) {
                    self.refreshed_after_reset = true;
                    let _ = self.refresh_tx.send(());
                }
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
    }
}

fn main() {
    let event_loop = EventLoop::<FetchResult>::with_user_event().build().expect("event loop");
    let proxy = event_loop.create_proxy();
    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();

    std::thread::spawn(move || loop {
        if proxy.send_event(usage::fetch()).is_err() {
            break; // event loop gone
        }
        match refresh_rx.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {
                while refresh_rx.try_recv().is_ok() {} // coalesce repeated clicks
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    });

    let mut app = App {
        font: render::load_font(),
        window: None,
        surface: None,
        usage: None,
        error: None,
        refresh_tx,
        next_tick: Instant::now(),
        position: None,
        position_dirty: false,
        refreshed_after_reset: false,
    };
    event_loop.run_app(&mut app).expect("run app");
}
