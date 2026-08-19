//! The sky, as a volume of space the ship is somewhere inside rather than as a
//! box in front of the camera.
//!
//! Two decisions carry the weight here and everything else follows from them.
//!
//! **The stars are in the world, not on the screen.** Both fields this replaces
//! sampled a position *on the canvas* and back-projected it through a depth,
//! which keeps apparent density even at every range and is the reason a camera
//! swung ninety degrees round the ship used to show the same sky it had been
//! showing: the pool was glued to the eye. A world-space pool laid out
//! isotropically has no preferred axis to reveal, so no angle can turn up a
//! thin patch, and a swing moves nothing at all — it re-projects. That deletes
//! the fold, the rim, the near cap, the entering-surface sampler and the five
//! fast paths the band needed to hide the seams a swing tore open.
//!
//! **The distances are real, so the parallax is too.** A star is placed by its
//! own photometry: pick what kind of star it is, pick how far into its own
//! visibility sphere it sits, and its brightness is then the inverse square of
//! where that put it. The nearest come out about ten light years off and the
//! brightest giants thousands, which is what makes impulse look like nothing
//! happening — at 0.9 c the nearest star crawls a hundredth of a subpixel a
//! second — while warp, at five and a half light years a second, tears the near
//! sky past a far one that barely moves. That difference *is* the depth cue,
//! and the old sky, whose whole volume was three hundred ship-lengths deep,
//! could not have one.

use crate::camera::{Camera, Streak};
use crate::view::Eye;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::f32::consts::TAU;

/// How the number of stars brighter than a magnitude grows with it.
///
/// `N(<m)` proportional to `10^(0.6 m)` is what a uniform density of stars in
/// space gives: five magnitudes is ten times the distance, a thousand times the
/// volume, and `10^(0.6·5)` is a thousand. The measured sky is flatter than
/// that — about 0.5, because the Galaxy is a disc that thins out rather than an
/// infinite even scatter — and 0.5 is *not* what belongs here, for the same
/// reason the exterior band settled on the one profile a flow leaves alone: a
/// rigid translation through space preserves a uniform density and nothing
/// else. Sample the flatter law and the sky would relax into this one over the
/// first minutes of a flight, thickening as it went.
const COUNT_SLOPE: f32 = 0.6;

/// The magnitude the census below is quoted at, and how many stars are brighter
/// than it over the whole celestial sphere.
///
/// 6.5 is the naked-eye limit under a dark sky and about 9 100 stars beat it —
/// the figure star atlases are cut at. Everything else scales off the pair:
/// `--magnitude` moves the limit and [`Universe::population`] moves the count
/// with it.
const REFERENCE_LIMIT: f32 = 6.5;
const REFERENCE_COUNT: f32 = 9_110.0;

/// The apparent magnitude that lands on one unit of canvas light.
///
/// Derived rather than chosen, and derived against what the renderer used to
/// draw so `--exposure` does not have to move. A typical star under the old
/// model laid down `luminosity·magnitude·depth^1.4`, which averages
/// `1.05 · 0.355 · 0.417 = 0.155`. Under the count law the mean of
/// `10^(-0.4 m)` over everything brighter than the limit is
/// `slope/(slope − 0.4) · 10^(-0.4 · limit)`, or `0.00754` at 6.5 — so the two
/// agree when `10^(0.4 z) = 20.6`, which is this.
///
/// What it puts on the canvas: a star at the limit comes out at 0.05, the
/// median one at 0.08, a fourth-magnitude star at 0.5, and the handful brighter
/// than second magnitude past 3 and into the tonemap's shoulder. That last
/// group is meant to saturate. There are eighteen of them in the whole sky.
const ZERO_POINT: f32 = 3.28;

/// How much of the faint end is spent fading up out of nothing, in magnitudes.
///
/// A star leaves the catalogue by crossing its own visibility sphere, and it
/// crosses at exactly the limiting magnitude — the faintest anything here can
/// be. Faint is not nothing, though, and a star winking out at 0.05 of a unit
/// is a star winking out. The ramp is squared so it meets zero with zero slope,
/// which is the courtesy the old far plane paid with `depth.powf(1.4)` and for
/// exactly the same reason: stars should fade up out of the dark rather than
/// blink into it.
const FADE_MAGNITUDES: f32 = 0.75;

/// How long an exposure a lit warp drive draws the sky with, in seconds of
/// flight, at full warp.
///
/// This is the constant that keeps the tunnel, and it replaces a multiplier
/// that cannot survive honest distances. The old sky was three hundred
/// ship-lengths deep and a star crossed a good fraction of it every step, so
/// stretching one step's motion by six drew a streak across the frame. Out here
/// a step of 1/120 s moves a ten-light-year star three subpixels, and six of
/// those is a smear rather than a track.
///
/// So the streak stops being a stretch and becomes what it always looked like:
/// the track a star actually flew, over the last fifth of a second. On an 80x24
/// terminal at full warp that is 35 subpixels for a star at ten light years, 21
/// at seventy, 8 at three hundred and 2.7 at a thousand — near stars tearing
/// past, mid ones dashing, far ones holding still as points. The old sky drew
/// every streak the same length because every star was the same distance away.
///
/// A fifth of a second because the exposure wants to be about half the canvas
/// for a typical star, and it is a *time* rather than a length so it stays that
/// fraction on every terminal: `Camera::focal` follows the canvas height and so
/// does the canvas. It was three seconds while the ship covered 5.5 ly a
/// second; at 383 that would reach eleven hundred light years, past most of the
/// sky, and every trail would end on the vanishing point.
const TRAIL_SECONDS: f32 = 0.2;

