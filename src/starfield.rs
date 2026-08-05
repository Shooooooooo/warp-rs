//! The universe, or the part of it you can see from the cockpit.
//!
//! Stars live in camera space and stream toward the viewer. Each frame a star
//! is projected to the screen and remembers where it was last frame; the
//! segment between those two points is the streak that sells the speed. At
//! sublight the segment is sub-pixel and reads as a point, and at warp it
//! stretches into the classic hyperspace smear without any special-casing.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::f32::consts::TAU;

/// Stars are recycled once they get this close — they'd fly past the canopy.
pub const Z_NEAR: f32 = 0.9;
/// The far plane. Stars fade up from nothing here rather than popping in.
pub const Z_FAR: f32 = 260.0;

/// Stars live in a *rectangular* screen-space frustum this much larger than
/// the visible area. A circular bound wastes most of the star budget on the
/// corners of a disc that never intersects a wide terminal; the margin that
/// remains is what a turn has to bite into.
const SPAWN_MARGIN: f32 = 1.3;

/// How sharply a star dims with distance. Real inverse-square falloff spans
/// five orders of magnitude over this depth range and leaves all but the
/// nearest handful invisible, so the curve is flattened to something the
/// tonemap can show: far stars stay faintly present instead of vanishing.
const DEPTH_FALLOFF: f32 = 1.4;

/// Stars also differ in how bright they intrinsically are, and the real
/// distribution is lopsided: a sky is mostly faint pinpricks with a handful of
/// standouts. Cubing a uniform sample gives that shape. Without it every star
/// renders at much the same brightness and the field reads as television
/// static rather than as space.
const MAGNITUDE_FLOOR: f32 = 0.14;

/// A star's spectral class: colour and relative luminosity.
#[derive(Debug, Clone, Copy)]
pub struct SpectralClass {
    pub rgb: [f32; 3],
    pub luminosity: f32,
    /// Relative frequency in the pool.
    weight: f32,
}

/// Roughly the colours of the main sequence. The weights lean toward the hot
/// end relative to a true census, because what fills a real sky is apparent
/// brightness, and a field of honest M dwarfs is a dim red smudge.
pub const CLASSES: [SpectralClass; 7] = [
    // O
    SpectralClass { rgb: [0.61, 0.69, 1.00], luminosity: 2.20, weight: 1.0 },
    // B
    SpectralClass { rgb: [0.67, 0.75, 1.00], luminosity: 1.80, weight: 5.0 },
    // A
    SpectralClass { rgb: [0.79, 0.84, 1.00], luminosity: 1.40, weight: 14.0 },
    // F
    SpectralClass { rgb: [0.97, 0.97, 1.00], luminosity: 1.15, weight: 18.0 },
    // G
    SpectralClass { rgb: [1.00, 0.96, 0.92], luminosity: 1.00, weight: 22.0 },
    // K
    SpectralClass { rgb: [1.00, 0.82, 0.63], luminosity: 0.85, weight: 24.0 },
    // M
    SpectralClass { rgb: [1.00, 0.65, 0.44], luminosity: 0.70, weight: 16.0 },
];

/// Colour a star shifts toward when it is dead ahead and you are moving fast.
const BLUE_SHIFT: [f32; 3] = [0.45, 0.66, 1.00];
/// Colour a star shifts toward out at the edges, where it is falling behind.
const RED_SHIFT: [f32; 3] = [1.00, 0.42, 0.24];

/// Where the eye is and how it sees. Sizes are in canvas subpixels, not cells.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub width: f32,
    pub height: f32,
    /// The vanishing point — the centre, plus any shake offset.
    pub cx: f32,
    pub cy: f32,
    pub focal: f32,
    /// Roll, applied after projection.
    pub bank: f32,
}

impl Camera {
    pub fn new(width: usize, height: usize) -> Self {
        let (w, h) = (width as f32, height as f32);
        Self {
            width: w,
            height: h,
            cx: w * 0.5,
            cy: h * 0.5,
            focal: h.max(1.0) * 0.85,
            bank: 0.0,
        }
    }

    /// Half-width and half-height: the screen's extent from the centre.
    fn half_extent(&self) -> (f32, f32) {
        (self.width * 0.5, self.height * 0.5)
    }

