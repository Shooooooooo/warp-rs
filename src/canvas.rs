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

/// Entries in the tonemap table. The curve is sampled on a square-root
/// compressed domain, which spends resolution where the output moves fastest;
/// 1024 holds the worst error to a single 8-bit level and keeps the whole
/// table inside L1.
const TONEMAP_LUT: usize = 1024;

/// The tonemap curve, precomputed.
///
/// Exposure and gamma are fixed for the life of a run and the output is only
/// eight bits wide, so the entire curve fits in a table. That turns what was
/// an `exp` and a `powf` per channel per subpixel — around 98k of each per
/// frame on a large terminal, and the most expensive thing the renderer did —
/// into a square root and an index.
pub struct Tonemap {
    lut: Box<[u8; TONEMAP_LUT]>,
    /// Reciprocal of the smallest HDR value that already resolves to 255.
    inv_ceiling: f32,
}

impl Tonemap {
    pub fn new(exposure: f32, gamma: f32) -> Self {
        let ceiling = saturation_point(exposure, gamma);
        let mut lut = Box::new([0u8; TONEMAP_LUT]);
        for (i, slot) in lut.iter_mut().enumerate() {
            let t = i as f32 / (TONEMAP_LUT - 1) as f32;
            *slot = map_exact(t * t * ceiling, exposure, gamma);
        }
        Self {
            lut,
            inv_ceiling: 1.0 / ceiling,
        }
    }

    /// Map one HDR channel to eight bits. Negatives and NaN land on black.
    fn channel(&self, v: f32) -> u8 {
        let t = (v.max(0.0) * self.inv_ceiling).min(1.0).sqrt();
        // A float-to-int cast saturates, so even a rogue value stays in range.
        self.lut[((t * (TONEMAP_LUT - 1) as f32) as usize).min(TONEMAP_LUT - 1)]
    }
}