/// Nearest any star is, in light years.
///
/// Proxima Centauri, rounded down, and it is a fact about space rather than a
/// safety margin: there is nothing between here and 4.2 light years, so a model
/// that puts a star closer is drawing something that does not exist.
///
/// It is needed because the catalogue is built from photometry rather than from
/// a volume. A star's distance falls out of how bright it is and how bright it
/// looks, so the faint end of the spread — an M dwarf four deviations down —
/// comes out with a visibility sphere under two light years across and is
/// therefore always very close, whatever the limit. Those are the stars that
/// tore past at impulse, thirty seconds of it moving one twelve thousand
/// subpixels, in a sky whose whole point is to hold still down there.
///
/// The answer is to leave them out rather than to move them: a star whose
/// sphere does not reach this far is not visible from anywhere it could
/// actually be, so [`Universe::spawn`] draws another. That shifts the census
/// toward the luminous classes as the limit comes down, which is not a side
/// effect but the observation — a magnitude-two sky really is all giants, and
/// the M dwarfs really are all too faint to be in it.
const NEAREST_STAR: f32 = 4.0;

/// Nearest a star is projected from, in light years.
///
/// Not the hull's near plane. `crate::starfield::Z_NEAR` is 0.9 in units where
/// the ship is about one, and a star that came within nine tenths of a light
/// year would be clipped by a number that has nothing to do with it — so the
/// projection is asked for this one instead. A star this close is passing
/// through the ship in all but name and there is nothing sensible left to draw.
const STAR_NEAR: f32 = 0.001;

/// A parsec, in light years, and ten of them — the distance a star's absolute
/// magnitude is quoted at, so this is the whole of the distance modulus.
const PARSEC_LY: f32 = 3.261_563;
const TEN_PARSECS_LY: f32 = PARSEC_LY * 10.0;

/// How far either side of its class's absolute magnitude a star may fall.
///
/// **Not decoration.** With one magnitude per class a star's brightness would
/// *determine* its distance, so at warp every equally bright star would move at
/// exactly the same rate and brightness would read as speed. A magnitude and a
/// half of scatter is roughly what the real population carries and is enough to
/// break the correlation.
const ABSOLUTE_SPREAD: f32 = 1.5;

/// How many times [`Universe::spawn`] will redraw a star too faint to be as far
/// off as the nearest one really is.
const TRIES: usize = 32;

/// A star's spectral class: what colour it burns, how bright it really is, and
/// how much of the sky is made of it.
#[derive(Debug, Clone, Copy)]
pub struct SpectralClass {
    pub rgb: [f32; 3],
    /// Absolute visual magnitude — brightness at ten parsecs, which together
    /// with how bright a star *looks* is the whole of how far away it is.
    ///
    /// These are the *naked-eye sky's* numbers rather than the main sequence's,
    /// and the difference is the point: what fills a real sky is apparent
    /// brightness, so its O, B and A stars are mostly giants seen from hundreds
    /// or thousands of light years rather than dwarfs seen from next door. It
    /// replaced a plain multiplier on a star's brightness, which could say one
    /// star was three times another and could say nothing whatever about where
    /// either of them was.
    pub absolute: f32,
    /// Relative frequency in the sky. Visible to the crate because
    /// [`Universe`] draws from this census.
    pub(crate) weight: f32,
}

/// Roughly the colours of the main sequence. The weights lean toward the hot
/// end relative to a true census, because what fills a real sky is apparent
/// brightness, and a field of honest M dwarfs is a dim red smudge — and the
/// absolute magnitudes lean the same way and for the same reason.
pub const CLASSES: [SpectralClass; 7] = [
    // O
    SpectralClass {
        rgb: [0.61, 0.69, 1.00],
        absolute: -5.0,
        weight: 1.0,
    },
    // B
    SpectralClass {
        rgb: [0.67, 0.75, 1.00],
        absolute: -1.6,
        weight: 5.0,
    },
    // A
    SpectralClass {
        rgb: [0.79, 0.84, 1.00],
        absolute: 0.9,
        weight: 14.0,
    },
    // F
    SpectralClass {
        rgb: [0.97, 0.97, 1.00],
        absolute: 2.6,
        weight: 18.0,
    },
    // G
    SpectralClass {
        rgb: [1.00, 0.96, 0.92],
        absolute: 4.4,
        weight: 22.0,
    },
    // K
    SpectralClass {
        rgb: [1.00, 0.82, 0.63],
        absolute: 5.6,
        weight: 24.0,
    },
    // M
    SpectralClass {
        rgb: [1.00, 0.65, 0.44],
        absolute: 8.2,
        weight: 16.0,
    },
];

/// Colour a star shifts toward when it is dead ahead and you are moving fast.
const BLUE_SHIFT: [f32; 3] = [0.45, 0.66, 1.00];
/// Colour a star shifts toward out at the edges, where it is falling behind.
const RED_SHIFT: [f32; 3] = [1.00, 0.42, 0.24];

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

/// One star, in the inertial world frame.
#[derive(Debug, Clone, Copy)]
struct Star {
    /// Light years. Never moved and never rotated: the ship carries the
    /// position and the attitude, which is what keeps a step of 2e-5 ly out of
    /// an `f32` that is already a thousand.
    pos: [f32; 3],
    class: usize,
    /// Brightness at unit distance, in canvas light — the whole of this star's
    /// photometry, folded once at spawn so drawing it is one divide.
    lumen: f32,
    /// The square of how far away it may be and still beat the limit. Also
    /// folded, and it is the same number: `lumen / limiting intensity`.
    reach_sq: f32,
    phase: f32,
}

