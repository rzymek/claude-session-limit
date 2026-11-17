//! Minimal software rendering into a 0RGB `u32` pixel buffer.

use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont, point};

pub type Font = FontVec;

const FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

pub fn load_font() -> Font {
    for p in FONT_PATHS {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(f) = FontVec::try_from_vec(bytes) {
                return f;
            }
        }
    }
    eprintln!("claude-session-limit: no usable font found in {FONT_PATHS:?}");
    std::process::exit(1);
}

pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
}

impl Canvas<'_> {
    pub fn clear(&mut self, color: u32) {
        self.buf.fill(color);
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let x0 = x.clamp(0, self.w as i32) as usize;
        let y0 = y.clamp(0, self.h as i32) as usize;
        let x1 = (x + w).clamp(0, self.w as i32) as usize;
        let y1 = (y + h).clamp(0, self.h as i32) as usize;
        if x1 <= x0 {
            return;
        }
        for row in y0..y1 {
            self.buf[row * self.w + x0..row * self.w + x1].fill(color);
        }
    }

    pub fn border(&mut self, color: u32) {
        let (w, h) = (self.w as i32, self.h as i32);
        self.rect(0, 0, w, 1, color);
        self.rect(0, h - 1, w, 1, color);
        self.rect(0, 0, 1, h, color);
        self.rect(w - 1, 0, 1, h, color);
    }

    /// Draws text with its baseline at `baseline`; only pixels with `clip.0 <= x < clip.1` are drawn.
    /// Returns the advance width.
    #[allow(clippy::too_many_arguments)]
    pub fn text_clip(
        &mut self,
        font: &Font,
        size: f32,
        x: i32,
        baseline: i32,
        s: &str,
        color: u32,
        clip: (i32, i32),
    ) -> i32 {
        let scaled = font.as_scaled(PxScale::from(size));
        let mut pen = x as f32;
        for ch in s.chars() {
            let id = scaled.glyph_id(ch);
            let glyph = id.with_scale_and_position(scaled.scale(), point(pen, baseline as f32));
            pen += scaled.h_advance(id);
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let b = outline.px_bounds();
            let (x0, x1) = (clip.0.max(0), clip.1.min(self.w as i32));
            let h = self.h as i32;
            outline.draw(|gx, gy, cov| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                let a = (cov.clamp(0.0, 1.0) * 255.0).round() as u32;
                if a > 0 && px >= x0 && px < x1 && py >= 0 && py < h {
                    let i = py as usize * self.w + px as usize;
                    self.buf[i] = blend(self.buf[i], color, a);
                }
            });
        }
        (pen - x as f32).round() as i32
    }

    pub fn text_width(font: &Font, size: f32, s: &str) -> i32 {
        let scaled = font.as_scaled(PxScale::from(size));
        s.chars().map(|c| scaled.h_advance(scaled.glyph_id(c))).sum::<f32>().round() as i32
    }

    /// Progress bar with `label` centered inside it. The label is drawn in
    /// `on_fill` over the filled part and `on_track` over the rest. The font
    /// shrinks if the label would not fit.
    #[allow(clippy::too_many_arguments)]
    pub fn labeled_bar(
        &mut self,
        font: &Font,
        size: f32,
        (x, y, w, h): (i32, i32, i32, i32),
        frac: f64,
        (fill, track): (u32, u32),
        label: &str,
        (on_fill, on_track): (u32, u32),
    ) {
        self.rect(x, y, w, h, track);
        let filled = (w as f64 * frac.clamp(0.0, 1.0)).round() as i32;
        self.rect(x, y, filled, h, fill);

        let mut size = size;
        let mut tw = Self::text_width(font, size, label);
        let max_w = w - 4;
        if tw > max_w && tw > 0 {
            size *= max_w as f32 / tw as f32;
            tw = Self::text_width(font, size, label);
        }
        let tx = x + (w - tw) / 2;
        // Center the digits' cap height (~0.72 em) vertically.
        let baseline = y + ((h as f32 + 0.72 * size) / 2.0).round() as i32;
        let split = x + filled;
        self.text_clip(font, size, tx, baseline, label, on_fill, (i32::MIN, split));
        self.text_clip(font, size, tx, baseline, label, on_track, (split, i32::MAX));
    }
}

fn blend(dst: u32, src: u32, a: u32) -> u32 {
    let ch = |shift: u32| {
        let d = (dst >> shift) & 0xff;
        let s = (src >> shift) & 0xff;
        ((s * a + d * (255 - a)) / 255) << shift
    };
    ch(16) | ch(8) | ch(0)
}