/// The exponential tonemap, evaluated honestly: monotonic, never clips, and
/// rolls highlights off instead of flattening them. Only the table is built
/// from this now; the per-pixel path goes through `Tonemap::channel`.
fn map_exact(v: f32, exposure: f32, gamma: f32) -> u8 {
    let mapped = 1.0 - (-v.max(0.0) * exposure).exp();
    (mapped.powf(1.0 / gamma) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The smallest HDR value that already resolves to 255. Sampling the table
/// beyond here would spend entries on a curve that has stopped moving.
fn saturation_point(exposure: f32, gamma: f32) -> f32 {
    let top = (254.5f32 / 255.0).powf(gamma);
    let ceiling = -(1.0 - top).ln() / exposure.max(f32::MIN_POSITIVE);
    if ceiling.is_finite() && ceiling > 0.0 {
        ceiling
    } else {
        1.0
    }
}

pub struct Canvas {
    width: usize,
    height: usize,
    buf: Vec<[f32; 3]>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self {
            width,
            height,
            buf: vec![[0.0; 3]; width * height],
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            // Cleared, not just re-length-ed: the row stride follows the width,
            // so light left over from the old layout would reappear somewhere
            // it was never drawn. Every frame clears before it draws, but that
            // is the renderer's habit, not something resize may lean on.
            self.buf.clear();
            self.buf.resize(width * height, [0.0; 3]);
        }
    }

    pub fn clear(&mut self) {
        self.buf.fill([0.0; 3]);
    }

    /// Add light at a subpixel position, spread bilinearly over the four
    /// neighbouring cells. The interpolation is what keeps slow stars from
    /// visibly hopping from cell to cell.
    ///
    /// Anywhere at all: off the canvas, NaN, negative. Everything is checked.
    pub fn splat(&mut self, x: f32, y: f32, color: [f32; 3], weight: f32) {
        if weight <= 0.0 || !x.is_finite() || !y.is_finite() {
            return;
        }
        if x >= 0.0 && y >= 0.0 && x <= self.max_x() && y <= self.max_y() {
            self.splat_inside(x, y, color, weight);
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

    /// The same splat for a point already known to be on the canvas, which is
    /// every sample of a clipped streak — the overwhelming majority of them.
    ///
    /// Worth the duplication because this is the innermost loop in the program:
    /// at warp a single frame can splat a couple of million times, and the
    /// general path spends most of that testing four taps it already knows the
    /// answer for. The one case still needing care is the far edge, where the
    /// `+1` neighbour does not exist — but a point can only sit on the last
    /// column when its fraction is exactly zero, so that tap's weight is zero
    /// too and folding it back onto the pixel itself adds nothing to it.
    ///
    /// Arithmetic and ordering are identical to `splat`, deliberately: this is
    /// meant to produce the same frame, not merely a similar one.
    fn splat_inside(&mut self, x: f32, y: f32, color: [f32; 3], weight: f32) {
        let (x0, y0) = (x as usize, y as usize); // non-negative, so a floor
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let right = usize::from(x0 + 1 < self.width);
        let below = if y0 + 1 < self.height { self.width } else { 0 };

        let base = y0 * self.width + x0;
        let taps = [
            (base, (1.0 - fx) * (1.0 - fy)),
            (base + right, fx * (1.0 - fy)),
            (base + below, (1.0 - fx) * fy),
            (base + right + below, fx * fy),
        ];
        for (idx, share) in taps {
            let px = &mut self.buf[idx];
            let w = weight * share;
            for i in 0..3 {
                px[i] += color[i] * w;
            }
        }
    }

    /// The last column and row, as the coordinates a sample may reach.
    fn max_x(&self) -> f32 {
        (self.width - 1) as f32
    }

    fn max_y(&self) -> f32 {
        (self.height - 1) as f32
    }

    /// Draw one star's contribution: a line from where it was to where it is,
    /// brightening toward the head.
    pub fn draw_streak(&mut self, streak: &Streak) {
        // A dark star has nothing to add, and a NaN one would spread across the
        // buffer — nothing recovers a pixel once it is not a number.
        if streak.intensity.is_nan() || streak.intensity <= 0.0 {
            return;
        }
        let Some((from, to)) = self.clip(streak.from, streak.to) else {
            return;
        };
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        // Clipping put both ends on the canvas. The clamps below are for the
        // last of the floating-point slack: an interpolated point can land a
        // hair outside the box its endpoints were clipped to.
        let (max_x, max_y) = (self.max_x(), self.max_y());

        if length < 0.75 {
            let (x, y) = (to.0.clamp(0.0, max_x), to.1.clamp(0.0, max_y));
            self.splat_inside(x, y, streak.color, streak.intensity);
            return;
        }

        let steps = (length.ceil() as usize).clamp(1, MAX_SAMPLES);
        // One reciprocal per streak rather than a division per sample. At warp
        // a frame walks a couple of million of them, and a float divide is
        // several times dearer than the multiply that replaces it: about six
        // percent of the time spent drawing, at twenty thousand stars. It buys
        // that for nothing — the frames come out byte-identical, so the
        // reference hashes did not move.
        let inv_steps = 1.0 / steps as f32;
        let per_sample = streak.intensity / (1.0 + length * LENGTH_FALLOFF);
        for i in 0..=steps {
            let t = i as f32 * inv_steps;
            // `from` is the tail, `to` the head: ramp brightness along it.
            let ramp = TAIL_BRIGHTNESS + (1.0 - TAIL_BRIGHTNESS) * t;
            let x = (from.0 + dx * t).clamp(0.0, max_x);
            let y = (from.1 + dy * t).clamp(0.0, max_y);
            self.splat_inside(x, y, streak.color, per_sample * ramp);
        }
    }

    /// Draw a straight segment at an even brightness, end to end.
    ///
    /// A streak ramps from tail to head because that is what a moving point
    /// leaves behind. A hull edge is not moving anywhere, so it gets neither
    /// the ramp nor the length falloff: a long spar and a short one are the
    /// same piece of metal, and dimming the long one would read as a lighting
    /// cue that is not there.
    pub fn draw_line(&mut self, from: (f32, f32), to: (f32, f32), color: [f32; 3], intensity: f32) {
        if intensity.is_nan() || intensity <= 0.0 {
            return;
        }
        let Some((from, to)) = self.clip(from, to) else {
            return;
        };
        let (max_x, max_y) = (self.max_x(), self.max_y());
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 0.75 {
            let (x, y) = (to.0.clamp(0.0, max_x), to.1.clamp(0.0, max_y));
            self.splat_inside(x, y, color, intensity);
            return;
        }

        let steps = (length.ceil() as usize).clamp(1, MAX_SAMPLES);
        let inv_steps = 1.0 / steps as f32;
        for i in 0..=steps {
            let t = i as f32 * inv_steps;
            let x = (from.0 + dx * t).clamp(0.0, max_x);
            let y = (from.1 + dy * t).clamp(0.0, max_y);
            self.splat_inside(x, y, color, intensity);
        }
    }

    /// Draw a streak that has been bent into a curve: the same light a
    /// [`Canvas::draw_streak`] would lay down, following a polyline instead of
    /// a straight segment.
    ///
    /// The ramp and the length falloff are measured over the *whole* path
    /// rather than per segment. Doing it per segment would scallop the streak —
    /// every joint would restart at the tail brightness — and would dim a
    /// finely subdivided curve far more than a coarsely subdivided one, so the
    /// picture would change with the subdivision rather than with the physics.
    pub fn draw_path(&mut self, points: &[(f32, f32)], color: [f32; 3], intensity: f32) {
        if intensity.is_nan() || intensity <= 0.0 || points.is_empty() {
            return;
        }
        let total: f32 = points
            .windows(2)
            .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
            .sum();
        // A path that went nowhere, or one with a NaN in it, is a point.
        if !total.is_finite() || total < 0.75 {
            let head = points[points.len() - 1];
            self.draw_streak(&Streak {
                from: head,
                to: head,
                color,
                intensity,
            });
            return;
        }

        let (max_x, max_y) = (self.max_x(), self.max_y());
        let per_sample = intensity / (1.0 + total * LENGTH_FALLOFF);
        let inv_total = 1.0 / total;
        let mut travelled = 0.0f32;
        // Where the previous segment stopped, so the vertex the next one
        // starts from is not splatted twice. Without this every joint comes
        // out as a bright bead, and a finely subdivided curve is a dotted
        // line rather than a smooth one.
        let mut resume_at: Option<(f32, f32)> = None;

        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let span = (b.0 - a.0).hypot(b.1 - a.1);
            // Clipping moves the endpoints, so the ramp has to be evaluated
            // against where they ended up along the *original* segment — not
            // against the clipped one, which would stretch the ramp back out
            // over whatever fragment survived.
            if let Some((from, to)) = self.clip(a, b) {
                let at = |p: (f32, f32)| (travelled + (p.0 - a.0).hypot(p.1 - a.1)) * inv_total;
                let (t0, t1) = (at(from), at(to));
                let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                let length = (dx * dx + dy * dy).sqrt();
                let steps = (length.ceil() as usize).clamp(1, MAX_SAMPLES);
                let inv_steps = 1.0 / steps as f32;
                let first = usize::from(resume_at == Some(from));
                resume_at = Some(to);
                for i in first..=steps {
                    let s = i as f32 * inv_steps;
                    let t = t0 + (t1 - t0) * s;
                    let ramp = TAIL_BRIGHTNESS + (1.0 - TAIL_BRIGHTNESS) * t;
                    let x = (from.0 + dx * s).clamp(0.0, max_x);
                    let y = (from.1 + dy * s).clamp(0.0, max_y);
                    self.splat_inside(x, y, color, per_sample * ramp);
                }
            }
            travelled += span;
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
        // The curve is quadratic in the distance, so it can be evaluated on the
        // *squared* distance directly — taking a square root here only to square
        // it again is a root per subpixel that never had to be taken.
        let inv_sq = inv * inv;
        for y in 0..self.height {
            for x in 0..self.width {
                let d_sq = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)) * inv_sq;
                let factor = 1.0 - strength * d_sq.min(1.0);
                for channel in &mut self.buf[y * self.width + x] {
                    *channel *= factor;
                }
            }
        }
    }

    /// Total light at one subpixel, for tests outside this module that care
    /// about where the light landed rather than what colour it came out.
    #[cfg(test)]
    pub fn light_at(&self, x: usize, y: usize) -> f32 {
        let px = self.buf[y * self.width + x];
        px[0] + px[1] + px[2]
    }

    /// Collapse the HDR buffer to 8-bit sRGB-ish pixels, row-major, into a
    /// caller-owned buffer. Taking one rather than returning a fresh `Vec`
    /// keeps a 98 KB allocation out of every single frame.
    pub fn resolve_into(&self, tone: &Tonemap, out: &mut Vec<[u8; 3]>) {
        out.clear();
        out.extend(self.buf.iter().map(|px| {
            [
                tone.channel(px[0]),
                tone.channel(px[1]),
                tone.channel(px[2]),
            ]
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streak(from: (f32, f32), to: (f32, f32)) -> Streak {
        Streak {
            from,
            to,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }
    }

    fn total_light(canvas: &Canvas) -> f32 {
        canvas.buf.iter().map(|p| p[0] + p[1] + p[2]).sum()
    }

    fn resolve(canvas: &Canvas, exposure: f32, gamma: f32) -> Vec<[u8; 3]> {
        let mut out = Vec::new();
        canvas.resolve_into(&Tonemap::new(exposure, gamma), &mut out);
        out
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
    fn the_last_row_and_column_have_no_neighbours_to_spill_into() {
        // The fast path folds the `+1` taps back onto the pixel itself rather
        // than testing for them. That is only sound because a point can sit on
        // the last column or row solely when its fraction is zero — so nothing
        // is folded in, and the weight neither leaks nor doubles.
        for (x, y) in [(7.0, 7.0), (7.0, 3.5), (3.5, 7.0), (0.0, 7.0), (7.0, 0.0)] {
            let mut canvas = Canvas::new(8, 8);
            canvas.splat(x, y, [1.0, 1.0, 1.0], 3.0);
            assert!(
                (total_light(&canvas) - 9.0).abs() < 1e-5,
                "({x}, {y}) deposited {} instead of 3 across 3 channels",
                total_light(&canvas) / 3.0
            );
        }
        // And the corner really is the corner, not the pixel before it.
        let mut canvas = Canvas::new(8, 8);
        canvas.splat(7.0, 7.0, [1.0, 1.0, 1.0], 3.0);
        assert_eq!(canvas.buf[7 * 8 + 7][0], 3.0);
    }

    #[test]
    fn a_streak_with_nothing_to_add_adds_nothing() {
        // A NaN is not less than or equal to zero, so an unguarded intensity
        // used to walk a line of them across the buffer, and no later pass
        // recovers a pixel that is not a number.
        let mut canvas = Canvas::new(32, 32);
        for intensity in [0.0, -1.0, f32::NAN] {
            canvas.draw_streak(&Streak {
                intensity,
                ..streak((2.0, 2.0), (28.0, 20.0))
            });
        }
        assert_eq!(total_light(&canvas), 0.0);
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn longer_streaks_are_dimmer_per_pixel() {
        let mut short = Canvas::new(128, 128);
        short.draw_streak(&streak((10.0, 64.0), (14.0, 64.0)));
        let mut long = Canvas::new(128, 128);
        long.draw_streak(&streak((10.0, 64.0), (110.0, 64.0)));
        let peak = |c: &Canvas| c.buf.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        assert!(
            peak(&long) < peak(&short),
            "a fast smear should not burn in"
        );
        assert!(
            total_light(&long) > total_light(&short),
            "but it should glow more"
        );
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
    fn a_line_is_as_bright_at_one_end_as_the_other() {
        // The opposite of a streak, and the reason it is a separate call: a
        // wireframe hull drawn with the streak ramp looks lit from one side.
        let mut canvas = Canvas::new(128, 8);
        canvas.draw_line((10.0, 4.0), (110.0, 4.0), [1.0, 1.0, 1.0], 1.0);
        let head = canvas.buf[4 * 128 + 108][0];
        let tail = canvas.buf[4 * 128 + 12][0];
        assert!(
            (head - tail).abs() < 1e-5,
            "head {head} against tail {tail}"
        );
        // And it does not dim as it gets longer, the way a smear does.
        let mut short = Canvas::new(128, 8);
        short.draw_line((10.0, 4.0), (20.0, 4.0), [1.0, 1.0, 1.0], 1.0);
        assert!((short.buf[4 * 128 + 15][0] - head).abs() < 1e-5);
    }

    #[test]
    fn a_line_off_the_canvas_draws_nothing_and_stays_in_bounds() {
        let mut canvas = Canvas::new(64, 32);
        for pair in [
            ((-100.0, -100.0), (-50.0, -80.0)),
            ((f32::NAN, 0.0), (10.0, 10.0)),
            ((1e9, 1e9), (-1e9, -1e9)),
        ] {
            canvas.draw_line(pair.0, pair.1, [1.0; 3], 1.0);
        }
        assert_eq!(canvas.buf.len(), 64 * 32);
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn a_two_point_path_is_exactly_the_streak_it_stands_in_for() {
        // The lensed sky falls back to a straight path whenever the drive is
        // off, so the bent and unbent code paths have to agree pixel for pixel
        // — not merely look alike — or engaging warp would visibly re-render
        // the whole field rather than bending it.
        let (a, b) = ((12.0, 20.0), (96.0, 51.0));
        let mut straight = Canvas::new(128, 64);
        straight.draw_streak(&Streak {
            from: a,
            to: b,
            color: [0.8, 0.9, 1.0],
            intensity: 1.3,
        });
        let mut path = Canvas::new(128, 64);
        path.draw_path(&[a, b], [0.8, 0.9, 1.0], 1.3);
        assert_eq!(straight.buf, path.buf);
    }

    #[test]
    fn a_path_ramps_over_its_whole_length_rather_than_every_joint() {
        // Subdividing a streak more finely must not change how it looks. If the
        // ramp restarted at every joint the curve would come out scalloped, and
        // the picture would depend on the subdivision instead of the physics.
        let ends = [(8.0, 32.0), (120.0, 32.0)];
        let mut coarse = Canvas::new(128, 64);
        coarse.draw_path(&ends, [1.0; 3], 1.0);

        let fine: Vec<(f32, f32)> = (0..=16)
            .map(|i| {
                let t = i as f32 / 16.0;
                (ends[0].0 + (ends[1].0 - ends[0].0) * t, 32.0)
            })
            .collect();
        let mut subdivided = Canvas::new(128, 64);
        subdivided.draw_path(&fine, [1.0; 3], 1.0);

        for (i, (a, b)) in coarse.buf.iter().zip(&subdivided.buf).enumerate() {
            assert!(
                (a[0] - b[0]).abs() < 0.02,
                "subdividing changed pixel {i}: {} against {}",
                a[0],
                b[0]
            );
        }
        // And it is still a streak: brighter at the head than at the tail.
        let row = 32 * 128;
        assert!(coarse.buf[row + 110][0] > coarse.buf[row + 12][0]);
    }

    #[test]
    fn a_curved_path_bends_where_it_is_told_to() {
        // The whole point of the primitive: light lands on the arc, not on the
        // chord between its ends.
        let arc: Vec<(f32, f32)> = (0..=12)
            .map(|i| {
                let t = i as f32 / 12.0;
                let angle = std::f32::consts::PI * t;
                (64.0 + 40.0 * angle.cos(), 60.0 - 40.0 * angle.sin())
            })
            .collect();
        let mut canvas = Canvas::new(128, 64);
        canvas.draw_path(&arc, [1.0; 3], 2.0);
        // On the arc, at the top of the sweep.
        assert!(canvas.buf[20 * 128 + 64][0] > 0.0, "the arc is missing");
        // On the chord between the ends, which nothing should have drawn.
        assert_eq!(canvas.buf[60 * 128 + 64][0], 0.0, "it drew the chord");
    }

    #[test]
    fn a_path_made_of_nonsense_draws_nothing_dangerous() {
        let mut canvas = Canvas::new(64, 32);
        canvas.draw_path(&[], [1.0; 3], 1.0);
        assert_eq!(total_light(&canvas), 0.0, "an empty path is not a point");

        for path in [
            vec![(10.0, 10.0)],
            vec![(f32::NAN, 2.0), (5.0, 5.0)],
            vec![(1e9, 1e9), (-1e9, 4.0), (2.0, 2.0)],
            vec![(3.0, 3.0), (3.0, 3.0), (3.0, 3.0)],
        ] {
            canvas.draw_path(&path, [1.0; 3], 1.0);
        }
        for intensity in [0.0, -1.0, f32::NAN] {
            canvas.draw_path(&[(4.0, 4.0), (40.0, 20.0)], [1.0; 3], intensity);
        }
        assert_eq!(canvas.buf.len(), 64 * 32, "the buffer must not have grown");
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn tonemap_is_monotonic_and_stays_in_range() {
        let mut canvas = Canvas::new(64, 1);
        for x in 0..64 {
            let v = x as f32 * 0.5;
            canvas.splat(x as f32, 0.0, [1.0, 1.0, 1.0], v);
        }
        let out = resolve(&canvas, 1.0, 2.2);
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
        let out = resolve(&canvas, 1.0, 2.2);
        assert_eq!(
            out[4 + 1],
            [255, 255, 255],
            "pixel (1, 1) of a 4-wide canvas"
        );
    }

    #[test]
    fn the_tonemap_table_matches_the_curve_it_replaces() {
        // The table stands in for an `exp` and a `powf` per channel per
        // subpixel. It is allowed to be a shade off, but never by more than one
        // 8-bit level, and never out of order — the bloom is the tonemap's
        // monotonicity showing through, so a kink in it would be visible.
        for step in 1..=40 {
            let exposure = step as f32 * 0.075;
            let tone = Tonemap::new(exposure, 2.2);
            let ceiling = saturation_point(exposure, 2.2);
            let mut prev = 0u8;
            // Well past the ceiling: the buffer is HDR and streaks pile up.
            for k in 0..=20_000 {
                let v = k as f32 / 20_000.0 * ceiling * 3.0;
                let (got, want) = (tone.channel(v), map_exact(v, exposure, 2.2));
                assert!(
                    got.abs_diff(want) <= 1,
                    "exposure {exposure}, v {v}: table says {got}, curve says {want}"
                );
                assert!(got >= prev, "the table went backwards at v {v}");
                prev = got;
            }
            assert_eq!(tone.channel(0.0), 0, "black must stay black");
            assert_eq!(tone.channel(1e9), 255, "an enormous value must saturate");
            assert_eq!(tone.channel(f32::NAN), 0, "a NaN must land on black");
            assert_eq!(tone.channel(-5.0), 0, "negative light is still black");
        }
    }

    #[test]
    fn resolving_reuses_its_output_buffer() {
        // Regression: this allocated and freed a fresh ~98 KB `Vec` every
        // frame, in a field documented as being reused across frames.
        let mut canvas = Canvas::new(64, 32);
        let tone = Tonemap::new(1.9, 2.2);
        let mut out = Vec::new();

        canvas.resolve_into(&tone, &mut out);
        assert_eq!(out.len(), 64 * 32);
        let capacity = out.capacity();
        for _ in 0..10 {
            canvas.splat(1.0, 1.0, [1.0; 3], 1.0);
            canvas.resolve_into(&tone, &mut out);
        }
        assert_eq!(
            out.capacity(),
            capacity,
            "a steady canvas must not reallocate"
        );

        // The length has to follow a resize rather than the old dimensions.
        canvas.resize(40, 9);
        canvas.resolve_into(&tone, &mut out);
        assert_eq!(out.len(), 40 * 9);
        canvas.resize(200, 100);
        canvas.resolve_into(&tone, &mut out);
        assert_eq!(out.len(), 200 * 100);
    }

    #[test]
    fn an_empty_canvas_resolves_to_black() {
        let canvas = Canvas::new(16, 16);
        assert!(resolve(&canvas, 1.4, 2.2).iter().all(|p| *p == [0, 0, 0]));
    }

    #[test]
    fn resizing_does_not_smear_old_light_into_the_new_layout() {
        // A pixel's index is `y * width + x`, so changing the width moves every
        // row. Anything left behind would surface somewhere it was never drawn.
        let mut canvas = Canvas::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                canvas.splat(x as f32, y as f32, [1.0; 3], 1.0);
            }
        }
        assert!(total_light(&canvas) > 0.0);

        canvas.resize(9, 30); // taller and narrower: the buffer grows
        assert_eq!(canvas.dims(), (9, 30));
        assert_eq!(total_light(&canvas), 0.0, "old light survived the reflow");

        for y in 0..30 {
            for x in 0..9 {
                canvas.splat(x as f32, y as f32, [1.0; 3], 1.0);
            }
        }
        canvas.resize(40, 4); // and again, this time shrinking it
        assert_eq!(canvas.dims(), (40, 4));
        assert_eq!(total_light(&canvas), 0.0, "old light survived the reflow");
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
        assert_eq!(
            canvas.dims(),
            (1, 1),
            "degenerate sizes are clamped, not fatal"
        );
    }

    #[test]
    fn glow_is_brightest_at_its_centre_and_stays_inside_the_canvas() {
        let mut canvas = Canvas::new(32, 32);
        canvas.add_glow(16.0, 16.0, 8.0, [1.0, 1.0, 1.0], 1.0);
        assert!(canvas.buf[16 * 32 + 16][0] > canvas.buf[16 * 32 + 22][0]);
        assert_eq!(
            canvas.buf[0][0], 0.0,
            "the glow should not reach the corner"
        );
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
