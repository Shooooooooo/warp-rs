//! The sky, as a volume of space the ship is somewhere inside rather than as a
//! box in front of the camera.

use crate::camera::Camera;
use crate::track::Track;
use crate::view::Eye;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::f32::consts::TAU;

/// How the number of stars brighter than a magnitude grows with it.
const COUNT_SLOPE: f32 = 0.6;

/// The magnitude the census below is quoted at, and how many stars are brighter
/// than it over the whole celestial sphere.
const REFERENCE_LIMIT: f32 = 6.5;
const REFERENCE_COUNT: f32 = 9_110.0;

/// The apparent magnitude that lands on one unit of canvas light.
const ZERO_POINT: f32 = 3.08;

/// How much of the faint end is spent fading up out of nothing, in magnitudes.
const FADE_MAGNITUDES: f32 = 0.75;

/// How long an exposure a lit warp drive draws the sky with, in seconds of
/// flight, at full warp.
const TRAIL_SECONDS: f32 = 3.0;

/// Nearest any star is, in light years.
pub const NEAREST_STAR: f32 = 4.0;

/// Nearest a star is projected from, in light years.
const STAR_NEAR: f32 = 0.001;

/// Nearest the *tail* of an exposure is cut to, in light years.
const TAIL_NEAR: f32 = STAR_NEAR * 1.05;

/// How far off the camera's axis an exposure is followed before it is cut, as
/// the cosine of the angle.
const TAIL_COS: f32 = 0.15;

/// The most poses one exposure is drawn from.
const MAX_STATIONS: usize = 24;

/// How far a drawn exposure may fall short of the curve it stands for, in
/// subpixels, at the corner of the frame.
const SAGITTA: f32 = 0.25;

/// A parsec, in light years, and ten of them — the distance a star's absolute
/// magnitude is quoted at, so this is the whole of the distance modulus.
const PARSEC_LY: f32 = 3.261_563;
const TEN_PARSECS_LY: f32 = PARSEC_LY * 10.0;

/// How far either side of its class's absolute magnitude a star may fall.
const ABSOLUTE_SPREAD: f32 = 1.5;

/// How many times `Universe::spawn` will redraw a star too faint to be as far
/// off as the nearest one really is.
const TRIES: usize = 32;

/// A star's spectral class: what colour it burns, how bright it really is, and
/// how much of the sky is made of it.
#[derive(Debug, Clone, Copy)]
pub struct SpectralClass {
    pub rgb: [f32; 3],
    /// Absolute visual magnitude — brightness at ten parsecs, which together
    /// with how bright a star *looks* is the whole of how far away it is.
    pub absolute: f32,
    /// Relative frequency in the sky. Visible to the crate because [`Universe`]
    /// draws from this census.
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

/// How far a star is carried toward blue and how far toward red, as the two
/// weights the whole Doppler shift is spelled in.
fn shift_weights(forward: f32, amount: f32) -> (f32, f32) {
    (amount * forward.powi(3), amount * (1.0 - forward) * 0.75)
}

/// Blend a star's rest colour toward blue ahead and red at the periphery.
/// `forward` is cos of the angle off the nose folded onto 0..=1; `amount` is
/// the 0..=1 strength.
fn shift_color(rgb: [f32; 3], forward: f32, amount: f32) -> [f32; 3] {
    if amount <= 0.0 {
        return rgb;
    }
    let (blue, red) = shift_weights(forward, amount);
    let mut out = [0.0; 3];
    for i in 0..3 {
        let toward_blue = rgb[i] + (BLUE_SHIFT[i] - rgb[i]) * blue;
        out[i] = toward_blue + (RED_SHIFT[i] - toward_blue) * red;
    }
    out
}

/// What one unit of Doppler shift is worth in light.
const SHIFT_LIGHT: f32 = 0.6;

/// A star astern is dimmed, never put out. The red weight tops out at 0.75 when
/// `amount` is one, so the gain floors at `1 − 0.75·SHIFT_LIGHT` — 0.55, and
/// there is no value of this worth flying that reaches zero. It fails the build
/// rather than a frame because a gain of zero deletes a star instead of letting
/// it fall behind, and the sky's faint end is supposed to leave through the
/// fade in [`Universe::sweep`] instead.
const _: () = assert!(SHIFT_LIGHT * 0.75 < 1.0);

/// What the same shift does to a star's *light*: brighter ahead, fainter
/// astern. `forward` and `amount` are exactly [`shift_color`]'s, and by
/// [`shift_weights`] so is the curve.
fn shift_light(forward: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        return 1.0;
    }
    let (blue, red) = shift_weights(forward, amount);
    1.0 + SHIFT_LIGHT * (blue - red)
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
    /// The camera's own pose in the *hull's* frame — the identity from the
    /// seat, [`Eye::basis`] from outside — kept apart from `to_camera`, which
    /// is the two of them fused so `place` is three dot products.
    mount: [[f32; 3]; 3],
    /// 0..=1 across the superluminal range: it sets the Doppler shift — the
    /// colour in [`shift_color`] and the brightness in [`shift_light`], which
    /// are effect drawn twice — and how much twinkle is left.
    warp: f32,
}

impl Observer {
    /// From the pilot's seat, where the camera is the hull.
    pub fn cockpit(axes: [[f32; 3]; 3], origin: [f64; 3], warp: f32) -> Self {
        Self {
            to_camera: axes,
            origin,
            nose: [0.0, 0.0, 1.0],
            mount: crate::ship::LEVEL_AXES,
            warp,
        }
    }

