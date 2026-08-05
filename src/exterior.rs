//! The sky as seen from alongside the ship.
//!
//! [`crate::starfield`] holds a cone of stars opening forward from the canopy,
//! which is exactly what the view out of the cockpit needs and exactly the
//! wrong shape to look at side-on: there is nothing at all behind the ship, so
//! the trailing half of an exterior frame would be empty space. This is the
//! same idea rebuilt for a camera parked off the ship's beam — a band of sky
//! the ship flies *through* rather than a cone it flies *into*.
//!
//! Stars live in the side camera's own space, so the projection, the streaks
//! and the tonemap are all the ones the cockpit already uses. The only real
//! difference is which way the universe moves: the ship travels toward screen
//! right, so the sky streams left, and depth is the range out from the camera
//! rather than the distance down the nose. Travel therefore never changes a
//! star's depth — which means a star that runs off the trailing edge has not
//! flown past anything, and belongs back out in front at the range it had.

use crate::canvas::Canvas;
use crate::lens::{Image, Lens};
use crate::starfield::{shift_color, Camera, Streak, CLASSES};
use crate::view::{HULL_REACH, SHIP_DISTANCE};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::f32::consts::TAU;

/// Nearest a star may pass the side camera.
///
/// This is the whole of the exterior view's depth sorting: it is far enough out
/// that nothing can ever come between the eye and the hull, so the ship can be
/// drawn over the sky without a depth buffer and without being wrong.
pub const Z_NEAR: f32 = 18.0;
/// The far wall of the band. Stars fade up from nothing here rather than
/// popping in.
pub const Z_FAR: f32 = 320.0;

const _: () = assert!(
    Z_NEAR > SHIP_DISTANCE + HULL_REACH,
    "a star could pass in front of the ship, and nothing here would sort it"
);

/// How far past the frame the band reaches, as a multiple of the visible area.
/// The margin is the run-up a star gets: it comes round off-camera and sweeps
/// into view rather than appearing in it.
const SPAWN_MARGIN: f32 = 1.15;

/// How sharply a star dims with range — flatter than reality, for the same
/// reason the cockpit's is: honest inverse-square leaves all but the nearest
/// handful invisible.
const DEPTH_FALLOFF: f32 = 1.3;
/// Intrinsic brightness is lopsided: a sky is mostly faint pinpricks with a
/// handful of standouts. Cubing a uniform sample gives that shape.
const MAGNITUDE_FLOOR: f32 = 0.14;

/// Subpixels of arc per piece when a streak is chopped up to be bent. A curve
/// drawn as one straight segment between its bent ends cuts across the very
/// region doing the bending.
const ARC_STEP: f32 = 5.0;
/// Ceiling on that subdivision, so a streak stretching several screen widths at
/// full warp cannot turn into an unbounded amount of work.
const MAX_ARCS: usize = 24;
/// Below this the counter-image is not worth drawing. It is most of the pool —
/// the further a star is from the lens the fainter its counter-image — and what
/// is left of it lands in an invisible pile at the centre.
const FAINTEST_COUNTER_IMAGE: f32 = 0.03;

#[derive(Debug, Clone, Copy)]
struct Star {
    /// In the side camera's space: `x` runs along the ship's track, `y` is
    /// vertical, `z` is the range out from the camera.
    pos: [f32; 3],
    /// Where it projected to last frame, if it had a place to project from.
    prev: Option<(f32, f32)>,
    class: usize,
    magnitude: f32,
    phase: f32,
}

/// A band of sky the ship flies through, seen side-on.
pub struct ExteriorField {
    stars: Vec<Star>,
    rng: StdRng,
    /// Half-extent of the band in screen space, margin included.
    bound: (f32, f32),
    focal: f32,
    /// Scratch for bending a streak, reused across every star of every frame.
    source: Vec<(f32, f32)>,
    bent: Vec<(f32, f32)>,
}

