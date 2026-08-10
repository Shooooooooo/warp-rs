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
//!
//! All of which is the camera *abeam*, which is where this was written and is
//! no longer where it has to stay. Swing it and travel picks up a depth
//! component: a star's range changes as it flies, the trailing edge stops being
//! the only way out of the band, and anything here that turns a distance into a
//! size on the screen has to say which of a step's two ranges it means. That
//! last one is the fold's arithmetic below, and getting it wrong is visible —
//! as a lattice of streaks lying across the flow rather than running with it.
//!
//! The deeper consequence of that swing, and the one the fold cannot answer on
//! its own. A star's place in the frame is `focal · pos / z`, so a step that
//! moves its range does not translate it across the band — it *scales* it about
//! the middle of the frame. Wrapping is exactly right for a translation and
//! says nothing at all about a scale, so left to the fold the sky falls in on
//! itself: contracting, the band widens away from a pool shrinking inside it
//! and nothing refills the frame edges; expanding, a star is put back on the
//! very edge it was carried over and the next step carries it over again, so
//! the pool piles into the off-screen margin and the middle drains. Two seconds
//! of holding the camera round was enough to empty the top and bottom of the
//! frame and leave a bar of stars across the middle.
//!
//! What answers that is the *profile*, not a correction applied to it, and it is
//! the one thing to understand here. Holding a flat count per unit range against
//! a flow that magnifies costs a relocation per star per step, and a relocation
//! is a star appearing or disappearing where somebody is looking: it was doing
//! twenty of a 256-star pool a step. Advection through a frustum leaves exactly
//! one profile alone — `n ∝ z²`, an even sky seen through a cone — so the band
//! stopped being held at any particular one, and everything that leaves now comes
//! back in through a boundary the flow is *entering* by and nowhere else, which
//! is the rule [`crate::starfield`] has always used. Both ends of every move are
//! then off the screen or already faded to nothing. The near wall gets the ramp
//! the far wall has always had for the same reason: it is the one boundary that
//! can take a star out of the middle of the frame, because it is flat in `z`
//! where the band is a rectangle on the screen. All of it is switched off, not
//! reduced to an identity, wherever the range is still.

use crate::canvas::Canvas;
use crate::lens::{Image, Lens};
use crate::starfield::{shift_color, Camera, Streak, CLASSES};
use crate::view::{Orbit, HULL_REACH, MAX_SHIP_DISTANCE};
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

/// Measured against the *furthest* the zoom can put the ship, since that is the
/// case that would fail: pushing the camera back walks the hull out toward this
/// wall, and the first star to get behind it would be drawn over a ship it is
/// supposed to be behind. `MAX_SHIP_DISTANCE` is what bounds the zoom, and this
/// is what bounds `MAX_SHIP_DISTANCE`.
const _: () = assert!(
    Z_NEAR > MAX_SHIP_DISTANCE + HULL_REACH,
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

/// How much warning a star gets before the near wall takes it, as a multiple of
/// the range the flow covers in one step.
///
/// The near wall is the one boundary of the band that can take a star out of the
/// middle of the frame at full brightness. Every other seam is hidden by the
/// geometry: the far wall is where `depth` has already faded a star to nothing,
/// and a star the magnification carries out through the rim is off the screen by
/// the time it goes. The near wall is a flat plane in `z` while the band is a
/// fixed rectangle on the *screen*, so a star arriving at it can be anywhere at
/// all — including dead centre, where the flow is slowest and the eye follows it
/// best. Measured astern at full warp it was taking four stars a step out of a
/// pool of 256, at a mean brightness of 0.36 against a sky whose stars average
/// half that; at sublight it was twenty-six a second. That it went so long
/// without a ramp is because abeam `travel[2]` is a hard zero and nothing has
/// ever crossed it.
///
/// Sixteen because that is where the two ends of it stop pulling. A star's last
/// drawn frame is up to one whole step short of the wall, so what it still
/// carries when it goes is about `1 / steps` of what it would have — measured
/// astern at full warp, the brightest star taken went from 0.65 of the mean sky
/// at eight steps to 0.29 at twelve and 0.28 at sixteen, where it stops falling
/// because the ramp is no longer what is limiting it. Pushing on to
/// twenty-four bought nothing and cost a tenth of the sky's brightness, since a
/// ramp this deep is half the band at full warp.
///
/// That it is affordable at all is a consequence of the recycle: a flow leaves
/// the band at `z²`, so the range this dims holds five percent of the pool
/// rather than the third it would have held under a flat one. The cost measured
/// on the sky's mean brightness is five percent, and sixteen steps is an eighth
/// of a second of fading at any speed — a fade rather than a blink.
const NEAR_FADE_STEPS: f32 = 16.0;

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

/// Thinnest a re-entry sliver may be, as a fraction of the band's depth.
///
/// The sliver is how far the flow carries a star in one step, so at a crawl it
/// is a rounding error and at a standstill it is nothing at all —
/// `random_range` panics on an empty range, and at `Z_NEAR` an addition of
/// `f32::MIN_POSITIVE` is not even a different number. A floor a thousandth of
/// the band deep costs nothing at any speed that can be seen and cannot
/// degenerate.
const THINNEST_SLIVER: f32 = 0.001;

/// Where a star put back into the band comes in.
///
/// One rule decides all of this, and it is the rule
/// [`crate::starfield::StarField`] has always used: **a star that leaves comes
/// back in through a boundary the flow is entering by, and nowhere else.** Every
/// star that is moved has to leave somewhere and arrive somewhere, so matching
/// the two up is free — and what it buys is that both ends of the move are
/// either off the screen or already faded to nothing, which is the whole
/// difference between a sky that streams and a sky that fizzes.
///
/// The band has three boundaries and the flow's direction says which of them are
/// exits. Running *toward* the camera, ranges shrink: stars leave by the near
/// wall and, magnified past the edge of the frame, by the rim, and the only way
/// in is the far wall — [`Reentry::Wall`], where `depth` is zero anyway.
/// Running *away*, ranges grow: the only exit is the far wall and the way back
/// in is everything else, which is [`Reentry::Entering`].
///
/// What this replaced was a pair of relocations that held the pool at a fixed
/// count per unit range by moving stars about wherever they happened to be —
/// right about the density and wrong about the picture. Astern at full warp it
/// was scattering twenty of a 256-star pool back onto the screen every step out
/// of nowhere; from ahead of the beam it was taking fourteen a step *off* the
/// screen to fill the rim. Both are gone. The band is no longer held at any
/// particular count per unit range: it settles at the one profile a flow leaves
/// alone, which is `z²` — see [`ExteriorField::update`].
#[derive(Debug, Clone, Copy)]
enum Reentry {
    /// Anywhere in the band. What a fresh pool is laid out with, and what a
    /// star swung through a wall gets, having no flow to come back along.
    Anywhere,
    /// Just inside the wall the flow is coming from.
    Wall(f32, f32),
    /// Somewhere on the whole surface the flow is entering through — the near
    /// cap and the rim together — carrying the range one step covers and how
    /// far it slides the band sideways while it does.
    Entering { sliver: f32, drift: [f32; 2] },
}

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
    /// Where the camera was pointed when the pool was last stepped. Kept so
    /// that a swing can be applied as the *difference*, which is what makes it
    /// free when there is no swing.
    orbit: Orbit,
    /// And which way the ship is pointing in that camera's space, cached off it
    /// because [`Self::streaks`] takes `&self` and the Doppler is measured
    /// against the direction of travel rather than against the frame.
    nose: [f32; 3],
    /// How far into the band the near wall's warning reaches, in range, and
    /// exactly zero when the near wall is not a boundary anything crosses.
    ///
    /// Kept here because [`Self::streaks`] takes `&self` and has no travel to
    /// read, and scaled by the travel rather than fixed for two reasons. A fixed
    /// width gated on a flag would dim a tenth of the pool in the single frame
    /// the camera stopped being abeam, where this goes to zero smoothly as the
    /// flow flattens. And a star then gets the same number of *frames* of warning
    /// whatever the ship is doing, which is what the ramp is for.
    near_fade: f32,
    /// Scratch for bending a streak, reused across every star of every frame.
    source: Vec<(f32, f32)>,
    bent: Vec<(f32, f32)>,
}

impl ExteriorField {
    /// The orbit is where the camera already is, not where it starts from.
    /// The band is laid out in the camera's own frustum and so does not depend
    /// on it at all — but the first step would otherwise read a swing that
    /// never happened and turn the whole pool through it.
    pub fn new(count: usize, seed: u64, cam: &Camera, orbit: Orbit) -> Self {
        let mut field = Self {
            stars: Vec::with_capacity(count),
            rng: StdRng::seed_from_u64(seed),
            bound: bound_for(cam),
            focal: cam.focal.max(f32::MIN_POSITIVE),
            orbit: orbit.held(),
            nose: orbit.held().nose_in_camera(),
            near_fade: 0.0,
            source: Vec::with_capacity(MAX_ARCS + 1),
            bent: Vec::with_capacity(MAX_ARCS + 1),
        };
        for _ in 0..count {
            let star = field.spawn(Reentry::Anywhere);
            field.stars.push(star);
        }
        field
    }

    pub fn len(&self) -> usize {
        self.stars.len()
    }