/// Where the sky is being watched from, and how fast.
///
/// A bundle rather than five arguments threaded side by side, and the honest
/// unit anyway — the universe has no opinion about orbits or hulls, only about
/// where an eye is and which way it is going.
#[derive(Debug, Clone, Copy)]
pub struct Observer {
    /// Rows are the camera's axes in the world frame, so a world offset becomes
    /// camera coordinates in three dot products.
    to_camera: [[f32; 3]; 3],
    /// Where the ship is, in light years. `f64` for the reason
    /// [`crate::ship::Ship::position`] gives.
    origin: [f64; 3],
    /// Which way the ship is going, in the camera's space. Exactly `(0, 0, 1)`
    /// from the cockpit.
    nose: [f32; 3],
    /// 0..=1 across the superluminal range: it sets the exposure, the Doppler
    /// shift and how much twinkle is left.
    warp: f32,
    /// Light years per second.
    velocity: f32,
}

impl Observer {
    /// From the pilot's seat, where the camera is the hull.
    pub fn cockpit(axes: [[f32; 3]; 3], origin: [f64; 3], warp: f32, velocity: f32) -> Self {
        Self {
            to_camera: axes,
            origin,
            nose: [0.0, 0.0, 1.0],
            warp,
            velocity,
        }
    }

    /// From outside, where the camera is swung round the hull by the orbit.
    ///
    /// The eye's *standoff* is deliberately not applied. `Eye::to_camera` adds
    /// nine to fifteen hull units of it, and a hull is a few hundred metres, so
    /// against a sky measured in light years that is a translation of about
    /// 1e-13 of a star's range — no parallax at all, and applying it would be a
    /// unit error rather than a refinement. The hull still goes through
    /// `Eye::to_camera` with the standoff in it, because for the hull it is the
    /// whole of the framing.
    pub fn outside(
        axes: [[f32; 3]; 3],
        origin: [f64; 3],
        eye: &Eye,
        nose: [f32; 3],
        warp: f32,
        velocity: f32,
    ) -> Self {
        let mut to_camera = [[0.0f32; 3]; 3];
        for (row, turn) in to_camera.iter_mut().zip(eye.basis) {
            for (place, axis) in row.iter_mut().enumerate() {
                *axis =
                    turn[0] * axes[0][place] + turn[1] * axes[1][place] + turn[2] * axes[2][place];
            }
        }
        Self {
            to_camera,
            origin,
            nose,
            warp,
            velocity,
        }
    }

    /// A star's world position in this camera's space.
    fn place(&self, pos: [f32; 3]) -> [f32; 3] {
        let rel = [
            (pos[0] as f64 - self.origin[0]) as f32,
            (pos[1] as f64 - self.origin[1]) as f32,
            (pos[2] as f64 - self.origin[2]) as f32,
        ];
        let [r, d, f] = self.to_camera;
        [
            rel[0] * r[0] + rel[1] * r[1] + rel[2] * r[2],
            rel[0] * d[0] + rel[1] * d[1] + rel[2] * d[2],
            rel[0] * f[0] + rel[1] * f[1] + rel[2] * f[2],
        ]
    }
}

/// Every star the observer could see, wherever the camera is pointed.
pub struct Universe {
    stars: Vec<Star>,
    rng: StdRng,
    limit: f32,
}

impl Universe {
    pub fn new(limit: f32, seed: u64) -> Self {
        let mut sky = Self {
            stars: Vec::new(),
            rng: StdRng::seed_from_u64(seed),
            limit,
        };
        sky.stock();
        sky
    }

    /// How many stars beat a limiting magnitude, over the whole sphere.
    ///
    /// Saturated rather than allowed to wrap: `--magnitude` is bounded where it
    /// is parsed, but this is `pub` and a caller with a preposterous limit
    /// should get a preposterous number rather than a small one.
    pub fn population(limit: f32) -> usize {
        let n = REFERENCE_COUNT * 10f32.powf(COUNT_SLOPE * (limit - REFERENCE_LIMIT));
        if n.is_finite() && n >= 1.0 {
            n as usize
        } else {
            0
        }
    }

    /// The faintest star this sky holds.
    pub fn limit(&self) -> f32 {
        self.limit
    }

    /// How many it holds, over the whole sphere — most of which is behind the
    /// camera at any moment, which is the price of an isotropic sky and the
    /// whole of what a swing costs.
    pub fn len(&self) -> usize {
        self.stars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stars.is_empty()
    }

    /// Where every star is, for a test that wants to say a thing did or did not
    /// move the sky. Positions rather than the stars themselves, because what
    /// those tests are about is the one field a camera could conceivably
    /// disturb.
    pub fn positions(&self) -> Vec<[f32; 3]> {
        self.stars.iter().map(|s| s.pos).collect()
    }

    /// Ask for a different limiting magnitude.
    ///
    /// Every star's reach scales by the same factor, since a reach is a lumen
    /// over a limiting intensity and only the second moved, so the pool that is
    /// already out there stays where it is and only the count changes.
    pub fn set_limit(&mut self, limit: f32) {
        if limit == self.limit {
            return;
        }
        let scale = 10f32.powf(0.4 * (limit - self.limit));
        for star in &mut self.stars {
            star.reach_sq *= scale;
        }
        self.limit = limit;
        self.stock();
    }

    /// Grow or shrink the pool to what the limit asks for.
    fn stock(&mut self) {
        let wanted = Self::population(self.limit);
        self.stars.truncate(wanted);
        while self.stars.len() < wanted {
            let star = self.spawn();
            self.stars.push(star);
        }
    }