    /// From outside, where the camera is swung round the hull by the orbit.
    pub fn outside(
        axes: [[f32; 3]; 3],
        origin: [f64; 3],
        eye: &Eye,
        nose: [f32; 3],
        warp: f32,
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
            mount: eye.basis,
            warp,
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

/// One pose the exposure was open at, as what it does to a star the current
/// camera has already placed.
#[derive(Debug, Clone, Copy)]
struct Station {
    /// Out of the camera's current frame and into this one.
    turn: [[f32; 3]; 3],
    /// Where the ship went in between, in this pose's own frame.
    offset: [f32; 3],
    /// How far through the exposure this pose stands, 1 at the star and 0 at
    /// the oldest end. It is carried onto every point the walk lays down,
    /// because [`crate::canvas::spread`] needs to know how long the star's
    /// image dwelt on a stretch and not merely how far it went.
    moment: f32,
}

impl Station {
    /// A pose that has not moved: what an unused slot of the frame's array
    /// holds, so nothing has to reason about a partly filled one.
    const HELD: Self = Self {
        turn: crate::ship::LEVEL_AXES,
        offset: [0.0; 3],
        moment: 1.0,
    };
}

/// Every star the observer could see, wherever the camera is pointed.
pub struct Universe {
    stars: Vec<Star>,
    rng: StdRng,
    limit: f32,
    /// How far back along the track the exposure currently reaches, in light
    /// years, and it is *state* rather than a number worked out afresh each
    /// frame. That is the whole of the fix for a trail that grew toward the
    /// middle of the screen.
    trail: f32,
    /// The flight the exposure is drawn along: where the ship has been, and
    /// which way it was pointed while it was there.
    track: Track,
}

impl Universe {
    pub fn new(limit: f32, seed: u64) -> Self {
        let mut sky = Self {
            stars: Vec::new(),
            rng: StdRng::seed_from_u64(seed),
            limit,
            trail: 0.0,
            track: Track::new(),
        };
        sky.stock();
        sky
    }

    /// How many stars beat a limiting magnitude, over the whole sphere.
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

    /// How far back along the track the exposure currently reaches, in light
    /// years.
    pub fn exposure(&self) -> f32 {
        self.trail
    }

    /// How far back the flight is straight, in light years — see
    /// [`crate::track::Track::straight_run`], which answers with infinity when
    /// nothing has turned at all.
    pub fn straight(&self) -> f32 {
        self.track.straight_run()
    }

    /// Where every star is, for a test that wants to say a thing did or did not
    /// move the sky. Positions rather than the stars themselves, because what
    /// those tests are about is the one field a camera could conceivably
    /// disturb.
    pub fn positions(&self) -> Vec<[f32; 3]> {
        self.stars.iter().map(|s| s.pos).collect()
    }

    /// Ask for a different limiting magnitude.
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
        let origin = self.track.pose_at(0.0).0;
        while self.stars.len() < wanted {
            let star = self.spawn(origin);
            self.stars.push(star);
        }
    }

    /// Step the sky: remember where the ship is, unroll the exposure a little
    /// further, then take away whatever has fallen past the limit and put the
    /// same number back where the flow is bringing them in from.
    pub fn advance(
        &mut self,
        origin: [f64; 3],
        axes: [[f32; 3]; 3],
        dt: f32,
        warp: f32,
        speed: f32,
    ) {
        let nose = axes[2];
        self.track.record(origin, axes);
        // The exposure lengthens by the distance the ship actually flew and not
        // one light year more, which is what keeps a trail behind its star
        // rather than reaching past it.
        let settled = TRAIL_SECONDS * warp * speed;
        self.trail = if settled > self.trail {
            settled.min(self.trail + speed * dt)
        } else {
            settled
        };

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

    /// Hand every star's exposure to `draw`, as the track it swept across the
    /// frame: at least two points, tail first, head last.
    pub fn sweep(
        &self,
        cam: &Camera,
        eye: &Observer,
        time: f64,
        mut draw: impl FnMut(&[crate::canvas::Trace], [f32; 3], f32),
    ) {
        // How far back along the track the exposure reaches, as a displacement
        // in the camera's own space.
        let reach = self.trail;
        let back = [
            eye.nose[0] * reach,
            eye.nose[1] * reach,
            eye.nose[2] * reach,
        ];
        // Where the exposure was open from, when the ship did not simply fly
        // straight through it.
        let mut stations = [Station::HELD; MAX_STATIONS];
        let legs = self.stations(cam, eye, reach, &mut stations);
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

        let mut points = [(0.0f32, 0.0f32, 0.0f32); MAX_STATIONS];
        for star in &self.stars {
            let pos = eye.place(star.pos);
            let Some(to) = cam.project_beyond(pos, STAR_NEAR) else {
                continue;
            };

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
                continue;
            }

            // How far off the nose it sits.
            let along = pos[0] * eye.nose[0] + pos[1] * eye.nose[1] + pos[2] * eye.nose[2];
            let forward = if range_sq > f32::MIN_POSITIVE {
                (0.5 + 0.5 * along / range_sq.sqrt()).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let color = shift_color(CLASSES[star.class].rgb, forward, doppler);
            // Multiplied onto the finished intensity and deliberately not into
            // `lumen`, `reach_sq` or the limit.
            let intensity = intensity * shift_light(forward, doppler);

            // Where the star was while the shutter was open.
            let path: &[crate::canvas::Trace] = if legs == 0 {
                let from = if reach > 0.0 {
                    let Some(tail) = tail_of(pos, back) else {
                        continue;
                    };
                    let Some(from) = cam.project_beyond(tail, STAR_NEAR) else {
                        continue;
                    };
                    from
                } else {
                    to
                };
                // The whole exposure in one leg, so the pace the falloff is
                // asked for is the streak's own length and the arithmetic is
                // what it always was.
                let pace = crate::canvas::length_of(to.0 - from.0, to.1 - from.1);
                points[0] = (from.0, from.1, pace);
                points[1] = (to.0, to.1, pace);
                &points[..2]
            } else {
                let lo = walk_back(&stations[..legs], pos, to, cam, &mut points);
                if lo > legs - 1 {
                    // The walk found nowhere for the exposure to have been: the
                    // star is so far off the axis that no part of its track is
                    // in the picture.
                    points[legs - 1] = points[legs];
                    &points[legs - 1..=legs]
                } else {
                    &points[lo..=legs]
                }
            };
            draw(path, color, intensity);
        }
    }

    /// Where the exposure was open from, as what each pose does to a star this
    /// camera has already placed. Answers how many legs the track was cut into,
    /// and zero when there is nothing to cut.
    fn stations(
        &self,
        cam: &Camera,
        eye: &Observer,
        reach: f32,
        out: &mut [Station; MAX_STATIONS],
    ) -> usize {
        if reach <= 0.0 || reach <= self.track.straight_run() {
            return 0;
        }
        // How finely to cut it, from the *picture* rather than from an angle.
        let corner = crate::canvas::length_of(cam.width * 0.5, cam.height * 0.5);
        let per_leg = (8.0 * SAGITTA / corner.max(1.0)).sqrt();
        // How much further than the camera's own turn a star can be carried by
        // the ship simply going somewhere.
        let turned = self.track.turn_over(reach);
        let swing = turned * (1.0 + reach / NEAREST_STAR);
        let legs =
            ((swing / per_leg.max(f32::MIN_POSITIVE)).ceil() as usize).clamp(1, MAX_STATIONS - 1);

        let inv = 1.0 / legs as f32;
        for (leg, station) in out.iter_mut().take(legs).enumerate() {
            // Spaced by equal *turn* rather than by equal track.
            let back = if leg + 1 == legs {
                reach
            } else {
                self.track
                    .back_at_turn(turned * (leg + 1) as f32 * inv, reach)
            };
            let (was, axes) = self.track.pose_at(back);
            // `mount · axes(then)`, the camera re-mounted on the pose the ship
            // was holding: the row `r` of it is the camera's `r`th axis written
            // in the world frame, which is the same composition `Observer::
            // outside` does for now.
            let mut then = [[0.0f32; 3]; 3];
            for (row, turn) in then.iter_mut().zip(eye.mount) {
                for (place, axis) in row.iter_mut().enumerate() {
                    *axis = turn[0] * axes[0][place]
                        + turn[1] * axes[1][place]
                        + turn[2] * axes[2][place];
                }
            }
            // A star already in this camera's space gets there by turning out
            // of it and back into that one — `M · M₀ᵀ` — and then shifting by
            // how far the ship travelled in between.
            let mut turn = [[0.0f32; 3]; 3];
            for (row, mine) in turn.iter_mut().zip(then) {
                for (place, theirs) in row.iter_mut().zip(eye.to_camera) {
                    *place = mine[0] * theirs[0] + mine[1] * theirs[1] + mine[2] * theirs[2];
                }
            }
            let gone = [
                (eye.origin[0] - was[0]) as f32,
                (eye.origin[1] - was[1]) as f32,
                (eye.origin[2] - was[2]) as f32,
            ];
            let mut offset = [0.0f32; 3];
            for (place, row) in offset.iter_mut().zip(then) {
                *place = gone[0] * row[0] + gone[1] * row[1] + gone[2] * row[2];
            }
            *station = Station {
                turn,
                offset,
                moment: 1.0 - back / reach,
            };
        }
        legs
    }

    /// Make a star, somewhere uniform in its own visibility sphere, centred on
    /// where the ship is standing.
    ///
    /// `--seed` reproducibility is a property of the draw order below, the
    /// rejection loop included: hoisting a draw out of a struct literal, or
    /// changing how many values a star takes, gives a different sky.
    fn spawn(&mut self, origin: [f64; 3]) -> Star {
        // Bounded rather than a bare loop.
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
        // recycle below does not slowly walk the sky away from.
        let reach = reach_sq.sqrt();
        let depth: f32 = self.rng.random_range(0.0..1.0);
        let hole = (NEAREST_STAR / reach).min(1.0).powi(3);
        let radius = reach * (hole + (1.0 - hole) * depth).cbrt();
        let dir = self.anywhere();
        Star {
            // Spelled exactly the way the recycle in `advance` spells it, which
            // is the point rather than a coincidence: the two are the same
            // question — where does a star sit relative to the ship — asked at
            // two moments, and they used to answer it differently.
            pos: [
                (origin[0] + (dir[0] * radius) as f64) as f32,
                (origin[1] + (dir[1] * radius) as f64) as f32,
                (origin[2] + (dir[2] * radius) as f64) as f32,
            ],
            class,
            lumen,
            reach_sq,
            phase: self.rng.random_range(0.0..TAU),
        }
    }

    /// A star's brightness at unit distance, and the square of how far off it
    /// may be and still be worth drawing.
    fn photometry(&self, absolute: f32) -> (f32, f32) {
        let at_ten = 10f32.powf(-0.4 * (absolute - ZERO_POINT));
        let lumen = at_ten * TEN_PARSECS_LY * TEN_PARSECS_LY;
        let at_limit = 10f32.powf(0.4 * (self.limit - absolute));
        (lumen, at_limit * TEN_PARSECS_LY * TEN_PARSECS_LY)
    }

    /// A sample from a bell about zero, of unit width and bounded at three.
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
    fn entering(&mut self, nose: [f32; 3]) -> [f32; 3] {
        let u: f32 = self.rng.random_range(0.0..1.0);
        let turn: f32 = self.rng.random_range(0.0..TAU);
        let along = u.sqrt();
        let out = around(along, turn);
        // Turned onto the nose.
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

/// Where an exposure opened, in camera space, cut against the near plane.
fn tail_of(pos: [f32; 3], back: [f32; 3]) -> Option<[f32; 3]> {
    let depth = pos[2] + back[2];
    if depth > TAIL_NEAR {
        return Some([pos[0] + back[0], pos[1] + back[1], depth]);
    }
    if pos[2] <= TAIL_NEAR || back[2] >= -f32::MIN_POSITIVE {
        return None;
    }
    // Solved for the crossing rather than stepped toward it, and the depth is
    // *set* rather than recomputed: `pos[2] + back[2] * t` subtracts two
    // numbers of about the exposure's length to leave a thousandth of a light
    // year, which is the one place in this arithmetic that cancels badly.
    let t = ((TAIL_NEAR - pos[2]) / back[2]).clamp(0.0, 1.0);
    Some([pos[0] + back[0] * t, pos[1] + back[1] * t, TAIL_NEAR])
}

/// Where a leg of an exposure leaves the cone [`TAIL_COS`] describes.
fn leaves_the_cone(from: [f32; 3], to: [f32; 3]) -> Option<[f32; 3]> {
    let c2 = TAIL_COS * TAIL_COS;
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let square = |v: [f32; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let a = d[2] * d[2] - c2 * square(d);
    let b = 2.0 * (from[2] * d[2] - c2 * dot(from, d));
    let c = from[2] * from[2] - c2 * square(from);
    // Inside the cone is `z > c·|p|` with `z` positive, which is `c > 0` here.
    if c <= 0.0 || from[2] <= 0.0 {
        return None;
    }
    let t = if a.abs() <= f32::MIN_POSITIVE {
        if b >= 0.0 {
            return None;
        }
        -c / b
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            return None;
        }
        let root = disc.sqrt();
        // The two roots, in order, taking the first that leaves the segment's
        // near end behind.
        let (r0, r1) = ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
        let (lo, hi) = if r0 <= r1 { (r0, r1) } else { (r1, r0) };
        if lo > 0.0 {
            lo
        } else if hi > 0.0 {
            hi
        } else {
            return None;
        }
    };
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some([from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t])
}

/// Walk an exposure back through the poses the ship held, filling `points` from
/// the head downward so the path comes out tail first — which is the order
/// [`crate::canvas::Canvas::draw_path`] ramps in. Answers the index the path
/// begins at.
fn walk_back(
    stations: &[Station],
    pos: [f32; 3],
    head: (f32, f32),
    cam: &Camera,
    points: &mut [crate::canvas::Trace; MAX_STATIONS],
) -> usize {
    let legs = stations.len();
    points[legs] = (head.0, head.1, 1.0);
    // The moment each point sits at, kept beside the path while it is walked
    // and spent at the end on the pace of every leg.
    let mut moments = [0.0f32; MAX_STATIONS];
    moments[legs] = 1.0;
    let mut lo = legs;
    let mut previous = pos;
    let mut held = 1.0f32;
    for station in stations {
        let [r, d, f] = station.turn;
        let at = [
            pos[0] * r[0] + pos[1] * r[1] + pos[2] * r[2] + station.offset[0],
            pos[0] * d[0] + pos[1] * d[1] + pos[2] * d[2] + station.offset[1],
            pos[0] * f[0] + pos[1] * f[1] + pos[2] * f[2] + station.offset[2],
        ];
        let square = at[0] * at[0] + at[1] * at[1] + at[2] * at[2];
        if at[2] > TAIL_NEAR && at[2] * at[2] > TAIL_COS * TAIL_COS * square {
            if let Some(p) = cam.project_beyond(at, STAR_NEAR) {
                lo -= 1;
                points[lo] = (p.0, p.1, 0.0);
                moments[lo] = station.moment;
            }
            previous = at;
            held = station.moment;
            continue;
        }
        // The leg left the picture.
        let square_of = |p: [f32; 3]| p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        let inside = previous[2] > TAIL_NEAR
            && previous[2] * previous[2] > TAIL_COS * TAIL_COS * square_of(previous);
        let cut = if inside {
            leaves_the_cone(previous, at).or_else(|| {
                let back = [
                    at[0] - previous[0],
                    at[1] - previous[1],
                    at[2] - previous[2],
                ];
                tail_of(previous, back)
            })
        } else {
            None
        };
        if let Some(cut) = cut {
            if let Some(p) = cam.project_beyond(cut, STAR_NEAR) {
                // Where along the leg the cut landed, so the stump keeps the
                // share of the exposure it really stands for rather than the
                // whole leg's.
                let span = length_of_3(previous, at);
                let t = if span > f32::MIN_POSITIVE {
                    (length_of_3(previous, cut) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                lo -= 1;
                points[lo] = (p.0, p.1, 0.0);
                moments[lo] = held + (station.moment - held) * t;
            }
        }
        break;
    }
    // What the exposure spent on each leg, as the length the whole of it would
    // have covered at that leg's pace.
    for i in lo..legs {
        let span =
            crate::canvas::length_of(points[i + 1].0 - points[i].0, points[i + 1].1 - points[i].1);
        let share = moments[i + 1] - moments[i];
        points[i].2 = if share > f32::MIN_POSITIVE {
            span / share
        } else {
            f32::INFINITY
        };
    }
    lo
}

/// How far apart two camera-space points are.
fn length_of_3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (x, y, z) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    (x * x + y * y + z * z).sqrt()
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
    fn a_sublight_sky_is_the_one_the_drive_is_not_lit_for() {
        // Bitwise, and it is the whole of the guard.
        for f in 0..=40 {
            let forward = f as f32 / 40.0;
            assert_eq!(
                shift_light(forward, 0.0),
                1.0,
                "a cold drive changed a star at forward {forward}"
            );
            // A star's light times the gain has to be the star's light, not a
            // number very near it: this is the multiply `sweep` performs.
            let light = 0.037_213_5f32;
            assert_eq!(light * shift_light(forward, 0.0), light);
        }
    }

    #[test]
    fn the_shift_brightens_ahead_and_dims_astern() {
        // Brighter where it blues, fainter where it reddens, at every strength
        // the ramp can reach.
        for step in 1..=9 {
            let amount = step as f32 / 10.0 * 0.9;
            let (ahead, abeam, astern) = (
                shift_light(1.0, amount),
                shift_light(0.5, amount),
                shift_light(0.0, amount),
            );
            assert!(
                ahead > abeam && abeam > astern,
                "at {amount} the beam ran {ahead} / {abeam} / {astern}"
            );
            assert!(
                ahead > 1.0 && astern < 1.0,
                "at {amount} the shift did not straddle one: {ahead} / {astern}"
            );
            assert!(
                astern > 0.0,
                "at {amount} the shift put a star out rather than dimming it"
            );
        }
    }

    #[test]
    fn the_light_turns_over_where_the_colour_stops_leaning_blue() {
        // The best thing the shared weights buy, and the reason [`SHIFT_LIGHT`]
        // is one constant on their difference rather than a gain and a loss
        // tuned apart: a star is drawn brighter than at rest exactly while it
        // is being carried toward blue, and fainter exactly while it is being
        // carried toward red.
        for f in 0..=200 {
            let forward = f as f32 / 200.0;
            for step in 1..=9 {
                let amount = step as f32 / 10.0 * 0.9;
                let (blue, red) = shift_weights(forward, amount);
                assert_eq!(
                    blue > red,
                    shift_light(forward, amount) > 1.0,
                    "at forward {forward} the colour and the light disagree \
                     about which way the sky is going"
                );
                assert_eq!(blue < red, shift_light(forward, amount) < 1.0);
            }
        }

        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if shift_light(mid, 0.9) < 1.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let turnover = (2.0 * (0.5 * (lo + hi)) - 1.0).acos().to_degrees();
        assert!(
            (72.0..75.0).contains(&turnover),
            "the sky stops brightening at {turnover} degrees off the nose"
        );
    }

    #[test]
    fn the_drive_spends_a_little_less_light_than_it_was_given() {
        // Over a whole sky the shift is meant to move light about rather than
        // make it.
        for step in 1..=9 {
            let amount = step as f32 / 10.0 * 0.9;
            const SAMPLES: usize = 20_001;
            let mut total = 0.0f64;
            for i in 0..SAMPLES {
                total += shift_light(i as f32 / (SAMPLES - 1) as f32, amount) as f64;
            }
            let mean = total / SAMPLES as f64;
            assert!(
                (0.90..1.0).contains(&mean),
                "at {amount} a whole sky came out at {mean} of its light"
            );
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

    /// Every exposure the sky hands over this frame, as the paths themselves.
    fn swept(
        sky: &Universe,
        cam: &Camera,
        eye: &Observer,
        time: f64,
    ) -> Vec<Vec<crate::canvas::Trace>> {
        let mut out = Vec::new();
        sky.sweep(cam, eye, time, |points, _, _| out.push(points.to_vec()));
        out
    }

    /// The two ends of an exposure: where the star is, and where it was.
    fn ends(path: &[crate::canvas::Trace]) -> ((f32, f32), (f32, f32)) {
        let (a, b) = (path[0], path[path.len() - 1]);
        ((a.0, a.1), (b.0, b.1))
    }

    fn seated(ship: &Ship) -> Observer {
        Observer::cockpit(ship.axes, ship.position, ship.warp_intensity())
    }

    /// Fly a sky and a ship together for `steps` of a sim step.
    fn fly(sky: &mut Universe, ship: &mut Ship, steps: usize) {
        for _ in 0..steps {
            ship.update(1.0 / 120.0);
            sky.advance(
                ship.position,
                ship.axes,
                1.0 / 120.0,
                ship.warp_intensity(),
                ship.velocity_ly_per_s(),
            );
        }
    }

    #[test]
    fn no_direction_holds_a_thinner_sky_than_any_other() {
        // The acceptance test for the fault this module was written to fix: a
        // camera swung round the ship used to turn up a sparse patch, because
        // the pool was a rectangle laid out in front of whichever eye was
        // flying and a swing had to re-lay it.
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
        // What the user asked for, as a number.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(4.5, 3);
        // Up to speed first: the ease takes about a second and this is about
        // where the sky goes once it is there.
        fly(&mut sky, &mut ship, 600);

        let cam = cam();
        let seen = |sky: &Universe, ship: &Ship| -> Vec<Option<(f32, f32)>> {
            let eye = seated(ship);
            sky.stars
                .iter()
                .map(|s| cam.project_beyond(eye.place(s.pos), STAR_NEAR))
                .collect()
        };
        let places = sky.positions();
        let opened = seen(&sky, &ship);
        fly(&mut sky, &mut ship, 30 * 120);
        let after = seen(&sky, &ship);

        // Star by star, and skipping any that crossed its own limit and came
        // back somewhere else — a few of the nearest do even at this crawl, and
        // one of those is not a star that moved, it is a different star.
        let mut moved = 0.0f32;
        let mut counted = 0;
        let ends = sky.positions();
        let on_canvas =
            |p: &(f32, f32)| (0.0..cam.width).contains(&p.0) && (0.0..cam.height).contains(&p.1);
        for (i, (was, is)) in opened.iter().zip(&after).enumerate() {
            if places[i] != ends[i] {
                continue;
            }
            if let (Some(a), Some(b)) = (was, is) {
                if !on_canvas(a) || !on_canvas(b) {
                    continue;
                }
                moved = moved.max(crate::canvas::length_of(b.0 - a.0, b.1 - a.1));
                counted += 1;
            }
        }
        assert!(
            counted > 20,
            "only {counted} stars stayed in front of the eye"
        );
        assert!(
            moved < 1.0,
            "thirty seconds of full impulse moved a star {moved} subpixels"
        );
    }

    #[test]
    fn the_sky_streams_at_warp() {
        // The other half, and the one that says the first is a sky rather than
        // a still.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut sky = Universe::new(6.5, 3);
        fly(&mut sky, &mut ship, 600);

        let cam = cam();
        let eye = seated(&ship);
        let mut longest = 0.0f32;
        for path in swept(&sky, &cam, &eye, 0.0) {
            let (from, to) = ends(&path);
            longest = longest.max(crate::canvas::length_of(to.0 - from.0, to.1 - from.1));
        }
        assert!(
            longest > 20.0,
            "the longest streak at full warp is {longest} subpixels on a 48-tall canvas"
        );
    }

    #[test]
    fn a_trail_only_ever_grows_away_from_the_vanishing_point() {
        // The reported fault, as the property that rules it out.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(5.5, 23);
        fly(&mut sky, &mut ship, 240);
        ship.toggle_warp();

        // Only tails that are actually in the picture, which is the same
        // restriction `the_sky_holds_still_at_impulse` needs and for the same
        // reason.
        let cam = cam();
        let centre = (cam.cx, cam.cy);
        let on_canvas =
            |p: &(f32, f32)| (0.0..cam.width).contains(&p.0) && (0.0..cam.height).contains(&p.1);
        let radii = |sky: &Universe, ship: &Ship| -> Vec<Option<f32>> {
            let eye = seated(ship);
            let back = sky.trail;
            sky.stars
                .iter()
                .map(|star| {
                    let pos = eye.place(star.pos);
                    cam.project_beyond(
                        [
                            pos[0] + eye.nose[0] * back,
                            pos[1] + eye.nose[1] * back,
                            pos[2] + eye.nose[2] * back,
                        ],
                        STAR_NEAR,
                    )
                    .filter(&on_canvas)
                    .map(|p| crate::canvas::length_of(p.0 - centre.0, p.1 - centre.1))
                })
                .collect()
        };

        // Four seconds of spool-up, a frame at a time, which covers the whole
        // of the band the fault lived in.
        let mut watched = 0;
        let mut worst = 0.0f32;
        let mut before = (sky.positions(), radii(&sky, &ship));
        for _ in 0..240 {
            fly(&mut sky, &mut ship, 2);
            let after = (sky.positions(), radii(&sky, &ship));
            for (i, (was, is)) in before.1.iter().zip(&after.1).enumerate() {
                // A star that crossed its own limit and came back elsewhere is
                // a different star, not one that moved.
                if before.0[i] != after.0[i] {
                    continue;
                }
                if let (Some(a), Some(b)) = (was, is) {
                    worst = worst.max(a - b);
                    watched += 1;
                }
            }
            before = after;
        }
        assert!(watched > 100_000, "only {watched} tails were followed");
        // A tenth of a subpixel of slack for the arithmetic, against a fault
        // that moved tails most of the way to the middle of the frame.
        assert!(
            worst < 0.1,
            "a trail grew {worst} subpixels toward the vanishing point"
        );
    }

    #[test]
    fn the_exposure_never_outruns_the_flight_that_earned_it() {
        // The same statement one level down, where the arithmetic is: the trail
        // is a distance the ship has actually flown, so it can never be longer
        // than the distance flown since the drive lit.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(4.5, 29);
        fly(&mut sky, &mut ship, 240);
        ship.toggle_warp();

        let lit_at = ship.distance_ly;
        for _ in 0..600 {
            fly(&mut sky, &mut ship, 1);
            let flown = (ship.distance_ly - lit_at) as f32;
            assert!(
                sky.trail <= flown + 1e-4,
                "the exposure reaches {} ly after {flown} ly of flight",
                sky.trail
            );
        }
        fly(&mut sky, &mut ship, 2400);
        let settled = TRAIL_SECONDS * ship.warp_intensity() * ship.velocity_ly_per_s();
        assert!(
            (sky.trail - settled).abs() < 1e-3,
            "the exposure settled at {} rather than {settled}",
            sky.trail
        );
    }

    /// A spread of camera angles reaching both halves of the azimuth, so a
    /// question asked over it cannot be answered by the half where the track
    /// runs away from the eye.
    fn both_halves() -> Vec<crate::view::Orbit> {
        let mut spread = Vec::new();
        for azimuth in [-150.0f32, -90.0, -55.0, 0.0, 40.0, 90.0, 125.0, 179.0] {
            for elevation in [-40.0f32, 0.0, 35.0] {
                for roll in [0.0f32, 25.0] {
                    spread.push(crate::view::Orbit {
                        azimuth: azimuth.to_radians(),
                        elevation: elevation.to_radians(),
                        roll: roll.to_radians(),
                    });
                }
            }
        }
        spread
    }

    /// A ship at full warp with the exposure fully unrolled, and a sky to fly
    /// it through.
    fn spooled_up(limit: f32, seed: u64) -> (Universe, Ship) {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(limit, seed);
        fly(&mut sky, &mut ship, 240);
        ship.toggle_warp();
        fly(&mut sky, &mut ship, 1200);
        (sky, ship)
    }

    #[test]
    fn the_drive_throws_the_sky_forward() {
        // The shift at the level it is applied at rather than the level it is
        // defined at, which is the difference between knowing the curve is
        // right and knowing it is wired to anything.
        let (sky, ship) = spooled_up(6.5, 5);
        let cam = cam();
        let watching = |warp: f32| {
            let orbit = crate::view::Orbit::LEVEL;
            let eye = Observer::outside(
                ship.axes,
                ship.position,
                &Eye::new(orbit, crate::view::ZOOM_DEFAULT),
                orbit.nose_in_camera(),
                warp,
            );
            let mut out = Vec::new();
            sky.sweep(&cam, &eye, 3.5, |points, _, intensity| {
                let head = *points.last().expect("an exposure with no points");
                out.push((head.0, head.1, intensity));
            });
            out
        };

        let (soft, hard) = (0.4f32, 0.9f32);
        let (dim, lit) = (watching(soft), watching(hard));
        assert_eq!(
            dim.len(),
            lit.len(),
            "the drive changed which stars were drawn, not only how brightly"
        );
        assert!(!dim.is_empty(), "nothing was drawn to compare");

        let (mut ahead, mut astern) = (0, 0);
        for (a, b) in dim.iter().zip(&lit) {
            assert_eq!((a.0, a.1), (b.0, b.1), "the drive moved a star");
            let (u, v) = ((a.0 - cam.cx) / cam.focal, (a.1 - cam.cy) / cam.focal);
            let forward = 0.5 + 0.5 * u / (u * u + v * v + 1.0).sqrt();
            if forward > 0.85 {
                ahead += 1;
            } else if forward < 0.25 {
                astern += 1;
            }
            let want = shift_light(forward, hard * 0.9) / shift_light(forward, soft * 0.9);
            let got = b.2 / a.2;
            assert!(
                (got - want).abs() <= want * 1e-3,
                "a star at forward {forward} gained {got} where the shift asks {want}"
            );
            // Well clear of the turnover at 0.644 on both sides, so the bins
            // say something about the shift rather than about the root.
            if forward > 0.85 {
                assert!(got > 1.05, "a star ahead of the ship did not brighten");
            } else if forward < 0.25 {
                assert!(got < 0.95, "a star astern of the ship did not dim");
            }
        }
        assert!(
            ahead > 20 && astern > 20,
            "the camera saw {ahead} stars ahead and {astern} astern, which is \
             not both halves of the effect"
        );
    }

    #[test]
    fn no_camera_angle_collapses_a_streak_to_a_point() {
        // The reported fault: from a camera ahead of the ship, stars sweeping
        // past it had no trails at all.
        let (sky, ship) = spooled_up(5.0, 41);
        let cam = cam();
        assert!(sky.trail > 0.0, "the exposure never opened");

        let mut thinnest = usize::MAX;
        for orbit in both_halves() {
            let eye = Observer::outside(
                ship.axes,
                ship.position,
                &crate::view::Eye::new(orbit, 1.0),
                orbit.nose_in_camera(),
                ship.warp_intensity(),
            );
            let mut seen = 0;
            for path in swept(&sky, &cam, &eye, 0.0) {
                let (from, to) = ends(&path);
                assert!(
                    from != to,
                    "a streak collapsed to a point at {:?} degrees, nose z {}",
                    (
                        orbit.azimuth.to_degrees(),
                        orbit.elevation.to_degrees(),
                        orbit.roll.to_degrees()
                    ),
                    orbit.nose_in_camera()[2]
                );
                seen += 1;
            }
            thinnest = thinnest.min(seen);
        }
        // Every angle has to have had something to look at, or an angle that
        // drew nothing would pass this by drawing nothing.
        assert!(
            thinnest > 200,
            "the emptiest camera angle offered only {thinnest} streaks"
        );
    }

    #[test]
    fn the_exposure_is_cut_where_it_crosses_the_near_plane() {
        // The same statement down where the arithmetic is, and the other half
        // of it: the cut has to leave the tail in front of the near plane, and
        // it has to leave a tail that never reached the plane exactly alone.
        let (sky, ship) = spooled_up(4.5, 43);
        let mut cut = 0;
        let mut untouched = 0;
        for orbit in both_halves() {
            let eye = Observer::outside(
                ship.axes,
                ship.position,
                &crate::view::Eye::new(orbit, 1.0),
                orbit.nose_in_camera(),
                ship.warp_intensity(),
            );
            let back = [
                eye.nose[0] * sky.trail,
                eye.nose[1] * sky.trail,
                eye.nose[2] * sky.trail,
            ];
            for star in &sky.stars {
                let pos = eye.place(star.pos);
                if pos[2] <= STAR_NEAR {
                    continue;
                }
                let Some(tail) = tail_of(pos, back) else {
                    // Only the sliver between the two planes may be given up,
                    // and it is a star square abeam at a thousandth of a light
                    // year of depth — vanishingly rare and millions of
                    // subpixels off the canvas when it happens.
                    assert!(
                        pos[2] <= TAIL_NEAR,
                        "a tail at depth {} was given up",
                        pos[2]
                    );
                    continue;
                };
                assert!(
                    tail[2] > STAR_NEAR,
                    "a cut tail sits at depth {}, behind {STAR_NEAR}",
                    tail[2]
                );
                if pos[2] + back[2] > TAIL_NEAR {
                    assert_eq!(
                        tail,
                        [pos[0] + back[0], pos[1] + back[1], pos[2] + back[2]],
                        "a tail that never reached the near plane was moved anyway"
                    );
                    untouched += 1;
                } else {
                    cut += 1;
                }
            }
        }
        assert!(cut > 1000, "only {cut} tails were ever cut");
        assert!(untouched > 1000, "only {untouched} tails were left alone");
    }

    #[test]
    fn a_streak_is_the_track_the_star_actually_flew() {
        // The exposure, checked against the thing it claims to be rather than
        // against itself: the tail is where the star projected from where the
        // ship was three seconds ago.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut sky = Universe::new(5.0, 11);
        fly(&mut sky, &mut ship, 600);

        let cam = cam();
        let eye = seated(&ship);
        let back = sky.trail;
        let drawn = swept(&sky, &cam, &eye, 0.0);
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
            let Some(path) = drawn.iter().find(|p| ends(p).1 == to) else {
                continue;
            };
            assert_eq!(path.len(), 2, "a flight nobody steered drew a curve");
            assert_eq!(ends(path).0, from, "the exposure is not the track");
            checked += 1;
        }
        assert!(
            checked > 50,
            "only {checked} stars were in front of the eye"
        );
    }

    /// The same flight as [`spooled_up`], with a hand buried on the stick: at
    /// warp, turning hard, which is the case the exposure has to bend for.
    fn spooled_up_turning(limit: f32, seed: u64) -> (Universe, Ship) {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(limit, seed);
        fly(&mut sky, &mut ship, 240);
        ship.toggle_warp();
        for _ in 0..1200 {
            ship.nudge_yaw(1.0);
            ship.nudge_pitch(-0.4);
            fly(&mut sky, &mut ship, 1);
        }
        (sky, ship)
    }

    #[test]
    fn a_trail_thins_away_at_the_end_it_is_forgetting() {
        // The reported fault, flown the way it was reported: turn at low warp,
        // let the stick go, and a second or so later every trail on screen is
        // erased at once rather than fading out.
        let mut sky = Universe::new(5.0, 83);
        let mut ship = Ship::new();
        ship.throttle = 0.25;
        fly(&mut sky, &mut ship, 240);
        ship.toggle_warp();
        fly(&mut sky, &mut ship, 600);
        for _ in 0..120 {
            ship.nudge_yaw(1.0);
            fly(&mut sky, &mut ship, 1);
        }
        let cam = cam();
        let inside = |p: (f32, f32)| {
            p.0 > 2.0 && p.0 < cam.width - 2.0 && p.1 > 2.0 && p.1 < cam.height - 2.0
        };
        for frame in 0..5 {
            // A quarter of a second at a time, so the five of them span the
            // window the exposure forgets over.
            fly(&mut sky, &mut ship, 30);
            let eye = seated(&ship);
            let (mut sum, mut counted) = (0.0f32, 0usize);
            for path in swept(&sky, &cam, &eye, 0.0) {
                let (tail, head) = ends(&path);
                let span = crate::canvas::length_of(head.0 - tail.0, head.1 - tail.1);
                if span < 8.0 || !inside(tail) || !inside(head) {
                    continue;
                }
                // One exposure at a time on a canvas of its own, because the
                // question is about this star's own ends and a frame with the
                // whole sky on it has other stars lying across them.
                let mut canvas = crate::canvas::Canvas::new(80, 48);
                canvas.draw_path(&path, [1.0; 3], 1.0);
                // The brightest subpixel each end landed in the neighbourhood
                // of: a sample is splatted over four taps by its bilinear
                // weights, so the one subpixel it nominally sits on holds only
                // a share of it.
                let brightest = |p: (f32, f32)| {
                    let mut most = 0.0f32;
                    for dy in -1..=1i32 {
                        for dx in -1..=1i32 {
                            let (x, y) = (p.0 as i32 + dx, p.1 as i32 + dy);
                            most = most.max(canvas.light_at(x as usize, y as usize));
                        }
                    }
                    most
                };
                let (old, new) = (brightest(tail), brightest(head));
                if new <= 0.0 {
                    continue;
                }
                sum += old / new;
                counted += 1;
            }
            assert!(
                counted > 20,
                "frame {frame} had only {counted} trails long enough to have two ends"
            );
            let mean = sum / counted as f32;
            assert!(
                mean < 0.12,
                "frame {frame}: the end being forgotten holds {mean} of the head                  across {counted} trails, which is a blunt edge rather than a fade"
            );
        }
    }

    #[test]
    fn a_trail_bends_the_way_the_ship_turned() {
        // The reported fault, as the property that rules it out.
        let (sky, ship) = spooled_up_turning(5.0, 47);
        let cam = cam();
        let eye = seated(&ship);
        let mut bent = 0;
        let mut worst = 0.0f32;
        for path in swept(&sky, &cam, &eye, 0.0) {
            if path.len() < 3 {
                continue;
            }
            let (tail, head) = ends(&path);
            let (dx, dy) = (head.0 - tail.0, head.1 - tail.1);
            let span = crate::canvas::length_of(dx, dy);
            if span < 1.0 {
                continue;
            }
            // How far the middle of the track falls off the line between its
            // ends, which is zero exactly when the two are the same picture.
            for p in &path[1..path.len() - 1] {
                let off = ((p.0 - tail.0) * dy - (p.1 - tail.1) * dx).abs() / span;
                worst = worst.max(off);
            }
            bent += 1;
        }
        assert!(
            bent > 100,
            "only {bent} exposures were cut into a curve at all"
        );
        assert!(
            worst > 2.0,
            "the furthest any track strayed from its own chord was {worst} subpixels"
        );
    }

    #[test]
    fn a_flight_that_never_turns_draws_no_curve_at_all() {
        // The other half, and the one the reference frames rest on: an exposure
        // that does not reach back past a turn is two points and the arithmetic
        // it always was.
        let (sky, ship) = spooled_up(5.0, 47);
        let cam = cam();
        let eye = seated(&ship);
        let mut seen = 0;
        for path in swept(&sky, &cam, &eye, 0.0) {
            assert_eq!(path.len(), 2, "a flight nobody steered drew a curve");
            seen += 1;
        }
        assert!(seen > 100, "only {seen} exposures were drawn");
    }

    #[test]
    fn a_streak_is_the_track_the_star_actually_flew_when_the_ship_was_turning() {
        // The sibling of `a_streak_is_the_track_the_star_actually_flew`, asked
        // where that one cannot reach.
        let cam = Camera::new(200, 120);
        let on_canvas =
            |p: &(f32, f32)| (0.0..cam.width).contains(&p.0) && (0.0..cam.height).contains(&p.1);
        let mut checked = 0;
        let mut worst = 0.0f32;
        for seed in [51, 67, 71] {
            let (sky, ship) = spooled_up_turning(4.5, seed);
            let eye = seated(&ship);
            let reach = sky.trail;
            let drawn = swept(&sky, &cam, &eye, 0.0);
            // How many poses the frame cut the exposure into.
            let legs = drawn.iter().map(|p| p.len() - 1).max().unwrap_or(0);
            assert!(legs >= 2, "the frame drew no curve to check");
            for path in &drawn {
                if path.len() < 3 {
                    continue;
                }
                // The head says which star this is: nothing else about it
                // moved.
                let head = ends(path).1;
                let Some(star) = sky
                    .stars
                    .iter()
                    .find(|star| cam.project_beyond(eye.place(star.pos), STAR_NEAR) == Some(head))
                else {
                    continue;
                };
                let turned = sky.track.turn_over(reach);
                for (leg, point) in path[1..path.len() - 1].iter().rev().enumerate() {
                    let back = sky
                        .track
                        .back_at_turn(turned * (leg + 1) as f32 / legs as f32, reach);
                    let (was, axes) = sky.track.pose_at(back);
                    let then = Observer::cockpit(axes, was, eye.warp);
                    let placed = then.place(star.pos);
                    let Some(want) = cam.project_beyond(placed, STAR_NEAR) else {
                        continue;
                    };
                    // Only what is actually in the picture, and for the reason
                    // the trail tests above need the same filter: the two
                    // routes reach a star's place from the two ends of a
                    // subtraction that is a thousand light years wide, so they
                    // agree to about a ten-thousandth of a light year — and
                    // `focal · lateral / z` turns that into subpixels the
                    // moment `z` is a hundredth of one.
                    if !on_canvas(&want) {
                        continue;
                    }
                    worst = worst.max(crate::canvas::length_of(point.0 - want.0, point.1 - want.1));
                    checked += 1;
                }
            }
        }
        assert!(checked > 400, "only {checked} poses were checked");
        assert!(
            worst < 0.05,
            "a station landed {worst} subpixels from the pose it stands for"
        );
    }

    #[test]
    fn the_head_of_a_trail_is_where_the_star_is_now() {
        // Curved or straight, the last point is where the star actually is, and
        // exactly so: everything about a star's brightness is worked out there,
        // and a head that had drifted would put the light somewhere the star is
        // not.
        for (sky, ship) in [spooled_up(5.0, 53), spooled_up_turning(5.0, 53)] {
            let cam = cam();
            let eye = seated(&ship);
            let heads: Vec<_> = swept(&sky, &cam, &eye, 0.0)
                .iter()
                .map(|p| ends(p).1)
                .collect();
            let mut found = 0;
            for star in &sky.stars {
                let Some(to) = cam.project_beyond(eye.place(star.pos), STAR_NEAR) else {
                    continue;
                };
                if heads.contains(&to) {
                    found += 1;
                }
            }
            assert!(found > 100, "only {found} heads were where their star is");
        }
    }

    #[test]
    fn no_camera_angle_collapses_a_bent_streak_to_a_point() {
        // The steered sibling of
        // `no_camera_angle_collapses_a_streak_to_a_point`.
        let (sky, ship) = spooled_up_turning(5.0, 59);
        let cam = cam();
        let on_canvas =
            |p: &(f32, f32)| (0.0..cam.width).contains(&p.0) && (0.0..cam.height).contains(&p.1);
        assert!(sky.trail > 0.0, "the exposure never opened");

        let mut thinnest = usize::MAX;
        let mut curved = 0;
        for orbit in both_halves() {
            let eye = Observer::outside(
                ship.axes,
                ship.position,
                &crate::view::Eye::new(orbit, 1.0),
                orbit.nose_in_camera(),
                ship.warp_intensity(),
            );
            let mut seen = 0;
            for path in swept(&sky, &cam, &eye, 0.0) {
                let (from, to) = ends(&path);
                // A star whose whole track is outside the cone is drawn as the
                // point it has become — see `walk_back`.
                if from == to && !on_canvas(&to) {
                    continue;
                }
                assert!(
                    from != to,
                    "an exposure collapsed to a point at {:?} degrees, nose z {}",
                    (
                        orbit.azimuth.to_degrees(),
                        orbit.elevation.to_degrees(),
                        orbit.roll.to_degrees()
                    ),
                    orbit.nose_in_camera()[2]
                );
                curved += usize::from(path.len() > 2);
                seen += 1;
            }
            thinnest = thinnest.min(seen);
        }
        assert!(
            thinnest > 200,
            "the emptiest camera angle offered only {thinnest} exposures"
        );
        assert!(curved > 1000, "only {curved} of them were drawn as curves");
    }

    #[test]
    fn a_star_the_ship_turned_away_from_is_cut_at_the_near_plane() {
        // The cut fires in the *cockpit* now, which it could not before and
        // which the crate's notes said it could not: going back in time used to
        // move a star straight away from the nose, so from the seat the tail's
        // depth only ever increased.
        let (sky, ship) = spooled_up_turning(5.0, 61);
        let cam = cam();
        let eye = seated(&ship);
        let reach = sky.trail;
        let mut cut = 0;
        for star in &sky.stars {
            let pos = eye.place(star.pos);
            if pos[2] <= STAR_NEAR {
                continue;
            }
            // Where the walk would have gone if nothing stopped it: the oldest
            // pose the exposure reaches back to.
            let (was, axes) = sky.track.pose_at(reach);
            let then = Observer::cockpit(axes, was, eye.warp);
            if then.place(star.pos)[2] <= TAIL_NEAR {
                cut += 1;
            }
        }
        assert!(
            cut > 0,
            "a hard turn at warp put no star behind the near plane at all"
        );
        let on_canvas =
            |p: &(f32, f32)| (0.0..cam.width).contains(&p.0) && (0.0..cam.height).contains(&p.1);
        for path in swept(&sky, &cam, &eye, 0.0) {
            let (from, to) = ends(&path);
            assert!(
                from != to || !on_canvas(&to),
                "a cut exposure collapsed to a point in the picture"
            );
        }
    }

    #[test]
    fn a_star_too_far_off_the_axis_still_has_its_exposure_cut() {
        // The cone leaking through its own fallback.
        let (sky, ship) = spooled_up_turning(5.0, 67);
        let cam = Camera::new(600, 48);
        let eye = seated(&ship);
        let mut worst = 0.0f32;
        let mut seen = 0;
        for path in swept(&sky, &cam, &eye, 0.0) {
            let (tail, head) = ends(&path);
            // A star the walk found nowhere for is drawn as its own head twice
            // over, and a head is not a tail — see the test below.
            if tail == head {
                continue;
            }
            // Every point but the last.
            for p in &path[..path.len() - 1] {
                worst = worst.max(crate::canvas::length_of(p.0 - cam.cx, p.1 - cam.cy));
                seen += 1;
            }
        }
        assert!(seen > 1000, "only {seen} tail points were drawn at all");
        // On the cone, lateral over depth is `sqrt(1 - c²)/c`, so nothing the
        // walk admits can project further out than the focal length times that.
        let bound = cam.focal * (1.0 - TAIL_COS * TAIL_COS).sqrt() / TAIL_COS;
        assert!(
            worst < bound * 1.01,
            "a tail landed {worst} subpixels out, against a cone at {bound}"
        );
    }

    #[test]
    fn a_star_the_walk_gives_up_on_is_still_a_star() {
        // The other half: a star whose whole track is outside the cone has no
        // exposure to draw, and it used to be dropped head and all.
        let (sky, ship) = spooled_up_turning(5.0, 71);
        let cam = Camera::new(600, 48);
        let eye = seated(&ship);
        let mut drawn = 0;
        sky.sweep(&cam, &eye, 0.0, |path, _, _| {
            drawn += 1;
            assert!(
                path.len() >= 2,
                "an exposure came over as {} points",
                path.len()
            );
        });
        // Every star bright enough to draw is drawn, whatever the walk made of
        // its tail.
        assert!(drawn > 500, "only {drawn} exposures were handed over");
    }

    #[test]
    fn a_star_is_as_bright_as_the_inverse_square_of_its_own_range() {
        // Magnitudes are logarithms and the canvas is linear light, so the one
        // transform between them is the only thing here that has to be right.
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
        // somebody picked.
        for limit in [4.0f32, 5.0, 6.0, 7.0, 8.0] {
            let here = Universe::population(limit) as f32;
            let brighter = Universe::population(limit - 1.0) as f32;
            let ratio = here / brighter;
            assert!(
                (ratio - 10f32.powf(COUNT_SLOPE)).abs() < 0.05,
                "a magnitude at {limit} is {ratio} times as many stars"
            );
        }
        assert_eq!(Universe::population(-2.0), 0);
    }

    #[test]
    fn the_exposure_is_cut_finely_enough_to_be_a_curve() {
        // `SAGITTA` and the spacing it drives were named by no test at all, and
        // the first attempt at one asserted *counts* — a handful of legs for a
        // weave, the ceiling for a buried stick.
        let (legs, worst) = fit_of_the_exposure(&Camera::new(60, 36), 0.01);
        assert!(
            legs < MAX_STATIONS - 1,
            "the ceiling bound this turn at {legs} legs, so it says nothing about the spacing"
        );
        assert!(
            worst < 0.25,
            "a {legs}-leg exposure fell {worst} subpixels short of its own curve"
        );
    }

    /// Fly a turn, then ask how many legs one star's exposure was cut into and
    /// how far the drawn polyline strays from the curve it stands for.
    fn fit_of_the_exposure(cam: &Camera, stick: f32) -> (usize, f32) {
        // A sky dense enough that the band below holds a few hundred stars
        // rather than a few: what is being sampled is the *curve*, and one star
        // is one curve however finely it is walked.
        let mut sky = Universe::new(6.0, 4);
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..1200 {
            ship.nudge_yaw(stick);
            ship.nudge_pitch(-stick);
            fly(&mut sky, &mut ship, 1);
        }

        let reach = sky.exposure();
        let eye = seated(&ship);
        let mut stations = [Station::HELD; MAX_STATIONS];
        let legs = sky.stations(cam, &eye, reach, &mut stations);
        assert!(legs > 0, "the flight never turned enough to cut anything");

        let mut worst = 0.0f32;
        let mut checked = 0;
        let mut points = [(0.0f32, 0.0f32, 0.0f32); MAX_STATIONS];
        for star in &sky.stars {
            let pos = eye.place(star.pos);
            let Some(head) = cam.project_beyond(pos, STAR_NEAR) else {
                continue;
            };
            // Out where the budget was derived, and on screen: the spacing is
            // set by the frame corner, so a star near the axis is inside the
            // budget however coarsely it is cut.
            let radius = crate::canvas::length_of(head.0, head.1);
            if radius < cam.width * 0.2 || radius > cam.width * 0.6 {
                continue;
            }
            let lo = walk_back(&stations[..legs], pos, head, cam, &mut points);
            if lo > legs - 1 {
                continue;
            }
            let drawn = &points[lo..=legs];

            // The same exposure, walked finely and independently.
            for i in 0..=200 {
                let back = reach * i as f32 / 200.0;
                let (origin, axes) = sky.track.pose_at(back);
                let then = Observer::cockpit(axes, origin, ship.warp_intensity());
                let Some(p) = cam.project_beyond(then.place(star.pos), STAR_NEAR) else {
                    continue;
                };
                let away = drawn
                    .windows(2)
                    .map(|leg| distance_to_segment(p, (leg[0].0, leg[0].1), (leg[1].0, leg[1].1)))
                    .fold(f32::INFINITY, f32::min);
                if away.is_finite() && away > worst {
                    worst = away;
                }
                checked += 1;
            }
            if checked > 4000 {
                break;
            }
        }
        assert!(checked > 600, "only {checked} samples were compared");
        (legs, worst)
    }

    /// How far `p` is from the segment `a`-`b`.
    fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let square = dx * dx + dy * dy;
        let t = if square > f32::MIN_POSITIVE {
            (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / square).clamp(0.0, 1.0)
        } else {
            0.0
        };
        crate::canvas::length_of(a.0 + dx * t - p.0, a.1 + dy * t - p.1)
    }

    #[test]
    fn a_sky_asked_for_mid_flight_arrives_around_the_ship() {
        // Regression, and the shape of it is the one this tree keeps warning
        // about: every guard on `set_limit` flew a ship that had never left the
        // origin, so all of them passed a `spawn` that placed its stars about
        // the world's origin rather than about the observer.
        let mut sky = Universe::new(6.0, 11);
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        fly(&mut sky, &mut ship, 6000);
        assert!(
            ship.distance_ly > 100.0,
            "the ship needs to be well clear of the origin, and reached {}",
            ship.distance_ly
        );

        let before = sky.len();
        sky.set_limit(6.5);
        let fresh = &sky.positions()[before..];
        assert!(!fresh.is_empty(), "a fainter limit found nothing new");

        let mut mean = [0.0f32; 3];
        for star in fresh {
            let away = [
                (star[0] as f64 - ship.position[0]) as f32,
                (star[1] as f64 - ship.position[1]) as f32,
                (star[2] as f64 - ship.position[2]) as f32,
            ];
            let range = length_of_3([0.0; 3], away).max(f32::MIN_POSITIVE);
            for (axis, component) in mean.iter_mut().zip(away) {
                *axis += component / range;
            }
        }
        let bias = length_of_3([0.0; 3], mean.map(|axis| axis / fresh.len() as f32));
        assert!(
            bias < 0.1,
            "the {} stars a fainter limit added lean {bias} off the ship",
            fresh.len()
        );

        let strays = sky.stars[before..]
            .iter()
            .filter(|star| distance_sq(star.pos, ship.position) > star.reach_sq)
            .count();
        assert_eq!(
            strays,
            0,
            "{strays} of {} fresh stars were placed past their own reach",
            sky.len() - before
        );
    }

    #[test]
    fn asking_for_a_fainter_sky_leaves_the_one_already_out_there_alone() {
        // A limit is a property of the observer, so moving it must not move a
        // star: the sky that was visible before is visible still and in the
        // same place, with more of it beyond.
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
        // star winking out.
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
            sky.advance(
                ship.position,
                ship.axes,
                1.0 / 120.0,
                ship.warp_intensity(),
                ship.velocity_ly_per_s(),
            );
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
                let fade_ratio = 10f32.powf(0.4 * FADE_MAGNITUDES);
                let fade = ((over - 1.0) / (fade_ratio - 1.0)).clamp(0.0, 1.0);
                let lit = fade * fade * shift_light(1.0, 0.9);
                assert!(
                    lit < 1e-9,
                    "a recycled star arrived with {lit} of a unit already lit"
                );
            }
        }
        assert!(arrived > 0, "nothing recycled in a second at full warp");
    }

    #[test]
    fn the_flow_brings_stars_in_from_the_side_it_is_coming_from() {
        // The recycle's distribution, which is the other half of nothing
        // arriving in view.
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
        // at: nothing about *looking* at the sky can move it.
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
            );
            let mut drawn = 0;
            sky.sweep(&cam, &eye, turn as f64, |_, _, _| drawn += 1);
            assert!(drawn > 0, "nothing at all was visible from {orbit:?}");
        }
        assert_eq!(parked, sky.positions(), "looking at the sky moved it");

        let mut ship = Ship::new();
        ship.throttle = 0.0;
        fly(&mut sky, &mut ship, 2000);
        assert_eq!(parked, sky.positions(), "a parked ship moved the sky");
    }
}
