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

/// How finely a hull's outline is measured, per subpixel and per axis, when
/// `--aa` is left alone. Nine samples, so ten levels of coverage.
///
/// Square rather than tall, because a canvas subpixel is roughly square — a
/// cell is about one by two and holds two subpixel rows — and a three-by-two
/// grid would sharpen the ship along one axis while softening it along the
/// other. Odd, so the grid contains the subpixel's own centre: that is the one
/// sample the rasteriser this replaced took, which keeps the brightest part of
/// a hairline plate on the subcolumn the old fill would have chosen.
///
/// Two is not enough. Four levels, no centre sample, and the tonemap makes the
/// *first* rung the biggest — a quarter of a hull colour already resolves to
/// about half the hull's brightness, so an edge fading in still arrives as a
/// step, where a ninth lands a third of the way up.
///
/// Four was compared against three on the shot this ships pictures of, and it
/// is very slightly the smoother: sixteen levels, and a first rung gentler
/// again. It was not taken, because what it buys at the bottom of the ramp it
/// pays for by dropping the centre sample, and because the difference between
/// three and four is a fraction of the difference between one and three. The
/// margin is small enough to be a matter of taste rather than of fact, which is
/// most of why `--aa` takes a number instead of an on and an off.
pub const HULL_SAMPLES: usize = 3;

/// Ceiling on `--aa`. It enters squared, like `--scale` does: the band is
/// `width * n * n` samples. Past a handful there is nothing left to buy.
pub const MAX_HULL_SAMPLES: usize = 8;

const _: () = assert!(
    HULL_SAMPLES >= 1 && HULL_SAMPLES <= MAX_HULL_SAMPLES,
    "the default sample count is outside the range the command line admits"
);

/// The curve `LENGTH_FALLOFF` describes, in one place.
///
/// `draw_streak` measures the length it passes here *after* clipping and
/// `draw_path` measures the whole polyline before it, which is a real
/// difference rather than an oversight — a bent streak is subdivided, and
/// measuring only the visible part of each piece would change the picture with
/// the subdivision. Only the shape of the falloff is shared.
fn spread(length: f32) -> f32 {
    1.0 + length * LENGTH_FALLOFF
}

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

/// One face of a hull, ready to paint: where its outline lies on the canvas,
/// and what colour it came out.
///
/// Everything else a [`crate::models`] plate carries — its depth, its shade —
/// is the caller's business, and settled by the time it gets here. Faces arrive
/// as a slice rather than one at a time, which is the whole of the design
/// rather than a convenience; the essay is on [`Canvas::fill_hull`].
pub struct Facet<'a> {
    pub points: &'a [(f32, f32)],
    pub color: [f32; 3],
}

/// One subsample of the band: what colour landed on it, and whether anything
/// did at all.
///
/// A flag rather than a test for black. Nothing in the fleet paints a plate
/// exactly zero — `AMBIENT` of any hull colour is not — but that is a
/// relationship nothing checks, and the failure mode of leaning on it is a hole
/// through the shadowed side of a ship.
#[derive(Clone, Copy)]
struct Sample {
    color: [f32; 3],
    covered: bool,
}

impl Sample {
    const EMPTY: Self = Self {
        color: [0.0; 3],
        covered: false,
    };
}

/// A face moved into sample space and ready for the band loop: its outline's
/// slice of the flattened point buffer, the colour to write, and the sample
/// rows it can reach — which lets a band skip a face it cannot see in two
/// comparisons rather than by walking its edges.
struct BandFace {
    at: (usize, usize),
    color: [f32; 3],
    rows: (f32, f32),
}

/// Where a convex outline crosses one sample row: the outermost two crossings,
/// which convexity makes the whole span — no sorting, no parity rule.
///
/// A crossing is worked out from the edge's own two endpoints taken in the
/// order their `y` puts them in, rather than the order the face happens to walk
/// them. Two faces sharing an edge therefore evaluate the identical expression
/// on the identical floats and agree to the bit about where it crosses a row,
/// so the seam between two plates of one hull cannot open a crack that neither
/// of them fills.
fn crossings(points: &[(f32, f32)], row: f32) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        let (upper, lower) = if a.1 <= b.1 { (a, b) } else { (b, a) };
        let span = lower.1 - upper.1;
        if row < upper.1 || row > lower.1 || span <= f32::EPSILON {
            continue;
        }
        let x = upper.0 + (lower.0 - upper.0) * (row - upper.1) / span;
        lo = lo.min(x);
        hi = hi.max(x);
    }
    (lo <= hi).then_some((lo, hi))
}