impl ExteriorField {
    pub fn new(count: usize, seed: u64, cam: &Camera) -> Self {
        let mut field = Self {
            stars: Vec::with_capacity(count),
            rng: StdRng::seed_from_u64(seed),
            bound: bound_for(cam),
            focal: cam.focal.max(f32::MIN_POSITIVE),
            source: Vec::with_capacity(MAX_ARCS + 1),
            bent: Vec::with_capacity(MAX_ARCS + 1),
        };
        for _ in 0..count {
            let star = field.spawn();
            field.stars.push(star);
        }
        field
    }

    pub fn len(&self) -> usize {
        self.stars.len()
    }

    /// Always false in practice — `resize_pool` keeps at least one star — but a
    /// pool with a length is expected to answer this too.
    pub fn is_empty(&self) -> bool {
        self.stars.is_empty()
    }

    /// Adapt to a new canvas size, keeping the stars already in flight.
    ///
    /// Every trail is dropped: a resize moves the whole band at once, and a
    /// streak drawn from where a star used to be under the old projection to
    /// where it is under the new one is a scratch across the frame.
    pub fn retarget(&mut self, cam: &Camera) {
        self.bound = bound_for(cam);
        self.focal = cam.focal.max(f32::MIN_POSITIVE);
        let (half_width, focal) = (self.bound.0, self.focal);
        for star in &mut self.stars {
            star.pos[0] = fold(star.pos[0], band(half_width, focal, star.pos[2])).0;
            star.prev = None;
        }
    }

    /// Grow or shrink the pool, keeping the stars already in flight.
    pub fn resize_pool(&mut self, count: usize) {
        let count = count.max(1);
        while self.stars.len() > count {
            self.stars.pop();
        }
        while self.stars.len() < count {
            let star = self.spawn();
            self.stars.push(star);
        }
    }

    /// Make a star. As in the cockpit field the position is sampled on the
    /// screen and back-projected through the range, which keeps apparent
    /// density even at every distance: sampling a world-space volume instead
    /// would crowd the far wall and starve the near one.
    fn spawn(&mut self) -> Star {
        let z = self.rng.random_range(Z_NEAR..Z_FAR);
        let (hw, hh) = self.bound;
        let (sx, sy) = (
            self.rng.random_range(-hw..hw),
            self.rng.random_range(-hh..hh),
        );

        let scale = z / self.focal;
        let u: f32 = self.rng.random_range(0.0..1.0);
        Star {
            pos: [sx * scale, sy * scale, z],
            prev: None,
            class: self.pick_class(),
            magnitude: MAGNITUDE_FLOOR + (1.0 - MAGNITUDE_FLOOR) * u * u * u,
            phase: self.rng.random_range(0.0..TAU),
        }
    }

    fn pick_class(&mut self) -> usize {
        let total: f32 = CLASSES.iter().map(|c| c.weight).sum();
        let mut pick: f32 = self.rng.random_range(0.0..total);
        for (i, class) in CLASSES.iter().enumerate() {
            pick -= class.weight;
            if pick <= 0.0 {
                return i;
            }
        }
        CLASSES.len() - 1
    }