    /// True only when the count asked for was zero, which `--stars 0` is
    /// allowed to ask for. The same asymmetry [`crate::starfield::StarField`]
    /// has and for the same reason: [`Self::new`] builds exactly what it is
    /// handed, where [`Self::resize_pool`] keeps a star back, so a sky can be
    /// started empty and cannot be emptied.
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
        let (half_width, half_height, focal) = (self.bound.0, self.bound.1, self.focal);
        for star in &mut self.stars {
            star.pos[0] = fold(star.pos[0], band(half_width, focal, star.pos[2])).0;
            star.pos[1] = fold(star.pos[1], band(half_height, focal, star.pos[2])).0;
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
            let star = self.spawn(Reentry::Anywhere);
            self.stars.push(star);
        }
    }

    /// Make a star. As in the cockpit field the position is sampled on the
    /// screen and back-projected through the range, which keeps apparent
    /// density even at every distance: sampling a world-space volume instead
    /// would crowd the far wall and starve the near one.
    fn spawn(&mut self, reentry: Reentry) -> Star {
        let (hw, hh) = self.bound;
        let (z, sx, sy) = match reentry {
            Reentry::Anywhere => self.across_the_band(Z_NEAR, Z_FAR, hw, hh),
            Reentry::Wall(lo, hi) => self.across_the_band(lo, hi, hw, hh),
            Reentry::Entering { sliver, drift } => {
                self.on_the_entering_surface(sliver, drift, hw, hh)
            }
        };

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

    /// A range from `lo..hi` and a place uniform across the band at it, in
    /// screen units.
    ///
    /// The draw order is load-bearing rather than incidental: `--seed`
    /// reproducibility is a property of the sequence, and this is what lays out
    /// the pool every side-view flight opens with. Range, then across, then
    /// down, and the caller draws the magnitude, the class and the phase after.
    fn across_the_band(&mut self, lo: f32, hi: f32, hw: f32, hh: f32) -> (f32, f32, f32) {
        let z = self.rng.random_range(lo..hi);
        (
            z,
            self.rng.random_range(-hw..hw),
            self.rng.random_range(-hh..hh),
        )
    }

    /// Somewhere on the surface the flow is entering through when ranges are
    /// growing, drawn uniformly by the area of it.
    ///
    /// That surface is two things at once and the arithmetic joins them. A step
    /// of `sliver` contracts the band about the middle of the frame by
    /// `(z - sliver) / z`, so at range `z` it vacates everything outside that
    /// fraction — a rim wanting `z² - (z - sliver)²` stars — while the near cap,
    /// which the flow has simply moved off, wants `Z_NEAR²` of them. Summed over
    /// the band those come to exactly `Z_FAR²`, which is why one draw across
    /// that picks the boundary and the range together, and why the far wall's
    /// outflow of `Z_FAR²` a step is precisely the supply. The two sides meeting
    /// is the whole reason nothing has to be counted.
    ///
    /// The cap is a five-hundredth of the surface and could not be dropped for
    /// it: it is the only way back into the ranges nearest the camera, and
    /// without it that end of the band would drain and stay drained.
    ///
    /// `drift` is what makes this more than a rim. A step does not only shrink
    /// the band about the middle of the frame, it slides it sideways as well,
    /// and the gap the two leave together is the rim *of the slid band* — an
    /// even frame about the place the flow has moved the middle to, not about
    /// the middle. Off by a whole rim width or more whenever there is any
    /// sideways flow at all, since the ratio of the two is `travel[0] / travel[2]`
    /// scaled by the band's own shape and does not fall off with range: at 45
    /// degrees off the beam the gap misses the frame it was being filled into
    /// entirely, which put a third of the pool in the margin and left the frame
    /// 2.2 to 1 across, against 1.3 abeam.
    fn on_the_entering_surface(
        &mut self,
        sliver: f32,
        drift: [f32; 2],
        hw: f32,
        hh: f32,
    ) -> (f32, f32, f32) {
        let across: f32 = self.rng.random_range(0.0..Z_FAR * Z_FAR);
        if across < Z_NEAR * Z_NEAR {
            return self.across_the_band(Z_NEAR, Z_NEAR + sliver, hw, hh);
        }
        // The square root is the draw: a rim whose share of the surface grows
        // as `2z` is a range whose distribution is the root of a uniform one.
        let z = across.sqrt();
        let lambda = ((z - sliver) / z).clamp(0.0, 1.0);
        let (sx, sy) = self.on_the_rim(hw, hh, lambda);
        // The band wraps, so the rim about a moved centre is the moved rim, and
        // the fold is all it takes to say so.
        let slide = self.focal / z;
        (
            z,
            fold(sx + drift[0] * slide, hw).0,
            fold(sy + drift[1] * slide, hh).0,
        )
    }

    /// Somewhere uniform *by area* on the rim a step of contraction has just
    /// vacated: the frame between `lambda` of the band and the band itself.
    ///
    /// Split into the two kinds of slab and picked by the share of the rim each
    /// pair covers rather than by rejection, so the cost is three draws and no
    /// loop however thin the rim gets. The left and right slabs are the band's
    /// full height and so hold `1 / (1 + lambda)` of it between them.
    fn on_the_rim(&mut self, bx: f32, by: f32, lambda: f32) -> (f32, f32) {
        let pick: f32 = self.rng.random_range(0.0..1.0);
        let along: f32 = self.rng.random_range(-1.0..1.0);
        let out: f32 = self.rng.random_range(-1.0..1.0);
        let edge = (lambda + (1.0 - lambda) * out.abs()) * out.signum();
        if pick * (1.0 + lambda) < 1.0 {
            (bx * edge, by * along)
        } else {
            (bx * lambda * along, by * edge)
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

    /// Fly the band past the camera, from wherever the camera is watching.
    ///
    /// The ship's steering is deliberately not an argument. The camera rides
    /// with the ship rather than being bolted to the sky, so a turn swings the
    /// *hull* in frame and leaves the stars streaming the way they were — the
    /// view from a wingman's canopy, where the horizon does not tip because
    /// your neighbour rolled.
    ///
    /// The camera's own attitude is an argument, and is the exception that
    /// proves it: swinging the *eye* round the ship has to take the sky with
    /// it, because that is the only thing that distinguishes it from the ship
    /// turning. Lifting the camera over the hull at zero azimuth is the case
    /// that makes this sharp — the flow direction does not change at all, so a
    /// pool left alone would sit perfectly still while the ship rotated in
    /// front of it, which is precisely what a barrel roll looks like.
    ///
    /// Two things happen here that did not before, and both are switched off
    /// entirely at [`Orbit::LEVEL`] rather than reduced to an identity: the pool
    /// is turned by however much the camera turned since the last step, and
    /// travel is a vector rather than a number. At the level shot that vector
    /// is exactly `(-1, 0, 0)`, so the range still never changes, the fold below
    /// is still the only way out of the band, and the arithmetic is still what
    /// the reference frames were recorded from.
    pub fn update(&mut self, dt: f32, speed: f32, cam: &Camera, orbit: Orbit) {
        let step = speed * dt;
        let orbit = orbit.held();
        let travel = orbit.sky_travel();
        let focal = self.focal;
        let (half_width, half_height) = self.bound;
        let (bank_sin, bank_cos) = cam.bank.sin_cos();

        // How much the camera has turned since the pool was last laid against
        // it, as a rotation in the camera's own space. Compared rather than
        // composed to an identity: the angles are unchanged bit for bit when
        // nothing has moved them, so this is `None` on every frame of a flight
        // nobody is swinging the camera on.
        let swing = (orbit != self.orbit).then(|| {
            let turn = between(&orbit.basis(), &self.orbit.basis());
            self.orbit = orbit;
            self.nose = orbit.nose_in_camera();
            turn
        });
        // Depth and height only move when something moves them, and at the
        // level shot nothing does. Held as flags rather than tested per star so
        // the two blocks below cost nothing at all rather than costing a
        // comparison.
        //
        // Height goes with depth and not only with its own travel, which is
        // not obvious and was a real bug: the band is a fixed size on the
        // *screen*, so its half-height in world units follows the range. A star
        // whose `y` never moved is outside the band the moment it is carried
        // nearer the camera, and `x` only escapes noticing because it is folded
        // every frame regardless.
        let turns_z = swing.is_some() || travel[2] != 0.0;
        let turns_y = turns_z || travel[1] != 0.0;

        // How much warning the near wall gives, in range. Exactly zero abeam,
        // where `travel[2]` is a hard zero, and exactly zero at rest — switched
        // off rather than reduced to an identity, so the shot the reference
        // frames are recorded from goes on being drawn from the bytes it always
        // was.
        self.near_fade = NEAR_FADE_STEPS * (travel[2] * step).abs();

        // Where a star that has left comes back in. A step that moves no range
        // cannot put one out in the first place, so the `Anywhere` arm is only
        // ever reached by a swing that turned one out — which has no direction
        // of flow to come back along.
        let reentry = if travel[2] > 0.0 {
            // Ranges growing. The far wall is the only exit and everything else
            // is the way in, so a leaver goes back onto the surface the flow is
            // entering through — mostly the rim the contraction has vacated,
            // once in five hundred the near cap.
            Reentry::Entering {
                sliver: (travel[2] * step).max((Z_FAR - Z_NEAR) * THINNEST_SLIVER),
                drift: [travel[0] * step, travel[1] * step],
            }
        } else if travel[2] < 0.0 {
            // Ranges shrinking. The far wall is the only way in, and it is the
            // one place a star can be put back without anyone seeing it arrive.
            let sliver = (-travel[2] * step).max((Z_FAR - Z_NEAR) * THINNEST_SLIVER);
            Reentry::Wall(Z_FAR - sliver, Z_FAR)
        } else {
            Reentry::Anywhere
        };

        let mut stars = std::mem::take(&mut self.stars);
        for star in &mut stars {
            star.prev = cam.project(star.pos);
            // The range `prev` was projected through, kept because the fold
            // below has to undo that projection and cannot use the range the
            // star ends the step at. Taken here rather than three lines down so
            // it is the depth of the point `prev` actually is, swing included.
            //
            // The whole position is kept, not only that range, and the other
            // two components are wanted for a different question: whether the
            // step's *scale* carried the star out of the band, which has to be
            // asked of where it started because a swing translates it as well.
            // Moving this line breaks both at once.
            let before = star.pos;
            let z_prev = before[2];

            // The trail is deliberately left where it was. A camera that is
            // being swung really does smear what it sweeps past, and at the
            // rate the orbit eases that smear is a fraction of a subpixel —
            // rewinding it would cost a projection per star to hide something
            // nobody can see and that ought to be there anyway.
            if let Some(turn) = &swing {
                star.pos = turned(turn, star.pos);
            }

            if travel[2] != 0.0 {
                star.pos[2] += travel[2] * step;
            }
            let z = star.pos[2];

            // Everything that has left the band, by whichever boundary. Two
            // quite different departures, and they are one case because they
            // have one answer.
            //
            // A wall crossing is the plain one. The other is the frame itself:
            // a star's place in it is `focal * pos / z`, so a step that shortens
            // the range magnifies the whole band about the middle of the picture
            // and carries the outermost of it past the edge. Asked of where the
            // star *started* rather than where it now is, which is the same
            // question while only the range is moving and a different one under
            // a swing — a star panned over the edge has not been magnified past
            // anything and belongs to the fold below.
            //
            // Measured against the band at the range the star *came from*, and
            // that is a step's grace rather than an off-by-one. One step of the
            // magnification can be worth more than the margin the band carries —
            // at full warp `z_prev / z` reaches 1.5 against a margin of 1.15 —
            // so a star can go from inside the frame to outside the band without
            // ever being drawn in between, and taking it there is the reported
            // fault in miniature: it vanishes a couple of cells short of the
            // edge instead of sweeping through it. Left alone for the one frame
            // it spends outside, it draws the streak that carries it off the
            // edge, the canvas clips it, and it is taken next step from a place
            // nobody can see. That is why the fold below has to let it past too.
            //
            // What used to happen here instead was a relocation, back across the
            // band at the range the star had. That is the density restored
            // exactly and a star conjured into the middle of the picture: twenty
            // of a 256-star pool a step, astern at full warp. The band is not
            // held at a fixed count per unit range any more. Left alone, a flow
            // drives it to the only profile it does leave alone — `z²`, the
            // profile of an even sky seen through a frustum, which is where the
            // cockpit's has always sat because the cockpit has always recycled
            // this way.
            if turns_z
                && (!(Z_NEAR..Z_FAR).contains(&z)
                    || before[0].abs() >= band(half_width, focal, z_prev)
                    || before[1].abs() >= band(half_height, focal, z_prev))
            {
                *star = self.spawn(reentry);
                // Given the trail it would have had. Without this a recycled
                // star draws a bare point on its first frame, and off the beam
                // at full warp two to four percent of the pool recycles every
                // frame — which is the sky flickering between streaks and dots
                // that the fold above exists to avoid, arriving by the other
                // door. One step back along the track is exactly where it came
                // from, and is always well clear of the projection's near plane.
                let was = [
                    star.pos[0] - travel[0] * step,
                    star.pos[1] - travel[1] * step,
                    star.pos[2] - travel[2] * step,
                ];
                star.prev = cam.project(was);
                continue;
            }

            // The star the magnification has just carried past the edge, on the
            // one frame it is allowed before the recycle above takes it. It is
            // put down exactly where the scale left it and nothing else is done
            // to it: no travel, no fold.
            //
            // Neither half of that is fussiness. Folding it would wrap it in at
            // the opposite edge, where the flow runs the wrong way to bring it
            // home and the next step carries it straight out again — the sky
            // piling into the margin, which is the failure this band already
            // knows by name. And carrying it the sideways part of the step would
            // leave the recycle above unable to be sure of it: the travel can
            // pull it back inside the very band it just left, so it lingers at
            // the edge for another step, and another. Measured at 45 degrees off
            // the beam that alone took the share of the pool on screen from 0.76
            // to 0.60. Held still, the test above is exact — the band at the
            // range it came from is the band it went out through — and every
            // star costs one frame and no more.
            let half = band(half_width, focal, z);
            let leaving = turns_z
                && (before[0].abs() >= half || before[1].abs() >= band(half_height, focal, z));
            if leaving {
                continue;
            }

            // Range never changes when the camera is abeam — travel is along
            // the track — so the only way out of the band is off the trailing
            // edge, and the honest place for a star the ship has just overtaken
            // is back out in front at the range it already had.
            let (folded, shift) = fold(star.pos[0] + travel[0] * step, half);
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
                    let d = shift * focal / z_prev;
                    p.0 += d * bank_cos;
                    p.1 += d * bank_sin;
                }
            }

            if turns_y {
                star.pos[1] += travel[1] * step;
                // Folded only when it has actually gone over the edge, where
                // `x` above is folded every frame regardless. The asymmetry is
                // on purpose and is not a tidying opportunity: `fold` is not an
                // exact identity for a value already inside its band, so
                // folding a `y` that never moves would round it, and the shot
                // the reference frames are recorded from is exactly the shot
                // where it never moves.
                let half = band(half_height, focal, z);
                if star.pos[1] < -half || star.pos[1] >= half {
                    let (folded, shift) = fold(star.pos[1], half);
                    star.pos[1] = folded;
                    if shift != 0.0 {
                        if let Some(p) = &mut star.prev {
                            let d = shift * focal / z_prev;
                            p.0 -= d * bank_sin;
                            p.1 += d * bank_cos;
                        }
                    }
                }
            }
        }
        self.stars = stars;
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
        // Read out here rather than off `self` per star: it is fixed for the
        // frame, and this is the innermost loop the band has.
        let near_fade = self.near_fade;

        // Where a trail run backward along the track ends up, if it ends up
        // anywhere at all — see [`trail_head`] for what that is for. `None`
        // covers three cases and every one of them wants the plain stretch
        // below: a sublight sky, where `stretch` is exactly one and there is
        // nothing to overreach; the camera abeam, where the nose lies flat in
        // the image plane and `vanishing_point` refuses a direction that does
        // not recede; and the camera anywhere forward of the beam, where a
        // trail runs *toward* the eye and its projection diverges rather than
        // converging on anything. Switched off in all three rather than reduced
        // to an identity, which is what keeps every frame recorded at one of
        // those angles the bytes it has always been.
        let vanish = (stretch > 1.0)
            .then(|| cam.vanishing_point(self.nose))
            .flatten();

        self.stars.iter().filter_map(move |star| {
            let to = cam.project(star.pos)?;
            // Stretch backward along the direction of travel, exactly as the
            // cockpit does: a star close to the camera sweeps further per frame
            // and so draws the longer streak, with no special casing.
            let from = match star.prev {
                Some(prev) => match vanish {
                    Some(point) => trail_head(to, prev, point, stretch),
                    None => (
                        to.0 + (prev.0 - to.0) * stretch,
                        to.1 + (prev.1 - to.1) * stretch,
                    ),
                },
                None => to,
            };

            let class = &CLASSES[star.class];
            let intensity = lit(star, near_fade, twinkle_amt, twinkle_phase);
            if intensity <= 0.0 {
                return None;
            }

            // Doppler about the direction of travel rather than about the
            // vanishing point: from out here the sky the ship is running into
            // blues, and the sky it is leaving behind reddens. Dead abeam — the
            // middle of the frame — is neither, which is the half-way point of
            // the ramp `shift_color` already takes.
            //
            // Against the *nose* rather than against camera `+x`, which is only
            // the direction of travel while the camera is abeam. Swing it round
            // and the sky the ship is running into is no longer to screen
            // right; measured against the frame, a chase view would redden the
            // sky ahead and blue the wake. Exact where the two agree: the nose
            // is `(1, 0, 0)` abeam, so this is `pos[0]` to the bit.
            let length = (star.pos[0].powi(2) + star.pos[1].powi(2) + star.pos[2].powi(2)).sqrt();
            let ahead = star.pos[0] * self.nose[0]
                + star.pos[1] * self.nose[1]
                + star.pos[2] * self.nose[2];
            let forward = if length > f32::EPSILON {
                (0.5 + 0.5 * ahead / length).clamp(0.0, 1.0)
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
            subdivide(&streak, lens, &mut source);
            for image in [Image::Primary, Image::Secondary] {
                // The head is where the star actually is, so its magnification
                // is the one that speaks for the whole streak — and its
                // position is what says whether there is anything to draw at
                // all. Asking the bubble *first* is most of what makes this
                // affordable: the counter-image of everything beyond about
                // two-thirds of an Einstein radius lands inside the shadow, so
                // without this check the great majority of the pool is
                // subdivided, mapped and then thrown away.
                let head = lens.map(streak.to, image);
                if head.gain < FAINTEST_COUNTER_IMAGE || lens.shadowed(head.at) {
                    continue;
                }
                let gain = head.gain;
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

/// Where a star's trail begins, when the track it is stretched back along
/// recedes and so has a point it vanishes at.
///
/// The bug this is here for, and it is [`crate::models::draw_trail`]'s, arrived
/// at from the other end. A trail is stretched in *screen* space — the one step
/// the star actually swept, multiplied by `stretch` — and a straight ray running
/// away from the eye does not project onto a screen-space ray. It projects onto
/// the segment between the star and the point its direction vanishes at, which
/// it *approaches* and never reaches, so past that point there is no trail left
/// to draw. Multiplied out regardless, the nearest stars in the band ran
/// clean through it and out the far side, and every star at the one depth where
/// the stretch comes to exactly the whole distance laid its trailing end
/// precisely on it — a spike sitting inside a spread, which between them make a
/// cusp. Swing the camera aft of the beam so that point is on the canvas and
/// the cusp is a hot spot: at `--orbit -75,12,0` and full warp, within two
/// subpixels of it the mean was 2.3 times the surrounding sky and every
/// subpixel of it was lit, so the sky lost the black between its streaks and
/// stopped reading as distance.
///
/// The arithmetic is one division and needs no depth, exactly as the lance's
/// does. `along / span` is the fraction of the way to the point that one step
/// covered, taken as a dot product against the run to the point and divided by
/// the square of it; `k` steps then cover `k·u / (1 + (k-1)·u)`, which is under
/// one for every `u` under one — so a trail can reach for the point and never
/// touch it, whatever the stretch. Both guards are the arithmetic's backstop
/// rather than cases that arise: the star lies between its own past and that
/// point, so `along` is positive and short of `span` whenever the projection
/// gave an honest answer.
fn trail_head(to: (f32, f32), prev: (f32, f32), point: (f32, f32), stretch: f32) -> (f32, f32) {
    let (ax, ay) = (point.0 - to.0, point.1 - to.1);
    let span = ax * ax + ay * ay;
    let along = (prev.0 - to.0) * ax + (prev.1 - to.1) * ay;
    if !(0.0..span).contains(&along) {
        return (
            to.0 + (prev.0 - to.0) * stretch,
            to.1 + (prev.1 - to.1) * stretch,
        );
    }
    let frac = stretch * along / (span + (stretch - 1.0) * along);
    (to.0 + ax * frac, to.1 + ay * frac)
}

/// How brightly a star burns this frame, before the lens touches it and before
/// [`Canvas::draw_streak`](crate::canvas::Canvas::draw_streak) divides its own
/// length back out.
///
/// Its own function rather than four lines inside [`ExteriorField::streaks`],
/// because what a test most wants to ask of this band is how bright a star was
/// on the last frame before the band took it away — and once it has been taken
/// there is no `Streak` left to ask. A copy of the arithmetic in the test module
/// would answer a question about the copy.
fn lit(star: &Star, near_fade: f32, twinkle_amt: f32, twinkle_phase: f32) -> f32 {
    let z = star.pos[2];
    // Reaches zero exactly at the far wall, with zero slope, so stars fade up
    // out of nothing instead of blinking into existence.
    let depth = (1.0 - (z - Z_NEAR) / (Z_FAR - Z_NEAR)).clamp(0.0, 1.0);
    // And the same courtesy at the near wall, which needs it far more: the far
    // wall is where a star is faint anyway, where this one is where it is
    // brightest and can be anywhere at all on the screen. Folded in *before*
    // the exponent below rather than applied after it, which costs nothing and
    // is what gives the ramp its zero slope at the wall, exactly as the far one
    // has. Branched rather than written as a width that runs off to infinity,
    // because a flat flow gives a width of exactly zero and the division would
    // hand the whole sky a NaN — and because a wall nothing crosses wants no
    // ramp at all.
    let depth = if near_fade > 0.0 {
        depth * ((z - Z_NEAR) / near_fade).clamp(0.0, 1.0)
    } else {
        depth
    };
    // Exactly as in [`crate::starfield`], and for the same reason: the amount
    // is a hard zero at warp, and `1 + 0·s` is one to the bit.
    let twinkle = if twinkle_amt > 0.0 {
        1.0 + twinkle_amt * (twinkle_phase + star.phase).sin()
    } else {
        1.0
    };
    CLASSES[star.class].luminosity * star.magnitude * depth.powf(DEPTH_FALLOFF) * twinkle
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
///
/// How finely depends on how hard the lens is bending things where the streak
/// is, not merely on how long it is. Out at the edge of the frame the whole
/// streak is displaced by much the same amount, which is a shift rather than a
/// curve and needs no subdivision at all — and the edge of the frame is where
/// nearly all of the sky is.
fn subdivide(streak: &Streak, lens: &Lens, out: &mut Vec<(f32, f32)>) {
    out.clear();
    let (dx, dy) = (streak.to.0 - streak.from.0, streak.to.1 - streak.from.1);
    let length = crate::canvas::length_of(dx, dy);
    // The head, where the star actually is, speaks for the streak.
    let bend = lens.curvature(streak.to).max(lens.curvature(streak.from));
    let pieces = if length.is_finite() {
        ((length * bend / ARC_STEP).ceil() as usize).clamp(1, MAX_ARCS)
    } else {
        1
    };
    let inv = 1.0 / pieces as f32;
    for i in 0..=pieces {
        let t = i as f32 * inv;
        out.push((streak.from.0 + dx * t, streak.from.1 + dy * t));
    }
}

/// The rotation that carries the camera's old frame onto its new one, in the
/// camera's own space: `M_new · M_oldᵀ`.
///
/// Both bases map the hull's frame onto a camera's, so composing one with the
/// other's inverse — its transpose, these being rotations — leaves something
/// that acts on positions already in camera space, which is where every star
/// lives.
fn between(new: &[[f32; 3]; 3], old: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut turn = [[0.0; 3]; 3];
    for (i, row) in turn.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = new[i][0] * old[j][0] + new[i][1] * old[j][1] + new[i][2] * old[j][2];
        }
    }
    turn
}

/// A camera-space position, put through one of those.
fn turned(turn: &[[f32; 3]; 3], p: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (axis, row) in out.iter_mut().zip(turn) {
        *axis = p[0] * row[0] + p[1] * row[1] + p[2] * row[2];
    }
    out
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

    /// The nose as [`crate::view::Orbit::LEVEL`] hands it over: the camera
    /// abeam of the track, which is the shot every one of these was written
    /// against.
    const ABEAM: [f32; 3] = [1.0, 0.0, 0.0];
    use crate::canvas::Tonemap;
    use crate::render::Renderer;
    use crate::ship::Ship;
    use crate::term::ColorMode;
    use crate::view::{ship_half_on_screen, ZOOM_DEFAULT};

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
            let mut field = ExteriorField::new(300, 3, &cam, Orbit::LEVEL);
            for _ in 0..600 {
                let ranges: Vec<f32> = field.stars.iter().map(|s| s.pos[2]).collect();
                field.update(1.0 / 120.0, speed, &cam, Orbit::LEVEL);
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
        let mut field = ExteriorField::new(1, 5, &cam, Orbit::LEVEL);
        // Put it a hair inside the trailing edge, so one step folds it.
        let z = Z_NEAR * 1.2;
        let band = band(field.bound.0, field.focal, z);
        field.stars[0].pos = [-band * 0.98, 0.0, z];
        field.update(1.0 / 120.0, 400.0, &cam, Orbit::LEVEL);

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
    fn swinging_the_camera_sweeps_the_sky_past_it() {
        // The regression test for the bug this whole design is arranged
        // around, and the one that is easiest to ship without noticing.
        //
        // Lifting the camera over the hull at zero azimuth does not change the
        // direction the sky flows in at all — the rotation is about the flow
        // axis itself. So a field that was only told the *travel* direction
        // would sit perfectly still while the ship rotated in front of it,
        // which is exactly what a barrel roll looks like, and the new control
        // would be indistinguishable from the one it replaced. The pool has to
        // turn as well.
        let cam = Camera::new(120, 72);
        let level = |field: &ExteriorField| -> Vec<(f32, f32)> {
            field
                .stars
                .iter()
                .filter_map(|s| cam.project(s.pos))
                .collect()
        };

        let mut still = ExteriorField::new(400, 11, &cam, Orbit::LEVEL);
        let mut swung = ExteriorField::new(400, 11, &cam, Orbit::LEVEL);
        let lifted = Orbit {
            azimuth: 0.0,
            elevation: 0.4,
            roll: 0.0,
        };
        assert_eq!(
            Orbit::LEVEL.sky_travel(),
            lifted.sky_travel(),
            "this test is only worth anything while the flow direction is the same"
        );
        for _ in 0..8 {
            still.update(1.0 / 120.0, 60.0, &cam, Orbit::LEVEL);
            swung.update(1.0 / 120.0, 60.0, &cam, lifted);
        }
        let (a, b) = (level(&still), level(&swung));
        let moved = a
            .iter()
            .zip(&b)
            .filter(|(p, q)| (p.1 - q.1).abs() > 1.0)
            .count();
        assert!(
            moved > a.len() / 2,
            "lifting the camera moved only {moved} of {} stars — the sky is not \
             coming with it, and the control is a barrel roll wearing new keys",
            a.len()
        );

        // And the other way: with the camera held where it is, the sky is
        // bitwise where it always was. This is the gate the reference frames
        // depend on, asked of the pool rather than of a hash.
        let mut held = ExteriorField::new(400, 11, &cam, Orbit::LEVEL);
        let mut untouched = ExteriorField::new(400, 11, &cam, Orbit::LEVEL);
        for _ in 0..8 {
            held.update(1.0 / 120.0, 60.0, &cam, Orbit::LEVEL);
            untouched.update(1.0 / 120.0, 60.0, &cam, Orbit::LEVEL);
        }
        for (p, q) in held.stars.iter().zip(&untouched.stars) {
            assert_eq!(p.pos, q.pos, "a level step is not deterministic");
            assert_eq!(p.pos[1], q.pos[1], "the level fold moved a star sideways");
        }
    }

    #[test]
    fn the_band_holds_together_from_every_angle() {
        // Off the beam the sky gains depth, which is the one thing this module
        // was written never to have to handle: stars cross the near and far
        // walls and have to be put back. The range bound is the one that has to
        // hold absolutely, because it is the whole of the depth sorting the hull
        // relies on.
        //
        // Sideways the bound is deliberately weaker than it looks, and the
        // weaker form is the sharper statement. A star the magnification has
        // carried past the edge is left outside for the one frame that carries
        // it off the screen, so "inside the band" is not true of every star on
        // every step — but "outside the band means outside the picture" is, and
        // that is what the band's margin is *for*. Asserting the strong form
        // instead is what made a star vanish a couple of cells short of the edge
        // rather than sweeping through it.
        //
        // Containment only. A pool collapsed onto the vanishing point passes
        // this trivially, which is how the sky came to fall in on itself off
        // the beam with every test in the module still green;
        // `the_sky_is_as_even_off_the_beam_as_it_is_abeam` is the half that
        // asks where in the band the stars actually are.
        let cam = Camera::new(120, 72);
        for (az, el, roll) in [
            (1.2f32, 0.0f32, 0.0f32),
            (-1.2, 0.0, 0.0),
            (std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            (2.6, 0.9, 1.4),
            (-0.8, -1.3, -2.2),
            // Over the top, where the vertical fold and the wall recycle have
            // never been: an elevation past the quarter turn was unreachable
            // until the clamp came off.
            (0.0, 2.4, 0.0),
            (0.7, -2.9, 1.1),
        ] {
            let orbit = Orbit {
                azimuth: az,
                elevation: el,
                roll,
            }
            .held();
            let mut field = ExteriorField::new(600, 13, &cam, Orbit::LEVEL);
            for step in 0..400 {
                // Swung in on the first step and then held, so both the pool
                // rotation and the steady flow are exercised.
                field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, orbit);
                for star in &field.stars {
                    assert!(
                        star.pos.iter().all(|v| v.is_finite()),
                        "a star went to pieces at {orbit:?} on step {step}: {:?}",
                        star.pos
                    );
                    assert!(
                        (Z_NEAR..Z_FAR).contains(&star.pos[2]),
                        "a star left the band at {orbit:?} on step {step}: {:?}",
                        star.pos
                    );
                    let (bx, by) = (
                        band(field.bound.0, field.focal, star.pos[2]),
                        band(field.bound.1, field.focal, star.pos[2]),
                    );
                    if star.pos[0].abs() <= bx && star.pos[1].abs() <= by {
                        continue;
                    }
                    // Outside the band, which is allowed for exactly as long as
                    // it takes to leave — and only ever outside the picture.
                    let (x, y) = cam
                        .project(star.pos)
                        .expect("a star in the band is in front of the camera");
                    assert!(
                        x < 0.0 || x >= cam.width || y < 0.0 || y >= cam.height,
                        "a star outside the band at {orbit:?} on step {step} is \
                         still in the picture, at {x},{y}: {:?}",
                        star.pos
                    );
                }
            }
        }
    }

    #[test]
    fn a_star_put_back_through_a_wall_still_draws_a_streak() {
        // The depth recycle is the one that cannot carry its trail across, so
        // it is handed the trail it would have had. Without that, off the beam
        // at full warp a few percent of the pool draws a bare point every
        // frame — the sky flickering between streaks and dots that the fold
        // exists to avoid, arriving by the other door.
        let cam = Camera::new(120, 72);
        let ahead = Orbit {
            azimuth: 1.4,
            elevation: 0.0,
            roll: 0.0,
        };
        let mut field = ExteriorField::new(3000, 17, &cam, ahead);
        let mut points = 0usize;
        let mut drawn = 0usize;
        for _ in 0..120 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, ahead);
            for star in &field.stars {
                if cam.project(star.pos).is_some() {
                    drawn += 1;
                    if star.prev.is_none() {
                        points += 1;
                    }
                }
            }
        }
        assert!(drawn > 0, "nothing was on screen to check");
        assert_eq!(
            points, 0,
            "{points} of {drawn} stars came back without the streak they swept"
        );
    }

    #[test]
    fn a_trail_carried_over_the_fold_is_scaled_by_the_range_it_was_drawn_at() {
        // The bug this is here for: the fold turns a world-space jump into a
        // screen-space one by dividing by the star's range, and it was dividing
        // by the range the star *ends* the step at, where `prev` had been
        // projected through the range it *started* it at. Abeam that is not a
        // bug at all — travel is exactly (-1, 0, 0), so the two ranges are the
        // same bit for bit — which is why every reference frame but the orbited
        // one was blind to it, and why the shot on the README's front page had
        // a lattice of streaks lying across the flow instead of running with
        // it. Off the beam the trail landed short or long by
        // `shift · focal · (1/z_new − 1/z_old)`, along the camera's own x for
        // the sideways fold and y for the vertical one, and `streaks` then
        // multiplied the whole thing by six at full warp.
        //
        // The invariant is stronger than the fold and covers the whole pool: a
        // star's trail is where it was one step back along the track, whether
        // it folded, crossed a wall, or simply flew. Measured on `prev` rather
        // than on a `Streak` so it is one per star in order, and at the ranges
        // and angles that actually differ.
        //
        // With one exemption, and it is named rather than fudged. A star the
        // magnification has carried out of the band is held still sideways for
        // the single frame that carries it off the screen, so for that frame it
        // has *not* travelled the whole step and its trail says so — honestly,
        // since `prev` is where it really was. It is off the picture by then, and
        // the containment test above is what says so.
        let cam = cam();
        let orbits = [
            Orbit::LEVEL,
            Orbit {
                azimuth: 0.96,
                elevation: 0.61,
                roll: 0.35,
            },
            Orbit {
                azimuth: 4.28,
                elevation: 0.52,
                roll: 0.0,
            },
            Orbit {
                azimuth: 1.4,
                elevation: -0.9,
                roll: 2.7,
            },
        ];
        let mut checked = 0usize;
        for orbit in orbits {
            let travel = orbit.sky_travel();
            for speed in [40.0, crate::ship::WARP_MAX] {
                let step = speed / 120.0;
                let mut field = ExteriorField::new(500, 23, &cam, orbit);
                for _ in 0..90 {
                    field.update(1.0 / 120.0, speed, &cam, orbit);
                    for (i, star) in field.stars.iter().enumerate() {
                        let Some(prev) = star.prev else { continue };
                        // On its way out, and exempt — see above.
                        let z = star.pos[2];
                        if star.pos[0].abs() >= band(field.bound.0, field.focal, z)
                            || star.pos[1].abs() >= band(field.bound.1, field.focal, z)
                        {
                            continue;
                        }
                        let was = [
                            star.pos[0] - travel[0] * step,
                            star.pos[1] - travel[1] * step,
                            star.pos[2] - travel[2] * step,
                        ];
                        let Some(want) = cam.project(was) else {
                            continue;
                        };
                        checked += 1;
                        let off = crate::canvas::length_of(prev.0 - want.0, prev.1 - want.1);
                        assert!(
                            off < 0.05,
                            "star {i} trails from {prev:?} where it swept from \
                             {want:?}, {off} subpixels out"
                        );
                    }
                }
            }
        }
        assert!(checked > 100_000, "only {checked} trails were looked at");
    }

    #[test]
    fn nothing_ever_comes_between_the_camera_and_the_ship() {
        // The whole of the exterior renderer's depth sorting, asserted rather
        // than assumed: the hull is drawn over the sky, so no star may be in
        // front of it — at any zoom, so measured against the furthest back the
        // camera can be pushed.
        let cam = cam();
        let mut field = ExteriorField::new(2000, 7, &cam, Orbit::LEVEL);
        for _ in 0..600 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, Orbit::LEVEL);
        }
        for star in &field.stars {
            assert!(
                star.pos[2] > MAX_SHIP_DISTANCE + HULL_REACH,
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
        let mut field = ExteriorField::new(2, 5, &cam, Orbit::LEVEL);
        field.stars[0].pos = [0.0, 0.0, Z_NEAR * 1.1];
        field.stars[1].pos = [0.0, 0.0, Z_FAR * 0.9];
        let before = [screen_x(&field, &cam, 0), screen_x(&field, &cam, 1)];
        field.update(1.0 / 120.0, 60.0, &cam, Orbit::LEVEL);
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
        let mut field = ExteriorField::new(2000, 21, &cam, Orbit::LEVEL);
        for _ in 0..900 {
            field.update(1.0 / 120.0, 25.0, &cam, Orbit::LEVEL);
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
        let mut field = ExteriorField::new(400, 3, &cam, Orbit::LEVEL);
        field.update(1.0 / 120.0, 0.0, &cam, Orbit::LEVEL);
        field.update(1.0 / 120.0, 0.0, &cam, Orbit::LEVEL);
        let still: f32 = field
            .streaks(&cam, 0.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(still < 1.0, "a parked ship should not streak: {still}");

        for _ in 0..30 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, Orbit::LEVEL);
        }
        let moving: f32 = field
            .streaks(&cam, 1.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .sum();
        assert!(moving > 100.0, "warp should smear the band: {moving}");
    }

    #[test]
    fn a_trail_never_runs_past_the_point_the_track_vanishes_at() {
        // The bug this is here for. A trail is stretched in *screen* space, and
        // a ray running away from the eye does not project onto a screen-space
        // ray: it projects onto the segment between the star and the point its
        // own direction vanishes at, approaching that point and never reaching
        // it. Multiplied out regardless, the nearest stars ran clean through
        // and out the far side, and every star at the depth where the stretch
        // comes to exactly one laid its trailing end precisely on it — so the
        // two together piled a cusp of light on one spot. Over this pool:
        // 95 of 800 trails finished within a tenth of the point and 77 of those
        // went past it, the worst half again as far as it. The frame that shows
        // it is `--orbit -75,12,0` at full warp, where within two subpixels of
        // the point the mean was 2.3 times the surrounding sky and every
        // subpixel of it was lit, so the sky lost the black between its streaks
        // and read as a hot spot rather than as distance. Nothing now reaches
        // past two thirds of the way.
        //
        // Asked of the whole pool rather than of a reference frame, because
        // until this landed no pinned flight put the camera aft of the beam —
        // the only half of the range where this point exists at all, and
        // exactly why it went unnoticed for as long as it did.
        let cam = cam();
        let astern = Orbit {
            azimuth: (-75.0f32).to_radians(),
            elevation: 12.0f32.to_radians(),
            roll: 0.0,
        }
        .held();
        assert!(
            astern.nose_in_camera()[2] > 0.0,
            "this camera has to be aft of the beam or there is no point to run past"
        );
        let mut field = ExteriorField::new(800, 29, &cam, astern);
        for _ in 0..600 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, astern);
        }
        let vanish = cam
            .vanishing_point(field.nose)
            .expect("a nose that recedes vanishes somewhere");

        let mut asked = 0usize;
        for (star, streak) in field.stars.iter().zip(field.streaks(&cam, 1.0, 0.0)) {
            let Some(prev) = star.prev else { continue };
            let (ax, ay) = (vanish.0 - streak.to.0, vanish.1 - streak.to.1);
            let span = ax * ax + ay * ay;
            let step = ((prev.0 - streak.to.0) * ax + (prev.1 - streak.to.1) * ay) / span;
            let drawn =
                ((streak.from.0 - streak.to.0) * ax + (streak.from.1 - streak.to.1) * ay) / span;
            if !(0.0..1.0).contains(&step) {
                continue;
            }
            asked += 1;
            assert!(
                drawn < 1.0,
                "a trail reached {drawn} of the way to the point its track vanishes at, \
                 where the whole of the track only ever approaches it"
            );
            // And it still stretches: the drawn trail reaches further than the
            // one step the star actually swept, or the smear has gone with the
            // overshoot.
            assert!(
                drawn >= step,
                "a trail was drawn shorter than the step it was stretched from: \
                 {drawn} against {step}"
            );
        }
        assert!(
            asked > 400,
            "only {asked} trails were pointed at the vanishing point, which is too few \
             to be sure of noticing one running through it"
        );
    }

    #[test]
    fn a_sky_with_nowhere_to_vanish_is_stretched_exactly_as_it_always_was() {
        // The other half, and it is what every pinned exterior frame rests on.
        // Abeam the nose lies flat in the image plane and forward of the beam a
        // trail runs toward the eye, so in neither case is there a point to be
        // held back from — and the trail has to come out not merely close to
        // the segment the plain stretch drew, but the same floats.
        let cam = cam();
        let ahead = Orbit {
            azimuth: 1.0,
            elevation: -0.3,
            roll: 0.4,
        }
        .held();
        assert!(
            ahead.nose_in_camera()[2] < 0.0,
            "this camera has to be forward of the beam"
        );
        for orbit in [Orbit::LEVEL, ahead] {
            let mut field = ExteriorField::new(500, 31, &cam, orbit);
            for _ in 0..300 {
                field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, orbit);
            }
            assert!(
                cam.vanishing_point(field.nose).is_none(),
                "{orbit:?} was given a point to vanish at"
            );
            let stretch = 1.0 + 1.0 * 1.0 * 5.0;
            for (star, streak) in field.stars.iter().zip(field.streaks(&cam, 1.0, 0.0)) {
                let Some(prev) = star.prev else { continue };
                let want = (
                    streak.to.0 + (prev.0 - streak.to.0) * stretch,
                    streak.to.1 + (prev.1 - streak.to.1) * stretch,
                );
                assert_eq!(
                    streak.from, want,
                    "the trail moved at {orbit:?}, where nothing may touch it"
                );
            }
        }
    }

    #[test]
    fn a_sublight_trail_is_the_step_the_star_swept_and_nothing_more() {
        // `stretch` is exactly one below the drive, so a trail is the segment
        // the projection handed over and there is nothing to hold back — said
        // as an equality because the branch is gated on `stretch > 1.0` and
        // would otherwise round `prev` onto the line to the vanishing point.
        let cam = cam();
        let astern = Orbit {
            azimuth: -1.3,
            elevation: 0.2,
            roll: 0.0,
        }
        .held();
        let mut field = ExteriorField::new(400, 37, &cam, astern);
        for _ in 0..300 {
            field.update(1.0 / 120.0, 40.0, &cam, astern);
        }
        for (star, streak) in field.stars.iter().zip(field.streaks(&cam, 0.0, 0.0)) {
            let Some(prev) = star.prev else { continue };
            assert_eq!(
                streak.from, prev,
                "a sublight trail is not where the star was a step ago"
            );
        }
    }

    #[test]
    fn the_sky_is_bent_only_where_the_lens_reaches() {
        // With the drive shut down the exterior view has to draw exactly what
        // it would if the lens did not exist — not something very close to it.
        let cam = cam();
        let mut field = ExteriorField::new(600, 13, &cam, Orbit::LEVEL);
        for _ in 0..120 {
            field.update(1.0 / 120.0, 200.0, &cam, Orbit::LEVEL);
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
        let mut field = ExteriorField::new(8000, 17, &cam, Orbit::LEVEL);
        for _ in 0..120 {
            field.update(1.0 / 120.0, 300.0, &cam, Orbit::LEVEL);
        }
        // Built the way the renderer builds one, so it is seated astern of the
        // ship as a real bubble is. Eleven subpixels of ship come out at the
        // twenty-two the bands below are written against.
        let lens = Lens::for_warp((cam.cx, cam.cy), 1.0, 11.0, ABEAM);

        // Mean brightness over a band, not the total: the bands compared below
        // cover different areas, and the canvas clips the outer ones. Measured
        // in rings rather than in subpixels — the bubble is not round, so a
        // circle drawn about its centre crosses the dark disc, the bright rim
        // and the sky beyond it all at once.
        let light_within = |canvas: &Canvas, lo: f32, hi: f32| -> f32 {
            let (mut total, mut n) = (0.0, 0u32);
            for y in 0..100usize {
                for x in 0..200usize {
                    let m = lens.offset((x as f32, y as f32));
                    if m >= lo && m < hi {
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
            light_within(&unbent, 0.0, 0.636),
            light_within(&bent, 0.0, 0.636),
        );
        assert_eq!(
            after, 0.0,
            "the bubble is still lit: {before} became {after}"
        );
        // The ring: the light swept out of the middle piled up just outside it,
        // where the magnification is highest.
        let (before, after) = (
            light_within(&unbent, 1.0, 1.227),
            light_within(&bent, 1.0, 1.227),
        );
        assert!(
            after > before * 1.3,
            "no ring formed: {before} became {after}"
        );

        // And it is a *ring*, not a general brightening: inside the lensed
        // frame the rim outshines the sky further out, which in the unlensed
        // frame it does not.
        let (rim, out) = (
            light_within(&bent, 1.0, 1.227),
            light_within(&bent, 1.636, 2.091),
        );
        assert!(
            rim > out * 1.1,
            "the rim does not stand out: {rim} against {out}"
        );
        let (rim, out) = (
            light_within(&unbent, 1.0, 1.227),
            light_within(&unbent, 1.636, 2.091),
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
    fn bending_the_sky_stops_allocating_once_it_is_under_way() {
        // Every star at warp is chopped into arcs and mapped twice over. Doing
        // that into a fresh `Vec` each time would be thousands of allocations a
        // frame, in the same spirit as the buffer `resolve_into` reuses.
        let cam = cam();
        let mut field = ExteriorField::new(3000, 9, &cam, Orbit::LEVEL);
        let mut canvas = Canvas::new(200, 100);
        let lens = Lens::for_warp(
            (cam.cx, cam.cy),
            1.0,
            ship_half_on_screen(cam.height, ZOOM_DEFAULT),
            ABEAM,
        );

        let mut settled = None;
        for frame in 0..120 {
            field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, Orbit::LEVEL);
            field.draw(&mut canvas, &cam, 1.0, frame as f64 / 60.0, &lens);
            let capacity = (field.source.capacity(), field.bent.capacity());
            match settled {
                // Twenty frames to reach the longest streak it will ever draw.
                None if frame == 20 => settled = Some(capacity),
                Some(settled) => assert_eq!(capacity, settled, "it grew again at {frame}"),
                None => {}
            }
        }
    }

    #[test]
    fn resizing_the_pool_and_the_canvas_keeps_it_valid() {
        let cam = cam();
        let mut field = ExteriorField::new(100, 1, &cam, Orbit::LEVEL);
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
                field.update(1.0 / 120.0, 400.0, &cam, Orbit::LEVEL);
                let lens = Lens::for_warp(
                    (cam.cx, cam.cy),
                    1.0,
                    ship_half_on_screen(cam.height, ZOOM_DEFAULT),
                    ABEAM,
                );
                field.draw(&mut canvas, &cam, 1.0, 0.0, &lens);
            }
            assert!(field
                .stars
                .iter()
                .all(|s| s.pos.iter().all(|c| c.is_finite())));
        }
    }

    /// How evenly the pool covers the frame: the fullest patch of sky over the
    /// emptiest, the fullest *band* of sky over the emptiest, and what share of
    /// the pool is on screen at all.
    ///
    /// The bands are the patches summed across, and they are worth their own
    /// number rather than being covered by the patches: the fault this was
    /// written for empties the top and bottom of the frame and leaves the
    /// middle, so it is the bands it shows up in first and hardest, and it is
    /// the bands that say whether it has come back.
    ///
    /// Measured on the pool rather than on a rendered frame on purpose. A
    /// frame carries the streak lengths, the vignette, the lens glow and the
    /// brightness ramp on top of the sky, and off the beam every one of those
    /// moves too — so a count of lit cells cannot say whether the stars went
    /// away or merely stopped smearing. Projecting the pool asks the one
    /// question.
    fn evenness(field: &ExteriorField, cam: &Camera) -> (f32, f32, f32) {
        const COLS: usize = 6;
        const ROWS: usize = 4;
        let mut grid = [0usize; COLS * ROWS];
        for star in &field.stars {
            let Some((x, y)) = cam.project(star.pos) else {
                continue;
            };
            if x < 0.0 || x >= cam.width || y < 0.0 || y >= cam.height {
                continue;
            }
            let c = ((x / cam.width * COLS as f32) as usize).min(COLS - 1);
            let r = ((y / cam.height * ROWS as f32) as usize).min(ROWS - 1);
            grid[r * COLS + c] += 1;
        }
        let ratio = |(lo, hi): (usize, usize)| hi as f32 / lo.max(1) as f32;
        let span = |acc: (usize, usize), v: usize| (acc.0.min(v), acc.1.max(v));

        let patches = grid.iter().fold((usize::MAX, 0), |a, &v| span(a, v));
        let bands = grid
            .chunks_exact(COLS)
            .map(|row| row.iter().sum::<usize>())
            .fold((usize::MAX, 0), span);
        let on: usize = grid.iter().sum();
        (
            ratio(patches),
            ratio(bands),
            on as f32 / field.stars.len() as f32,
        )
    }

    /// A range change takes exactly the stars the frame no longer holds, and
    /// leaves what is left as evenly spread as it found it.
    ///
    /// The sharp statement of what the recycle in `update` is for, where
    /// `the_sky_is_as_even_off_the_beam_as_it_is_abeam` is the acceptance test
    /// over a whole flight. The whole pool is put on one range shell, spread
    /// evenly across the frame, and stepped once with the camera dead astern so
    /// that travel is purely along the view axis: nothing translates, the fold
    /// cannot fire, and the only thing acting is the scale.
    ///
    /// It pins the rate, which is the part most easily got wrong — the natural
    /// mistake is `1 - 1/lambda` per axis, which double-counts the corners and
    /// comes out nearly twice too large — and it pins the shape of what stayed,
    /// by measuring the spread afterwards rather than only the count. And it
    /// pins the thing that replaced a relocation: what went is not a share taken
    /// from anywhere, it is precisely the stars whose own place in the frame has
    /// gone past the edge of it.
    #[test]
    fn a_range_change_takes_exactly_the_stars_the_frame_no_longer_holds() {
        const POOL: usize = 20_000;
        // A near shell and a fast step, so the scale is a fifth rather than a
        // few percent: `1 - 1/lambda` and `1 - 1/lambda^2` are then far enough
        // apart that the tolerance below can tell them apart, which is the
        // whole reason this test exists.
        const AT: f32 = 40.0;
        const SPEED: f32 = 600.0;
        const DT: f32 = 1.0 / 120.0;
        let cam = cam();
        // Dead astern: `sky_travel` is `(0, 0, -1)` to the bit, so `travel[0]`
        // cannot move a star sideways and the range shrinks by the whole step.
        let orbit = Orbit {
            azimuth: -std::f32::consts::FRAC_PI_2,
            elevation: 0.0,
            roll: 0.0,
        };
        let mut field = ExteriorField::new(POOL, 4, &cam, orbit);
        for star in &mut field.stars {
            star.pos[2] = AT;
        }
        let (hw, hh) = field.bound;
        let focal = field.focal;
        // Spread evenly over the band, in screen units, which is the measure
        // `spawn` lays a fresh pool out in, then back-projected the way `spawn`
        // back-projects it.
        let scale = AT / focal;
        let placed: Vec<(f32, f32)> = (0..POOL)
            .map(|_| {
                (
                    field.rng.random_range(-hw..hw) * scale,
                    field.rng.random_range(-hh..hh) * scale,
                )
            })
            .collect();
        for (star, &(x, y)) in field.stars.iter_mut().zip(&placed) {
            star.pos[0] = x;
            star.pos[1] = y;
        }
        let born: Vec<(f32, f32)> = field.stars.iter().map(|s| (s.phase, s.magnitude)).collect();

        field.update(DT, SPEED, &cam, orbit);
        let lambda = AT / (AT - SPEED * DT);
        let expected = 1.0 - 1.0 / (lambda * lambda);

        // One step on, nothing has been taken yet: a star the magnification
        // carries out is left outside for the frame that draws it leaving. So
        // the ones on their way out are the ones outside the band, and this is
        // where the arithmetic can be read off cleanly.
        let z = AT - SPEED * DT;
        let (bx, by) = (band(hw, focal, z), band(hh, focal, z));
        let (mut going, mut kept, mut inside) = (0usize, 0usize, [0usize; 3]);
        let mut leaving = vec![false; POOL];
        for (i, star) in field.stars.iter().enumerate() {
            let out = (star.pos[0] / bx).abs().max((star.pos[1] / by).abs());
            if out >= 1.0 {
                going += 1;
                leaving[i] = true;
                // And it is going because the frame stopped holding it: the
                // band is a fixed rectangle on the screen, so the test is where
                // it sat in the old one against the scale.
                let (x, y) = placed[i];
                let was = (x / (AT / focal * hw))
                    .abs()
                    .max((y / (AT / focal * hh)).abs());
                assert!(
                    was * lambda >= 1.0,
                    "star {i} is leaving from {was:.3} of the band, which a \
                     scale of {lambda:.4} leaves well inside the frame"
                );
                continue;
            }
            kept += 1;
            // How far out it sits, as a fraction of the band in the max norm —
            // the coordinate a uniform square is uniform in.
            for (k, edge) in [0.25f32, 0.5, 0.75].iter().enumerate() {
                if out <= *edge {
                    inside[k] += 1;
                }
            }
        }

        let share = going as f32 / POOL as f32;
        assert!(
            (share - expected).abs() < 0.02,
            "a step put {share:.3} of the shell on its way out where the scale \
             of {lambda:.4} asks for {expected:.3}"
        );
        for (k, edge) in [0.25f32, 0.5, 0.75].iter().enumerate() {
            let got = inside[k] as f32 / kept as f32;
            let want = edge * edge;
            assert!(
                (got - want).abs() < 0.02,
                "after a step the shell has {got:.3} of what it kept inside \
                 {edge} of the band, where an even one has {want:.3} — the \
                 magnification did not leave it evenly spread"
            );
        }

        // And the step after that takes exactly those and nobody else. A star
        // that was taken came back as a fresh one, and the two values it is
        // given at birth and never again say so.
        field.update(DT, SPEED, &cam, orbit);
        for (i, star) in field.stars.iter().enumerate() {
            let taken = (star.phase, star.magnitude) != born[i];
            assert_eq!(
                taken,
                leaving[i],
                "star {i} was {} on the step after it was {} out of the frame",
                if taken { "taken" } else { "kept" },
                if leaving[i] { "carried" } else { "not carried" }
            );
        }
    }

    /// Nothing lit is ever taken out of the middle of the frame.
    ///
    /// The regression test for the reported fault, and for the larger one
    /// beside it. The band moves stars about for two reasons — one has crossed
    /// a wall, or the magnification has carried it past the edge of the frame —
    /// and neither of them used to care whether anyone was looking. Astern at
    /// full warp it was taking four of a 256-star pool a step out of a visible
    /// position at a mean brightness of 0.36, which is a bright star winking out
    /// mid-frame eight times a second; from ahead of the beam it was fourteen a
    /// step. Both are what a viewer reports as stars disappearing before they
    /// reach the edge of the screen.
    ///
    /// Asked of the brightness the star had on its *last drawn frame*, because
    /// that is what the eye saw go, and through `lit` rather than a copy of it.
    /// The bound is a share of what the visible sky averages, so it cannot be
    /// satisfied by dimming everything.
    #[test]
    fn nothing_lit_is_ever_taken_out_of_the_middle_of_the_frame() {
        // Every angle that has a depth flow at all, both signs, and both a
        // pure one and one with the sideways travel alongside it.
        const ANGLES: [(f32, f32, f32); 6] = [
            (-75.0, 6.0, 20.0),
            (55.0, 35.0, 20.0),
            (90.0, 0.0, 0.0),
            (-90.0, 0.0, 0.0),
            (45.0, 0.0, 0.0),
            (-135.0, -20.0, 0.0),
        ];
        // A share of what the visible sky averages, and it has to be a share
        // rather than nothing: a star inside the near wall's ramp is dimmed as
        // it approaches, not blanked, and one step of the flow can be worth
        // several units of range, so the last frame it is drawn on is a little
        // way short of the wall and still carries a trace. Measured over all
        // twelve flights below the worst is 0.28 of the mean; before any of this
        // it was 6.5 times the mean, so the bound has sixteen times the margin
        // it needs in the direction that matters.
        const TRACE: f32 = 0.4;
        let cam = Camera::new(120, 72);
        let pool = crate::cli::DEFAULT_STARS;

        for (az, el, roll) in ANGLES {
            for speed in [crate::ship::WARP_MAX, 42.0] {
                let orbit = Orbit {
                    azimuth: az.to_radians(),
                    elevation: el.to_radians(),
                    roll: roll.to_radians(),
                }
                .held();
                let mut field = ExteriorField::new(pool, 1, &cam, orbit);
                // Settled first: the profile a flow leaves alone is not the one
                // a fresh pool is laid out with, and the crossing from one to
                // the other is not what this is asking about.
                for _ in 0..600 {
                    field.update(1.0 / 120.0, speed, &cam, orbit);
                }

                let (mut worst, mut seen, mut sky, mut lit_count) =
                    (0.0f32, 0usize, 0.0f32, 0usize);
                for _ in 0..400 {
                    let fade = field.near_fade;
                    let before: Vec<(f32, f32, f32)> = field
                        .stars
                        .iter()
                        .map(|s| {
                            let visible = cam.project(s.pos).is_some_and(|(x, y)| {
                                (0.0..cam.width).contains(&x) && (0.0..cam.height).contains(&y)
                            });
                            let light = lit(s, fade, 0.0, 0.0);
                            if visible {
                                sky += light;
                                lit_count += 1;
                            }
                            (s.phase, s.magnitude, if visible { light } else { 0.0 })
                        })
                        .collect();
                    field.update(1.0 / 120.0, speed, &cam, orbit);
                    for (star, &(phase, magnitude, light)) in field.stars.iter().zip(&before) {
                        if (star.phase, star.magnitude) != (phase, magnitude) {
                            seen += 1;
                            worst = worst.max(light);
                        }
                    }
                }

                let mean = sky / lit_count.max(1) as f32;
                assert!(
                    seen > 100,
                    "only {seen} stars were taken at {az},{el},{roll} at speed \
                     {speed} — too few to be sure of catching a bright one"
                );
                assert!(
                    worst < TRACE * mean,
                    "a star of brightness {worst:.3} was taken out of the frame \
                     at {az},{el},{roll} at speed {speed}, against a visible sky \
                     averaging {mean:.3} — that is a star winking out where \
                     someone is looking at it"
                );
            }
        }
    }

    /// The band settles at the one profile a flow leaves alone.
    ///
    /// What the recycle buys, stated as the thing that makes it free. Advection
    /// through a frustum takes a count per unit range of `n(z + d)` to
    /// `n(z + d)·(z/(z + d))²`, so the only profile a step returns unchanged is
    /// `n ∝ z²` — an even sky, seen through a cone. Hold any other one and every
    /// step owes a relocation per star, which is the fizz this replaced;
    /// let it alone and the flow keeps it, whichever way the flow runs.
    ///
    /// The far end is where the stars are, and the near end is where they are
    /// bright: that is the same bargain [`crate::starfield`] has always struck,
    /// and it is why the cockpit reads as distance rather than as a curtain.
    #[test]
    fn the_band_settles_at_the_one_profile_a_flow_leaves_alone() {
        const POOL: usize = 20_000;
        const SLICES: usize = 5;
        let cam = Camera::new(120, 72);
        // Toward the camera and away from it. Both are the same statement, and
        // the pool has to arrive at it from opposite directions.
        for azimuth in [-90.0f32, 90.0] {
            let orbit = Orbit {
                azimuth: azimuth.to_radians(),
                elevation: 0.0,
                roll: 0.0,
            }
            .held();
            let mut field = ExteriorField::new(POOL, 5, &cam, orbit);
            // Two full crossings of the band at this speed, which is what it
            // takes for the pool a flight opens with to have flown through.
            for _ in 0..2000 {
                field.update(1.0 / 120.0, 280.0, &cam, orbit);
            }

            let mut got = [0usize; SLICES];
            for star in &field.stars {
                let i = ((star.pos[2] - Z_NEAR) / (Z_FAR - Z_NEAR) * SLICES as f32) as usize;
                got[i.min(SLICES - 1)] += 1;
            }
            // The share of `z²` each equal slice of range holds, integrated
            // rather than sampled at the middle: the slices are wide and `z²`
            // is not flat across them.
            let edge = |i: usize| Z_NEAR + (Z_FAR - Z_NEAR) * i as f32 / SLICES as f32;
            let cube = |z: f32| z * z * z;
            let whole = cube(Z_FAR) - cube(Z_NEAR);
            for (i, &count) in got.iter().enumerate() {
                let want = (cube(edge(i + 1)) - cube(edge(i))) / whole;
                let share = count as f32 / POOL as f32;
                assert!(
                    (share - want).abs() < 0.02,
                    "at azimuth {azimuth} slice {i} of the band holds \
                     {share:.3} of the pool where an even sky holds \
                     {want:.3} — got {got:?}"
                );
            }
        }
    }

    /// The near wall is not a boundary at all when the sky runs flat.
    ///
    /// The other half of the ramp, and the half the reference frames rest on.
    /// Abeam, `travel[2]` is a hard zero, nothing ever reaches the near wall,
    /// and the ramp is switched off rather than reduced to an identity — so the
    /// shot `side.txt` is recorded at is drawn from the bytes it always was.
    /// Asserted on the `Streak`s themselves and by equality, because "very
    /// nearly the same sky" is precisely what a golden hash cannot survive.
    #[test]
    fn the_near_wall_is_not_a_boundary_at_all_when_the_sky_runs_flat() {
        let cam = cam();
        for (elevation, roll) in [(0.0f32, 0.0f32), (0.9, 0.0), (-1.4, 2.2)] {
            let orbit = Orbit {
                azimuth: 0.0,
                elevation,
                roll,
            }
            .held();
            let mut field = ExteriorField::new(500, 19, &cam, orbit);
            for _ in 0..300 {
                field.update(1.0 / 120.0, crate::ship::WARP_MAX, &cam, orbit);
                assert_eq!(
                    field.near_fade, 0.0,
                    "the near wall grew a ramp at elevation {elevation}, roll \
                     {roll}, where the flow never reaches it"
                );
            }
            // And the brightness really is the unramped one, star for star.
            for star in &field.stars {
                assert_eq!(
                    lit(star, field.near_fade, 0.22, 1.0),
                    lit(star, 0.0, 0.22, 1.0),
                    "a star was dimmed at elevation {elevation}, roll {roll}"
                );
            }
        }
    }

    /// The orbit a held camera key produces, one sim step at a time: the target
    /// runs on by [`crate::view::ORBIT_STEP`] per autorepeat press — about
    /// thirty a second — and the eased value chases it exactly the way
    /// `Flight::advance` does.
    fn leaned_on(orbit: Orbit, target: &mut Orbit, presses_per_second: f32) -> Orbit {
        const STEP: f32 = 1.0 / 120.0;
        target.azimuth += presses_per_second * STEP * crate::view::ORBIT_STEP;
        *target = target.held();
        let ease = 1.0 - (-crate::view::ORBIT_EASE * STEP).exp();
        Orbit {
            azimuth: orbit.azimuth
                + crate::ship::wrap_signed(target.azimuth - orbit.azimuth) * ease,
            ..orbit
        }
        .held()
    }

    /// The sky is as even off the beam as it is abeam.
    ///
    /// The regression test for the fault the recycle in `update` exists for.
    /// The band is held inside a range-scaled screen frustum by folding `x` and
    /// `y` modulo `band(half, focal, z)`, which is the right answer for the
    /// *translation* the flow applies and no answer at all for the *scale* that
    /// any change of range applies alongside it — a star's place in the frame
    /// being `focal * pos / z`. Left alone that emptied the top and bottom of
    /// the frame and left a bar of stars across the middle, which is what a
    /// couple of seconds on `A` or `D` used to produce and what this holds.
    ///
    /// It has since caught two more of the same shape, both from the change that
    /// took the relocations out. Filling a contraction's rim about the middle of
    /// the frame rather than about the place the flow has slid the middle to put
    /// the fill where the gap was not: 2.2 to 1 across the frame at 45 degrees,
    /// with 0.57 of the pool on screen. And letting a star that is on its way
    /// out keep travelling sideways let the travel pull it back inside the band
    /// it had just left, so it hovered at the edge instead of leaving: 0.60 on
    /// screen at −45. Both wear the symptom this test measures and neither is
    /// visible in any reference frame.
    ///
    /// Measured before the fix, at the settings below and against an abeam
    /// baseline of 1.30 over six seeds: parked at 45 degrees the sky was 25 to
    /// 1 across the frame, at 135 it was 27, and with a key held it reached 45
    /// and went on climbing. On the other side of the beam it wore the other
    /// face — the frame stayed smooth at about 1.5 while three quarters of the
    /// pool piled into the off-screen margin — which is why this measures
    /// coverage as well as spread. Every one of those is now inside the
    /// baseline's own scatter.
    ///
    /// **What is not fixed, and is asserted loosely on purpose.** While a key
    /// is actually down the frame still carries a left-to-right gradient, 3.5
    /// to 1 at its worst. It is not the reported fault and not the same shape:
    /// the *bands* stay flat, so the sky is even top to bottom and merely
    /// leans toward the side the camera is turning away from, and it is gone
    /// within a step of letting go. It is what is left of the swing, whose
    /// scale is neither uniform across the frame nor the same on both axes —
    /// a pan stretches the frame sideways by the square of the range ratio
    /// where it stretches it vertically by the ratio itself, and the deficit
    /// that leaves is spread over the leading half of the frame rather than
    /// standing on the rim. Putting it right needs a placement that follows that
    /// profile, which is a change of its own with its own measurement; both a
    /// uniform placement and the exact anisotropic rate were tried here and both
    /// made the gradient worse, because a bigger correction placed symmetrically
    /// is worse than a smaller one. So the bands are held tightly, the frame
    /// loosely, and the loose bound is a ratchet against the 45 it came from
    /// rather than a statement that this is right.
    ///
    /// The reference frames cannot see any of this. `side.txt` and `warp.txt`
    /// are shot at [`Orbit::LEVEL`], where `travel[2]` is exactly zero, and
    /// `orbit.txt` is shot at `--throttle 1.0`, where one step is longer than
    /// the whole band so a star is recycled before it can drift anywhere.
    #[test]
    fn the_sky_is_as_even_off_the_beam_as_it_is_abeam() {
        // Wide enough that a bin holds hundreds of stars, so the baseline is
        // set by the sky rather than by counting noise: 8000 stars over
        // twenty-four bins is about 250 each, which measures 1.20 to 1.30
        // abeam over six seeds. A finer grid needs a bigger pool to say the
        // same thing, and this runs on every `cargo test`.
        const POOL: usize = 8_000;
        const EVEN: f32 = 2.0;
        /// The residual gradient while a key is held, which is a ratchet on
        /// the 45 to 1 this was written for rather than a target.
        const EVEN_SWINGING: f32 = 4.5;
        const BANDS_EVEN: f32 = 1.6;
        const ON_SCREEN: f32 = 0.65;
        // 10.8 c, which is where the report was shot, and slow enough that a
        // star takes a couple of hundred steps to cross the band. At full warp
        // one step is longer than the band and nothing can accumulate, which
        // is why the reference frame with the camera off the beam is recorded
        // in the one regime where this cannot happen.
        const SPEED: f32 = 280.0;
        const DT: f32 = 1.0 / 120.0;

        // Two and a half seconds, which is about what it took to report. The
        // fault was well past a factor of ten by then and still climbing.
        const STEPS: usize = 300;

        let cam = cam();
        let parked = |orbit: Orbit| {
            let mut field = ExteriorField::new(POOL, 1, &cam, orbit);
            for _ in 0..STEPS {
                field.update(DT, SPEED, &cam, orbit);
            }
            evenness(&field, &cam)
        };

        // Abeam and dead astern are the two angles where `travel[2]` is
        // exactly zero, and the two the fault could never reach. They are here
        // to say the measurement is of the sky and not of the instrument.
        let astern = Orbit {
            azimuth: std::f32::consts::PI,
            elevation: 0.0,
            roll: 0.0,
        };
        for orbit in [Orbit::LEVEL, astern] {
            let (patches, bands, on) = parked(orbit);
            assert!(
                patches <= EVEN && bands <= BANDS_EVEN && on >= ON_SCREEN,
                "the sky went uneven at {orbit:?}, where nothing moves the \
                 range: {patches:.2} to 1 across the frame, {bands:.2} top to \
                 bottom, {on:.2} of the pool on screen"
            );
        }

        // Parked off the beam, which is where a pan leaves the camera and
        // where it spends every second the key is not down.
        for (az, el) in [
            (45.0f32, 0.0f32),
            (-45.0, 0.0),
            (90.0, 0.0),
            (-90.0, 0.0),
            (135.0, 0.0),
            (90.0, 25.0),
            (-120.0, -20.0),
        ] {
            let orbit = Orbit {
                azimuth: az.to_radians(),
                elevation: el.to_radians(),
                roll: 0.0,
            };
            let (patches, bands, on) = parked(orbit);
            assert!(
                patches <= EVEN,
                "the sky is {patches:.2} to 1 across the frame with the camera \
                 parked at {az}, {el} — abeam it is 1.3"
            );
            assert!(
                bands <= BANDS_EVEN,
                "the sky is {bands:.2} to 1 top to bottom with the camera \
                 parked at {az}, {el} — this is the bar across the middle of \
                 the frame, and abeam it is 1.05"
            );
            assert!(
                on >= ON_SCREEN,
                "only {on:.2} of the pool is on screen at {az}, {el} — abeam \
                 it is 0.76, and the rest has piled into the margin"
            );
        }

        // And with a key leaned on, which is how it was found: measured while
        // the camera is still swinging, and again once it has been let go, so
        // a fault that only lives in the rotation cannot hide behind one that
        // only lives in the flow, or the other way about.
        for presses in [30.0f32, -30.0] {
            let mut field = ExteriorField::new(POOL, 1, &cam, Orbit::LEVEL);
            let (mut orbit, mut target) = (Orbit::LEVEL, Orbit::LEVEL);
            for _ in 0..STEPS {
                orbit = leaned_on(orbit, &mut target, presses);
                field.update(DT, SPEED, &cam, orbit);
            }
            let (patches, bands, on) = evenness(&field, &cam);
            assert!(
                bands <= BANDS_EVEN && on >= ON_SCREEN,
                "the sky is {bands:.2} to 1 top to bottom with the camera \
                 swinging at {presses} presses a second, {on:.2} of it on \
                 screen — this is the reported fault, and it is the one thing \
                 here that is not allowed back"
            );
            assert!(
                patches <= EVEN_SWINGING,
                "the sky is {patches:.2} to 1 across the frame with the camera \
                 swinging at {presses} presses a second, against a residual of \
                 3.5 and a baseline of 1.3"
            );

            for _ in 0..STEPS {
                field.update(DT, SPEED, &cam, orbit);
            }
            let (patches, bands, on) = evenness(&field, &cam);
            assert!(
                patches <= EVEN && bands <= BANDS_EVEN && on >= ON_SCREEN,
                "the sky is still {patches:.2} to 1 across the frame and \
                 {bands:.2} top to bottom long after the camera was let go at \
                 {presses} presses a second, {on:.2} of it on screen"
            );
        }
    }
}