    /// Step the sky: take away whatever has fallen past the limit and put the
    /// same number back where the flow is bringing them in from.
    ///
    /// The ship does the moving — see [`crate::ship::Ship::coast`] — so there
    /// is nothing to translate here and no screen for anything to fall off.
    /// What is left is one distance test per star.
    pub fn advance(&mut self, origin: [f64; 3], nose: [f32; 3]) {
        let mut stars = std::mem::take(&mut self.stars);
        for star in &mut stars {
            if distance_sq(star.pos, origin) <= star.reach_sq {
                continue;
            }
            let reach = star.reach_sq.sqrt();
            let dir = self.entering(nose);
            star.pos = [
                (origin[0] + (dir[0] * reach) as f64) as f32,
                (origin[1] + (dir[1] * reach) as f64) as f32,
                (origin[2] + (dir[2] * reach) as f64) as f32,
            ];
            star.phase = self.rng.random_range(0.0..TAU);
        }
        self.stars = stars;
    }

    /// Turn the sky into drawable segments for this frame.
    ///
    /// `time` feeds the sublight twinkle and nothing else.
    pub fn streaks<'a>(
        &'a self,
        cam: &'a Camera,
        eye: &'a Observer,
        time: f64,
    ) -> impl Iterator<Item = Streak> + 'a {
        // How far back along the track the exposure reaches, as a displacement
        // in the camera's own space. Exactly zero at sublight, where there is
        // no drive lit to smear anything and the sky is a fixed backdrop.
        //
        // The warp ramp is in here as the switch rather than as a second
        // helping of the throttle. It used to be both: while the throttle was
        // linear in `Ship::speed` this multiplied an already-exponential
        // velocity by a linear ramp, so the trail shortened *fastest* exactly
        // where there was least motion to see it — a fiftieth of a subpixel at
        // the moment the drive lit. With the stick shaped in warp factors the
        // ramp spans 0.46 to 1.0 and is very nearly a constant, which is all it
        // was ever meant to be.
        let reach = TRAIL_SECONDS * eye.warp * eye.velocity;
        let back = [
            eye.nose[0] * reach,
            eye.nose[1] * reach,
            eye.nose[2] * reach,
        ];
        // `FADE_MAGNITUDES` expressed as the brightness ratio the arithmetic
        // below actually works in, taken once for the frame rather than written
        // out as a second constant that could disagree with the first.
        let fade_ratio = 10f32.powf(0.4 * FADE_MAGNITUDES);
        let doppler = eye.warp * 0.9;
        let twinkle_amt = (1.0 - eye.warp * 3.0).clamp(0.0, 1.0) * 0.22;
        // Folded into a single turn once per frame, in `f64`, so the per-star
        // `sin` below can stay `f32` without the phase going coarse — and
        // eventually static — once the process has been up for days.
        let twinkle_phase = (time * 2.3).rem_euclid(std::f64::consts::TAU) as f32;

        self.stars.iter().filter_map(move |star| {
            let pos = eye.place(star.pos);
            let to = cam.project_beyond(pos, STAR_NEAR)?;

            let range_sq = pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2];
            let brightness = star.lumen / range_sq.max(f32::MIN_POSITIVE);
            // One ratio answers two questions: it is above one exactly while
            // the star still beats the limit, and how far above says how much
            // of the fade is left to climb.
            let over = star.reach_sq / range_sq.max(f32::MIN_POSITIVE);
            let fade = ((over - 1.0) / (fade_ratio - 1.0)).clamp(0.0, 1.0);
            let twinkle = if twinkle_amt > 0.0 {
                1.0 + twinkle_amt * (twinkle_phase + star.phase).sin()
            } else {
                1.0
            };
            let intensity = brightness * fade * fade * twinkle;
            if intensity <= 0.0 {
                return None;
            }

            // Where the star was when the exposure opened. Going back along the
            // track moves it *away* from the eye, so this cannot fall behind
            // the near plane however long the exposure is — which is why there
            // is no clamp here and no vanishing point to chase.
            let from = if reach > 0.0 {
                cam.project_beyond(
                    [pos[0] + back[0], pos[1] + back[1], pos[2] + back[2]],
                    STAR_NEAR,
                )
                .unwrap_or(to)
            } else {
                to
            };

            // How far off the nose it sits. Dead ahead blue-shifts; astern,
            // where it is falling behind, it reddens.
            let along = pos[0] * eye.nose[0] + pos[1] * eye.nose[1] + pos[2] * eye.nose[2];
            let forward = if range_sq > f32::MIN_POSITIVE {
                (0.5 + 0.5 * along / range_sq.sqrt()).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let color = shift_color(CLASSES[star.class].rgb, forward, doppler);

            Some(Streak {
                from,
                to,
                color,
                intensity,
            })
        })
    }

    /// Make a star, somewhere uniform in its own visibility sphere.
    ///
    /// The draw order is load-bearing rather than incidental — `--seed`
    /// reproducibility is a property of the sequence — and it is: class, then
    /// the three that scatter its absolute magnitude, then how deep into its
    /// sphere it sits, then the two that point it, then its twinkle.
    fn spawn(&mut self) -> Star {
        // Bounded rather than a bare loop. Nothing here can reject forever —
        // the brightest class clears [`NEAREST_STAR`] by three orders of
        // magnitude at any limit worth flying — but a rejection loop with no
        // ceiling is a hang waiting for a constant to move under it, and the
        // frames this would cost are a sky slightly off its census rather than
        // a program that stops.
        let mut class = self.pick_class();
        let mut absolute = CLASSES[class].absolute + ABSOLUTE_SPREAD * self.scatter();
        let (mut lumen, mut reach_sq) = self.photometry(absolute);
        for _ in 0..TRIES {
            if reach_sq >= NEAREST_STAR * NEAREST_STAR {
                break;
            }
            class = self.pick_class();
            absolute = CLASSES[class].absolute + ABSOLUTE_SPREAD * self.scatter();
            (lumen, reach_sq) = self.photometry(absolute);
        }
        // Uniform by volume, which is the cube root: the only profile a rigid
        // flow through it returns unchanged, and therefore the only one the
        // recycle below does not slowly walk the sky away from. Over the
        // *shell* rather than the whole sphere, because the hole in the middle
        // is [`NEAREST_STAR`] and it is really there.
        let reach = reach_sq.sqrt();
        let depth: f32 = self.rng.random_range(0.0..1.0);
        let hole = (NEAREST_STAR / reach).min(1.0).powi(3);
        let radius = reach * (hole + (1.0 - hole) * depth).cbrt();
        let dir = self.anywhere();
        Star {
            pos: [dir[0] * radius, dir[1] * radius, dir[2] * radius],
            class,
            lumen,
            reach_sq,
            phase: self.rng.random_range(0.0..TAU),
        }
    }

    /// A star's brightness at unit distance, and the square of how far off it
    /// may be and still be worth drawing.
    ///
    /// Both fall out of the distance modulus once, at spawn, so nothing at draw
    /// time has to take a logarithm: apparent magnitude is
    /// `absolute + 5·log10(d / 10 pc)`, so intensity is
    /// `10^(-0.4(absolute − ZERO_POINT)) · (10 pc / d)²`, and the reach is
    /// where that falls to what the limit allows.
    fn photometry(&self, absolute: f32) -> (f32, f32) {
        let at_ten = 10f32.powf(-0.4 * (absolute - ZERO_POINT));
        let lumen = at_ten * TEN_PARSECS_LY * TEN_PARSECS_LY;
        let faintest = 10f32.powf(-0.4 * (self.limit - ZERO_POINT));
        (lumen, lumen / faintest.max(f32::MIN_POSITIVE))
    }

    /// A sample from a bell about zero, of unit width and bounded at three.
    ///
    /// Three uniforms rather than a Box-Muller pair, and the bound is the
    /// reason rather than the cost: a Gaussian's tail is unbounded, and an
    /// absolute magnitude six deviations out would put one star three hundred
    /// times further away than its class belongs and hand the pool a radius
    /// nothing else comes near.
    fn scatter(&mut self) -> f32 {
        let a: f32 = self.rng.random_range(0.0..1.0);
        let b: f32 = self.rng.random_range(0.0..1.0);
        let c: f32 = self.rng.random_range(0.0..1.0);
        (a + b + c - 1.5) * 2.0
    }

    /// A direction uniform over the whole sphere.
    fn anywhere(&mut self) -> [f32; 3] {
        let up: f32 = self.rng.random_range(-1.0..1.0);
        let turn: f32 = self.rng.random_range(0.0..TAU);
        around(up, turn)
    }

    /// A direction on the hemisphere the flow is entering through, distributed
    /// by how much of it crosses there.
    ///
    /// A uniform flow crossing a sphere delivers stars in proportion to the
    /// cosine of the angle from the direction it is coming from, so the sampler
    /// is the square root of a uniform rather than a uniform. Get that wrong
    /// and the sky refills evenly over a hemisphere the flow only grazes at the
    /// edges, which piles stars up around the rim of the frame.
    ///
    /// Nothing arrives in view: a star put back at its own reach is by
    /// construction at exactly the limiting magnitude, where the fade in
    /// [`Self::streaks`] has it at nothing at all.
    fn entering(&mut self, nose: [f32; 3]) -> [f32; 3] {
        let u: f32 = self.rng.random_range(0.0..1.0);
        let turn: f32 = self.rng.random_range(0.0..TAU);
        let along = u.sqrt();
        let out = around(along, turn);
        // Turned onto the nose. Built from the nose rather than from a fixed
        // axis so there is no pole for the frame to go singular at.
        let (right, down) = frame(nose);
        [
            right[0] * out[0] + down[0] * out[1] + nose[0] * out[2],
            right[1] * out[0] + down[1] * out[1] + nose[1] * out[2],
            right[2] * out[0] + down[2] * out[1] + nose[2] * out[2],
        ]
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
}