pub struct Canvas {
    width: usize,
    height: usize,
    buf: Vec<[f32; 3]>,
    /// How finely [`Self::fill_hull`] measures an outline, per axis.
    hull_samples: usize,
    /// One output row of subpixels, `hull_samples` sample rows deep: the whole
    /// of what `fill_hull` has in flight at once. It grows with the width alone
    /// and not with the area, which is the point of banding — the hull's
    /// bounding box can be the whole canvas, and at the largest terminal the
    /// flags admit that would be half a gigabyte to draw a ship eighteen
    /// subpixels long.
    band: Vec<Sample>,
    /// The hull's outline in sample space, flattened, with one `BandFace` per
    /// face naming its slice of it. Rebuilt every call and kept between them
    /// because a side-view frame does this forty times a second.
    hull_points: Vec<(f32, f32)>,
    hull_faces: Vec<BandFace>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self {
            width,
            height,
            buf: vec![[0.0; 3]; width * height],
            hull_samples: HULL_SAMPLES,
            band: vec![Sample::EMPTY; width * HULL_SAMPLES * HULL_SAMPLES],
            hull_points: Vec::new(),
            hull_faces: Vec::new(),
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// How finely [`Self::fill_hull`] measures an outline, in samples per
    /// subpixel on each axis. One is the hard-edged rasteriser this replaced —
    /// not approximately, exactly.
    pub fn set_hull_samples(&mut self, samples: usize) {
        self.hull_samples = samples.clamp(1, MAX_HULL_SAMPLES);
        let band = self.width * self.hull_samples * self.hull_samples;
        self.band.resize(band, Sample::EMPTY);
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
            // The band is cleared every time it is used, so stale samples are
            // not the hazard here — its *length* is. A canvas narrowed with a
            // band still sized for the old width indexes past the end of it.
            let band = width * self.hull_samples * self.hull_samples;
            self.band.resize(band, Sample::EMPTY);
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

    /// How far a streak's light would be spread along this segment: the factor
    /// [`Self::draw_streak`] is about to divide its per-sample brightness by.
    ///
    /// Public because of one caller with an unusual problem. The falloff exists
    /// so that a *star*, whose streak length is its own motion, spreads its
    /// light rather than burning a line through the frame — the length is the
    /// physics and dimming with it is the point. The engine trail is the other
    /// case: above light speed its reach is measured against the frame, so
    /// without dividing this back out the width of the terminal would decide
    /// how bright the drive burns, and the same flight would not look the same
    /// on two machines.
    ///
    /// Measured on the clipped segment, and answering 1.0 for the short-streak
    /// case, because both are what `draw_streak` will actually do — a caller
    /// dividing this out wants the number that is going to be applied, not the
    /// one the geometry suggests.
    pub fn streak_spread(&self, from: (f32, f32), to: (f32, f32)) -> f32 {
        let Some((from, to)) = self.clip(from, to) else {
            return 1.0;
        };
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 0.75 {
            return 1.0;
        }
        spread(length)
    }

    /// Draw one star's contribution: a line from where it was to where it is,
    /// brightening toward the head.
    pub fn draw_streak(&mut self, streak: &Streak) {
        self.streak_with_tail::<false>(streak);
    }

    /// The same segment, for a caller whose tail is where the light *runs out*
    /// rather than where it has been: the ramp goes to nothing instead of
    /// stopping at [`TAIL_BRIGHTNESS`].
    ///
    /// The floor is right for a star, whose tail is simply where it was a frame
    /// ago — light that stopped dead there would read as a dash with no star on
    /// it. The engine lance is the other case. It is aimed at the point its own
    /// direction vanishes at, and every bell on a ship shares that one point, so
    /// a floor puts a third of full brightness from every lane of every bell on
    /// the same subpixel: a bead hanging in the sky exactly where the exhaust
    /// was meant to have gone. Ramping to nothing is what lets the lance be
    /// aimed at the point at all — the sample that lands on it carries a ramp of
    /// exactly zero, so the margin that used to hold it back buys nothing.
    ///
    /// The ramp is measured on the *whole* streak here, and for the star path it
    /// is measured on what survived clipping. That is the whole of what
    /// separates the two, and it is not a detail of where. A lance abeam is
    /// stretched to the frame's diagonal and leaves by the edge, so its tail is
    /// off-screen; ramped over the clipped remainder it would fade to nothing at
    /// the edge of the picture, and a drive whose reach is the frame's would
    /// stop short of the frame on every terminal. Where the window happens to
    /// cut a plume is not a fact about the plume.
    pub fn draw_fading_streak(&mut self, streak: &Streak) {
        self.streak_with_tail::<true>(streak);
    }

    /// The body both of those share.
    ///
    /// Both ramps come out as `tail + lift * t` over the drawn segment, so the
    /// two differ in a pair of coefficients worked out once and not in anything
    /// the sampling loop does. The star path's pair are the constants the
    /// expression here used to hold literally, so it is the same multiply, the
    /// same add and the same bits it was, and no reference frame moved for the
    /// split.
    ///
    /// `FADING` is a const parameter rather than an argument so that each caller
    /// is monomorphised with its own coefficients folded in, and the star loop
    /// keeps the immediates it has always had instead of taking them from
    /// registers behind a branch. That is an argument from what the compiler is
    /// left able to do, not from a measurement: at 20 000 stars on 200x60 from
    /// the side, six interleaved runs put this at 22.14 ms of drawing against
    /// 22.26 before the split and 22.37 with a runtime flag, which is half a
    /// percent across the three and inside the run-to-run spread of any one of
    /// them. Measuring the builds in separate sittings said otherwise and said
    /// it consistently; interleave them before believing a number here.
    fn streak_with_tail<const FADING: bool>(&mut self, streak: &Streak) {
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
        let per_sample = streak.intensity / spread(length);
        // Where the drawn segment starts and finishes on the ramp. For a star
        // that is the whole of it, by the constants the ramp was written with.
        // For a fading one it is where clipping left the two ends of the *full*
        // streak, so a lance running out of the frame is as bright at the edge
        // as the plume is that far from its nozzle, rather than being faded out
        // by the window it is seen through.
        let (tail, lift) = if FADING {
            let full = (streak.to.0 - streak.from.0, streak.to.1 - streak.from.1);
            // The clipped length is at least 0.75, so the full one cannot be
            // zero and this cannot divide by it.
            let inv_full = 1.0 / (full.0 * full.0 + full.1 * full.1);
            let along = |p: (f32, f32)| {
                ((p.0 - streak.from.0) * full.0 + (p.1 - streak.from.1) * full.1) * inv_full
            };
            let (near, far) = (along(from), along(to));
            (near, far - near)
        } else {
            (TAIL_BRIGHTNESS, 1.0 - TAIL_BRIGHTNESS)
        };
        for i in 0..=steps {
            let t = i as f32 * inv_steps;
            // `from` is the tail, `to` the head: ramp brightness along it.
            let ramp = tail + lift * t;
            let x = (from.0 + dx * t).clamp(0.0, max_x);
            let y = (from.1 + dy * t).clamp(0.0, max_y);
            self.splat_inside(x, y, streak.color, per_sample * ramp);
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
        let per_sample = intensity / spread(total);
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

    /// Paint a hull: a set of opaque convex faces, covering whatever was under
    /// them, with the outline measured finer than a subpixel.
    ///
    /// The one thing in this module that does not add light. Everything else
    /// here accumulates, because everything else is light: a hundred streaks
    /// crossing a subpixel should pile up. A hull is not light, it is a solid
    /// object, and a star shining through a starship reads as a fault rather
    /// than as glass — so this covers, and what was behind it stays behind it.
    ///
    /// A subpixel the hull only partly stands on is written in proportion,
    /// `buf * (1 - cov) + colour * cov`, and that is the one place a hull and
    /// the sky behind it are ever mixed. It is not a hole in the rule above:
    /// the coverage is geometry rather than transparency, and it is counted in
    /// whole samples and divided, so a fully covered subpixel comes out at
    /// exactly one and exactly none of what was under it survives. It matters
    /// here more than the arithmetic suggests, because the subject is tiny — at
    /// the framing the shot opens on, a plate is one or two subpixels thick,
    /// and a hard edge on one of those does not move when the ship rolls. It
    /// sits still and then jumps a whole subpixel, which reads as crawling.
    ///
    /// **The whole hull arrives at once, and that is the design rather than a
    /// convenience.** Coverage composes only once per sample. Blend the faces
    /// in turn instead and every edge two of them share *inside* the hull is
    /// blended twice: two plates each covering half of the subpixel their
    /// shared edge runs down leave `(1 - ½)(1 - ½)` of the sky, in a line,
    /// through the middle of the ship. There is no per-face fix — the
    /// composition is what is wrong. Taken together, the far-to-near order the
    /// caller has already sorted them into can be applied *between the samples*
    /// of a subpixel, where it settles a seam the way it settles everything
    /// else. The runner-up was a coverage mask walked near to far, paying for
    /// each sample once; it costs less memory and inverts the painting order
    /// this module's whole hidden-surface story is told in.
    ///
    /// Convex is the caller's promise, and it is what lets a row of a face be
    /// filled between the outermost two edge crossings without sorting them.
    /// The order is the caller's promise too, and it is far to near.
    pub fn fill_hull(&mut self, faces: &[Facet<'_>]) {
        let n = self.hull_samples;
        let grid_w = self.width * n;
        let (max_u, max_v) = ((grid_w - 1) as f32, (self.height * n - 1) as f32);
        // `(x + 0.5) * n - 0.5`, spelled as a multiply and an add so that at
        // one sample a subpixel it is a multiply by one and an add of zero —
        // the identity for every finite float. That is what makes this *be* the
        // rasteriser it replaced at `--aa 1` rather than merely agree with it,
        // and `one_sample_a_subpixel_is_the_rasteriser_this_replaced` is what
        // holds it. Everything below then runs the old span arithmetic against
        // a grid `n` times finer, unchanged.
        let shift = (n - 1) as f32 * 0.5;

        self.hull_points.clear();
        self.hull_faces.clear();
        let (mut top, mut bottom) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
        for face in faces {
            // A face that cannot be measured is dropped and the rest of the
            // hull still drawn. Giving up on the whole ship because one plate
            // came out NaN turns a wrong subpixel into a missing starship.
            if face.points.len() < 3
                || !face.color.iter().all(|c| c.is_finite())
                || face
                    .points
                    .iter()
                    .any(|p| !p.0.is_finite() || !p.1.is_finite())
            {
                continue;
            }
            let start = self.hull_points.len();
            let (mut face_top, mut face_bottom) = (f32::INFINITY, f32::NEG_INFINITY);
            for p in face.points {
                let (u, v) = (p.0 * n as f32 + shift, p.1 * n as f32 + shift);
                self.hull_points.push((u, v));
                face_top = face_top.min(v);
                face_bottom = face_bottom.max(v);
                left = left.min(u);
                right = right.max(u);
            }
            top = top.min(face_top);
            bottom = bottom.max(face_bottom);
            self.hull_faces.push(BandFace {
                at: (start, self.hull_points.len()),
                color: face.color,
                rows: (face_top, face_bottom),
            });
        }
        if self.hull_faces.is_empty() || bottom < 0.0 || top > max_v || right < 0.0 || left > max_u
        {
            return;
        }

        // The sample columns anything below may write to. `round` is monotone,
        // so one row's span can never reach outside the hull's own bound;
        // clamping into it states that rather than changes it.
        let u_lo = left.round().max(0.0) as usize;
        let u_hi = (right.round().min(max_u)).max(0.0) as usize;
        if u_lo > u_hi {
            return;
        }
        // And the wider span that has to be *cleared*, which is not the same
        // pair of numbers however much it looks like it should be. The resolve
        // loop below reads whole subpixels — every sample of the column a
        // written sample falls in — so it reaches `u_lo % n` samples below
        // `u_lo` and `n - 1 - u_hi % n` above `u_hi`. Clearing only the written
        // span left those holding whatever the previous call put there, and a
        // stale sample still flagged `covered` hands its old colour and its
        // whole share of the coverage to the hull's outermost subpixel column:
        // at the default three samples, up to six ninths of one column taken
        // from the band row above it, or on a frame's first row from the frame
        // before. In range by construction — `u_hi <= grid_w - 1` gives
        // `u_hi / n <= width - 1`, so this comes to at most `grid_w - 1`.
        let clear_lo = (u_lo / n) * n;
        let clear_hi = (u_hi / n) * n + n - 1;
        let first = top.round().max(0.0) as usize;
        let last = (bottom.round().min(max_v)).max(0.0) as usize;
        let n2 = (n * n) as f32;

        for y in (first / n)..=(last / n) {
            for j in 0..n {
                let base = j * grid_w;
                self.band[base + clear_lo..=base + clear_hi].fill(Sample::EMPTY);
            }
            // Far to near, exactly as handed over: a sample belongs to the last
            // face that reached it, which is the nearest one that did.
            for face in &self.hull_faces {
                for j in 0..n {
                    let row = (y * n + j) as f32;
                    if row < face.rows.0 || row > face.rows.1 {
                        continue;
                    }
                    let (a, b) = face.at;
                    let Some((lo, hi)) = crossings(&self.hull_points[a..b], row) else {
                        continue;
                    };
                    if hi < 0.0 || lo > max_u {
                        continue;
                    }
                    // Rounded rather than floored and ceiled, so a plate seen
                    // almost edge-on still covers the one subcolumn it is
                    // standing in instead of falling through the gap between
                    // two of them. The old rule on a finer grid, and it is what
                    // keeps honest coverage from being a new way to vanish: a
                    // plate thinner than a sample comes out dim, never absent.
                    let start = (lo.round().max(0.0) as usize).max(u_lo);
                    let end = ((hi.round().min(max_u)).max(0.0) as usize).min(u_hi);
                    if start > end {
                        continue;
                    }
                    let base = j * grid_w;
                    self.band[base + start..=base + end].fill(Sample {
                        color: face.color,
                        covered: true,
                    });
                }
            }

            for x in (u_lo / n)..=(u_hi / n) {
                let (mut count, mut sum) = (0usize, [0.0f32; 3]);
                for j in 0..n {
                    let base = j * grid_w + x * n;
                    for s in &self.band[base..base + n] {
                        if s.covered {
                            count += 1;
                            for (channel, add) in sum.iter_mut().zip(s.color) {
                                *channel += add;
                            }
                        }
                    }
                }
                if count == 0 {
                    continue;
                }
                // Counted in whole samples and divided, rather than multiplied
                // by a reciprocal: a fully covered subpixel has to come out at
                // *exactly* one, or the sky behind the middle of the ship
                // survives at a part in ten million and the rule at the top of
                // this comment stops being true where it matters most.
                let cov = count as f32 / n2;
                let px = &mut self.buf[y * self.width + x];
                for (channel, add) in px.iter_mut().zip(sum) {
                    *channel = *channel * (1.0 - cov) + add / n2;
                }
            }
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

    /// The same pool of light, drawn as an ellipse — the wash inside the warp
    /// bubble, which is neither round nor necessarily square to the frame.
    ///
    /// A separate function rather than the one above generalised, and
    /// deliberately so. `add_glow` draws the cockpit's tunnel glare, whose
    /// bytes are pinned by three of the five reference frames; rewriting it as
    /// `add_glow_oval(.., r, r, ..)` is exactly the obviously-equivalent edit
    /// that moves a float by an ulp and repaints a sky it was never meant to
    /// touch. The duplication is four lines and it is cheaper than that.
    ///
    /// `turn` is which way the long axis lies, as `(cos, sin)`, because the
    /// camera outside can be swung round until the ship's track no longer runs
    /// across the frame. It is `(1.0, 0.0)` broadside and the rotation is then
    /// a multiply by one and an addition of zero. The arguments are grouped
    /// into pairs rather than listed flat only because clippy — which CI runs
    /// as an error — stops at seven of them.
    pub fn add_glow_oval(
        &mut self,
        center: (f32, f32),
        axes: (f32, f32),
        turn: (f32, f32),
        color: [f32; 3],
        strength: f32,
    ) {
        let ((cx, cy), (rx, ry)) = (center, axes);
        if strength <= 0.0 || rx <= 0.0 || ry <= 0.0 {
            return;
        }
        // The turned ellipse's own bounding box, which is what a support
        // function gives back: `√((rx·cos)² + (ry·sin)²)` across and the same
        // with the two swapped down. Squaring up to the long axis on both sides
        // would draw the same pixels — everything past the rim is skipped
        // below — and scan twice as many of them at this elongation.
        let hx = ((rx * turn.0).powi(2) + (ry * turn.1).powi(2)).sqrt();
        let hy = ((rx * turn.1).powi(2) + (ry * turn.0).powi(2)).sqrt();
        let x0 = ((cx - hx).floor().max(0.0)) as usize;
        let y0 = ((cy - hy).floor().max(0.0)) as usize;
        let x1 = ((cx + hx).ceil().min(self.width as f32 - 1.0)).max(0.0) as usize;
        let y1 = ((cy + hy).ceil().min(self.height as f32 - 1.0)).max(0.0) as usize;
        let (inv_x, inv_y) = (1.0 / rx, 1.0 / ry);

        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let (along, across) = (dx * turn.0 + dy * turn.1, dy * turn.0 - dx * turn.1);
                let d = ((along * inv_x).powi(2) + (across * inv_y).powi(2)).sqrt();
                if d >= 1.0 {
                    continue;
                }
                // The same quartic as the round one, and for the same reason:
                // anything flatter reads as a solid body hanging in space
                // rather than as light.
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
        // *squared* distance directly — taking a square root here only to
        // square it again is a root per subpixel that never had to be taken.
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

    /// The three channels at one subpixel, for the tests that do care what
    /// colour it came out — the drive's, which is the one thing here whose
    /// hue carries information rather than decoration.
    #[cfg(test)]
    pub fn color_at(&self, x: usize, y: usize) -> [f32; 3] {
        self.buf[y * self.width + x]
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

    /// The whole reason [`Canvas::splat`] exists beside `splat_inside` is the
    /// branch below the fast path, and until this test nothing in the tree ran
    /// it: every call site, all of them in tests, passed coordinates already on
    /// the canvas, so the checked path was documented and unexercised.
    #[test]
    fn a_splat_over_the_edge_keeps_only_the_taps_that_landed() {
        // Half a pixel past the right edge: two of the four taps are off the
        // canvas, and the two that remain carry the fractions that faced left.
        let mut canvas = Canvas::new(8, 8);
        canvas.splat(7.5, 4.0, [1.0, 1.0, 1.0], 4.0);
        assert!(
            (total_light(&canvas) - 6.0).abs() < 1e-5,
            "half a splat should have fallen off, not {}",
            total_light(&canvas) / 3.0
        );

        // The same weight, entirely outside — including the negative side,
        // where a cast to `usize` would wrap into the buffer rather than miss
        // it, and the far corner, where both axes are out at once.
        for (x, y) in [
            (-4.0, 4.0),
            (4.0, -4.0),
            (99.0, 4.0),
            (4.0, 99.0),
            (-9.0, -9.0),
        ] {
            let mut canvas = Canvas::new(8, 8);
            canvas.splat(x, y, [1.0, 1.0, 1.0], 4.0);
            assert_eq!(
                total_light(&canvas),
                0.0,
                "({x}, {y}) is off the canvas and still lit something"
            );
        }

        // And the guards at the top of it, which the fast path never reaches
        // because `draw_streak` has already dropped these.
        let mut canvas = Canvas::new(8, 8);
        for (x, y, weight) in [
            (f32::NAN, 4.0, 1.0),
            (4.0, f32::NAN, 1.0),
            (f32::INFINITY, 4.0, 1.0),
            (4.0, 4.0, 0.0),
            (4.0, 4.0, -1.0),
        ] {
            canvas.splat(x, y, [1.0, 1.0, 1.0], weight);
        }
        assert_eq!(
            total_light(&canvas),
            0.0,
            "a splat with nothing to add added something"
        );
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
    fn a_fading_streak_runs_out_where_a_star_streak_stops_dead() {
        // What the engine lance is drawn with, and why it is allowed to be
        // aimed at the point its own exhaust vanishes at. A star's tail is
        // where it was a frame ago and holds `TAIL_BRIGHTNESS`, which on a
        // lance puts a third of full brightness from every lane of every bell
        // on the one subpixel they all end on — a bead sitting exactly where
        // the exhaust was meant to have gone.
        let ends = |fading: bool| {
            let mut canvas = Canvas::new(64, 64);
            let s = streak((8.0, 32.0), (56.0, 32.0));
            if fading {
                canvas.draw_fading_streak(&s);
            } else {
                canvas.draw_streak(&s);
            }
            (canvas.light_at(8, 32), canvas.light_at(56, 32))
        };
        let (star_tail, star_head) = ends(false);
        let (fading_tail, fading_head) = ends(true);
        assert_eq!(
            fading_tail, 0.0,
            "a fading streak left {fading_tail} at the end it runs out at"
        );
        assert!(
            (star_tail - star_head * TAIL_BRIGHTNESS).abs() < 1e-6,
            "a star's tail is {star_tail}, not {TAIL_BRIGHTNESS} of its head"
        );
        assert!(
            (fading_head - star_head).abs() < 1e-6,
            "the two disagree about the head: {fading_head} against {star_head}"
        );
    }

    #[test]
    fn a_fading_streak_is_ramped_by_its_own_length_and_not_by_the_window() {
        // The bug this is here for. A lit warp drive's lance is stretched to
        // the frame's diagonal and leaves by the edge, so from the beam its
        // tail is off-screen. Ramped over what survived clipping it faded to
        // nothing at the edge of the picture instead of at the end of the
        // plume, and the drive stopped short of the frame on every terminal —
        // which is what `a_lit_warp_drive_trails_off_the_edge_of_the_frame`
        // caught. Where the window cuts a plume is not a fact about the plume.
        //
        // Drawn the way `draw_trail` draws it, `Canvas::streak_spread` divided
        // back out, so what is being compared is the ramp and not the length
        // falloff — which is measured on the clipped segment by design and
        // would otherwise be the whole of the difference.
        let lance = |canvas: &mut Canvas, from, to| {
            let held = canvas.streak_spread(from, to);
            canvas.draw_fading_streak(&Streak {
                intensity: held,
                ..streak(from, to)
            });
        };
        let mut cut = Canvas::new(64, 64);
        lance(&mut cut, (-200.0, 32.0), (56.0, 32.0));
        let mut whole = Canvas::new(320, 64);
        lance(&mut whole, (56.0, 32.0), (312.0, 32.0));
        // The same place on the plume, reached two ways: subpixel 0 of the
        // clipped canvas is subpixel 256 of the one with room for all of it.
        let (edge, same) = (cut.light_at(0, 32), whole.light_at(256, 32));
        assert!(edge > 0.0, "the lance faded out before it reached the edge");
        assert!(
            (edge - same).abs() < same * 0.05,
            "the frame edge got {edge} where the unclipped plume has {same}"
        );
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
    fn a_streak_divided_by_its_own_spread_keeps_its_brightness() {
        // What `streak_spread` promises the engine trail: multiply an intensity
        // by it and the head comes out the same however long the streak is, so
        // a caller whose length is chosen by the frame rather than by the
        // physics can state a brightness and get it.
        //
        // Pinned against `draw_streak`'s real arithmetic rather than against
        // the formula, because agreeing with the formula is not the property —
        // agreeing with the division that is actually applied is, and the two
        // measure their lengths differently either side of the clip.
        let head_of = |from: (f32, f32), to: (f32, f32)| {
            let mut canvas = Canvas::new(128, 8);
            let held = canvas.streak_spread(from, to);
            canvas.draw_streak(&Streak {
                from,
                to,
                color: [1.0; 3],
                intensity: 0.5 * held,
            });
            canvas.buf[4 * 128 + to.0 as usize][0]
        };
        let short = head_of((60.0, 4.0), (66.0, 4.0));
        let long = head_of((10.0, 4.0), (110.0, 4.0));
        // And one running well off the canvas, which is the trail's own case:
        // the spread has to answer for the part that lands, not the part asked
        // for, or the compensation overshoots by whatever was clipped away.
        let clipped = head_of((-400.0, 4.0), (110.0, 4.0));
        assert!(
            (long - short).abs() < short * 0.05,
            "a hundred subpixels came out {long} against six at {short}"
        );
        assert!(
            (clipped - short).abs() < short * 0.05,
            "a streak mostly off the canvas came out {clipped} against {short}"
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
    fn a_fill_covers_what_was_under_it_rather_than_adding_to_it() {
        // The only thing here that is not light. A hull painted additively over
        // a starfield is a hologram of a hull: the bright streak behind it goes
        // on shining through, and the eye reads glass.
        let mut canvas = Canvas::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                canvas.splat(x as f32, y as f32, [1.0; 3], 4.0);
            }
        }
        let square = [(8.0, 8.0), (20.0, 8.0), (20.0, 20.0), (8.0, 20.0)];
        canvas.fill_hull(&[Facet {
            points: &square,
            color: [0.25, 0.25, 0.25],
        }]);

        // Exact, and not by luck, now that a covered subpixel is the mean of
        // `n²` samples rather than a value written straight in. The coverage is
        // counted in whole samples and divided, so a full subpixel is at
        // *exactly* one and takes exactly all of what was under it; and `n²`
        // copies of a power of two sum and divide back exactly whatever `n` is.
        // Which is worth saying out loud, so that the next person to move
        // `HULL_SAMPLES` knows this assertion is holding for a reason.
        assert_eq!(
            canvas.buf[14 * 32 + 14],
            [0.25; 3],
            "the fill did not cover"
        );
        assert_eq!(
            canvas.buf[2 * 32 + 2],
            [4.0; 3],
            "it covered the wrong thing"
        );
        assert!(
            total_light(&canvas) < 32.0 * 32.0 * 3.0 * 4.0,
            "an opaque fill has to be able to take light away"
        );

        // And the honest general statement of the same thing: a colour that is
        // not a power of two comes back within a rounding of itself. What is
        // lost is an ulp, downstream of which sits a 1024-entry tonemap and a
        // terminal with eight bits to say it in.
        let mut canvas = Canvas::new(32, 32);
        canvas.fill_hull(&[Facet {
            points: &square,
            color: [0.3, 0.7, 0.1],
        }]);
        for (got, want) in canvas.buf[14 * 32 + 14].iter().zip([0.3, 0.7, 0.1]) {
            assert!(
                (got - want).abs() < 1e-6,
                "a covered subpixel came out {got} rather than {want}"
            );
        }
    }

    #[test]
    fn a_partly_covered_subpixel_lands_between_the_hull_and_the_sky() {
        // What has actually changed, and the one place a hull and the sky
        // behind it are ever mixed. The edge used to fall on one side or the
        // other of a subpixel and jump between them.
        //
        // Swept across the whole of subpixel 20 — which spans 19.5 to 20.5 —
        // rather than parked at one position, so this says nothing about where
        // the sample grid happens to sit.
        let mut seen = Vec::new();
        for step in 0..=12 {
            let mut canvas = Canvas::new(32, 32);
            for y in 0..32 {
                for x in 0..32 {
                    canvas.splat(x as f32, y as f32, [1.0; 3], 4.0);
                }
            }
            let right = 19.5 + step as f32 / 12.0;
            let square = [(8.0, 8.0), (right, 8.0), (right, 20.0), (8.0, 20.0)];
            canvas.fill_hull(&[Facet {
                points: &square,
                color: [0.25; 3],
            }]);
            seen.push(canvas.buf[14 * 32 + 20][0]);
        }
        assert!(
            seen.iter().any(|&v| v > 0.25 && v < 4.0),
            "an edge crossing a subpixel never left it between the hull and \
             the sky, so it is still landing on one side or the other: {seen:?}"
        );
        assert!(
            seen.windows(2).all(|w| w[1] <= w[0]),
            "a hull advancing over a subpixel did not darken it evenly: {seen:?}"
        );
    }

    #[test]
    fn a_fill_stays_inside_its_own_outline() {
        let mut canvas = Canvas::new(64, 64);
        let triangle = [(10.0, 10.0), (50.0, 12.0), (30.0, 45.0)];
        canvas.fill_hull(&[Facet {
            points: &triangle,
            color: [1.0, 0.0, 0.0],
        }]);
        // Inside, and outside on each side of it. The outside probes are all
        // fifteen subpixels or more clear of the nearest edge, so a fringe a
        // subpixel wide has no bearing on them.
        assert_eq!(canvas.buf[20 * 64 + 30][0], 1.0);
        for (x, y) in [(2usize, 2usize), (60, 60), (12, 40), (50, 40), (30, 60)] {
            assert_eq!(canvas.buf[y * 64 + x][0], 0.0, "it leaked to ({x}, {y})");
        }
    }

    #[test]
    fn a_fill_of_nonsense_paints_nothing_and_panics_at_nothing() {
        let mut canvas = Canvas::new(32, 16);
        for points in [
            vec![],
            vec![(1.0, 1.0)],
            vec![(1.0, 1.0), (5.0, 5.0)],
            vec![(f32::NAN, 0.0), (5.0, 5.0), (1.0, 8.0)],
            vec![(1e9, 1e9), (-1e9, 4.0), (2.0, 2.0)],
            vec![(-50.0, -50.0), (-10.0, -50.0), (-30.0, -20.0)],
            vec![(0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
        ] {
            canvas.fill_hull(&[Facet {
                points: &points,
                color: [1.0; 3],
            }]);
        }
        let triangle = [(1.0, 1.0), (9.0, 1.0), (5.0, 9.0)];
        canvas.fill_hull(&[Facet {
            points: &triangle,
            color: [f32::NAN; 3],
        }]);
        assert_eq!(canvas.buf.len(), 32 * 16, "the buffer must not have grown");
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));

        // A polygon hanging off every edge covers what it should and no more.
        let corner = [(-20.0, -20.0), (60.0, -20.0), (60.0, 40.0)];
        canvas.fill_hull(&[Facet {
            points: &corner,
            color: [0.5; 3],
        }]);
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn a_face_of_nonsense_does_not_take_the_rest_of_the_hull_with_it() {
        // The whole hull arrives in one call now, so a face that cannot be
        // measured has to be dropped on its own. Giving up on the call would
        // turn one wrong subpixel into a missing starship.
        let mut canvas = Canvas::new(32, 16);
        let bad = [(f32::NAN, 1.0), (9.0, 1.0), (5.0, 9.0)];
        let good = [(12.0, 2.0), (24.0, 2.0), (24.0, 12.0), (12.0, 12.0)];
        canvas.fill_hull(&[
            Facet {
                points: &bad,
                color: [1.0; 3],
            },
            Facet {
                points: &good,
                color: [0.5; 3],
            },
        ]);
        assert_eq!(
            canvas.buf[7 * 32 + 18],
            [0.5; 3],
            "the good face was lost with the bad one"
        );
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn one_sample_a_subpixel_is_the_rasteriser_this_replaced() {
        // The property the whole design is arranged around, and the reason the
        // sample grid is entered by scaling the coordinates rather than by
        // offsetting them: at one sample the transform is a multiply by one and
        // an add of zero, which is the identity for every finite float.
        //
        // The oracle below is the body of the `fill_convex` this replaced,
        // copied out before it was deleted. It is kept here deliberately —
        // removing a rasteriser must not also remove the thing it was being
        // checked against.
        fn fill_convex(canvas: &mut Canvas, points: &[(f32, f32)], color: [f32; 3]) {
            if points.len() < 3
                || !color.iter().all(|c| c.is_finite())
                || points.iter().any(|p| !p.0.is_finite() || !p.1.is_finite())
            {
                return;
            }
            let (mut top, mut bottom) = (f32::INFINITY, f32::NEG_INFINITY);
            for p in points {
                top = top.min(p.1);
                bottom = bottom.max(p.1);
            }
            let (max_x, max_y) = (canvas.max_x(), canvas.max_y());
            if bottom < 0.0 || top > max_y {
                return;
            }
            let first = top.round().max(0.0) as usize;
            let last = (bottom.round().min(max_y)).max(0.0) as usize;
            for y in first..=last {
                let row = y as f32;
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for i in 0..points.len() {
                    let (a, b) = (points[i], points[(i + 1) % points.len()]);
                    let (upper, lower) = if a.1 <= b.1 { (a, b) } else { (b, a) };
                    let span = lower.1 - upper.1;
                    if row < upper.1 || row > lower.1 || span <= f32::EPSILON {
                        continue;
                    }
                    let x = upper.0 + (lower.0 - upper.0) * (row - upper.1) / span;
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
                if lo > hi || hi < 0.0 || lo > max_x {
                    continue;
                }
                let start = lo.round().max(0.0) as usize;
                let end = (hi.round().min(max_x)).max(0.0) as usize;
                let width = canvas.width;
                canvas.buf[y * width + start..=y * width + end].fill(color);
            }
        }

        let cases: [Vec<(f32, f32)>; 6] = [
            // Axis-aligned, on the grid.
            vec![(8.0, 8.0), (24.0, 8.0), (24.0, 20.0), (8.0, 20.0)],
            // Slanted, and off it by fractions.
            vec![(9.3, 6.7), (27.8, 11.2), (22.4, 21.9), (6.1, 17.5)],
            // A plate one subpixel thick, seen almost edge-on: the case the
            // round-to-nearest span rule exists for.
            vec![(5.0, 4.2), (28.0, 5.9), (28.0, 6.3), (5.0, 4.6)],
            // Hanging off two edges.
            vec![(-6.0, -3.0), (14.0, -3.0), (14.0, 9.0), (-6.0, 9.0)],
            // Hanging off the far corner.
            vec![(20.0, 14.0), (40.0, 14.0), (40.0, 30.0), (20.0, 30.0)],
            // Nowhere near the canvas at all.
            vec![(-90.0, -90.0), (-70.0, -90.0), (-70.0, -70.0)],
        ];
        for points in &cases {
            let color = [0.3, 0.55, 0.8];
            let mut want = Canvas::new(32, 24);
            fill_convex(&mut want, points, color);

            let mut got = Canvas::new(32, 24);
            got.set_hull_samples(1);
            got.fill_hull(&[Facet { points, color }]);

            assert_eq!(
                got.buf, want.buf,
                "at one sample a subpixel this has to be the old rasteriser, \
                 not an approximation of it: {points:?}"
            );
        }
    }

    #[test]
    fn two_plates_that_share_an_edge_paint_the_rectangle_they_make() {
        // The seam. Blended one at a time, two plates each covering half of the
        // subpixel their shared edge runs down leave a quarter of the sky in a
        // line through the middle of the ship; composed together at sample
        // resolution they are indistinguishable from the single shape they add
        // up to. Bit for bit, because there is nothing here to be approximate
        // about — the same samples are covered either way.
        let sky = |canvas: &mut Canvas| {
            for y in 0..32 {
                for x in 0..48 {
                    canvas.splat(x as f32, y as f32, [1.0; 3], 3.0);
                }
            }
        };
        // A seam down the frame, one across it, and a slanted one — the last
        // given to the two faces in opposite winding order, which is how a
        // closed hull actually presents a shared edge. The split points sit
        // exactly on the whole shape's own edges in every case, so the two
        // sides really do add up to it rather than nearly.
        /// The two halves and the shape they add up to.
        type Split = ([(f32, f32); 4], [(f32, f32); 4], [(f32, f32); 4]);
        let cases: [Split; 4] = [
            (
                [(8.0, 6.0), (21.5, 6.0), (21.5, 24.0), (8.0, 24.0)],
                [(21.5, 6.0), (37.0, 6.0), (37.0, 24.0), (21.5, 24.0)],
                [(8.0, 6.0), (37.0, 6.0), (37.0, 24.0), (8.0, 24.0)],
            ),
            (
                [(8.0, 6.0), (37.0, 6.0), (37.0, 16.25), (8.0, 16.25)],
                [(8.0, 16.25), (37.0, 16.25), (37.0, 24.0), (8.0, 24.0)],
                [(8.0, 6.0), (37.0, 6.0), (37.0, 24.0), (8.0, 24.0)],
            ),
            (
                [(8.0, 6.0), (20.0, 6.0), (24.0, 24.0), (8.0, 24.0)],
                [(20.0, 6.0), (37.0, 6.0), (37.0, 24.0), (24.0, 24.0)],
                [(8.0, 6.0), (37.0, 6.0), (37.0, 24.0), (8.0, 24.0)],
            ),
            (
                [(8.0, 6.0), (20.0, 6.0), (24.0, 24.0), (8.0, 24.0)],
                [(24.0, 24.0), (20.0, 6.0), (37.0, 6.0), (37.0, 24.0)],
                [(8.0, 6.0), (37.0, 6.0), (37.0, 24.0), (8.0, 24.0)],
            ),
        ];
        for (left, right, whole) in cases {
            let color = [0.2, 0.24, 0.31];
            let mut split = Canvas::new(48, 32);
            sky(&mut split);
            split.fill_hull(&[
                Facet {
                    points: &left,
                    color,
                },
                Facet {
                    points: &right,
                    color,
                },
            ]);

            let mut one = Canvas::new(48, 32);
            sky(&mut one);
            one.fill_hull(&[Facet {
                points: &whole,
                color,
            }]);

            assert_eq!(
                split.buf, one.buf,
                "the sky came through the seam between two plates of one hull"
            );
        }
    }

    #[test]
    fn a_hull_is_drawn_the_same_whatever_the_band_last_held() {
        // Regression: the band was cleared over the span an outline is
        // *written* to and read over the wider one a whole subpixel spans, so
        // the samples between the two arrived in the sum still carrying the
        // previous call's coverage. A hull drawn after another one wore a
        // fringe of it down both sides of its own bounding box — and on the
        // first band row of a frame, that previous call was the frame before,
        // which is the whole of why this is worth a test rather than an
        // inspection.
        let sky = |canvas: &mut Canvas| {
            for y in 0..32 {
                for x in 0..48 {
                    canvas.splat(x as f32, y as f32, [1.0; 3], 3.0);
                }
            }
        };
        // Neither edge of the second hull lands on a subpixel boundary, which
        // is what leaves it a fringe for the first one's samples to survive in.
        // A hull whose box happens to be aligned on both sides is the one case
        // that was never wrong, so it is the wrong one to test with.
        let first = [(1.0, 1.0), (46.0, 1.0), (46.0, 30.0), (1.0, 30.0)];
        let second = [(8.0, 6.0), (20.0, 6.0), (20.0, 24.0), (8.0, 24.0)];
        let color = [0.2, 0.24, 0.31];

        // The buffer is put back to exactly what the lone run starts from, so
        // the band is the only thing left that differs between the two.
        let mut after = Canvas::new(48, 32);
        after.fill_hull(&[Facet {
            points: &first,
            color: [0.9, 0.12, 0.1],
        }]);
        after.clear();
        sky(&mut after);
        after.fill_hull(&[Facet {
            points: &second,
            color,
        }]);

        let mut alone = Canvas::new(48, 32);
        sky(&mut alone);
        alone.fill_hull(&[Facet {
            points: &second,
            color,
        }]);

        assert_eq!(
            after.buf, alone.buf,
            "a hull came out differently for having been drawn after another one"
        );
    }

    #[test]
    fn an_outline_that_moves_by_a_third_of_a_subpixel_moves_on_screen() {
        // What anti-aliasing is *for*, stated as a property. Under the old
        // rasteriser this sweep took two values and the step between them was
        // the flicker: a plate one or two subpixels thick does not move when
        // the ship rolls, it sits still and then jumps a whole subpixel.
        let mut seen: Vec<f32> = Vec::new();
        for step in 0..=12 {
            // Across the whole of subpixel 10, which spans 9.5 to 10.5.
            let edge = 9.5 + step as f32 / 12.0;
            let mut canvas = Canvas::new(24, 8);
            let quad = [(2.0, 1.0), (edge, 1.0), (edge, 6.0), (2.0, 6.0)];
            canvas.fill_hull(&[Facet {
                points: &quad,
                color: [1.0; 3],
            }]);
            seen.push(canvas.buf[4 * 24 + 10][0]);
        }
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]),
            "an edge sweeping one way lit the subpixel unevenly: {seen:?}"
        );
        let mut distinct = seen.clone();
        distinct.sort_by(f32::total_cmp);
        distinct.dedup();
        // One rung per sample, which is the most an `n` by `n` grid can offer
        // and the whole of what the sample count buys.
        assert!(
            distinct.len() >= HULL_SAMPLES,
            "a subpixel an edge crossed took {} values rather than \
             {HULL_SAMPLES}, so the edge is still snapping rather than \
             moving: {seen:?}",
            distinct.len()
        );
    }

    #[test]
    fn a_hull_drawn_straight_after_a_resize_still_fits_the_canvas() {
        // The band is sized from the width, so a canvas narrowed between two
        // frames leaves it indexing past the end of itself.
        let mut canvas = Canvas::new(96, 32);
        let wide = [(0.0, 4.0), (95.0, 4.0), (95.0, 20.0), (0.0, 20.0)];
        canvas.fill_hull(&[Facet {
            points: &wide,
            color: [0.5; 3],
        }]);
        canvas.resize(20, 12);
        canvas.fill_hull(&[Facet {
            points: &wide,
            color: [0.5; 3],
        }]);
        assert_eq!(canvas.buf.len(), 20 * 12);
        assert!(canvas.buf.iter().all(|p| p.iter().all(|v| v.is_finite())));

        // And the other way, with the sample count moved under it as well.
        canvas.set_hull_samples(MAX_HULL_SAMPLES);
        canvas.resize(200, 60);
        canvas.fill_hull(&[Facet {
            points: &wide,
            color: [0.5; 3],
        }]);
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