    /// Fly the band past the camera.
    ///
    /// The ship's steering is deliberately not an argument. The camera rides
    /// with the ship rather than being bolted to the sky, so a turn swings the
    /// *hull* in frame and leaves the stars streaming the way they were — the
    /// view from a wingman's canopy, where the horizon does not tip because
    /// your neighbour rolled.
    pub fn update(&mut self, dt: f32, speed: f32, cam: &Camera) {
        let travel = speed * dt;
        let focal = self.focal;
        let half_width = self.bound.0;
        let (bank_sin, bank_cos) = cam.bank.sin_cos();

        for star in &mut self.stars {
            star.prev = cam.project(star.pos);

            // Range never changes — travel is along the track — so the only way
            // out of the band is off the trailing edge, and the honest place
            // for a star the ship has just overtaken is back out in front at
            // the range it already had.
            let z = star.pos[2];
            let (folded, shift) = fold(star.pos[0] - travel, band(half_width, focal, z));
            star.pos[0] = folded;

            // The fold is an exact whole number of band widths, so the trail
            // can come with it: the star still draws the segment it actually
            // swept, and the part that has gone off the edge is clipped away.
            // Blanking the trail instead — which is what the cockpit field does
            // for a star swung out by a turn — would leave the nearest stars
            // drawing bare points every few frames, and a sky that flickers
            // between streaks and dots reads as static rather than as speed.
            if shift != 0.0 {
                if let Some(p) = &mut star.prev {
                    let d = shift * focal / z;
                    p.0 += d * bank_cos;
                    p.1 += d * bank_sin;
                }
            }
        }
    }

    /// Turn the band into drawable segments for this frame, before any bending.
    ///
    /// `warp` is the 0..=1 superluminal ramp: it lengthens the streaks and
    /// drives the Doppler shift. `time` only feeds the sublight twinkle.
    pub fn streaks<'a>(
        &'a self,
        cam: &'a Camera,
        warp: f32,
        time: f64,
    ) -> impl Iterator<Item = Streak> + 'a {
        let stretch = 1.0 + warp * warp * 5.0;
        let doppler = warp * 0.9;
        let twinkle_amt = (1.0 - warp * 3.0).clamp(0.0, 1.0) * 0.22;
        // Folded once per frame in `f64` so the per-star `sin` can stay `f32`
        // without the phase going coarse after days aloft.
        let twinkle_phase = (time * 2.3).rem_euclid(std::f64::consts::TAU) as f32;

        self.stars.iter().filter_map(move |star| {
            let to = cam.project(star.pos)?;
            // Stretch backward along the direction of travel, exactly as the
            // cockpit does: a star close to the camera sweeps further per frame
            // and so draws the longer streak, with no special casing.
            let from = match star.prev {
                Some(prev) => (
                    to.0 + (prev.0 - to.0) * stretch,
                    to.1 + (prev.1 - to.1) * stretch,
                ),
                None => to,
            };

            let class = &CLASSES[star.class];
            let z = star.pos[2];
            // Reaches zero exactly at the far wall, with zero slope, so stars
            // fade up out of nothing instead of blinking into existence.
            let depth = (1.0 - (z - Z_NEAR) / (Z_FAR - Z_NEAR)).clamp(0.0, 1.0);
            let twinkle = 1.0 + twinkle_amt * (twinkle_phase + star.phase).sin();
            let intensity = class.luminosity * star.magnitude * depth.powf(DEPTH_FALLOFF) * twinkle;
            if intensity <= 0.0 {
                return None;
            }

            // Doppler about the direction of travel rather than about the
            // vanishing point: from out here the sky the ship is running into
            // blues, and the sky it is leaving behind reddens. Dead abeam — the
            // middle of the frame — is neither, which is the half-way point of
            // the ramp `shift_color` already takes.
            let length = (star.pos[0].powi(2) + star.pos[1].powi(2) + star.pos[2].powi(2)).sqrt();
            let forward = if length > f32::EPSILON {
                (0.5 + 0.5 * star.pos[0] / length).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let color = shift_color(class.rgb, forward, doppler);

            Some(Streak {
                from,
                to,
                color,
                intensity,
            })
        })
    }

    /// Draw the sky into `canvas`, bent by `lens` where it reaches.
    ///
    /// A star the lens does not reach takes the same path it would if none of
    /// this existed, which is what keeps a sublight frame costing what it
    /// always did and keeps engaging the drive from visibly re-rendering the
    /// whole field instead of bending part of it.
    pub fn draw(&mut self, canvas: &mut Canvas, cam: &Camera, warp: f32, time: f64, lens: &Lens) {
        // Taken out and put back so the streak iterator can borrow the pool
        // while the scratch buffers are being written.
        let mut source = std::mem::take(&mut self.source);
        let mut bent = std::mem::take(&mut self.bent);

        for streak in self.streaks(cam, warp, time) {
            if !lens.bends(streak.from, streak.to) {
                canvas.draw_streak(&streak);
                continue;
            }
            subdivide(&streak, &mut source);
            for image in [Image::Primary, Image::Secondary] {
                // The head is where the star actually is, so its magnification
                // is the one that speaks for the whole streak.
                let gain = lens.map(streak.to, image).gain;
                if gain < FAINTEST_COUNTER_IMAGE {
                    continue;
                }
                bent.clear();
                let mut swallowed = false;
                for p in &source {
                    let at = lens.map(*p, image).at;
                    // A counter-image that dips inside the bubble is dropped
                    // whole rather than being cut into the runs that survive.
                    // What that costs is a slightly soft inner edge to the
                    // ring; what it buys is that the disc where the ship sits
                    // is empty, which is the entire point of drawing this.
                    if lens.shadowed(at) {
                        swallowed = true;
                        break;
                    }
                    match bent.last() {
                        // Follow the sweep around the ring rather than cutting
                        // the chord across it.
                        Some(previous) => lens.arc_to(*previous, at, &mut bent),
                        None => bent.push(at),
                    }
                }
                if !swallowed {
                    canvas.draw_path(&bent, streak.color, streak.intensity * gain);
                }
            }
        }

        self.source = source;
        self.bent = bent;
    }
}