/// A unit vector `up` of the way along `+z`, turned `turn` about it.
fn around(up: f32, turn: f32) -> [f32; 3] {
    let flat = (1.0 - up * up).max(0.0).sqrt();
    let (s, c) = turn.sin_cos();
    [flat * c, flat * s, up]
}

/// Two directions square to `axis` and to each other, so a hemisphere sampled
/// about `+z` can be turned onto it.
///
/// Built off whichever cardinal `axis` leans on least, which is what keeps the
/// cross product away from zero however `axis` is pointed.
fn frame(axis: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let aside = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let mut right = [
        aside[1] * axis[2] - aside[2] * axis[1],
        aside[2] * axis[0] - aside[0] * axis[2],
        aside[0] * axis[1] - aside[1] * axis[0],
    ];
    let len = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
    let inv = if len > f32::MIN_POSITIVE {
        1.0 / len
    } else {
        1.0
    };
    for part in &mut right {
        *part *= inv;
    }
    let down = [
        axis[1] * right[2] - axis[2] * right[1],
        axis[2] * right[0] - axis[0] * right[2],
        axis[0] * right[1] - axis[1] * right[0],
    ];
    (right, down)
}

/// How far a world-space star is from a world-space ship, squared.
fn distance_sq(pos: [f32; 3], origin: [f64; 3]) -> f32 {
    let x = (pos[0] as f64 - origin[0]) as f32;
    let y = (pos[1] as f64 - origin[1]) as f32;
    let z = (pos[2] as f64 - origin[2]) as f32;
    x * x + y * y + z * z
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship::Ship;

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
        assert!(CLASSES
            .iter()
            .all(|c| c.weight > 0.0 && c.absolute.is_finite()));
    }

    /// The cockpit's camera on the canvas an 80x24 terminal gives, which is
    /// what the subpixel figures quoted through this module are measured on.
    fn cam() -> Camera {
        Camera::new(80, 48)
    }

    fn seated(ship: &Ship) -> Observer {
        Observer::cockpit(
            ship.axes,
            ship.position,
            ship.warp_intensity(),
            ship.velocity_ly_per_s(),
        )
    }

    /// Fly a sky and a ship together for `steps` of a sim step.
    fn fly(sky: &mut Universe, ship: &mut Ship, steps: usize) {
        for _ in 0..steps {
            ship.update(1.0 / 120.0);
            sky.advance(ship.position, ship.axes[2]);
        }
    }

    #[test]
    fn no_direction_holds_a_thinner_sky_than_any_other() {
        // The acceptance test for the fault this module was written to fix: a
        // camera swung round the ship used to turn up a sparse patch, because
        // the pool was a rectangle laid out in front of whichever eye was
        // flying and a swing had to re-lay it. Asked of the *pool* rather than
        // of a drawn frame, and that is the point rather than convenience — a
        // frame at any sane star count is Poisson-noisy at better than two to
        // one between its fullest and emptiest corner, so a frame cannot tell
        // an anisotropy from an ordinary evening.
        //
        // Two hundred directions spread over the sphere, counting the stars
        // within a cone of 0.2 sr about each. At `--magnitude 8` that is 91 100
        // stars and about 1 450 a cone, so the standard deviation is 38 and the
        // bound is five of them — tight enough that a sky thin anywhere by a
        // seventh fails, loose enough that nothing flakes.
        let sky = Universe::new(8.0, 20);
        let cone = 0.2f32;
        let cos_half = 1.0 - cone / (2.0 * std::f32::consts::PI);
        let expected = sky.len() as f32 * cone / (4.0 * std::f32::consts::PI);

        let mut worst = f32::MAX;
        let mut best = 0.0f32;
        for i in 0..200 {
            // A Fibonacci spiral: as even a spread of directions over a sphere
            // as 200 points can be, and no two of them clustered the way a
            // latitude-longitude grid would be at the poles.
            let up = 1.0 - 2.0 * (i as f32 + 0.5) / 200.0;
            let turn = i as f32 * 2.399_963_2;
            let axis = around(up, turn);
            let mut n = 0.0f32;
            for star in &sky.stars {
                let d = (star.pos[0] * star.pos[0]
                    + star.pos[1] * star.pos[1]
                    + star.pos[2] * star.pos[2])
                    .sqrt();
                if d <= f32::MIN_POSITIVE {
                    continue;
                }
                let along =
                    (star.pos[0] * axis[0] + star.pos[1] * axis[1] + star.pos[2] * axis[2]) / d;
                if along >= cos_half {
                    n += 1.0;
                }
            }
            worst = worst.min(n);
            best = best.max(n);
        }
        let slack = 5.0 * expected.sqrt();
        assert!(
            worst > expected - slack && best < expected + slack,
            "the sky runs from {worst} to {best} a cone against {expected} expected"
        );
    }

    #[test]
    fn the_sky_holds_still_at_impulse() {
        // What the user asked for, as a number, and it is a statement about the
        // *sky* rather than about every star in it. At 0.9 c the ship covers
        // 0.17 ly a second, so in thirty seconds it puts a fifth of a light
        // year behind it. Against the median star at seventy that is three
        // subpixels; against the nearest at four it is twenty, and that is not
        // a fault to be tuned away — a star four light years off really does
        // have proper motion you could watch, and the catalogue is built from
        // real distances now.
        //
        // So the bound is on the median and the margin is deliberately tight.
        // A threshold ten times clear would go on passing a speed map that had
        // regressed by an order of magnitude, which is exactly the fault this
        // family of tests exists to catch — twice over now, once when the sky
        // was a box three hundred ship-lengths deep and once when the exponent
        // meant for the dial was wired to the stars.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(5.5, 3);
        // Up to speed first: the ease takes about a second and this is about
        // where the sky goes once it is there.
        fly(&mut sky, &mut ship, 600);

        let moved = drift(&mut sky, &mut ship, 30 * 120);
        let median = moved[moved.len() / 2];
        assert!(
            moved.len() > 100,
            "only {} stars stayed in the picture",
            moved.len()
        );
        assert!(
            median < 4.0,
            "thirty seconds of full impulse moved the median star {median} subpixels"
        );
    }

    /// How far every star still in the picture moved over `steps`, sorted.
    ///
    /// Stars that crossed their own limit and came back somewhere else are left
    /// out — one of those is not a star that moved, it is a different star —
    /// and so is anything off the canvas at either end, since a star square
    /// abeam has a camera depth of very nearly nothing and projects thousands
    /// of subpixels off the side. Measuring one of those would be asking a
    /// question about the projection rather than about the sky.
    fn drift(sky: &mut Universe, ship: &mut Ship, steps: usize) -> Vec<f32> {
        let cam = cam();
        let seen = |sky: &Universe, ship: &Ship| -> Vec<Option<(f32, f32)>> {
            let eye = seated(ship);
            sky.stars
                .iter()
                .map(|s| cam.project_beyond(eye.place(s.pos), STAR_NEAR))
                .collect()
        };
        let places = sky.positions();
        let opened = seen(sky, ship);
        fly(sky, ship, steps);
        let after = seen(sky, ship);
        let ends = sky.positions();
        let on_canvas =
            |p: &(f32, f32)| (0.0..cam.width).contains(&p.0) && (0.0..cam.height).contains(&p.1);

        let mut moved: Vec<f32> = opened
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(i, _)| places[*i] == ends[*i])
            .filter_map(|(_, (was, is))| match (was, is) {
                (Some(a), Some(b)) if on_canvas(a) && on_canvas(b) => {
                    Some(crate::canvas::length_of(b.0 - a.0, b.1 - a.1))
                }
                _ => None,
            })
            .collect();
        moved.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        moved
    }

    #[test]
    fn the_sky_answers_the_throttle_the_way_the_dial_does() {
        // The invariant the impulse bound above is only a symptom of, asserted
        // directly: the sky's rate and the panel's velocity are one map, so the
        // ratio between full warp and full impulse on screen is the ratio the
        // dial reads — 2000 c against 0.9.
        //
        // This is the test that would have caught the fault the other way
        // round. Before the rebuild the sky ran on `Ship::speed`, which spans
        // 42 to 780, so this ratio came out at 19 against the dial's 2222 and
        // impulse streamed a hundred and twenty times too fast for its own
        // instruments.
        let rate = |warp: bool| {
            let mut ship = Ship::new();
            ship.throttle = 1.0;
            if warp {
                ship.toggle_warp();
            }
            let mut sky = Universe::new(5.5, 17);
            fly(&mut sky, &mut ship, 1200);
            ship.velocity_ly_per_s()
        };
        let ratio = rate(true) / rate(false);
        let dial = 2000.0 / 0.9;
        assert!(
            (ratio / dial - 1.0).abs() < 0.02,
            "the sky answers the throttle {ratio} times over where the dial says {dial}"
        );
    }

    #[test]
    fn the_sky_streams_at_warp() {
        // The other half, and the one that says the first is a sky rather than
        // a still. At full warp the ship covers 5.48 ly a second, so the same
        // near stars run at 22 subpixels a second and the exposure draws three
        // seconds of it.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut sky = Universe::new(6.5, 3);
        fly(&mut sky, &mut ship, 600);

        let cam = cam();
        let eye = seated(&ship);
        let mut longest = 0.0f32;
        for streak in sky.streaks(&cam, &eye, 0.0) {
            longest = longest.max(crate::canvas::length_of(
                streak.to.0 - streak.from.0,
                streak.to.1 - streak.from.1,
            ));
        }
        assert!(
            longest > 20.0,
            "the longest streak at full warp is {longest} subpixels on a 48-tall canvas"
        );
    }

    #[test]
    fn a_streak_is_the_track_the_star_actually_flew() {
        // The exposure, checked against the thing it claims to be rather than
        // against itself: the tail is where the star projected from where the
        // ship was three seconds ago. It used to be an extrapolation of one
        // step's motion multiplied by six, which is a chord across a curve —
        // and which collapses to a subpixel once the distances are honest.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut sky = Universe::new(5.0, 11);
        fly(&mut sky, &mut ship, 600);

        let cam = cam();
        let eye = seated(&ship);
        let back = TRAIL_SECONDS * eye.warp * eye.velocity;
        let drawn: Vec<_> = sky.streaks(&cam, &eye, 0.0).collect();
        let mut checked = 0;
        for star in &sky.stars {
            let pos = eye.place(star.pos);
            let (Some(to), Some(from)) = (
                cam.project_beyond(pos, STAR_NEAR),
                // Where it stood when the ship was `back` light years astern,
                // worked out the long way round: rewind the ship rather than
                // the star.
                cam.project_beyond(
                    [
                        pos[0] + eye.nose[0] * back,
                        pos[1] + eye.nose[1] * back,
                        pos[2] + eye.nose[2] * back,
                    ],
                    STAR_NEAR,
                ),
            ) else {
                continue;
            };
            // Projecting is not the same as being drawn: a star inside the last
            // three quarters of a magnitude of the limit is fading up out of
            // nothing and comes out at an intensity of zero, which `streaks`
            // drops rather than handing the canvas a segment worth no light.
            let Some(streak) = drawn.iter().find(|s| s.to == to) else {
                continue;
            };
            assert_eq!(streak.from, from, "the exposure is not the track");
            checked += 1;
        }
        assert!(
            checked > 50,
            "only {checked} stars were in front of the eye"
        );
    }

    #[test]
    fn a_star_is_as_bright_as_the_inverse_square_of_its_own_range() {
        // Magnitudes are logarithms and the canvas is linear light, so the one
        // transform between them is the only thing here that has to be right.
        // Halving a range is two magnitudes and four times the light.
        let sky = Universe::new(6.5, 2);
        let star = sky.stars[0];
        let near = star.lumen / 100.0;
        let far = star.lumen / 400.0;
        assert!(
            (near / far - 4.0).abs() < 1e-4,
            "halving a range gained {} rather than four times",
            near / far
        );
    }

    #[test]
    fn the_sky_follows_the_count_law() {
        // Each magnitude is `10^0.6` — about four — times as many stars, which
        // is what makes a limiting magnitude a density rather than a number
        // somebody picked. Checked over the range a hand on the keys can reach.
        for limit in [4.0f32, 5.0, 6.0, 7.0, 8.0] {
            let here = Universe::population(limit) as f32;
            let brighter = Universe::population(limit - 1.0) as f32;
            let ratio = here / brighter;
            assert!(
                (ratio - 10f32.powf(COUNT_SLOPE)).abs() < 0.05,
                "a magnitude at {limit} is {ratio} times as many stars"
            );
        }
        // And a sky can be asked for that holds nothing at all, which is how
        // the tunnel, the bubble and the hull are looked at with nothing
        // streaming past them.
        assert_eq!(Universe::population(-2.0), 0);
    }

    #[test]
    fn asking_for_a_fainter_sky_leaves_the_one_already_out_there_alone() {
        // A limit is a property of the observer, so moving it must not move a
        // star: the sky that was visible before is visible still and in the
        // same place, with more of it beyond. Bitwise, because a reach is a
        // lumen over a limiting intensity and only the second of those moved.
        let mut sky = Universe::new(5.5, 6);
        let before = sky.positions();
        sky.set_limit(6.5);
        assert!(
            sky.len() > before.len(),
            "a fainter limit found nothing new"
        );
        assert_eq!(
            before,
            sky.positions()[..before.len()],
            "raising the limit moved a star that was already visible"
        );
    }

    #[test]
    fn the_seed_is_the_whole_of_the_state() {
        let a = Universe::new(5.5, 77).positions();
        let b = Universe::new(5.5, 77).positions();
        let c = Universe::new(5.5, 78).positions();
        assert_eq!(a, b, "one seed gave two skies");
        assert_ne!(a, c, "two seeds gave one sky");
    }

    #[test]
    fn a_star_arrives_at_nothing_rather_than_blinking_on() {
        // A star crosses its own visibility sphere at exactly the limiting
        // magnitude, which is the faintest anything here can be — but faint is
        // not nothing, and a star winking out at a twentieth of a unit is a
        // star winking out. The fade is what answers that, and this is the one
        // seam in the sky that a recycle could show through.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut sky = Universe::new(5.5, 13);
        fly(&mut sky, &mut ship, 600);

        let cam = cam();
        // Over a second of flight rather than a single step: only the nearest
        // stars have a sphere small enough to cross often, so a step at a time
        // is as likely as not to recycle nothing at all.
        let mut arrived = 0;
        for _ in 0..120 {
            let before = sky.positions();
            ship.update(1.0 / 120.0);
            sky.advance(ship.position, ship.axes[2]);
            let eye = seated(&ship);
            for (i, was) in before.iter().enumerate() {
                if *was == sky.stars[i].pos {
                    continue;
                }
                arrived += 1;
                let pos = eye.place(sky.stars[i].pos);
                if cam.project_beyond(pos, STAR_NEAR).is_none() {
                    continue;
                }
                let range_sq = pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2];
                let over = sky.stars[i].reach_sq / range_sq;
                assert!(
                    over <= 1.0 + 1e-3,
                    "a recycled star arrived {over} times brighter than the limit"
                );
            }
        }
        assert!(arrived > 0, "nothing recycled in a second at full warp");
    }

    #[test]
    fn the_flow_brings_stars_in_from_the_side_it_is_coming_from() {
        // The recycle's distribution, which is the other half of nothing
        // arriving in view. A uniform flow crossing a sphere delivers stars in
        // proportion to the cosine of the angle from where it is coming, so
        // every one has to land on the forward hemisphere and they have to
        // bunch toward its pole rather than spread evenly over it.
        let mut sky = Universe::new(4.0, 21);
        let nose = [0.0, 0.0, 1.0];
        let mut alongs = Vec::new();
        for _ in 0..4000 {
            let d = sky.entering(nose);
            alongs.push(d[2]);
        }
        assert!(
            alongs.iter().all(|a| *a >= -1e-6),
            "the flow brought a star in through the side it is leaving by"
        );
        // A cosine-weighted hemisphere has a mean cosine of two thirds; a
        // uniform one would give a half, which is the bug this is guarding.
        let mean = alongs.iter().sum::<f32>() / alongs.len() as f32;
        assert!(
            (mean - 2.0 / 3.0).abs() < 0.02,
            "the entering surface averages {mean} rather than two thirds"
        );
    }

    #[test]
    fn the_sky_is_left_alone_by_everything_but_the_flight() {
        // The property the whole rebuild rests on, stated at the level it lives
        // at: nothing about *looking* at the sky can move it. Only `advance`
        // may, and only because the ship went somewhere.
        let mut sky = Universe::new(5.5, 31);
        let parked = sky.positions();
        let ship = Ship::new();
        let cam = cam();
        for turn in 0..40 {
            let orbit = crate::view::Orbit {
                azimuth: turn as f32 * 0.31,
                elevation: turn as f32 * 0.17,
                roll: turn as f32 * 0.07,
            };
            let eye = Observer::outside(
                ship.axes,
                ship.position,
                &Eye::new(orbit, 1.0 + turn as f32 * 0.03),
                orbit.nose_in_camera(),
                0.0,
                0.0,
            );
            let drawn = sky.streaks(&cam, &eye, turn as f64).count();
            assert!(drawn > 0, "nothing at all was visible from {orbit:?}");
        }
        assert_eq!(parked, sky.positions(), "looking at the sky moved it");

        // And a ship at rest does not move it either, however long it sits.
        let mut ship = Ship::new();
        ship.throttle = 0.0;
        fly(&mut sky, &mut ship, 2000);
        assert_eq!(parked, sky.positions(), "a parked ship moved the sky");
    }
}
