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

use crate::camera::Camera;
use crate::track::Track;
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
/// the ship covers 5.48 ly a second and the nearest star is ten light years
/// off, so one step of 1/120 s moves it 0.19 subpixels and six of those is
/// barely more than a point.
///
/// It is the length the exposure *settles* at rather than the length it has:
/// see [`Universe::trail`], which may only ever grow as fast as the ship
/// actually flies.
///
/// So the streak stops being a stretch and becomes what it always looked like:
/// the track a star actually flew, over the last three seconds. On an 80x24
/// terminal at full warp that is 67 subpixels for a star at ten light years,
/// 9.6 at seventy, 2.1 at three hundred and 0.7 at a thousand — near stars
/// tearing past, mid ones dashing, far ones holding still as points. The old
/// sky drew every star the same length because every star was the same distance
/// away.
///
/// Three seconds because that is where the near streaks reach across most of
/// the frame without the mid-distance ones running together. It is a fixed
/// fraction of the frame on every terminal, since `Camera::focal` follows the
/// canvas height and so does the canvas.
const TRAIL_SECONDS: f32 = 3.0;

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
/// actually be, so `Universe::spawn` draws another. That shifts the census
/// toward the luminous classes as the limit comes down, which is not a side
/// effect but the observation — a magnitude-two sky really is all giants, and
/// the M dwarfs really are all too faint to be in it.
pub const NEAREST_STAR: f32 = 4.0;

/// Nearest a star is projected from, in light years.
///
/// Not the hull's near plane. [`crate::camera::Z_NEAR`] is 0.9 in units where
/// the ship is about one, and a star that came within nine tenths of a light
/// year would be clipped by a number that has nothing to do with it — so the
/// projection is asked for this one instead. A star this close is passing
/// through the ship in all but name and there is nothing sensible left to draw.
const STAR_NEAR: f32 = 0.001;

/// Nearest the *tail* of an exposure is cut to, in light years.
///
/// A hair in front of [`STAR_NEAR`], and the margin is doing the same job
/// `models::PLUME_NEAR`'s does: the cut is solved by subtracting two numbers of
/// about the exposure's own length, so landing exactly on the plane is a
/// coin-toss at the last bit, and [`Camera::project_beyond`] refuses a point
/// *at* its near plane as well as behind it. Five percent of a thousandth of a
/// light year is four orders of magnitude more room than the cancellation can
/// take.
const TAIL_NEAR: f32 = STAR_NEAR * 1.05;

/// How far off the camera's axis an exposure is followed before it is cut, as
/// the cosine of the angle.
///
/// About eighty-one degrees, and a *cone* rather than a plane, which is the one
/// place the curved exposure needs a boundary the straight one never did.
/// [`TAIL_NEAR`] is a thousandth of a light year, which was ample while a tail
/// could only ever recede from the nose: from the seat, going back in time
/// pushed a star away, so nothing ever came near it. A turn takes that away.
/// The camera swings, so a star that is ahead now can have been square abeam
/// three seconds ago — and a star square abeam at a thousandth of a light year
/// of *depth* projects some hundreds of millions of subpixels off the side of
/// the canvas. It measured a sixth of every exposure drawn during a hard turn,
/// and what each one laid down was a straight line across the whole frame at
/// whatever angle the blow-up happened to point: the most conspicuous thing in
/// the picture, standing for a piece of track that is nowhere near it.
///
/// A plane cannot bound that and a cone can, because what has to stay bounded
/// is `focal · lateral / depth` and a cone is exactly a bound on the ratio.
/// This one holds it inside about nine frame-heights, which is off the canvas
/// on any terminal by a wide margin — so what it cuts is never seen, and what
/// it stops being drawn to is a point that was never anywhere near the picture.
const TAIL_COS: f32 = 0.15;