    /// Perspective-project a camera-space point. `None` if it is at or behind
    /// the near plane, where the projection would blow up.
    pub fn project(&self, pos: [f32; 3]) -> Option<(f32, f32)> {
        if pos[2] <= Z_NEAR {
            return None;
        }
        let inv = self.focal / pos[2];
        let (x, y) = (pos[0] * inv, pos[1] * inv);
        let (sin, cos) = self.bank.sin_cos();
        Some((self.cx + x * cos - y * sin, self.cy + x * sin + y * cos))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Star {
    pos: [f32; 3],
    /// Projected position last frame, if it had one.
    prev: Option<(f32, f32)>,
    class: usize,
    /// Intrinsic brightness, 0..=1, on top of the spectral class.
    magnitude: f32,
    /// Phase offset so stars don't all twinkle in unison.
    phase: f32,
}

/// One star's contribution to a frame: a coloured segment to add to the canvas.
#[derive(Debug, Clone, Copy)]
pub struct Streak {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub color: [f32; 3],
    pub intensity: f32,
}

pub struct StarField {
    stars: Vec<Star>,
    rng: StdRng,
    /// Half-extent of the screen-space frustum, margin included. Cached so
    /// stars can be respawned without a camera in hand.
    bound: (f32, f32),
    focal: f32,
}

impl StarField {
    pub fn new(count: usize, seed: u64, cam: &Camera) -> Self {
        let (hw, hh) = cam.half_extent();
        let mut field = Self {
            stars: Vec::with_capacity(count),
            rng: StdRng::seed_from_u64(seed),
            bound: (hw * SPAWN_MARGIN, hh * SPAWN_MARGIN),
            focal: cam.focal,
        };
        for _ in 0..count {
            let star = field.spawn(DepthRule::Anywhere);
            field.stars.push(star);
        }
        field
    }

    pub fn len(&self) -> usize {
        self.stars.len()
    }

    /// Adapt to a new canvas size. Existing stars keep their world positions;
    /// only the spawn cone changes.
    pub fn retarget(&mut self, cam: &Camera) {
        let (hw, hh) = cam.half_extent();
        self.bound = (hw * SPAWN_MARGIN, hh * SPAWN_MARGIN);
        self.focal = cam.focal;
    }

    /// Grow or shrink the pool, keeping the stars already in flight.
    pub fn resize_pool(&mut self, count: usize) {
        let count = count.max(1);
        while self.stars.len() > count {
            self.stars.pop();
        }
        while self.stars.len() < count {
            let star = self.spawn(DepthRule::Anywhere);
            self.stars.push(star);
        }
    }

    /// Make a star. The position is sampled on the *screen* and back-projected
    /// through the depth, which keeps apparent density even at every distance.
    /// Sampling a world-space volume instead would crowd the far plane and
    /// starve the near one, because a fixed world area covers fewer and fewer
    /// pixels as it recedes.
    fn spawn(&mut self, depth: DepthRule) -> Star {
        let z = match depth {
            DepthRule::Anywhere => self.rng.random_range(Z_NEAR..Z_FAR),
            DepthRule::FarPlane => self.rng.random_range(Z_FAR * 0.92..Z_FAR),
        };
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

    /// Advance the field: fly forward, swing with the ship's steering, and
    /// recycle anything that has left the view.
    pub fn update(&mut self, dt: f32, speed: f32, yaw: f32, pitch: f32, cam: &Camera) {
        let (sy, cy) = (yaw * dt).sin_cos();
        let (sp, cp) = (pitch * dt).sin_cos();
        let travel = speed * dt;
        let (bound_x, bound_y) = self.bound;

        for i in 0..self.stars.len() {
            let star = &mut self.stars[i];
            star.prev = cam.project(star.pos);

            let [x, y, z] = star.pos;
            // Yaw about the vertical axis, then pitch about the horizontal.
            let (x, z) = (x * cy - z * sy, x * sy + z * cy);
            let (y, z) = (y * cp - z * sp, y * sp + z * cp);
            star.pos = [x, y, z - travel];

            // A star leaves either by flying past the canopy or by fanning out
            // beyond the edge of the frustum on its way in. Both are the same
            // event — it is behind us now — and both put it back at the far
            // plane, which is the only place new stars can honestly come from.
            let gone = star.pos[2] <= Z_NEAR
                || match cam.project(star.pos) {
                    Some((px, py)) => {
                        (px - cam.cx).abs() > bound_x || (py - cam.cy).abs() > bound_y
                    }
                    None => true,
                };

            if gone {
                self.stars[i] = self.spawn(DepthRule::FarPlane);
            }
        }
    }

    /// Turn the field into drawable segments for this frame.
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
        // Folded into a single turn once per frame, in `f64`, so the per-star
        // `sin` below can stay `f32` without the phase going coarse — and
        // eventually static — once the process has been up for days.
        let twinkle_phase = (time * 2.3).rem_euclid(std::f64::consts::TAU) as f32;

        self.stars.iter().filter_map(move |star| {
            let to = cam.project(star.pos)?;
            // Stretch backward along the direction of travel. A star far from
            // the vanishing point moves further per frame, so this lengthens
            // exactly the streaks that should be long.
            let from = match star.prev {
                Some(prev) => (
                    to.0 + (prev.0 - to.0) * stretch,
                    to.1 + (prev.1 - to.1) * stretch,
                ),
                None => to,
            };

            let class = &CLASSES[star.class];
            let z = star.pos[2];
            // Reaches zero exactly at the far plane, with zero slope, so stars
            // fade up out of nothing instead of blinking into existence. A
            // star swung past the far plane by a turn simply stays dark until
            // it is close enough to matter again.
            let depth = (1.0 - (z - Z_NEAR) / (Z_FAR - Z_NEAR)).clamp(0.0, 1.0);
            let twinkle = 1.0 + twinkle_amt * (twinkle_phase + star.phase).sin();
            let intensity =
                class.luminosity * star.magnitude * depth.powf(DEPTH_FALLOFF) * twinkle;
            if intensity <= 0.0 {
                return None;
            }

            // How far off the nose the star sits. Dead ahead blue-shifts;
            // out at the edges, where it is falling behind, it reddens.
            let (dx, dy) = (to.0 - cam.cx, to.1 - cam.cy);
            let radius = (dx * dx + dy * dy).sqrt();
            let forward = cam.focal / (cam.focal * cam.focal + radius * radius).sqrt();
            let color = shift_color(class.rgb, forward, doppler);

            Some(Streak { from, to, color, intensity })
        })
    }
}

/// Which depth a new star should appear at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepthRule {
    /// Anywhere in the volume: for filling a fresh field, and for topping one
    /// up when the pool grows. Both want a sky that is already populated in
    /// depth — a batch of stars all arriving at the far plane together would
    /// read as a curtain sweeping in rather than as space.
    Anywhere,
    /// Out at the far plane, the only honest place for a star that has just
    /// left the view, and the fate of every star recycled by `update`.
    FarPlane,
}

/// Blend a star's rest colour toward blue ahead and red at the periphery.
/// `forward` is cos of the angle off the nose; `amount` is the 0..=1 strength.
fn shift_color(rgb: [f32; 3], forward: f32, amount: f32) -> [f32; 3] {
    if amount <= 0.0 {
        return rgb;
    }
    let blue = amount * forward.powi(3);
    let red = amount * (1.0 - forward) * 0.75;
    let mut out = [0.0; 3];
    for i in 0..3 {
        let toward_blue = rgb[i] + (BLUE_SHIFT[i] - rgb[i]) * blue;
        out[i] = toward_blue + (RED_SHIFT[i] - toward_blue) * red;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera::new(200, 100)
    }

    #[test]
    fn the_nose_projects_to_the_vanishing_point_at_any_depth() {
        let cam = cam();
        for z in [1.0, 10.0, 100.0, Z_FAR] {
            let p = cam.project([0.0, 0.0, z]).expect("in front of the near plane");
            assert!((p.0 - cam.cx).abs() < 1e-4 && (p.1 - cam.cy).abs() < 1e-4);
        }
    }

    #[test]
    fn nothing_projects_from_behind_the_near_plane() {
        let cam = cam();
        assert!(cam.project([1.0, 1.0, Z_NEAR]).is_none());
        assert!(cam.project([1.0, 1.0, -5.0]).is_none());
    }

    #[test]
    fn approaching_stars_move_away_from_the_centre() {
        let cam = cam();
        let far = cam.project([1.0, 0.0, 100.0]).unwrap();
        let near = cam.project([1.0, 0.0, 20.0]).unwrap();
        assert!((near.0 - cam.cx) > (far.0 - cam.cx));
    }

    #[test]
    fn flying_straight_keeps_every_star_inside_the_volume() {
        let cam = cam();
        let mut field = StarField::new(500, 7, &cam);
        for _ in 0..600 {
            field.update(1.0 / 60.0, 400.0, 0.0, 0.0, &cam);
            assert_eq!(field.len(), 500);
            for star in &field.stars {
                assert!(
                    star.pos[2] > Z_NEAR && star.pos[2] <= Z_FAR,
                    "z escaped the volume: {}",
                    star.pos[2]
                );
            }
        }
    }

    #[test]
    fn the_pool_survives_hard_steering() {
        let cam = cam();
        let mut field = StarField::new(500, 7, &cam);
        for _ in 0..600 {
            // Steering rotates stars, which can swing one out past the far
            // plane. That is legitimate — it goes dark until it comes back —
            // but nothing may go behind the camera or turn into a NaN.
            field.update(1.0 / 60.0, 400.0, 0.85, -0.6, &cam);
            assert_eq!(field.len(), 500);
            for star in &field.stars {
                assert!(star.pos[2] > Z_NEAR, "z went behind us: {}", star.pos[2]);
                assert!(star.pos.iter().all(|c| c.is_finite()));
            }
        }
    }

    #[test]
    fn most_of_the_star_budget_lands_on_screen() {
        // The whole point of the rectangular frustum: stars the viewer will
        // never see are wasted, and at sublight that is the difference
        // between a sky and an empty screen.
        let cam = cam();
        let mut field = StarField::new(3000, 21, &cam);
        for _ in 0..400 {
            field.update(1.0 / 60.0, 30.0, 0.0, 0.0, &cam);
        }
        let visible = field
            .streaks(&cam, 0.0, 0.0)
            .filter(|s| {
                (0.0..cam.width).contains(&s.to.0) && (0.0..cam.height).contains(&s.to.1)
            })
            .count();
        let fraction = visible as f32 / field.len() as f32;
        assert!(fraction > 0.5, "only {:.0}% of stars were on screen", fraction * 100.0);
    }

    #[test]
    fn a_sublight_sky_is_not_black() {
        // Regression: an over-steep depth falloff made everything but the
        // nearest stars invisible, so cruising looked like a dead terminal.
        let cam = cam();
        let mut field = StarField::new(2000, 13, &cam);
        for _ in 0..900 {
            field.update(1.0 / 60.0, 25.0, 0.0, 0.0, &cam);
        }
        let lit = field.streaks(&cam, 0.0, 0.0).filter(|s| s.intensity > 0.05).count();
        assert!(lit > 200, "only {lit} stars were bright enough to see");
    }

    #[test]
    fn resizing_the_pool_keeps_it_valid() {
        let cam = cam();
        let mut field = StarField::new(100, 1, &cam);
        field.resize_pool(2000);
        assert_eq!(field.len(), 2000);
        field.resize_pool(0);
        assert_eq!(field.len(), 1, "the pool never empties completely");
    }

    #[test]
    fn streaks_are_points_at_rest_and_stretch_under_way() {
        let cam = cam();
        let mut field = StarField::new(400, 3, &cam);
        field.update(1.0 / 60.0, 0.0, 0.0, 0.0, &cam);
        field.update(1.0 / 60.0, 0.0, 0.0, 0.0, &cam);
        let still: f32 = field
            .streaks(&cam, 0.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(still < 1.0, "a parked ship should not streak: {still}");

        for _ in 0..30 {
            field.update(1.0 / 60.0, crate::ship::WARP_MAX, 0.0, 0.0, &cam);
        }
        let moving: f32 = field
            .streaks(&cam, 1.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(moving > 100.0, "warp should streak: {moving}");
    }

    #[test]
    fn the_twinkle_still_moves_after_days_of_flight() {
        // Regression: the twinkle phase was `(time * 2.3 + phase)` computed in
        // `f32` from an `f32` clock. It went coarse within hours and stopped
        // advancing entirely after a few days, freezing the sky solid.
        let cam = cam();
        let mut field = StarField::new(400, 3, &cam);
        for _ in 0..10 {
            field.update(1.0 / 60.0, 20.0, 0.0, 0.0, &cam);
        }
        let sample = |time: f64| -> Vec<f32> {
            field.streaks(&cam, 0.0, time).map(|s| s.intensity).collect()
        };

        for t in [0.0f64, 3_600.0, 86_400.0, 524_288.0, 10_000_000.0] {
            let (a, b) = (sample(t), sample(t + 1.0 / 60.0));
            assert!(
                a.iter().zip(&b).any(|(x, y)| x != y),
                "the twinkle froze at {t} s ({:.1} days)",
                t / 86_400.0
            );
        }
    }

    #[test]
    fn doppler_is_identity_at_rest() {
        for class in CLASSES {
            for forward in [0.0, 0.5, 1.0] {
                assert_eq!(shift_color(class.rgb, forward, 0.0), class.rgb);
            }
        }
    }

    #[test]
    fn doppler_blues_the_centre_and_reddens_the_edge() {
        let g = CLASSES[4].rgb;
        let ahead = shift_color(g, 1.0, 1.0);
        assert!(ahead[2] > ahead[0], "dead ahead should read blue: {ahead:?}");
        let edge = shift_color(g, 0.05, 1.0);
        assert!(edge[0] > edge[2], "the periphery should read red: {edge:?}");
    }

    #[test]
    fn colors_stay_in_gamut() {
        for class in CLASSES {
            for f in 0..=20 {
                for a in 0..=20 {
                    let c = shift_color(class.rgb, f as f32 / 20.0, a as f32 / 20.0);
                    assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?}");
                }
            }
        }
    }

    #[test]
    fn spectral_weights_are_positive() {
        assert!(CLASSES.iter().all(|c| c.weight > 0.0 && c.luminosity > 0.0));
    }
}