/// Fold a position back into `[-half, half)`, and say how far it was moved.
///
/// `rem_euclid` rather than a reflection off the edge: at warp the nearest
/// stars cross several band widths in a single physics step, and a reflection
/// assumes an overshoot of less than one. Anything that resets an overshooting
/// star to the edge instead loses the remainder, which phase-locks every star
/// at a given range into the same column — a curtain sliding by rather than a
/// sky.
fn fold(x: f32, half: f32) -> (f32, f32) {
    if half <= 0.0 || !half.is_finite() || !x.is_finite() {
        return (x, 0.0);
    }
    let folded = (x + half).rem_euclid(2.0 * half) - half;
    (folded, folded - x)
}

/// Half the width of the band, in world units, at a given range. The band is a
/// fixed size on the *screen*, so it widens with distance exactly as the
/// frustum does.
fn band(half_width: f32, focal: f32, z: f32) -> f32 {
    half_width * z / focal
}

/// Chop a streak into pieces short enough that bending each one straight still
/// reads as a curve.
fn subdivide(streak: &Streak, out: &mut Vec<(f32, f32)>) {
    out.clear();
    let (dx, dy) = (streak.to.0 - streak.from.0, streak.to.1 - streak.from.1);
    let length = dx.hypot(dy);
    let pieces = if length.is_finite() {
        ((length / ARC_STEP).ceil() as usize).clamp(1, MAX_ARCS)
    } else {
        1
    };
    let inv = 1.0 / pieces as f32;
    for i in 0..=pieces {
        let t = i as f32 * inv;
        out.push((streak.from.0 + dx * t, streak.from.1 + dy * t));
    }
}