/// The most poses one exposure is drawn from.
///
/// A ceiling on the subdivision and not on the exposure, for the reason
/// [`crate::bend`] has one: a hard turn is drawn in full, and what this bounds
/// is how much work drawing it may become.
///
/// Twenty-four points is twenty-three legs, and it used to say here that this
/// covers the yaw stop's whole sweep at [`SAGITTA`] up to about 200x60. It does
/// not, and the arithmetic is worth writing out because the two constants work
/// against each other and this one silently wins. At the stop the *attitude*
/// turns about 3.1 radians over an exposure, against a per-leg budget of 0.131
/// on a 200x60 canvas: 24 legs before the near stars' parallax is counted, and
/// that multiplies it by five. So a buried stick asks for something over a
/// hundred legs and is given twenty-three.
///
/// The consequence is not a fault — a hard turn washes the sky out rather than
/// drawing it wrong, and the rates decay in under a second — but it is the
/// reason two plausible optimisations do not pay. Cutting the pose count by a
/// star's own parallax saves almost nothing, because the count is not set by
/// parallax at the stop; it is set here. And loosening `SAGITTA` saves nothing
/// either, for the same reason. What a turn costs is the arc length rasterised,
/// which is a question for `Canvas::draw_leg` rather than for either of these.
///
/// It is also the width of the array a frame walks, on the stack and reused by
/// every star, so it is a hundred and ninety bytes rather than an allocation.
const MAX_STATIONS: usize = 24;

