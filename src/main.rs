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
use usage::{Limit, Usage};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::x11::{WindowAttributesExtX11, WindowType};
use winit::window::{Window, WindowId, WindowLevel};

const POLL_INTERVAL: Duration = Duration::from_secs(3*60);
const SESSION_LEN_SECS: f64 = 5.0 * 3600.0;
/// Non-session limits at or above this percentage get their own bar.
const LIMIT_WARN_PCT: f64 = 75.0;
/// Hover time before the tooltip appears.
const TIP_DELAY: Duration = Duration::from_millis(400);

// Layout in logical pixels.
const WIN_W: f64 = 112.0;
const ROW_H: f64 = 14.0;
const PAD: f64 = 3.0;
const GAP: f64 = 2.0;
const FONT_SIZE: f32 = 11.0;
const TIP_FONT_SIZE: f32 = 12.0;
const TIP_PAD: f64 = 6.0;
const TIP_COL_GAP: f64 = 10.0;

const BG: u32 = 0x1e1e1e;
const BORDER: u32 = 0x444444;
const TRACK: u32 = 0x3a3a3a;
const TEXT: u32 = 0xeeeeee;
const TEXT_DIM: u32 = 0x999999;
const TEXT_ON_FILL: u32 = 0x111111;
const BLUE: u32 = 0x5b8def;
const GREEN: u32 = 0x4caf50;
const AMBER: u32 = 0xffb300;
const RED: u32 = 0xe53935;

type FetchResult = Result<Usage, String>;

/// Hover tooltip: a separate override-redirect window, created on demand.
struct Tip {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
}

enum TipLine {
    Limit { name: String, pct: String, color: u32, reset: String },
    Note(String, u32),
}

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
    /// When the cursor entered the main window (tooltip pending/shown).
    hover_since: Option<Instant>,
    tip: Option<Tip>,
}