fn bound_for(cam: &Camera) -> (f32, f32) {
    (
        cam.width * 0.5 * SPAWN_MARGIN,
        cam.height * 0.5 * SPAWN_MARGIN,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Tonemap;
    use crate::render::Renderer;
    use crate::ship::Ship;
    use crate::term::ColorMode;

    /// A side camera over a 200x100 canvas, built the way the renderer does.
    fn cam() -> Camera {
        Renderer::new(200, 50, ColorMode::Truecolor, 1.9).exterior_camera(&Ship::new(), 0.0)
    }

    fn screen_x(field: &ExteriorField, cam: &Camera, i: usize) -> f32 {
        cam.project(field.stars[i].pos)
            .expect("in front of the camera")
            .0
    }

    #[test]
    fn the_sky_streams_astern_and_never_leaves_the_band() {
        // At rest, at impulse, and at warp, where a near star crosses the whole
        // band several times in one step.
        let cam = cam();
        for speed in [0.0, 30.0, crate::ship::WARP_MAX] {
            let mut field = ExteriorField::new(300, 3, &cam);
            for _ in 0..600 {
                let ranges: Vec<f32> = field.stars.iter().map(|s| s.pos[2]).collect();
                field.update(1.0 / 120.0, speed, &cam);
                for (i, star) in field.stars.iter().enumerate() {
                    assert_eq!(star.pos[2], ranges[i], "star {i} changed range");
                    let band = band(field.bound.0, field.focal, star.pos[2]);
                    assert!(
                        star.pos[0] >= -band && star.pos[0] < band,
                        "star {i} left the band at {speed}: {} against {band}",
                        star.pos[0]
                    );
                    assert!(star.pos.iter().all(|c| c.is_finite()));
                }
            }
        }
    }

    #[test]
    fn a_star_that_comes_round_still_draws_the_arc_it_swept() {
        // Regression in waiting: the cockpit field blanks the trail of a star
        // it puts back, because there the star did not travel to where it was
        // put. Here it did — the band is a fold, not a teleport — and blanking
        // it would leave the nearest stars flickering between streaks and bare
        // points, which reads as television static rather than as speed.
        let cam = cam();
        let mut field = ExteriorField::new(1, 5, &cam);
        // Put it a hair inside the trailing edge, so one step folds it.
        let z = Z_NEAR * 1.2;
        let band = band(field.bound.0, field.focal, z);
        field.stars[0].pos = [-band * 0.98, 0.0, z];
        field.update(1.0 / 120.0, 400.0, &cam);

        let star = field.stars[0];
        assert!(star.prev.is_some(), "the trail was thrown away");
        let streak = field
            .streaks(&cam, 0.0, 0.0)
            .next()
            .expect("the star should still draw");
        let swept = (streak.to.0 - streak.from.0).abs();
        let honest = 400.0 / 120.0 * cam.focal / z;
        assert!(
            (swept - honest).abs() < 1.0,
            "swept {swept} where it travelled {honest}"
        );
        assert!(streak.to.0 < streak.from.0, "it should be heading astern");
    }

    #[test]
    fn nothing_ever_comes_between_the_camera_and_the_ship() {
        // The whole of the exterior renderer's depth sorting, asserted rather
        // than assumed: the hull is drawn over the sky, so no star may be in
        // front of it.
        let cam = cam();
        let mut field = ExteriorField::new(2000, 7, &cam);
        for _ in 0..600 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam);
        }
        for star in &field.stars {
            assert!(
                star.pos[2] > SHIP_DISTANCE + HULL_REACH,
                "a star got in front of the ship, at {}",
                star.pos[2]
            );
        }
    }

    #[test]
    fn nearer_stars_sweep_past_faster_than_far_ones() {
        // Parallax is the whole reason the band has depth: without it the view
        // is a flat curtain sliding by.
        let cam = cam();
        let mut field = ExteriorField::new(2, 5, &cam);
        field.stars[0].pos = [0.0, 0.0, Z_NEAR * 1.1];
        field.stars[1].pos = [0.0, 0.0, Z_FAR * 0.9];
        let before = [screen_x(&field, &cam, 0), screen_x(&field, &cam, 1)];
        field.update(1.0 / 120.0, 60.0, &cam);
        let after = [screen_x(&field, &cam, 0), screen_x(&field, &cam, 1)];
        let swept = |i: usize| (before[i] - after[i]).abs();
        assert!(
            swept(0) > swept(1) * 4.0,
            "near swept {} against far {}",
            swept(0),
            swept(1)
        );
        assert!(after[0] < before[0], "the sky goes the other way");
    }

    #[test]
    fn a_sublight_side_sky_is_not_black_and_most_of_it_is_on_screen() {
        // Both for the reasons the cockpit field has the same two tests: an
        // over-steep falloff makes cruising look like a dead terminal, and the
        // margin is a run-up rather than a place to hide the star budget.
        let cam = cam();
        let mut field = ExteriorField::new(2000, 21, &cam);
        for _ in 0..900 {
            field.update(1.0 / 120.0, 25.0, &cam);
        }
        let lit = field
            .streaks(&cam, 0.0, 0.0)
            .filter(|s| s.intensity > 0.05)
            .count();
        assert!(lit > 200, "only {lit} stars were bright enough to see");

        let visible = field
            .streaks(&cam, 0.0, 0.0)
            .filter(|s| (0.0..cam.width).contains(&s.to.0) && (0.0..cam.height).contains(&s.to.1))
            .count();
        let fraction = visible as f32 / field.len() as f32;
        assert!(
            fraction > 0.5,
            "only {:.0}% of stars were on screen",
            fraction * 100.0
        );
    }

    #[test]
    fn streaks_are_points_at_rest_and_smear_at_warp() {
        let cam = cam();
        let mut field = ExteriorField::new(400, 3, &cam);
        field.update(1.0 / 120.0, 0.0, &cam);
        field.update(1.0 / 120.0, 0.0, &cam);
        let still: f32 = field
            .streaks(&cam, 0.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(still < 1.0, "a parked ship should not streak: {still}");

        for _ in 0..30 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam);
        }
        let moving: f32 = field
            .streaks(&cam, 1.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(moving > 100.0, "warp should smear the band: {moving}");
    }

    #[test]
    fn the_sky_is_bent_only_where_the_lens_reaches() {
        // With the drive shut down the exterior view has to draw exactly what
        // it would if the lens did not exist — not something very close to it.
        let cam = cam();
        let mut field = ExteriorField::new(600, 13, &cam);
        for _ in 0..120 {
            field.update(1.0 / 120.0, 200.0, &cam);
        }

        let tone = Tonemap::new(1.9, 2.2);
        let resolve = |canvas: &Canvas| {
            let mut out = Vec::new();
            canvas.resolve_into(&tone, &mut out);
            out
        };
        let mut plain = Canvas::new(200, 100);
        for streak in field.streaks(&cam, 0.5, 1.0) {
            plain.draw_streak(&streak);
        }
        let mut through_lens = Canvas::new(200, 100);
        field.draw(&mut through_lens, &cam, 0.5, 1.0, &Lens::OFF);
        assert_eq!(
            resolve(&plain),
            resolve(&through_lens),
            "an unlit drive bent the sky"
        );
    }

    #[test]
    fn a_lit_drive_sweeps_the_middle_clear_and_lights_a_ring() {
        // The picture the lens is for: nothing survives inside the Einstein
        // radius, and the light it displaced piles up just outside it.
        let cam = cam();
        let mut field = ExteriorField::new(8000, 17, &cam);
        for _ in 0..120 {
            field.update(1.0 / 120.0, 300.0, &cam);
        }
        let centre = (cam.cx, cam.cy);
        let lens = Lens {
            center: centre,
            radius: 22.0,
        };

        // Mean brightness over an annulus, not the total: the annuli compared
        // below cover different areas, and the canvas clips the outer ones.
        let light_within = |canvas: &Canvas, lo: f32, hi: f32| -> f32 {
            let (mut total, mut n) = (0.0, 0u32);
            for y in 0..100usize {
                for x in 0..200usize {
                    let r = (x as f32 - centre.0).hypot(y as f32 - centre.1);
                    if r >= lo && r < hi {
                        total += canvas.light_at(x, y);
                        n += 1;
                    }
                }
            }
            total / n.max(1) as f32
        };

        let mut unbent = Canvas::new(200, 100);
        field.draw(&mut unbent, &cam, 0.8, 0.0, &Lens::OFF);
        let mut bent = Canvas::new(200, 100);
        field.draw(&mut bent, &cam, 0.8, 0.0, &lens);

        // The bubble: what was in the middle is gone, swallowed rather than
        // merely displaced — a transparent lens would fill it right back up
        // with a demagnified copy of the whole sky.
        let (before, after) = (
            light_within(&unbent, 0.0, 14.0),
            light_within(&bent, 0.0, 14.0),
        );
        assert_eq!(
            after, 0.0,
            "the bubble is still lit: {before} became {after}"
        );
        // The ring: the light swept out of the middle piled up just outside the
        // Einstein radius, where the magnification is highest.
        let (before, after) = (
            light_within(&unbent, 22.0, 27.0),
            light_within(&bent, 22.0, 27.0),
        );
        assert!(
            after > before * 1.3,
            "no ring formed: {before} became {after}"
        );

        // And it is a *ring*, not a general brightening: inside the lensed
        // frame the rim outshines the sky further out, which in the unlensed
        // frame it does not.
        let (rim, out) = (
            light_within(&bent, 22.0, 27.0),
            light_within(&bent, 36.0, 46.0),
        );
        assert!(
            rim > out * 1.1,
            "the rim does not stand out: {rim} against {out}"
        );
        let (rim, out) = (
            light_within(&unbent, 22.0, 27.0),
            light_within(&unbent, 36.0, 46.0),
        );
        assert!(
            (rim - out).abs() < out * 0.25,
            "the unlensed sky already had a ring in it: {rim} against {out}"
        );
    }

    #[test]
    fn folding_the_band_survives_anything_handed_to_it() {
        for (x, half) in [
            (0.0, 10.0),
            (-10.0, 10.0),
            (9.999, 10.0),
            (-1e6, 10.0),
            (1e6, 10.0),
            (f32::NAN, 10.0),
            (5.0, 0.0),
            (5.0, -1.0),
            (5.0, f32::NAN),
        ] {
            let (folded, shift) = fold(x, half);
            if !x.is_finite() || half <= 0.0 || !half.is_finite() {
                assert_eq!(shift, 0.0, "{x} in a band of {half} was moved anyway");
                continue;
            }
            assert!(
                folded >= -half && folded < half,
                "{x} folded to {folded}, outside {half}"
            );
            // The shift has to be a whole number of bands, or carrying the
            // trail with it would draw a segment the star never swept.
            let bands = shift / (2.0 * half);
            assert!(
                (bands - bands.round()).abs() < 1e-3,
                "{x} moved {bands} bands"
            );
        }
    }

    #[test]
    fn resizing_the_pool_and_the_canvas_keeps_it_valid() {
        let cam = cam();
        let mut field = ExteriorField::new(100, 1, &cam);
        field.resize_pool(2000);
        assert_eq!(field.len(), 2000);
        field.resize_pool(0);
        assert_eq!(field.len(), 1, "the pool never empties completely");

        let ship = Ship::new();
        for (cols, rows) in [(1usize, 1usize), (200, 60), (12, 400), (400, 2)] {
            let renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
            let cam = renderer.exterior_camera(&ship, 0.0);
            field.retarget(&cam);
            field.resize_pool(200);
            assert!(
                field.stars.iter().all(|s| s.prev.is_none()),
                "a resize kept a trail from the old layout"
            );
            let (w, h) = renderer.canvas_dims();
            let mut canvas = Canvas::new(w, h);
            for _ in 0..30 {
                field.update(1.0 / 120.0, 400.0, &cam);
                let lens = Lens::for_warp((cam.cx, cam.cy), 1.0, cam.height);
                field.draw(&mut canvas, &cam, 1.0, 0.0, &lens);
            }
            assert!(field
                .stars
                .iter()
                .all(|s| s.pos.iter().all(|c| c.is_finite())));
        }
    }
}