/// How far a drawn exposure may fall short of the curve it stands for, in
/// subpixels, at the corner of the frame.
///
/// A quarter of a subpixel, spent on the picture rather than on an angle — see
/// [`Universe::stations`], which turns it into a spacing that follows the
/// canvas the way [`Camera::focal`] does.
///
/// A quarter rather than a half because two curvatures share it and they are
/// the same size: the ship flies where it points, so the attitude's turn is
/// also the track's, and a spacing that answers for the camera swinging has to
/// answer for the ship's own arc through space as well.
const SAGITTA: f32 = 0.25;

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
    /// The camera's own pose in the *hull's* frame — the identity from the
    /// seat, [`Eye::basis`] from outside — kept apart from `to_camera`, which
    /// is the two of them fused so `place` is three dot products.
    ///
    /// It is the half of that composition an exposure needs on its own. A
    /// station on the track carries the attitude the ship was holding *then*,
    /// and the camera has to be re-mounted on it: `mount · axes(then)`, with
    /// the camera on the left, which is the order `outside` composes above.
    /// From the seat it is the identity and the multiply is taken rather than
    /// branched around — a branch here would be a second spelling of the
    /// composition, and two spellings of one idea is how the two come apart.
    mount: [[f32; 3]; 3],
    /// 0..=1 across the superluminal range: it sets the Doppler shift and how
    /// much twinkle is left.
    ///
    /// It used to set the exposure as well, multiplied by a velocity carried
    /// beside it. Both went to [`Universe::trail`], which is where a length
    /// that has to remember what it drew last frame belongs — an observer is
    /// where an eye is, and how long the drive has been smearing is not a fact
    /// about where anybody is standing.
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
///
/// `turn · p + offset`, and holding it that way round is what makes a curved
/// exposure affordable: the `f64` subtraction that puts a star in front of the
/// eye is done once per star, and each further pose costs a three-by-three
/// product on the answer. Rewinding the ship and re-placing the star from
/// scratch would be that subtraction again for every pose of every star.
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
    ///
    /// A trail is laid down behind a star as the ship flies, so on screen it
    /// may only ever extend outward, away from the point the track vanishes at.
    /// The tail sits at depth `z + trail`; the star closes at the ship's speed,
    /// so `z` falls by `v·dt` a step, and the tail therefore moves *inward*
    /// exactly when the trail lengthens faster than that. Computed from
    /// `TRAIL_SECONDS · warp · velocity` with no memory it did: lighting the
    /// drive takes the velocity from 3.4 c to 2000 c, a factor of 588, so for
    /// the whole of the spool-up the exposure reached back into the past faster
    /// than time was passing — up to twelve times faster in the first half
    /// second — and every streak grew from its middle outward in both
    /// directions at once. Nothing was moving inward. The renderer was changing
    /// its mind about how much history to draw.
    ///
    /// Held to the distance actually flown, it cannot. Growing at exactly
    /// `v·dt` pins the tail's *world* point while the ship flies away from it,
    /// which is what a trail is, and it makes the length honest as well: three
    /// seconds after the drive lights there are three seconds of track behind
    /// the ship and not a retroactive thirty. Shrinking is left alone — the
    /// tail moving outward is the direction it is allowed to move, and a drive
    /// shutting down should visibly stop smearing.
    trail: f32,
    /// The flight the exposure is drawn along: where the ship has been, and
    /// which way it was pointed while it was there.
    ///
    /// It lives here rather than on [`crate::ship::Ship`] because it is the
    /// other half of `trail` — an exposure is a length *and* the track that
    /// length is measured along, and separating the two is how they come apart.
    /// There is a mechanical reason as well: [`Observer`] is `Copy`, and a
    /// borrow of the track on it would push a lifetime through both of its
    /// constructors and every caller in [`crate::render`].
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

    /// How far back along the track the exposure currently reaches, in light
    /// years.
    ///
    /// State rather than an answer worked out on demand — see `Self::trail`
    /// — so a caller that wants to know how much history is being drawn has to
    /// ask the sky that drew it rather than recompute it from the ship, which
    /// is the whole of the bug that field exists to have fixed. The reference
    /// flights' own coverage test is what wants it: whether a flight reaches
    /// the near-plane cut at all is `exposure` against [`NEAREST_STAR`], and
    /// neither number can be guessed from the flags.
    pub fn exposure(&self) -> f32 {
        self.trail
    }

    /// How far back the flight is straight, in light years — see
    /// [`crate::track::Track::straight_run`], which answers with infinity when
    /// nothing has turned at all.
    ///
    /// Public for the same reason [`Self::exposure`] is, and it is the same
    /// caller: the reference flights' coverage test asks whether any of them
    /// ever draws an exposure that reaches back *past* a turn, which is what
    /// makes one a curve rather than a segment. Neither number can be guessed
    /// from the flags — a flight that steers hard at impulse never bends one,
    /// and a flight that lights the drive after it has stopped steering does
    /// not either — so the only way to ask is to fly it and look.
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
    ///
    /// The origin is read once and handed down, because a star is placed inside
    /// its own visibility sphere and that sphere is centred on the *observer*
    /// rather than on the world's origin. It only ever mattered on the second
    /// caller: [`Self::new`] runs before the ship has gone anywhere, so the two
    /// agreed for as long as `set_limit` was the thing nobody pressed
    /// mid-flight. [`Track::pose_at`] answers `[0.0; 3]` on an empty ring, so
    /// construction is the arithmetic it always was.
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
    ///
    /// The ship does the moving — see `crate::ship::Ship::coast` — so there
    /// is nothing to translate here and no screen for anything to fall off.
    /// What is left is one sample and one distance test per star.
    ///
    /// It is handed the whole attitude rather than the nose alone, and the
    /// other two axes are `Self::track`'s: a camera bolted to the hull rolls
    /// with it, so an exposure drawn from where the ship *was* has to be drawn
    /// through how it was pointed as well. The recycle reads the nose out of it
    /// and draws in exactly the place in the sequence it always did — `--seed`
    /// reproducibility is a property of the RNG's draw order, and a draw moved
    /// is a different sky.
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
        // rather than reaching past it. See `Self::trail`; shortening is not
        // held back, because a shorter trail moves its tail *outward* and that
        // is the direction a tail is allowed to move.
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
    ///
    /// A visitor rather than an iterator, and the shape follows the thing. An
    /// exposure is a *path* now, of a length this frame picks, and an iterator
    /// would have to hand each one over either in a fixed array wide enough for
    /// the worst turn — a hundred and ninety bytes copied per star, at seventy
    /// thousand stars a frame — or in a buffer it cannot lend out while it is
    /// still borrowing itself. Handing the slice to a closure costs neither.
    ///
    /// `time` feeds the sublight twinkle and nothing else.
    pub fn sweep(
        &self,
        cam: &Camera,
        eye: &Observer,
        time: f64,
        mut draw: impl FnMut(&[crate::canvas::Trace], [f32; 3], f32),
    ) {
        // How far back along the track the exposure reaches, as a displacement
        // in the camera's own space. Exactly zero at sublight, where there is
        // no drive lit to smear anything and the sky is a fixed backdrop, and
        // unrolled by [`Self::advance`] rather than worked out here — a length
        // this frame computed on its own could outrun the flight that earned
        // it, and a trail that grows faster than the ship flies grows the wrong
        // way.
        let reach = self.trail;
        let back = [
            eye.nose[0] * reach,
            eye.nose[1] * reach,
            eye.nose[2] * reach,
        ];
        // `FADE_MAGNITUDES` expressed as the brightness ratio the arithmetic
        // below actually works in, taken once for the frame rather than written
        // out as a second constant that could disagree with the first.
        // Where the exposure was open from, when the ship did not simply fly
        // straight through it. Empty is the fast path: `back` above is then the
        // whole answer and this frame is the frame it always was, to the bit.
        let mut stations = [Station::HELD; MAX_STATIONS];
        let legs = self.stations(cam, eye, reach, &mut stations);
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

            // How far off the nose it sits. Dead ahead blue-shifts; astern,
            // where it is falling behind, it reddens.
            let along = pos[0] * eye.nose[0] + pos[1] * eye.nose[1] + pos[2] * eye.nose[2];
            let forward = if range_sq > f32::MIN_POSITIVE {
                (0.5 + 0.5 * along / range_sq.sqrt()).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let color = shift_color(CLASSES[star.class].rgb, forward, doppler);

            // Where the star was while the shutter was open. On the fast path
            // that is one point, cut against the near plane by [`tail_of`] —
            // which from the seat is an identity and from a camera ahead of the
            // ship is the whole of whether a streak gets drawn at all. This
            // arm is the arithmetic every reference frame is pinned through and
            // is written out here rather than as the one-station case of the
            // walk below, because the walk composes the same rotation out of a
            // matrix product and would land an ulp away from it.
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
                // what it always was. It is measured here rather than left for
                // the canvas to work out because [`crate::bend`] may chop this
                // into pieces before it gets there, and every piece has to keep
                // the pace of the leg it came from.
                let pace = crate::canvas::length_of(to.0 - from.0, to.1 - from.1);
                points[0] = (from.0, from.1, pace);
                points[1] = (to.0, to.1, pace);
                &points[..2]
            } else {
                let lo = walk_back(&stations[..legs], pos, to, cam, &mut points);
                if lo > legs - 1 {
                    // The walk found nowhere for the exposure to have been:
                    // the star is so far off the axis that no part of its
                    // track is in the picture. It is still a star, and it is
                    // still where it is, so it is drawn as the point it has
                    // become rather than dropped — a silent total vanish is
                    // the wrong failure for a guard about a *tail*.
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
    ///
    /// Zero is the case worth understanding, because it is nearly every frame.
    /// The ship flies where it points, so an attitude that has not changed is a
    /// track that is a straight line — and an exposure that does not reach back
    /// past the last turn is a straight rewind along the nose, which is what
    /// [`Self::sweep`]'s other arm does and what the reference frames are pinned
    /// through. [`crate::track::Track::straight_run`] is bitwise for that
    /// reason.
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
        // A point at screen radius `r` swept through `phi` traces an arc whose
        // chord falls `r·phi²/8` short of it, so holding that under the budget
        // out at the frame corner is one square root. It has to follow the
        // canvas, the way `Camera::focal` does: a fixed angle facets the arc on
        // a tall terminal and buys nothing on a short one, and the same flight
        // would not look the same on two machines.
        let corner = crate::canvas::length_of(cam.width * 0.5, cam.height * 0.5);
        let per_leg = (8.0 * SAGITTA / corner.max(1.0)).sqrt();
        // How much further than the camera's own turn a star can be carried by
        // the ship simply going somewhere. A star at range `R` swings by about
        // `reach / R` while the shutter is open, so the nearest one there is
        // swings by `reach / NEAREST_STAR` — four radians at full warp, against
        // the two and a half a buried stick turns the camera through. Leaving
        // it out was measured and it showed: the poses came out six where the
        // curve wanted twenty, half the sky was drawn a subpixel off its own
        // track and a tenth of it nearly five, with the near stars visibly
        // cornered. It is the parallax that makes a curved exposure hard to
        // draw, not the rotation.
        let turned = self.track.turn_over(reach);
        let swing = turned * (1.0 + reach / NEAREST_STAR);
        let legs =
            ((swing / per_leg.max(f32::MIN_POSITIVE)).ceil() as usize).clamp(1, MAX_STATIONS - 1);

        let inv = 1.0 / legs as f32;
        for (leg, station) in out.iter_mut().take(legs).enumerate() {
            // Spaced by equal *turn* rather than by equal track. A turn is
            // rarely spread evenly over three seconds of flight, and spacing
            // these along the track put every pose but one in the straight part
            // of a bank that lasted half a second — which drew a smooth arc as
            // a hard corner. The poses want to be where the curvature is, and
            // the curvature is the turn. The last one is pinned to the far end
            // rather than searched for, so the exposure ends exactly where it
            // reaches to whatever the search made of the last few radians.
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
            // how far the ship travelled in between. Written this way round so
            // the `f64` subtraction happens once per *station* rather than once
            // per station per star, which is the difference between a matrix
            // product and seventy thousand of them.
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
    /// The origin is an argument rather than a field read because it is the
    /// whole of what a visibility sphere is about: a star is placed at a
    /// distance that makes it look as bright as it does *from here*, so a
    /// sphere hung on the world's origin is the right shape in the wrong place
    /// the moment the ship has flown anywhere. It went unnoticed because the
    /// two agree exactly at launch, and `set_limit` — the only other caller —
    /// arrives from a key rather than from a constructor.
    ///
    /// The draw order is load-bearing rather than incidental — `--seed`
    /// reproducibility is a property of the sequence — and it is: class, then
    /// the three that scatter its absolute magnitude, then how deep into its
    /// sphere it sits, then the two that point it, then its twinkle. The
    /// translation below happens after all of them and draws nothing, so a
    /// seeded sky is the sky it always was.
    fn spawn(&mut self, origin: [f64; 3]) -> Star {
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

/// Where an exposure opened, in camera space, cut against the near plane.
///
/// `back` is the whole displacement to the tail — the nose, times how far the
/// ship flew while the shutter was open — and from the pilot's seat adding it
/// is the entire answer, because there the nose *is* the depth axis and going
/// back in time can only push a star further off. That is what the code here
/// used to say, flatly, with an `unwrap_or(head)` behind it that was supposed
/// to be unreachable.
///
/// It is not, and the exception is a whole half of the range of a control.
/// From outside, the nose is [`crate::view::Orbit::nose_in_camera`], whose
/// depth component is `-cos(elevation)·sin(azimuth)` — negative across the
/// entire forward half of the azimuth, where the camera is ahead of the ship
/// looking back down its own track. There the tail runs *toward* the eye and
/// through it, the projection refuses it, and the fallback fired for real:
/// every star inside the exposure's reach, which at full warp is 16.4 light
/// years and a fifth of the whole pool, lost its trail and was splatted as a
/// point instead — at full brightness, since `draw_streak`'s short branch
/// skips the `spread` division, so *brighter* than the streaks it should have
/// matched. A field of bright dots exactly where the tunnel should be.
///
/// The cut is the one `models::draw_flame` already makes against the same
/// plane, for the same reason and in the same closed form: the track is a
/// straight segment in camera space, so where it crosses is one division. What
/// is left is the part of the exposure that happened in front of the lens,
/// which is the part there was ever anything to see of.
///
/// `None` when the head itself is inside the sliver between the two planes —
/// a star at a thousandth of a light year of *depth*, which is one square
/// abeam and projects some millions of subpixels off the side of any canvas.
/// There is no room to cut a tail into and nothing would be drawn of it
/// anyway; dropping it is cheaper than collapsing it and says the same thing.
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
    // year, which is the one place in this arithmetic that cancels badly. The
    // plane it lands on is known exactly; only the lateral offsets need
    // working out.
    let t = ((TAIL_NEAR - pos[2]) / back[2]).clamp(0.0, 1.0);
    Some([pos[0] + back[0] * t, pos[1] + back[1] * t, TAIL_NEAR])
}

/// Where a leg of an exposure leaves the cone [`TAIL_COS`] describes.
///
/// A quadratic, because the cone is one: `z = c·|p|` squared is
/// `z² = c²(x² + y² + z²)`, and a straight segment through it is
/// `A t² + B t + C` with the coefficients below. The near root is the one
/// wanted — the exposure is followed from the head backward and stops the first
/// time it leaves.
///
/// `None` when the head is not inside the cone to begin with, which is a star
/// already so far off the axis that no part of its exposure is in the picture,
/// and when the segment does not cross at all, which is the caller's signal
/// that it never had to ask.
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
///
/// It walks *back from the head* and stops at the first pose behind the near
/// plane rather than picking up whatever survives beyond it. A star that went
/// behind the eye and came back is then one trail with its earlier half missing,
/// which is the cheaper of the two mistakes: the alternative is two paths for
/// one star, and each would carry its own ramp from [`crate::canvas`]'s tail
/// brightness up to full — two beads where there should be one trail.
///
/// Nothing inside the loop can fail. A pose that clears [`TAIL_NEAR`] clears
/// [`STAR_NEAR`] as well, so its projection is infallible; the cut lands
/// exactly on `TAIL_NEAR`, so that one is too. What the cut needs instead is
/// the pose *before* it to be in front, which is the same condition
/// [`tail_of`] states for itself.
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
    // and spent at the end on the pace of every leg. Only the walk knows both
    // halves: the canvas is handed a pace because a pace is what survives being
    // bent, and the arithmetic that turns one into the other belongs here.
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
        // The leg left the picture. The cone is the boundary that matters and
        // the plane is what is left when a star is so nearly at the eye that
        // the cone has nothing to say; falling through from one to the other
        // costs a quadratic on the few exposures that reach either.
        //
        // The fallback is only reached from *inside* the cone, and that guard
        // is the whole of it rather than a tidiness. `leaves_the_cone` answers
        // `None` for two quite different reasons — the segment never crosses,
        // and the near end was never inside to begin with — and the plane below
        // cannot tell them apart: handed a leg that starts outside, it finds
        // the depth already clear and hands the station back *uncut*, which is
        // the unbounded blow-up the cone exists to stop, leaking out through
        // the cone's own fallback. Measured at thirty-one stars a frame on a
        // terminal wide enough to see them.
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
    // have covered at that leg's pace. A leg that took none of the exposure
    // caught none of the light, and infinity is how the falloff says so.
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
    ///
    /// The sky hands its paths to a closure rather than yielding them, because
    /// an exposure is a slice of a buffer the frame reuses — so a test that
    /// wants to look at all of them at once has to copy them out, and this is
    /// the one place that does it.
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
        // What the user asked for, as a number. The nearest star a magnitude
        // limit puts in the sky is about ten light years off, and at 0.9 c the
        // ship covers 0.0025 ly a second — so it subtends
        // `focal · v / d = 40.8 · 0.0025 / 10` of a subpixel a second, or 0.30
        // over thirty seconds of flight.
        //
        // The margin is 3.3x and deliberately not more. A threshold with a
        // hundredfold margin would go on passing a speed map that had regressed
        // by an order of magnitude, which is exactly the fault this replaced:
        // the old sky ran impulse at a twentieth of its warp rate while the dial
        // read a two-thousandth.
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
            // And only what is actually in the picture. A star square abeam has
            // a camera depth of very nearly nothing and projects thousands of
            // subpixels off the side, where the canvas clips it and where a
            // hundredth of a degree of parallax is still a large number — so
            // measuring one would be asking this question of the projection
            // rather than of the sky.
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
        // The reported fault, as the property that rules it out. A trail is
        // laid down behind a star as the ship flies, so on screen it may only
        // extend *outward*; an end that creeps toward the point the track
        // vanishes at is the renderer reaching further into the past than time
        // has passed, and nothing in the picture is moving that way.
        //
        // Watched across an engage, because that is where it showed: lighting
        // the drive takes the velocity from 3.4 c to 2000 c, and an exposure
        // computed fresh each frame from `TRAIL_SECONDS · warp · velocity`
        // lengthened up to twelve times faster than the ship flew. Every streak
        // grew from its middle in both directions at once for the whole of the
        // spool-up.
        //
        // The tail sits at depth `z + trail`. The star closes at the ship's
        // speed, so `z` falls by `v·dt` a step, and holding the trail's growth
        // to the same `v·dt` makes `z + trail` non-increasing — which is the
        // whole proof, and why this is asserted on the radius rather than on
        // the constant.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        let mut sky = Universe::new(5.5, 23);
        fly(&mut sky, &mut ship, 240);
        ship.toggle_warp();

        // Only tails that are actually in the picture, which is the same
        // restriction `the_sky_holds_still_at_impulse` needs and for the same
        // reason. A star square abeam has a camera depth of very nearly
        // nothing, so its tail projects tens of thousands of subpixels off the
        // side of a canvas eighty wide — and `focal · lateral / (z + trail)`
        // with a denominator of two hundredths is a division whose *noise* is a
        // third of a subpixel. Measuring one of those asks how well the
        // projection conditions, not which way a trail grows.
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
        // And it does get there, or the clamp would be hiding the exposure
        // rather than pacing it.
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
    ///
    /// The forward half is the half that matters here and it is the half the
    /// reference frames barely touch: `orbit.txt` is the only one of the nine
    /// with the camera ahead of the ship at all, and it is oblique. Dead ahead,
    /// where the exposure runs straight at the lens, nothing was watching.
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
    fn no_camera_angle_collapses_a_streak_to_a_point() {
        // The reported fault: from a camera ahead of the ship, stars sweeping
        // past it had no trails at all.
        //
        // The tail is the star projected from where the ship *was*, which is
        // `nose · reach` further along the track. From the seat that is `+z`
        // and the tail can only ever recede — the claim the code used to make
        // flatly, with an `unwrap_or(head)` behind it that was supposed to be
        // unreachable. From outside the nose is `Orbit::nose_in_camera`, whose
        // depth component is `-cos(elevation)·sin(azimuth)`, so across the
        // whole forward half of the azimuth the tail runs *toward* the eye and
        // through it. The fallback then fired for real, and every star inside
        // the exposure's reach — 16.4 light years at full warp, which is a
        // fifth of the pool and every one of the near stars that carry the
        // longest trails — collapsed to a point.
        //
        // Worse than losing the trail: `draw_streak`'s short branch splats at
        // full intensity with no `spread` division, so each one came out
        // *brighter* than the streaks it should have matched.
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
        //
        // Bit-exactness on the second half is the point rather than a nicety.
        // The cut is on the path every star in the program takes, and the four
        // cockpit reference flights are pinned through it.
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
        //
        // The retraction itself is honest and stays. An exposure is the track
        // of the last few seconds and those seconds age out, so the smear
        // genuinely shortens — down there it shortens fast, because the whole
        // window is `TRAIL_SECONDS · warp` and warp at a quarter throttle is a
        // little over a second. What was wrong was the *edge* it retracted
        // behind: a streak used to be laid down at a third of full brightness
        // where its light ran out, so the shortening dragged a bright blunt end
        // across the frame and then cut it, and every star shares one shutter
        // so they all went together.
        //
        // Said as a property of what is drawn rather than of the arithmetic:
        // through the whole window the exposure is forgetting, the light at the
        // end being forgotten has to be a small fraction of the light at the
        // end where the star is. Measured over these five frames the mean ratio
        // runs 0.027 to 0.073; with the floor back it runs 0.17 to 0.43, so the
        // bound below separates them by about a third either way rather than by
        // an order of magnitude — a threshold a factor of ten clear would go on
        // passing a floor half the size of the one this is about.
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
        // The reported fault, as the property that rules it out. With the drive
        // lit and the stick held over, a star's exposure is the track it swept
        // while the ship came round — so it is a curve, and drawing it as the
        // chord between its ends is the bug.
        //
        // Measured as the sagitta the chord would have missed by, which is the
        // number `Universe::stations` is spacing the poses against, so the two
        // are talking about the same thing.
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
        // it always was. Asked of the same sky at the same speed, so the only
        // difference between this and the test above is the stick.
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
        // where that one cannot reach. Every point of a bent exposure has to be
        // the star projected from a pose the ship really held — worked out the
        // long way round, by rewinding the ship and placing the star afresh,
        // rather than by the relative rotation the renderer composes. That
        // shortcut is the one thing here that could be quietly wrong, and this
        // is what says it is not.
        // A wider frame than the shared one, which is the only test here that
        // wants a different camera and says why: it is not comparing against a
        // figure quoted in subpixels, it is asking how many poses land in the
        // picture at all — and a hard turn sweeps most of the sky out of an
        // 80-column one.
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
            // How many poses the frame cut the exposure into. It is one number
            // for the whole frame, so the longest path is it — and a shorter
            // path is one the walk stopped early, whose points still stand for
            // the same poses counting back from the head. Only its *first*
            // point is something else: that one is where the walk cut, and it
            // answers to no pose at all.
            let legs = drawn.iter().map(|p| p.len() - 1).max().unwrap_or(0);
            assert!(legs >= 2, "the frame drew no curve to check");
            for path in &drawn {
                if path.len() < 3 {
                    continue;
                }
                // The head says which star this is: nothing else about it moved.
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
                    // Only what is actually in the picture, and for the reason the
                    // trail tests above need the same filter: the two routes reach
                    // a star's place from the two ends of a subtraction that is a
                    // thousand light years wide, so they agree to about a
                    // ten-thousandth of a light year — and `focal · lateral / z`
                    // turns that into subpixels the moment `z` is a hundredth of
                    // one. A pose that close to the plane projects some millions of
                    // subpixels off the side of any canvas and is thrown away by
                    // `Canvas::clip` before anything is drawn of it.
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
        // The steered sibling of `no_camera_angle_collapses_a_streak_to_a_point`.
        // That one flies `spooled_up`, which never touches the stick, so it says
        // nothing whatever about the walk that cuts a curved exposure against
        // the near plane — and the walk is where the forward half of the azimuth
        // bites, exactly as it did for the straight one.
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
                // point it has become — see `walk_back`. That is allowed and
                // invisible: such a star is six focal lengths off the axis and
                // hundreds of subpixels off the canvas. What is not allowed is
                // one collapsing *in the picture*, which is the fault this test
                // was written for.
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
        // depth only ever increased. A turn takes that away — the ship was
        // pointed somewhere else, so a star ahead of it now can have been
        // abeam or behind three seconds ago — and what catches it is the same
        // walk the view from outside needs.
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
        // And nothing that was cut came out as a bright point instead of a
        // trail, which is the fault the walk exists to have fixed. Only a star
        // whose whole track is outside the cone may be a point, and one of
        // those is far off the side of any canvas.
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
        // The cone leaking through its own fallback. `leaves_the_cone` answers
        // `None` for two different reasons — the leg never crosses, and the
        // near end was never inside — and the plane it falls through to cannot
        // tell them apart: handed a leg that starts outside, it finds the depth
        // already clear and hands the station back uncut, which is the
        // unbounded blow-up the cone exists to stop.
        //
        // Asked on a terminal wide enough for the cut to matter, because the
        // cone sits at six focal lengths and a square window hides everything
        // past it.
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
            // Every point but the last. The head is where the star actually is
            // and the cone says nothing about it — a star square abeam of the
            // camera *now* projects as far out as the arithmetic allows, which
            // is true today and was true before there was a cone.
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
        // exposure to draw, and it used to be dropped head and all. It is still
        // a star and it is still where it is, so it comes out as the point it
        // has become. Nothing in the picture changes either way — such a star
        // is six focal lengths off the axis — but a silent total vanish is the
        // wrong failure for a guard about a tail.
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
    fn the_exposure_is_cut_finely_enough_to_be_a_curve() {
        // `SAGITTA` and the spacing it drives were named by no test at all, and
        // the first attempt at one asserted *counts* — a handful of legs for a
        // weave, the ceiling for a buried stick. It caught nothing: a count
        // compared against `MAX_STATIONS` moves with it, and a bound written as
        // a multiple of `SAGITTA` moves with that. What the spacing is for is
        // the picture, so this measures the picture: how far the drawn polyline
        // falls from the curve it stands for, against a finely sampled walk of
        // the same track, at an absolute number of subpixels.
        //
        // Measured rather than chosen. The spacing as written holds a moderate
        // turn to 0.022 subpixels; dropping the parallax term from `swing` —
        // the one CLAUDE.md records as having been measured and kept — cuts the
        // same exposure from fifteen legs to three and takes it to 0.447. A
        // quarter of a subpixel sits an order of magnitude clear of the first
        // and half an order below the second, which is the margin that catches
        // a regression rather than merely passing.
        //
        // Asked at a turn the ceiling does not bind, on purpose: at the yaw
        // stop `MAX_STATIONS` wins and the exposure is faceted well past what
        // the budget asks for, so the deviation there is a fact about the cap
        // rather than about the spacing. That is written up beside the constant.
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
    ///
    /// The reference curve is walked independently of the thing under test:
    /// `Track::pose_at` is sampled two hundred times over the exposure's reach
    /// and each pose is projected the way `Observer::cockpit` projects the
    /// current one, so a wrong spacing shows as distance rather than as a
    /// different count.
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
        // the world's origin rather than about the observer. Ten minutes of
        // warp put the ship three thousand light years out, and the stars the
        // `+` key added arrived in a clump directly astern — outside their own
        // reach, so the next `advance` recycled every one of them to exactly
        // the limiting magnitude, where a star is at no brightness at all. The
        // key changed the count and did not change the picture.
        //
        // Asked as isotropy rather than as a position, because that is what
        // went wrong: the mean direction from the ship to a fresh star is about
        // nothing over a sky that surrounds it, and about -1 along the track
        // over one that is all behind.
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

        // And each one is somewhere it can actually be seen from, which is the
        // same statement one level down: a star outside its own reach is one
        // the very next step throws away and puts back at exactly the limiting
        // magnitude, where it is at no brightness at all. Every one of them
        // used to be such a star.
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
            );
            let mut drawn = 0;
            sky.sweep(&cam, &eye, turn as f64, |_, _, _| drawn += 1);
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