fn win_height(rows: usize) -> f64 {
    2.0 * PAD + rows as f64 * ROW_H + rows.saturating_sub(1) as f64 * GAP
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

/// Non-session limits high enough to deserve their own bar.
fn warn_limits(usage: &Option<Usage>) -> Vec<&Limit> {
    usage.iter().flat_map(|u| &u.others).filter(|l| l.utilization >= LIMIT_WARN_PCT).collect()
}

/// `2:13` below a day, `2d 3h` above.
fn fmt_remaining(d: TimeDelta) -> String {
    let mins = d.max(TimeDelta::zero()).num_minutes();
    if mins >= 24 * 60 {
        format!("{}d {}h", mins / (24 * 60), mins / 60 % 24)
    } else {
        format!("{}:{:02}", mins / 60, mins % 60)
    }
}

impl App {
    fn rows(&self) -> usize {
        2 + warn_limits(&self.usage).len()
    }

    fn tip_lines(&self) -> Vec<TipLine> {
        let mut lines = Vec::new();
        let now = Local::now();
        if let Some(u) = &self.usage {
            for l in std::iter::once(&u.session).chain(&u.others) {
                let reset = match l.resets_at {
                    Some(r) => {
                        let r30 = r + TimeDelta::seconds(30);
                        let fmt = if r30.date_naive() == now.date_naive() { "%H:%M" } else { "%a %H:%M" };
                        format!("resets {} (in {})", r30.format(fmt), fmt_remaining(r - now))
                    }
                    None => "—".into(),
                };
                lines.push(TipLine::Limit {
                    name: l.label(),
                    pct: format!("{:.0}%", l.utilization),
                    color: usage_color(l.utilization),
                    reset,
                });
            }
            lines.push(TipLine::Note(format!("updated {}", u.fetched_at.format("%H:%M:%S")), TEXT_DIM));
        }
        if let Some(e) = &self.error {
            lines.push(TipLine::Note(format!("last refresh failed: {e}"), RED));
        }
        lines
    }

    fn show_tip(&mut self, event_loop: &ActiveEventLoop) {
        self.tip = None;
        let Some(main) = &self.window else { return };
        let lines = self.tip_lines();
        if lines.is_empty() {
            return;
        }
        let s = main.scale_factor();
        let px = |v: f64| (v * s).round() as i32;
        let fsize = TIP_FONT_SIZE * s as f32;
        let tw = |t: &str| Canvas::text_width(&self.font, fsize, t);
        let (mut c0, mut c1, mut c2, mut note) = (0, 0, 0, 0);
        for l in &lines {
            match l {
                TipLine::Limit { name, pct, reset, .. } => {
                    c0 = c0.max(tw(name));
                    c1 = c1.max(tw(pct));
                    c2 = c2.max(tw(reset));
                }
                TipLine::Note(t, _) => note = note.max(tw(t)),
            }
        }
        let gap = px(TIP_COL_GAP);
        let line_h = (fsize * 1.4).round() as i32;
        let w = (c0 + gap + c1 + gap + c2).max(note) + 2 * px(TIP_PAD);
        let h = line_h * lines.len() as i32 + 2 * px(TIP_PAD);

        // Below the widget, or above it if there is no room; clamp to the monitor.
        let Ok(pos) = main.outer_position() else { return };
        let main_h = main.outer_size().height as i32;
        let (mut x, mut y) = (pos.x, pos.y + main_h + px(4.0));
        if let Some(m) = main.current_monitor() {
            let (mx, my) = (m.position().x, m.position().y);
            let (mw, mh) = (m.size().width as i32, m.size().height as i32);
            if y + h > my + mh {
                y = pos.y - h - px(4.0);
            }
            x = x.min(mx + mw - w).max(mx);
        }

        let attrs = Window::default_attributes()
            .with_title("Claude usage")
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_inner_size(PhysicalSize::new(w as u32, h as u32))
            .with_position(PhysicalPosition::new(x, y))
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_override_redirect(true)
            .with_x11_window_type(vec![WindowType::Tooltip]);
        let Ok(window) = event_loop.create_window(attrs) else { return };
        let window = Rc::new(window);
        let Ok(context) = Context::new(window.clone()) else { return };
        let Ok(surface) = Surface::new(&context, window.clone()) else { return };
        self.tip = Some(Tip { window, surface });
    }

    fn draw_tip(&mut self) {
        let lines = self.tip_lines();
        let Some(Tip { window, surface }) = &mut self.tip else { return };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        let s = window.scale_factor();
        let px = |v: f64| (v * s).round() as i32;
        let fsize = TIP_FONT_SIZE * s as f32;
        let font = &self.font;
        let tw = |t: &str| Canvas::text_width(font, fsize, t);
        let (mut c0, mut c1) = (0, 0);
        for l in &lines {
            if let TipLine::Limit { name, pct, .. } = l {
                c0 = c0.max(tw(name));
                c1 = c1.max(tw(pct));
            }
        }

        let mut c = Canvas { buf: &mut buf, w: w.get() as usize, h: h.get() as usize };
        c.clear(BG);
        c.border(BORDER);
        let gap = px(TIP_COL_GAP);
        let line_h = (fsize * 1.4).round() as i32;
        let x0 = px(TIP_PAD);
        let all = (i32::MIN, i32::MAX);
        for (i, l) in lines.iter().enumerate() {
            let baseline = px(TIP_PAD) + line_h * i as i32 + ((line_h as f32 + 0.72 * fsize) / 2.0).round() as i32;
            match l {
                TipLine::Limit { name, pct, color, reset } => {
                    c.text_clip(font, fsize, x0, baseline, name, TEXT, all);
                    let px1 = x0 + c0 + gap + c1 - tw(pct); // right-aligned
                    c.text_clip(font, fsize, px1, baseline, pct, *color, all);
                    c.text_clip(font, fsize, x0 + c0 + gap + c1 + gap, baseline, reset, TEXT, all);
                }
                TipLine::Note(t, color) => {
                    c.text_clip(font, fsize, x0, baseline, t, *color, all);
                }
            }
        }
        let _ = buf.present();
    }

    fn hide_tip(&mut self) {
        self.hover_since = None;
        self.tip = None;
    }

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

        let bar_w = c.w as i32 - 2 * px(PAD);
        let bar_h = px(ROW_H);
        let row = |i: i32| (px(PAD), px(PAD) + i * (bar_h + px(GAP)), bar_w, bar_h);
        let (row1, row2) = (row(0), row(1));
        let warn = warn_limits(&self.usage);
        let ink = (TEXT_ON_FILL, TEXT);

        match (&self.usage, &self.error) {
            (None, msg) => {
                let msg = msg.as_deref().unwrap_or("Loading…");
                let full = (px(PAD), px(PAD), bar_w, px(win_height(2) - 2.0 * PAD));
                c.labeled_bar(font, fsize, full, 0.0, (TRACK, BG), msg, ink);
            }
            (Some(u), err) => {
                // Bar 1: session usage.
                let pct = u.session.utilization.clamp(0.0, 100.0);
                let label = format!("{:.0}%", u.session.utilization);
                c.labeled_bar(font, fsize, row1, pct / 100.0, (usage_color(pct), TRACK), &label, ink);

                // Bar 2: elapsed part of the 5h window, time until reset.
                match u.session.resets_at {
                    Some(reset) => {
                        let remaining = (reset - Local::now()).max(TimeDelta::zero());
                        let elapsed = 1.0 - remaining.num_seconds() as f64 / SESSION_LEN_SECS;
                        let mins = remaining.num_minutes();
                        let label = format!("{}:{:02} @ {}", mins / 60, mins % 60, (reset + TimeDelta::seconds(30)).format("%H:%M"));
                        c.labeled_bar(font, fsize, row2, elapsed, (BLUE, TRACK), &label, ink);
                    }
                    None => c.labeled_bar(font, fsize, row2, 0.0, (BLUE, TRACK), "no session", ink),
                }

                // Extra bars: other limits (weekly, per-model, ...) above the threshold.
                for (i, l) in warn.iter().enumerate() {
                    let pct = l.utilization.clamp(0.0, 100.0);
                    let label = format!("{} {:.0}%", l.label(), l.utilization);
                    c.labeled_bar(font, fsize, row(2 + i as i32), pct / 100.0, (usage_color(pct), TRACK), &label, ink);
                }

                // Stale-data indicator: the last refresh failed.
                if err.is_some() {
                    let d = px(4.0);
                    c.rect(c.w as i32 - d, 0, d, d, RED);
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
            .with_inner_size(LogicalSize::new(WIN_W, win_height(2)))
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.tip.as_ref().is_some_and(|t| t.window.id() == id) {
            if let WindowEvent::RedrawRequested = event {
                self.draw_tip();
            }
            return;
        }
        match event {
            // Only a middle click quits; ignore WM close requests.
            WindowEvent::CloseRequested => {}
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
            WindowEvent::CursorEntered { .. } => {
                self.hover_since.get_or_insert_with(Instant::now);
            }
            WindowEvent::CursorLeft { .. } => self.hide_tip(),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => match button {
                MouseButton::Left => {
                    self.hide_tip();
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
                Key::Character("r") | Key::Named(NamedKey::F5) => {
                    let _ = self.refresh_tx.send(());
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, result: FetchResult) {
        let rows_before = self.rows();
        match result {
            Ok(u) => {
                self.refreshed_after_reset = false;
                self.usage = Some(u);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        if let Some(w) = &self.window {
            if self.rows() != rows_before {
                let _ = w.request_inner_size(LogicalSize::new(WIN_W, win_height(self.rows())));
            }
            w.request_redraw();
        }
        // Content (and size) may have changed: rebuild an open tooltip.
        if self.tip.is_some() {
            self.show_tip(event_loop);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.next_tick = now + Duration::from_secs(1);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            if let Some(t) = &self.tip {
                t.window.request_redraw();
            }
            if self.position_dirty {
                self.position_dirty = false;
                if let Some(p) = self.position {
                    save_position(p);
                }
            }
            // Fetch fresh data shortly after the window resets.
            if let Some(reset) = self.usage.as_ref().and_then(|u| u.session.resets_at) {
                if !self.refreshed_after_reset && Local::now() > reset + TimeDelta::seconds(5) {
                    self.refreshed_after_reset = true;
                    let _ = self.refresh_tx.send(());
                }
            }
        }
        let mut wake = self.next_tick;
        if let (Some(since), None) = (self.hover_since, &self.tip) {
            if now >= since + TIP_DELAY {
                self.show_tip(event_loop);
            } else {
                wake = wake.min(since + TIP_DELAY);
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
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
        hover_since: None,
        tip: None,
    };
    event_loop.run_app(&mut app).expect("run app");
}
