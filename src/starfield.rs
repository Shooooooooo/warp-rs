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
/// remains is the run-up a star gets before it reaches the screen, so nothing
/// a turn brings into view has to appear in view.
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
    /// Relative frequency in the pool. Visible to the crate because the
    /// exterior view draws from the same census.
    pub(crate) weight: f32,
}

/// Roughly the colours of the main sequence. The weights lean toward the hot
/// end relative to a true census, because what fills a real sky is apparent
/// brightness, and a field of honest M dwarfs is a dim red smudge.
pub const CLASSES: [SpectralClass; 7] = [
    // O
    SpectralClass {
        rgb: [0.61, 0.69, 1.00],
        luminosity: 2.20,
        weight: 1.0,
    },
    // B
    SpectralClass {
        rgb: [0.67, 0.75, 1.00],
        luminosity: 1.80,
        weight: 5.0,
    },
    // A
    SpectralClass {
        rgb: [0.79, 0.84, 1.00],
        luminosity: 1.40,
        weight: 14.0,
    },
    // F
    SpectralClass {
        rgb: [0.97, 0.97, 1.00],
        luminosity: 1.15,
        weight: 18.0,
    },
    // G
    SpectralClass {
        rgb: [1.00, 0.96, 0.92],
        luminosity: 1.00,
        weight: 22.0,
    },
    // K
    SpectralClass {
        rgb: [1.00, 0.82, 0.63],
        luminosity: 0.85,
        weight: 24.0,
    },
    // M
    SpectralClass {
        rgb: [1.00, 0.65, 0.44],
        luminosity: 0.70,
        weight: 16.0,
    },
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
        Some((self.cx + pos[0] * inv, self.cy + pos[1] * inv))
    }

    /// Where a direction vanishes: the screen point the projection of a ray
    /// pointing this way converges on as the ray runs off to infinity.
    ///
    /// `cx` and `cy` are this question already answered for the one direction
    /// the camera is looking down. This is the general form of it, and what
    /// wants the general form is anything drawn *along* a direction rather than
    /// at a point — the engine lance in [`crate::models`], which is stretched
    /// in screen space and so needs to know where the screen runs out of plume.
    ///
    /// `None` when there is no such point, which is a fact about the direction
    /// and not a failure to compute one: a ray coming back toward the eye
    /// projects to a line that runs off the frame rather than converging on
    /// anything, and a caller that wanted a bound has to find one elsewhere.
    ///
    /// The multiplication is associated the other way round from `project`'s,
    /// which takes `focal / z` once and applies it to both coordinates, and
    /// that is deliberate rather than a slip. A direction so nearly in the image
    /// plane that `focal / dir[2]` overflows on its own can still have a
    /// perfectly ordinary ratio, and this form answers for it instead of
    /// throwing the point away.
    pub fn vanishing_point(&self, dir: [f32; 3]) -> Option<(f32, f32)> {
        if dir[2] <= 0.0 {
            return None;
        }
        let (x, y) = (dir[0] / dir[2] * self.focal, dir[1] / dir[2] * self.focal);
        // The guard above lets a NaN through — every comparison against one is
        // false — and a direction lying almost exactly in the image plane
        // overflows the divide rather than answering a very distant point. Both
        // arrive here as a non-finite coordinate, and both mean the same thing
        // as no vanishing point at all.
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        Some((self.cx + x, self.cy + y))
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

    /// True only when the count asked for was zero, which `--stars 0` is
    /// allowed to ask for. The two ends of the pool are not symmetric about
    /// that and deliberately: [`Self::new`] builds exactly what it is handed,
    /// where [`Self::resize_pool`] keeps a star back however far down the `-`
    /// key is walked, so a sky can be *started* empty and cannot be *emptied*.
    pub fn is_empty(&self) -> bool {
        self.stars.is_empty()
    }

    /// Adapt to a new canvas size. Existing stars keep their world positions;
    /// only the spawn cone changes.
    ///
    /// Every trail is dropped, for the reason [`crate::exterior::ExteriorField`]
    /// drops its own: a streak drawn from where a star used to be under the old
    /// projection to where it is under the new one is a scratch across the
    /// frame. `cx`, `cy` and `focal` all move with the canvas, so that is the
    /// whole field at once rather than a few stars near an edge. Flying, the
    /// next step overwrites `prev` and it costs one frame; paused, `advance`
    /// never runs, so the scratches stayed until the flight was let go again.
    pub fn retarget(&mut self, cam: &Camera) {
        let (hw, hh) = cam.half_extent();
        self.bound = (hw * SPAWN_MARGIN, hh * SPAWN_MARGIN);
        self.focal = cam.focal;
        for star in &mut self.stars {
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
    ///
    /// `yaw`, `pitch` and `roll` are the ship's angular rates. Roll turns the
    /// stars rather than the projection so that it streaks like the other two
    /// axes do: a streak is the segment between a star's old and new positions,
    /// and a camera that rolled underneath a star that had not moved would draw
    /// no arc at all.
    ///
    /// **`Ship::bank` is deliberately not here, and must not be brought back.**
    /// The lean into a turn is a display affectation: it chases the yaw rate,
    /// centres itself, and is kept apart from `Ship::roll`, which is the
    /// attitude. There is no way to put a yaw-coupled angle into the sky's
    /// orientation without one of two faults, and both were shipped and
    /// reported before this comment was written.
    ///
    /// Rotate the *picture* by the lean and steer underneath it, and the flow
    /// comes out across the frame at exactly the bank angle — twenty degrees at
    /// the stops. A sky streaming diagonally while the stick asks for a flat
    /// turn is not read as a lean but as a *climb*, because a sky sliding
    /// downward is what going up looks like; and the lean's sign follows the
    /// yaw's, so both A and D lifted the nose.
    ///
    /// Steer in the frame the lean has turned the picture to instead, and a
    /// held turn streams flat — but the step becomes the steering with a roll
    /// of however much the lean moved in front of it, and rotations do not
    /// commute. Working the stick round a closed loop then leaves a residue
    /// about the camera's own `x`, going as the *square* of the yaw rate and so
    /// the same way round whichever way the stick went: about nine degrees a
    /// second of nose-down that nothing brings back. That is honest rigid-body
    /// kinematics — which is the point, since a cosmetic lean has no business
    /// accumulating into the flight's attitude.
    ///
    /// So the sky is steered about the camera's own axes and the picture is not
    /// rolled at all. The lean is a fact about the hull, and
    /// [`crate::models::attitude`] is where it shows.
    pub fn update(&mut self, dt: f32, speed: f32, yaw: f32, pitch: f32, roll: f32, cam: &Camera) {
        let (sy, cy) = (yaw * dt).sin_cos();
        let (sp, cp) = (pitch * dt).sin_cos();
        // Negated: the sky turns the opposite way from the ship, so dropping
        // the starboard wing swings the stars anticlockwise.
        let (sr, cr) = (-roll * dt).sin_cos();
        let travel = speed * dt;
        let (bound_x, bound_y) = self.bound;
        let focal = self.focal;

        // Where a star sits relative to the vanishing point, and whether that
        // has it out of the frustum. Behind the near plane counts as out.
        let offset = |pos| cam.project(pos).map(|(px, py)| (px - cam.cx, py - cam.cy));
        let escaped = |o: Option<(f32, f32)>| {
            o.is_none_or(|(sx, sy)| sx.abs() > bound_x || sy.abs() > bound_y)
        };

        for i in 0..self.stars.len() {
            let star = &mut self.stars[i];
            star.prev = cam.project(star.pos);

            let [x, y, z] = star.pos;
            // Where this frame's travel alone would have put it. Kept so that a
            // star which has left the frustum can say which way it went.
            let coasted = [x, y, z - travel];

            // Yaw about the vertical axis, pitch about the horizontal, roll
            // about the nose. Each is a rotation of camera space, so all three
            // stay relative to the ship: roll ninety degrees and the pitch axis
            // has come round to where the yaw axis was, exactly as it would in
            // something with wings.
            let (x, z) = (x * cy - z * sy, x * sy + z * cy);
            let (y, z) = (y * cp - z * sp, y * sp + z * cp);
            let (x, y) = (x * cr - y * sr, x * sr + y * cr);
            star.pos = [x, y, z - travel];
            let pos = star.pos;

            let here = offset(pos);
            if !escaped(here) {
                continue;
            }

            // Two quite different events wear the same disguise here. Flying
            // past the canopy — or fanning out beyond the edge of the frustum
            // on the way in, which is the same thing — leaves the star astern,
            // and the far plane is the only place it can honestly come back
            // from. But a turn that *swings* a star out of view has not passed
            // it: the sky making room is the sky on the other side, already at
            // whatever distance it had. Sending that to the far plane too is
            // what emptied the edges of the frame during a hard turn, and
            // roll, which sweeps the whole frame at once, makes it
            // impossible to miss.
            let swung_out = match (here, escaped(offset(coasted))) {
                (Some(offset), false) => Some(offset),
                _ => None,
            };
            let Some((sx, sy)) = swung_out else {
                self.stars[i] = self.spawn(DepthRule::FarPlane);
                continue;
            };

            // Out past one edge, back in at the one opposite, at the depth it
            // already had. The frustum is wider than the screen, so it returns
            // off-camera and sweeps into view rather than appearing in it.
            let sx = if sx.abs() > bound_x { -sx } else { sx };
            let sy = if sy.abs() > bound_y { -sy } else { sy };
            let (sx, sy) = (sx.clamp(-bound_x, bound_x), sy.clamp(-bound_y, bound_y));
            // Back-projected through the depth, the same way `spawn` places a
            // fresh star.
            let scale = pos[2] / focal;
            let star = &mut self.stars[i];
            star.pos = [sx * scale, sy * scale, pos[2]];
            // It did not travel there, so it must not draw a streak saying it
            // did — that segment would be a scratch clean across the frame.
            star.prev = None;
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
            // The amount is *exactly* zero from a third of light speed up, and
            // `1 + 0·s` is exactly one for any finite `s` — so this is the same
            // number the multiply would have reached, without a `sin` per star
            // on the frame that can least afford one.
            let twinkle = if twinkle_amt > 0.0 {
                1.0 + twinkle_amt * (twinkle_phase + star.phase).sin()
            } else {
                1.0
            };
            let intensity = class.luminosity * star.magnitude * depth.powf(DEPTH_FALLOFF) * twinkle;
            if intensity <= 0.0 {
                return None;
            }

            // How far off the nose the star sits. Dead ahead blue-shifts;
            // out at the edges, where it is falling behind, it reddens.
            let (dx, dy) = (to.0 - cam.cx, to.1 - cam.cy);
            let radius = (dx * dx + dy * dy).sqrt();
            let forward = cam.focal / (cam.focal * cam.focal + radius * radius).sqrt();
            let color = shift_color(class.rgb, forward, doppler);

            Some(Streak {
                from,
                to,
                color,
                intensity,
            })
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
///
/// Visible to the crate because the exterior view Doppler-shifts the same way,
/// about the ship's direction of travel rather than about the vanishing point:
/// it is the same physics seen from beside the ship instead of from inside it.
pub(crate) fn shift_color(rgb: [f32; 3], forward: f32, amount: f32) -> [f32; 3] {
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
            let p = cam
                .project([0.0, 0.0, z])
                .expect("in front of the near plane");
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
    fn a_direction_that_recedes_vanishes_somewhere_and_one_that_returns_does_not() {
        // The distinction the engine lance is clamped on. A plume pointed away
        // from the eye has a point on screen its exhaust converges toward and
        // can never pass; one pointed at the eye has none, and the ray it draws
        // runs off the frame instead.
        // And the way the camera is looking vanishes exactly where this struct's
        // own doc has called `cx` and `cy` for as long as it has had one, which
        // is that sentence made checkable. Exact rather than close: the
        // numerator is a zero, so there is nothing left to round.
        let cam = cam();
        assert!(cam.vanishing_point([0.3, -0.2, -1.0]).is_none());
        assert!(cam.vanishing_point([0.3, -0.2, 0.0]).is_none());
        assert!(cam.vanishing_point([0.3, -0.2, 1.0]).is_some());
        assert_eq!(cam.vanishing_point([0.0, 0.0, 1.0]), Some((cam.cx, cam.cy)));

        // The two ways the arithmetic fails rather than the direction, and they
        // are caught in different places. A NaN *depth* walks straight through
        // the guard, every comparison against one being false, so it is the
        // finite check after the divide that has to stop it; a direction lying
        // almost exactly in the image plane overflows that divide instead of
        // answering a very distant point. Both mean what no vanishing point
        // means.
        assert!(
            cam.vanishing_point([f32::NAN, 0.0, 1.0]).is_none(),
            "a direction that is not a direction answered with a point"
        );
        assert!(cam.vanishing_point([1.0, 0.0, f32::NAN]).is_none());
        assert!(cam.vanishing_point([1.0, 0.0, f32::MIN_POSITIVE]).is_none());
    }

    #[test]
    fn a_receding_ray_never_reaches_the_point_it_is_vanishing_toward() {
        // The half of this the test below does not ask, and the whole of what
        // the engine lance is clamped on: not where the point is, but that a
        // plume drawn along the ray closes on it and never arrives. `P(s) - V`
        // is `(H - V)` times the ratio of the two depths, which is checked here
        // rather than asserted — that identity is what lets `draw_trail` take
        // the clamp and the amount the fan narrows by out of one division,
        // without either of them ever holding a depth.
        //
        // Flown with the root off the axis, so the identity is asked of a
        // segment that runs across the frame rather than down its middle.
        let cam = cam();
        let root = [0.7, -0.4, 3.0];
        let head = cam.project(root).expect("the root is in front of the eye");
        for dir in [[0.0, 0.0, 1.0], [1.0, 0.0, 2.0], [-0.6, 0.9, 0.3]] {
            let far = cam.vanishing_point(dir).expect("this direction recedes");
            let (hx, hy) = (head.0 - far.0, head.1 - far.1);
            let start = hx.hypot(hy);
            let mut last = start;
            for step in 1..400 {
                let s = step as f32 * 0.5;
                let at = [
                    root[0] + dir[0] * s,
                    root[1] + dir[1] * s,
                    root[2] + dir[2] * s,
                ];
                let p = cam.project(at).expect("a receding point stays in front");
                let (dx, dy) = (p.0 - far.0, p.1 - far.1);
                assert!(
                    dx * hx + dy * hy > 0.0,
                    "{dir:?} crossed the point it was vanishing toward at {s}"
                );
                let now = dx.hypot(dy);
                assert!(now < last, "{dir:?} stopped closing on it at {s}");
                last = now;
                let want = start * root[2] / at[2];
                assert!(
                    (now - want).abs() < want * 1e-3,
                    "{dir:?} at {s}: {now} from the point against the depth ratio's {want}"
                );
            }
        }
    }

    #[test]
    fn a_direction_vanishes_where_a_far_enough_point_along_it_projects() {
        // Said against `project` rather than against the formula, because the
        // formula is the thing under test: a vanishing point is the limit of
        // projecting further and further along the ray, and the only way to be
        // sure the two agree is to walk one out and watch it arrive.
        let cam = cam();
        for dir in [[0.3, -0.2, 1.0], [-1.4, 0.9, 0.4], [0.0, 0.0, 1.0]] {
            let vanish = cam.vanishing_point(dir).expect("this direction recedes");
            let far = cam
                .project([dir[0] * 1e6, dir[1] * 1e6, dir[2] * 1e6])
                .expect("a point that far along it is in front of the eye");
            assert!(
                (vanish.0 - far.0).abs() < 0.05 && (vanish.1 - far.1).abs() < 0.05,
                "{dir:?} vanishes at {vanish:?} but projects to {far:?}"
            );
        }
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
            field.update(1.0 / 60.0, 400.0, 0.0, 0.0, 0.0, &cam);
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
            field.update(1.0 / 60.0, 400.0, 0.85, -0.6, 1.1, &cam);
            assert_eq!(field.len(), 500);
            for star in &field.stars {
                assert!(star.pos[2] > Z_NEAR, "z went behind us: {}", star.pos[2]);
                assert!(star.pos.iter().all(|c| c.is_finite()));
            }
        }
    }

    #[test]
    fn rolling_turns_the_sky_the_other_way() {
        // A roll to starboard drops the right wing, so the sky swings
        // anticlockwise: a star out on the right-hand horizon rides upward.
        let cam = cam();
        let mut field = StarField::new(1, 5, &cam);
        field.stars[0].pos = [20.0, 0.0, 50.0];
        let before = cam.project(field.stars[0].pos).unwrap();
        for _ in 0..20 {
            field.update(1.0 / 60.0, 0.0, 0.0, 0.0, 1.0, &cam);
        }
        let after = cam.project(field.stars[0].pos).unwrap();
        assert!(after.1 < before.1 - 1.0, "{before:?} then {after:?}");
        assert!(
            after.0 > cam.cx,
            "it should still be on the right: {after:?}"
        );

        // And the radius is preserved: a roll turns the sky, it does not fly
        // the ship anywhere.
        let radius = |p: (f32, f32)| (p.0 - cam.cx).hypot(p.1 - cam.cy);
        assert!((radius(after) - radius(before)).abs() < 0.5);
    }

    #[test]
    fn a_roll_streaks_along_the_arc_it_swept() {
        // Roll turns the stars rather than the projection precisely so that it
        // streaks. A camera that rolled underneath stationary stars would leave
        // every one of them a point.
        let cam = cam();
        let mut field = StarField::new(600, 17, &cam);
        for _ in 0..60 {
            field.update(1.0 / 60.0, 20.0, 0.0, 0.0, 0.0, &cam);
        }
        let length = |f: &StarField| -> f32 {
            f.streaks(&cam, 0.0, 0.0)
                .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
                .sum()
        };
        let coasting = length(&field);
        field.update(1.0 / 60.0, 20.0, 0.0, 0.0, 1.8, &cam);
        assert!(
            length(&field) > coasting * 2.0,
            "rolling should smear the field: {coasting} then {}",
            length(&field)
        );
    }

    #[test]
    fn a_flat_turn_streams_the_sky_flat_across_the_frame() {
        // What the pilot asks for with one key: a turn, and nothing else. It
        // used to come with a climb, because the lean rotated the picture and
        // the sky was steered underneath it, which laid the flow across the
        // frame at the whole bank angle — twenty degrees at the stops, downward
        // for a turn to port and to starboard alike, since the lean's sign
        // follows the yaw's.
        //
        // The mechanism is gone rather than corrected — `Camera` has no bank
        // any more — so this can no longer reproduce that fault, and it is not
        // trying to. It is the standing statement of the property, and it fails
        // for anything that couples the lean back into the sky.
        //
        // The lean is still read off a real `Ship`, because the assertion that
        // it stays out of the picture is worth nothing if the flight model has
        // quietly stopped producing one.
        //
        // Measured in the middle of the frame, where the expansion the travel
        // drives is smallest and the turn is nearly all of what is left.
        for dir in [1.0f32, -1.0] {
            let mut ship = crate::ship::Ship::new();
            for _ in 0..120 {
                ship.nudge_yaw(dir);
                ship.update(1.0 / 60.0);
            }
            assert!(ship.bank.abs() > 0.3, "a held turn should lean hard");

            let cam = cam();
            let mut field = StarField::new(4000, 11, &cam);
            for _ in 0..600 {
                field.update(1.0 / 60.0, 30.0, 0.0, 0.0, 0.0, &cam);
            }
            field.update(1.0 / 60.0, 30.0, ship.yaw_rate, 0.0, 0.0, &cam);

            let (mut dx, mut dy, mut n) = (0.0f32, 0.0f32, 0);
            for star in &field.stars {
                // A star that was recycled has had its trail cleared and did
                // not travel to where it now is, so it says nothing about the
                // flow. `prev` is exactly this frame's starting point for the
                // rest of them.
                let (Some(prev), Some(to)) = (star.prev, cam.project(star.pos)) else {
                    continue;
                };
                if (to.0 - cam.cx).abs() > cam.width * 0.15
                    || (to.1 - cam.cy).abs() > cam.height * 0.15
                {
                    continue;
                }
                dx += to.0 - prev.0;
                dy += to.1 - prev.1;
                n += 1;
            }
            assert!(n > 100, "too few stars in the middle to say anything: {n}");
            let (dx, dy) = (dx / n as f32, dy / n as f32);
            assert!(
                dx.abs() > 0.5 && dx.signum() == -dir,
                "a turn to {dir} should stream the sky the other way: {dx}"
            );
            // A twentieth is under three degrees, against the twenty the lean
            // used to tilt the flow by.
            assert!(
                dy.abs() < dx.abs() * 0.05,
                "steering at {} with the ship leaning {} streamed the sky \
                 ({dx}, {dy}) — {:.1} degrees off flat",
                ship.yaw_rate,
                ship.bank,
                dy.atan2(dx.abs()).to_degrees().abs()
            );
        }
    }

    #[test]
    fn working_the_stick_leaves_the_sky_where_it_found_it() {
        // Regression, and the second fault the lean caused. It used to be a
        // rotation of the *picture* with the sky steered in the frame it turned
        // the picture to, which made a step `R_z(dbank)` in front of the
        // steering — and rotations do not commute. Over a closed loop of the
        // stick that leaves a residue about the camera's own x, and the lean
        // takes its sign from the yaw rate, so the residue goes as the *square*
        // of it: the same way round whichever way the stick went. Working it
        // left and right while parked walked the sky steadily up the frame,
        // which is the nose dropping, at about nine degrees a second, and
        // nothing ever brought it back.
        //
        // Said exactly rather than within a tolerance, which is what the probes
        // are placed by hand for. A yaw turns `x` and `z` and leaves `y` alone
        // to the bit, so a star on `y = 0` projects to exactly `cy` however the
        // stick is worked — and the ship is parked, so nothing recycles and
        // there is no travel to carry one out of the frame.
        //
        // The lean is gone from the camera, so this can no longer reproduce
        // that fault by itself; what it holds is the property the fault broke,
        // and it fails for anything that puts a yaw-coupled angle back into the
        // sky's orientation. It was watched failing against the tree that had
        // one, by rebuilding the camera from `ship.bank` every frame here.
        let cam = cam();
        let mut field = StarField::new(5, 5, &cam);
        for (i, pos) in [
            [0.0, 0.0, 40.0],
            [3.0, 0.0, 30.0],
            [-4.0, 0.0, 55.0],
            [8.0, 0.0, 90.0],
            [-9.0, 0.0, 150.0],
        ]
        .into_iter()
        .enumerate()
        {
            field.stars[i].pos = pos;
        }

        let mut ship = crate::ship::Ship::new();
        ship.throttle = 0.0;
        let dt = 1.0 / 60.0;
        // Eight cycles of hard left and hard right, which is the report, then
        // let go so the rates decay to nothing and the sky settles.
        let worked: Vec<f32> = (0..8)
            .flat_map(|_| [1.0f32; 30].into_iter().chain([-1.0; 30]))
            .chain([0.0; 300])
            .collect();
        for dir in worked {
            if dir != 0.0 {
                ship.nudge_yaw(dir);
            }
            ship.update(dt);
            field.update(
                dt,
                ship.speed,
                ship.yaw_rate,
                ship.pitch_rate,
                ship.roll_rate,
                &cam,
            );
            for star in &field.stars {
                let p = cam.project(star.pos).expect("a parked star stays in front");
                assert_eq!(
                    p.1, cam.cy,
                    "the stick lifted a star off the horizontal: {:?} leaning {}",
                    star.pos, ship.bank
                );
            }
        }
        assert_eq!(ship.speed, 0.0, "the ship should still be parked");
    }

    #[test]
    fn a_hard_turn_does_not_empty_the_edges_of_the_frame() {
        // Regression: every star that left the frustum went back to the far
        // plane, where it is invisible and, at impulse, a good half minute from
        // being seen again. A turn sweeps the whole field out sideways, so the
        // edges of the frame — a third of it, on a wide terminal — simply went
        // dark and stayed dark. Roll sweeps every edge at once and made it
        // obvious, but yaw and pitch had it too.
        let cam = cam();
        // The outer sixth on each side: the first part to go, and the part a
        // rectangular frustum is least generous with.
        let edges = |field: &StarField| -> (usize, usize) {
            let mut left = 0;
            let mut right = 0;
            for s in field.streaks(&cam, 0.0, 0.0).filter(|s| s.intensity > 0.05) {
                let (x, y) = s.to;
                if !(0.0..cam.height).contains(&y) {
                    continue;
                }
                if (0.0..cam.width / 6.0).contains(&x) {
                    left += 1;
                } else if (cam.width * 5.0 / 6.0..cam.width).contains(&x) {
                    right += 1;
                }
            }
            (left, right)
        };

        // Impulse, where the field turns over slowly enough for a hole to last.
        let speed = 8.0;
        for (yaw, pitch, roll) in [(0.0, 0.0, 1.8), (0.85, 0.0, 0.0), (0.0, 0.6, 0.0)] {
            let mut field = StarField::new(4000, 42, &cam);
            for _ in 0..1200 {
                field.update(1.0 / 60.0, speed, 0.0, 0.0, 0.0, &cam);
            }
            let (l0, r0) = edges(&field);
            assert!(l0 > 40 && r0 > 40, "the settled field is thin: {l0}, {r0}");

            // Two full seconds of it, which for the roll is most of a turn.
            for _ in 0..120 {
                field.update(1.0 / 60.0, speed, yaw, pitch, roll, &cam);
            }
            let (l1, r1) = edges(&field);
            assert!(
                l1 * 2 > l0 && r1 * 2 > r0,
                "steering at ({yaw}, {pitch}, {roll}) emptied the edges: \
                 {l0} left and {r0} right became {l1} and {r1}"
            );
        }
    }

    #[test]
    fn a_star_swung_out_of_view_comes_back_off_camera() {
        // The replacement sky enters from beyond the edge of the *screen*, not
        // in the middle of it: a star that popped into an occupied frame would
        // read as a rendering fault rather than as sky coming round.
        let cam = cam();
        let mut field = StarField::new(3000, 8, &cam);
        for _ in 0..600 {
            field.update(1.0 / 60.0, 8.0, 0.0, 0.0, 0.0, &cam);
        }
        let mut swung_back = 0;
        for _ in 0..240 {
            field.update(1.0 / 60.0, 8.0, 0.0, 0.0, 1.8, &cam);
            for star in &field.stars {
                // A star that was put back anywhere has had its trail cleared.
                // The ones sent to the far plane are allowed on screen — they
                // are invisible out there and fade up as they come — so only
                // the ones kept at their own depth are of interest.
                if star.prev.is_some() || star.pos[2] >= Z_FAR * 0.92 {
                    continue;
                }
                swung_back += 1;
                let Some((x, y)) = cam.project(star.pos) else {
                    continue;
                };
                assert!(
                    x < 0.0 || x >= cam.width || y < 0.0 || y >= cam.height,
                    "a star reappeared on screen at ({x}, {y})"
                );
            }
        }
        assert!(swung_back > 100, "the roll swung nothing out: {swung_back}");
    }

    #[test]
    fn most_of_the_star_budget_lands_on_screen() {
        // The whole point of the rectangular frustum: stars the viewer will
        // never see are wasted, and at sublight that is the difference
        // between a sky and an empty screen.
        let cam = cam();
        let mut field = StarField::new(3000, 21, &cam);
        for _ in 0..400 {
            field.update(1.0 / 60.0, 30.0, 0.0, 0.0, 0.0, &cam);
        }
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
    fn a_sublight_sky_is_not_black() {
        // Regression: an over-steep depth falloff made everything but the
        // nearest stars invisible, so cruising looked like a dead terminal.
        let cam = cam();
        let mut field = StarField::new(2000, 13, &cam);
        for _ in 0..900 {
            field.update(1.0 / 60.0, 25.0, 0.0, 0.0, 0.0, &cam);
        }
        let lit = field
            .streaks(&cam, 0.0, 0.0)
            .filter(|s| s.intensity > 0.05)
            .count();
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
        field.update(1.0 / 60.0, 0.0, 0.0, 0.0, 0.0, &cam);
        field.update(1.0 / 60.0, 0.0, 0.0, 0.0, 0.0, &cam);
        let still: f32 = field
            .streaks(&cam, 0.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(still < 1.0, "a parked ship should not streak: {still}");

        for _ in 0..30 {
            field.update(1.0 / 60.0, crate::ship::WARP_MAX, 0.0, 0.0, 0.0, &cam);
        }
        let moving: f32 = field
            .streaks(&cam, 1.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(moving > 100.0, "warp should streak: {moving}");
    }

    #[test]
    fn a_retargeted_field_draws_no_trail_from_the_old_projection() {
        // Regression: `retarget` took the new canvas and left every star's
        // `prev` where the old projection had put it, so the next frame drew
        // the whole field as streaks running from one to the other. `cx`, `cy`
        // and `focal` all move with the canvas, so this was the sky, not a few
        // stars near an edge. `update` refreshes `prev` before it draws, which
        // held it to a single frame and is why it went unseen — but pause gates
        // `advance`, and a resize taken while paused left the scratches up
        // until the flight was let go again. `resizing_mid_flight_is_survivable`
        // in `render.rs` steps the field between the retarget and the draw,
        // which is exactly what masked it; this one draws straight after.
        let cam = cam();
        let mut field = StarField::new(400, 3, &cam);
        for _ in 0..30 {
            field.update(1.0 / 60.0, crate::ship::WARP_MAX, 0.0, 0.0, 0.0, &cam);
        }

        let resized = Camera::new(320, 60);
        field.retarget(&resized);
        let drawn: f32 = field
            .streaks(&resized, 1.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert_eq!(
            drawn, 0.0,
            "a field drawn straight after a retarget trailed from the \
             projection it was retargeted away from"
        );
    }

    #[test]
    fn the_twinkle_still_moves_after_days_of_flight() {
        // Regression: the twinkle phase was `(time * 2.3 + phase)` computed in
        // `f32` from an `f32` clock. It went coarse within hours and stopped
        // advancing entirely after a few days, freezing the sky solid.
        let cam = cam();
        let mut field = StarField::new(400, 3, &cam);
        for _ in 0..10 {
            field.update(1.0 / 60.0, 20.0, 0.0, 0.0, 0.0, &cam);
        }
        let sample = |time: f64| -> Vec<f32> {
            field
                .streaks(&cam, 0.0, time)
                .map(|s| s.intensity)
                .collect()
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
        assert!(
            ahead[2] > ahead[0],
            "dead ahead should read blue: {ahead:?}"
        );
        let edge = shift_color(g, 0.05, 1.0);
        assert!(edge[0] > edge[2], "the periphery should read red: {edge:?}");
    }

    #[test]
    fn colours_stay_in_gamut() {
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
