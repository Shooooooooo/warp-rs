//! A floating-point RGB canvas that everything is drawn into before it ever
//! becomes a character.
//!
//! Light is *added*, not written, so a hundred streaks crossing the same
//! subpixel pile up into something bright instead of the last one winning.
//! Values are allowed to run past 1.0 and are pulled back into range once, at
//! the end, by the tonemap — which is what makes overlapping streaks bloom
//! rather than clip into flat white.
//!
//! This is a look, not a colour-managed pipeline: exposure and gamma here are
//! tuned by eye against what the terminal actually shows.

use crate::starfield::Streak;

/// Per-sample brightness falls off with streak length, so a long smear spreads
/// its light instead of burning a line through the frame.
const LENGTH_FALLOFF: f32 = 0.12;
/// A streak's tail is this fraction as bright as its head, which leaves a
/// visible star at the leading end instead of a uniform dash.
const TAIL_BRIGHTNESS: f32 = 0.32;
/// Backstop on samples per streak; clipping already bounds this in practice.
const MAX_SAMPLES: usize = 4096;

pub struct Canvas {
    width: usize,
    height: usize,
    buf: Vec<[f32; 3]>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self { width, height, buf: vec![[0.0; 3]; width * height] }
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.buf.resize(width * height, [0.0; 3]);
        }
    }

    pub fn clear(&mut self) {
        self.buf.fill([0.0; 3]);
    }

    /// Add light at a subpixel position, spread bilinearly over the four
    /// neighbouring cells. The interpolation is what keeps slow stars from
    /// visibly hopping from cell to cell.
    pub fn splat(&mut self, x: f32, y: f32, color: [f32; 3], weight: f32) {
        if weight <= 0.0 || !x.is_finite() || !y.is_finite() {
            return;
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let taps = [
            (x0, y0, (1.0 - fx) * (1.0 - fy)),
            (x0 + 1.0, y0, fx * (1.0 - fy)),
            (x0, y0 + 1.0, (1.0 - fx) * fy),
            (x0 + 1.0, y0 + 1.0, fx * fy),
        ];
        for (tx, ty, share) in taps {
            if share <= 0.0 || tx < 0.0 || ty < 0.0 {
                continue;
            }
            let (tx, ty) = (tx as usize, ty as usize);
            if tx >= self.width || ty >= self.height {
                continue;
            }
            let px = &mut self.buf[ty * self.width + tx];
            let w = weight * share;
            for i in 0..3 {
                px[i] += color[i] * w;
            }
        }
    }

    /// Draw one star's contribution: a line from where it was to where it is,
    /// brightening toward the head.
    pub fn draw_streak(&mut self, streak: &Streak) {
        let Some((from, to)) = self.clip(streak.from, streak.to) else {
            return;
        };
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();

        if length < 0.75 {
            self.splat(to.0, to.1, streak.color, streak.intensity);
            return;
        }

        let steps = (length.ceil() as usize).clamp(1, MAX_SAMPLES);
        let per_sample = streak.intensity / (1.0 + length * LENGTH_FALLOFF);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            // `from` is the tail, `to` the head: ramp brightness along it.
            let ramp = TAIL_BRIGHTNESS + (1.0 - TAIL_BRIGHTNESS) * t;
            self.splat(
                from.0 + dx * t,
                from.1 + dy * t,
                streak.color,
                per_sample * ramp,
            );
        }
    }

    /// Clip a segment to the canvas (Liang–Barsky). Returning `None` for a
    /// fully off-screen streak is what keeps a hard turn from costing anything.
    fn clip(&self, a: (f32, f32), b: (f32, f32)) -> Option<((f32, f32), (f32, f32))> {
        if !(a.0.is_finite() && a.1.is_finite() && b.0.is_finite() && b.1.is_finite()) {
            return None;
        }
        let (max_x, max_y) = (self.width as f32 - 1.0, self.height as f32 - 1.0);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (mut t0, mut t1) = (0.0f32, 1.0f32);

        for (p, q) in [(-dx, a.0), (dx, max_x - a.0), (-dy, a.1), (dy, max_y - a.1)] {
            if p == 0.0 {
                if q < 0.0 {
                    return None; // parallel to this edge and outside it
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    if r > t1 {
                        return None;
                    }
                    t0 = t0.max(r);
                } else {
                    if r < t0 {
                        return None;
                    }
                    t1 = t1.min(r);
                }
            }
        }
        Some((
            (a.0 + dx * t0, a.1 + dy * t0),
            (a.0 + dx * t1, a.1 + dy * t1),
        ))
    }

    /// A soft radial pool of light — the glare down the throat of the tunnel.
    pub fn add_glow(&mut self, cx: f32, cy: f32, radius: f32, color: [f32; 3], strength: f32) {
        if strength <= 0.0 || radius <= 0.0 {
            return;
        }
        let x0 = ((cx - radius).floor().max(0.0)) as usize;
        let y0 = ((cy - radius).floor().max(0.0)) as usize;
        let x1 = ((cx + radius).ceil().min(self.width as f32 - 1.0)).max(0.0) as usize;
        let y1 = ((cy + radius).ceil().min(self.height as f32 - 1.0)).max(0.0) as usize;
        let inv = 1.0 / radius;

        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() * inv;
                if d >= 1.0 {
                    continue;
                }
                // A quartic falloff concentrates the light in the middle and
                // trails off to nothing at the rim. Anything flatter reads as
                // a solid sphere hanging in space rather than as glare.
                let fade = (1.0 - d).powi(4);
                let px = &mut self.buf[y * self.width + x];
                for i in 0..3 {
                    px[i] += color[i] * strength * fade;
                }
            }
        }
    }

    /// Uniform additive wash — the white-out when the drive catches.
    pub fn add_flash(&mut self, color: [f32; 3], amount: f32) {
        if amount <= 0.0 {
            return;
        }
        for px in &mut self.buf {
            for i in 0..3 {
                px[i] += color[i] * amount;
            }
        }
    }

    /// Darken toward the corners, pulling the eye to the vanishing point.
    pub fn apply_vignette(&mut self, cx: f32, cy: f32, strength: f32) {
        if strength <= 0.0 {
            return;
        }
        let max_r = ((self.width as f32).powi(2) + (self.height as f32).powi(2)).sqrt() * 0.5;
        let inv = 1.0 / max_r.max(1.0);
        for y in 0..self.height {
            for x in 0..self.width {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() * inv;
                let factor = 1.0 - strength * (d * d).min(1.0);
                for channel in &mut self.buf[y * self.width + x] {
                    *channel *= factor;
                }
            }
        }
    }

    /// Collapse the HDR buffer to 8-bit sRGB-ish pixels, row-major.
    pub fn resolve(&self, exposure: f32, gamma: f32) -> Vec<[u8; 3]> {
        let inv_gamma = 1.0 / gamma;
        self.buf
            .iter()
            .map(|px| {
                let mut out = [0u8; 3];
                for i in 0..3 {
                    // Exponential tonemap: monotonic, never clips, and rolls
                    // highlights off instead of flattening them.
                    let mapped = 1.0 - (-px[i].max(0.0) * exposure).exp();
                    out[i] = (mapped.powf(inv_gamma) * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                out
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streak(from: (f32, f32), to: (f32, f32)) -> Streak {
        Streak { from, to, color: [1.0, 1.0, 1.0], intensity: 1.0 }
    }

    fn total_light(canvas: &Canvas) -> f32 {
        canvas.buf.iter().map(|p| p[0] + p[1] + p[2]).sum()
    }

    #[test]
    fn wildly_offscreen_streaks_never_write_out_of_bounds() {
        let mut canvas = Canvas::new(64, 32);
        let far = 1e7;
        for pair in [
            ((-far, -far), (far, far)),
            ((far, 0.0), (-far, 31.0)),
            ((-5.0, -5.0), (-1.0, -1.0)),
            ((100.0, 100.0), (200.0, 200.0)),
            ((f32::NAN, 0.0), (10.0, 10.0)),
            ((f32::INFINITY, 1.0), (2.0, 2.0)),
        ] {
            canvas.draw_streak(&streak(pair.0, pair.1));
        }
        assert_eq!(canvas.buf.len(), 64 * 32, "buffer must not have grown");
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn a_streak_that_misses_the_canvas_draws_nothing() {
        let mut canvas = Canvas::new(64, 32);
        canvas.draw_streak(&streak((-100.0, -100.0), (-50.0, -80.0)));
        assert_eq!(total_light(&canvas), 0.0);
    }

    #[test]
    fn a_short_streak_lands_where_it_should() {
        let mut canvas = Canvas::new(64, 32);
        canvas.draw_streak(&streak((10.0, 10.0), (10.0, 10.0)));
        assert!(canvas.buf[10 * 64 + 10][0] > 0.0);
        assert_eq!(canvas.buf[0][0], 0.0);
    }

    #[test]
    fn light_accumulates_instead_of_overwriting() {
        let mut canvas = Canvas::new(8, 8);
        canvas.splat(4.0, 4.0, [1.0, 1.0, 1.0], 1.0);
        let once = canvas.buf[4 * 8 + 4][0];
        canvas.splat(4.0, 4.0, [1.0, 1.0, 1.0], 1.0);
        assert!((canvas.buf[4 * 8 + 4][0] - once * 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_splat_conserves_its_weight_across_the_four_taps() {
        let mut canvas = Canvas::new(8, 8);
        canvas.splat(3.25, 4.75, [1.0, 0.0, 0.0], 2.0);
        assert!((total_light(&canvas) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn longer_streaks_are_dimmer_per_pixel() {
        let mut short = Canvas::new(128, 128);
        short.draw_streak(&streak((10.0, 64.0), (14.0, 64.0)));
        let mut long = Canvas::new(128, 128);
        long.draw_streak(&streak((10.0, 64.0), (110.0, 64.0)));
        let peak = |c: &Canvas| c.buf.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        assert!(peak(&long) < peak(&short), "a fast smear should not burn in");
        assert!(total_light(&long) > total_light(&short), "but it should glow more");
    }

    #[test]
    fn streaks_are_brighter_at_the_head() {
        let mut canvas = Canvas::new(128, 8);
        canvas.draw_streak(&streak((10.0, 4.0), (110.0, 4.0)));
        let head = canvas.buf[4 * 128 + 108][0];
        let tail = canvas.buf[4 * 128 + 12][0];
        assert!(head > tail, "head {head} should outshine tail {tail}");
    }

    #[test]
    fn tonemap_is_monotonic_and_stays_in_range() {
        let mut canvas = Canvas::new(64, 1);
        for x in 0..64 {
            let v = x as f32 * 0.5;
            canvas.splat(x as f32, 0.0, [1.0, 1.0, 1.0], v);
        }
        let out = canvas.resolve(1.0, 2.2);
        let mut prev = 0u8;
        for px in out.iter().take(64) {
            assert!(px[0] >= prev, "tonemap must not go backwards");
            prev = px[0];
        }
        assert!(prev > 200, "the brightest sample should be near saturation");
    }

    #[test]
    fn enormous_values_saturate_rather_than_wrapping() {
        let mut canvas = Canvas::new(4, 4);
        canvas.splat(1.0, 1.0, [1.0, 1.0, 1.0], 1e9);
        let out = canvas.resolve(1.0, 2.2);
        assert_eq!(out[4 + 1], [255, 255, 255], "pixel (1, 1) of a 4-wide canvas");
    }

    #[test]
    fn an_empty_canvas_resolves_to_black() {
        let canvas = Canvas::new(16, 16);
        assert!(canvas.resolve(1.4, 2.2).iter().all(|p| *p == [0, 0, 0]));
    }

    #[test]
    fn clear_and_resize_keep_the_buffer_consistent() {
        let mut canvas = Canvas::new(16, 16);
        canvas.splat(8.0, 8.0, [1.0; 3], 5.0);
        canvas.clear();
        assert_eq!(total_light(&canvas), 0.0);
        canvas.resize(40, 9);
        assert_eq!(canvas.dims(), (40, 9));
        assert_eq!(canvas.buf.len(), 40 * 9);
        canvas.resize(0, 0);
        assert_eq!(canvas.dims(), (1, 1), "degenerate sizes are clamped, not fatal");
    }

    #[test]
    fn glow_is_brightest_at_its_centre_and_stays_inside_the_canvas() {
        let mut canvas = Canvas::new(32, 32);
        canvas.add_glow(16.0, 16.0, 8.0, [1.0, 1.0, 1.0], 1.0);
        assert!(canvas.buf[16 * 32 + 16][0] > canvas.buf[16 * 32 + 22][0]);
        assert_eq!(canvas.buf[0][0], 0.0, "the glow should not reach the corner");
        // A glow hanging off the edge must not panic or wrap.
        canvas.add_glow(-40.0, -40.0, 10.0, [1.0; 3], 1.0);
        canvas.add_glow(200.0, 200.0, 50.0, [1.0; 3], 1.0);
    }

    #[test]
    fn vignette_darkens_the_edges_and_spares_the_centre() {
        let mut canvas = Canvas::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                canvas.splat(x as f32, y as f32, [1.0; 3], 1.0);
            }
        }
        let centre_before = canvas.buf[32 * 64 + 32][0];
        canvas.apply_vignette(32.0, 32.0, 0.8);
        assert!((canvas.buf[32 * 64 + 32][0] - centre_before).abs() < 1e-6);
        assert!(canvas.buf[0][0] < centre_before * 0.6);
    }

    #[test]
    fn flash_lifts_every_pixel_equally() {
        let mut canvas = Canvas::new(8, 8);
        canvas.add_flash([1.0, 1.0, 1.0], 0.5);
        assert!(canvas.buf.iter().all(|p| (p[0] - 0.5).abs() < 1e-6));
    }
}
