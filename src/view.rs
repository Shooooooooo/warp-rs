//! Which camera the flight is being flown behind, and where the outside one
//! is parked.
//!
//! Nearly a leaf on purpose: the flight loop, the renderer, the panel and the
//! command line all need to name a view, and none of them should have to reach
//! through another to do it. The one thing it reaches for is
//! [`crate::ship::wrap_signed`], because an eye swung round a ship and a ship
//! rolled about its own nose have exactly the same arithmetic to do and there
//! is no sense in two answers to it.
//!
//! Two things live here that a reader might expect to find beside the hull or
//! beside the sky. [`Orbit`] is where the outside camera is pointed and [`Eye`]
//! is that plus how far off it is, and both are here rather than in
//! [`crate::models`] because they are what the *sky* is streamed against as
//! well as what the hull is posed by — putting them next to the hull would make
//! [`crate::render`] reach through the ship models to find out which way it is
//! being looked at.

use crate::ship::wrap_signed;
use std::f32::consts::{PI, TAU};

/// Focal length of the side camera, as a multiple of the canvas height. Longer
/// than the cockpit's 0.85: a long lens flattens the hull into a profile, which
/// is the shot, where a wide one would splay it into a three-quarter view.
pub const SIDE_FOCAL: f32 = 1.15;
/// Half the ship's length on screen at [`ZOOM_DEFAULT`], as a fraction of the
/// canvas height.
///
/// This used to be the whole answer, pulled between a ship too small to have
/// any detail left in it and a ship crowding the frame it is meant to be flying
/// through, and it sat at 0.24 — a compromise that satisfied neither end. The
/// zoom is what settles that argument now, so this is only where the shot
/// *opens*, and it opens wide: the hull spans a quarter of the canvas height,
/// far enough off that the ship reads as something crossing the view rather
/// than as something parked in it. Anyone who wants the old close-up presses
/// `]` about five times.
pub const SHIP_SCREEN_FRAC: f32 = 0.125;
/// The furthest any part of a hull reaches from the ship's own centre. Hulls
/// are modelled inside the unit box, so this is the box's diagonal, rounded up.
pub const HULL_REACH: f32 = 1.8;

/// How far in and out the side camera may be pushed, as a multiple of the
/// default framing.
///
/// The near end is held by a near plane rather than by taste, and it is checked
/// where that plane is declared: pushing in too far would put a rolled fin
/// through the projection's own near plane and drop the plate, which is the
/// `const` assertion in [`crate::models`]. The number here leaves it room.
///
/// The far end used to answer to a plane as well — pulling back too far would
/// let a star pass between the eye and the hull, which was the whole of the
/// exterior view's depth sorting. [`crate::universe`] holds its nearest star
/// four *light years* off against a hull measured in units where the ship is
/// about one, so that end of the sandwich is now twelve orders of magnitude
/// clear and answers to framing alone. The assertion that used to guard it is
/// gone, and says so where it stood.
pub const ZOOM_MIN: f32 = 0.6;
pub const ZOOM_MAX: f32 = 2.8;
/// Where the zoom starts, and where `R` puts it back.
pub const ZOOM_DEFAULT: f32 = 1.0;
/// One notch of the wheel, or one press of `[` or `]`. Geometric rather than
/// additive, so a notch is the same size of change at both ends of the range
/// instead of shoving the far view about and barely moving the near one. About
/// fourteen notches spans the lot: one flick of a wheel, and a comfortable
/// second or so of auto-repeat on a key.
pub const ZOOM_STEP: f32 = 1.12;
/// How quickly the camera catches up with the zoom it has been asked for. A
/// wheel arrives as a burst of notches, and a camera that answered each one on
/// the frame it landed would jump; this settles in about a third of a second.
pub const ZOOM_EASE: f32 = 9.0;

/// Range from the side camera to the ship. The focal length is proportional to
/// the canvas height, so this falls out of the framing alone and the ship is
/// the same fraction of the frame on a phone-sized terminal and a wall.
///
/// Zooming is a dolly and not a change of lens, and the reason has outlived the
/// mechanism that first gave it. It used to be that the star band was laid out
/// against `Camera::focal` and cached against it, so a zoom that touched the
/// focal length re-folded the whole field every notch and dropped every trail
/// doing it. There is no band and no cache: [`crate::universe`] is a volume in
/// the world, re-projected each frame, and it would survive a focal change
/// without noticing. What survives is the honest picture — a lens change is a
/// different shot, not a closer one — and the fact that a few units of dolly
/// against a sky in light years is no parallax at all.
pub fn ship_distance(zoom: f32) -> f32 {
    SIDE_FOCAL / (SHIP_SCREEN_FRAC * clamp_zoom(zoom))
}

/// The same statement from the other end: half the ship's length on screen, in
/// canvas subpixels. `focal / ship_distance` gives this back, and
/// `the_two_ways_of_asking_how_big_the_ship_is_agree` says so — this form is
/// the one to reach for, because it is what the framing actually promises and
/// it does not go through a division and back.
pub fn ship_half_on_screen(height: f32, zoom: f32) -> f32 {
    height * SHIP_SCREEN_FRAC * clamp_zoom(zoom)
}

/// A zoom held to the range the geometry allows. A NaN is sent to the default
/// rather than through `clamp`, which passes one straight out the other side.
fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_nan() {
        ZOOM_DEFAULT
    } else {
        zoom.clamp(ZOOM_MIN, ZOOM_MAX)
    }
}

/// The two ends of [`ship_distance`], as constants, so the near planes either
/// side of the hull can be guarded at compile time. `ship_distance` is a
/// function and cannot be one; these are the values it reaches.
pub const MAX_SHIP_DISTANCE: f32 = SIDE_FOCAL / (SHIP_SCREEN_FRAC * ZOOM_MIN);
pub const MIN_SHIP_DISTANCE: f32 = SIDE_FOCAL / (SHIP_SCREEN_FRAC * ZOOM_MAX);

/// The two run opposite ways — a bigger zoom is a *smaller* distance — and the
/// pair of guards that use them would both still compile with the names the
/// wrong way round, each checking the end it was not meant to.
const _: () = assert!(
    MIN_SHIP_DISTANCE < MAX_SHIP_DISTANCE,
    "the near and far ends of the zoom have been swapped"
);

/// One press of a camera key. Sixty of them go all the way round, which is a
/// visible nudge on a single tap and about two seconds of auto-repeat for a
/// full turn — the same bargain the zoom strikes with its fourteen notches.
/// Additive rather than geometric, because an angle has no far end to be shoved
/// about: every part of the swing is worth the same.
pub const ORBIT_STEP: f32 = TAU / 60.0;
/// How quickly the camera catches up with the angle it has been asked for.
///
/// Its own constant rather than [`ZOOM_EASE`], which it happens to equal: the
/// two settle at the same rate because a third of a second is what a camera
/// move wants either way, not because they are the same control. Tune one and
/// the other should not follow.
pub const ORBIT_EASE: f32 = 9.0;

/// An angle folded onto a single turn, and left exactly alone if it is already
/// on one.
///
/// The guard is not an optimisation. [`wrap_signed`] goes through `rem_euclid`
/// and back, which does not return an angle already in range bit for bit — and
/// this is applied on every press and every eased step, so without it a notch
/// and its opposite would not cancel. What used to turn on that cancelling was
/// the star band, which laid itself out afresh whenever the orbit differed from
/// the one it was last laid against — so an angle that came back a hair off
/// rebuilt the whole rotation path every frame of a flight nobody was touching
/// the camera on. There is no band now and a swing moves no star, but the guard
/// stays on a reason of its own: an [`Orbit`]'s equality is what
/// [`crate::app::Flight`] eases the camera against, and an angle that will not
/// cancel is a camera that never settles.
fn turn_of(angle: f32) -> f32 {
    if (-PI..PI).contains(&angle) {
        angle
    } else {
        wrap_signed(angle)
    }
}

/// Where the camera outside is pointed, as three angles about the ship.
///
/// All of them are the *camera's*, not the hull's. Out here the direction of
/// travel is the one thing that cannot move — the ship flies where its nose
/// points and the band of sky streams along that track — so the way to look at
/// the ship from somewhere else is to go somewhere else, and these three say
/// where. The ship goes on flying dead ahead through all of them.
///
/// All three go all the way round, and none of them stops. The elevation used
/// to stop at the quarter turn, on the argument that a turn of the azimuth
/// already reaches everything past it — which is true about *coverage* and
/// says nothing about how a camera behaves under a held key. A control that
/// stops dead halfway through the gesture that is being made with it is a
/// control that has failed, whatever the reachable set looks like. Going over
/// the top turns the picture upside down and keeps turning, because that is
/// what going over the top is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Orbit {
    /// Round the ship, about its own down axis. Zero is dead abeam to
    /// starboard — the shot as it opens — and positive walks the camera
    /// forward toward the nose, so at a quarter turn it is ahead looking back
    /// down the throat and at three-quarters it is astern in the wake. Wraps.
    pub azimuth: f32,
    /// Up over the hull and down under it, about the track. Zero is level with
    /// it, positive lifts the camera, and a quarter turn is the plan view.
    /// Wraps, so half a turn is the view from the far beam, inverted.
    ///
    /// The flight model used to stop short of straight up here, and the
    /// contrast was written down as the point rather than an inconsistency. It
    /// was neither: what stopped was the panel's *reading*, never the ship, and
    /// the clamp that stopped it was throwing away input the attitude went on
    /// taking. Both go all the way round now, and for the same reason — an
    /// angle with nothing to run into does not need a stop.
    pub elevation: f32,
    /// The camera's own roll, about the axis it is looking down. Turns the
    /// picture — hull and sky together — rather than anything in it. Wraps.
    pub roll: f32,
}

impl Orbit {
    /// The shot as it opens, and where `R` puts it back: off the starboard
    /// beam, level, and upright.
    pub const LEVEL: Orbit = Orbit {
        azimuth: 0.0,
        elevation: 0.0,
        roll: 0.0,
    };

    /// Whether this is that shot, exactly.
    ///
    /// Asked rather than approximated, and a test's question rather than the
    /// renderer's: what wants to know is an assertion that a camera came back
    /// to where it started. The doc here used to claim the star band and the
    /// bubble both took fast paths off this answer, which was never true — they
    /// compared against what they were last laid out for and against a zero
    /// sine ([`crate::lens`]), and neither ever called this. There is no band
    /// at all now, and nothing in a running program calls it, hence the gate.
    #[cfg(test)]
    pub fn is_level(self) -> bool {
        self == Self::LEVEL
    }

    /// This orbit, folded onto a single turn on all three axes, and sent home
    /// outright if any of them is not a number.
    ///
    /// The NaN case is the reason this is not three calls to `clamp`: `clamp`
    /// passes a NaN straight out the other side, and a NaN angle here is not a
    /// frame drawn slightly wrong, it is a frame with no sky in it at all.
    ///
    /// Nothing is clamped any more, and nothing needs to be. The basis below is
    /// orthonormal and right-handed at every angle on every axis — there is no
    /// pole for it to go singular at, because the screen-right axis does not
    /// depend on the elevation at all — so the only thing a limit ever bought
    /// was a smaller set of numbers to think about, at the price of a camera
    /// that stopped dead under a held key.
    pub fn held(self) -> Self {
        if !self.azimuth.is_finite() || !self.elevation.is_finite() || !self.roll.is_finite() {
            return Self::LEVEL;
        }
        Self {
            azimuth: turn_of(self.azimuth),
            elevation: turn_of(self.elevation),
            roll: turn_of(self.roll),
        }
    }

    /// The camera's axes, in the hull's own frame: screen right, screen down,
    /// and the direction it is looking.
    ///
    /// The hull's frame is `+z` out the nose, `+x` to starboard, `+y` down, so
    /// before the camera's own roll is applied:
    ///
    /// ```text
    /// R = (-sin a,       0,     cos a      )
    /// D = ( sin e cos a, cos e, sin e sin a)
    /// F = (-cos e cos a, sin e, -cos e sin a)
    /// ```
    ///
    /// and the roll turns the first two about the third. Orthonormal and of
    /// determinant one at every angle, which is what lets [`crate::models`] go
    /// on deciding which way a plate faces from the sign of its projected
    /// area: the mirror image of this would look almost right and invert every
    /// facing test.
    ///
    /// At [`Self::LEVEL`] every sine is exactly zero and every cosine exactly
    /// one, so this comes out as `R = (0, 0, 1)`, `D = (0, 1, 0)`,
    /// `F = (-1, 0, 0)` — the quarter turn about the ship's down axis that put
    /// the nose to screen right and the starboard side toward the camera for as
    /// long as this view has existed. Not approximately: every product in
    /// [`Eye::to_camera`] then has an exact zero or an exact one in it, so a
    /// level frame is drawn from the same bytes it always was. That is a
    /// property the reference frames check, not a claim.
    pub fn basis(self) -> [[f32; 3]; 3] {
        let (sa, ca) = self.azimuth.sin_cos();
        let (se, ce) = self.elevation.sin_cos();
        let (sk, ck) = self.roll.sin_cos();

        let right = [-sa, 0.0, ca];
        let down = [se * ca, ce, se * sa];
        let forward = [-ce * ca, se, -ce * sa];

        let mut rolled_right = [0.0; 3];
        let mut rolled_down = [0.0; 3];
        for i in 0..3 {
            rolled_right[i] = right[i] * ck + down[i] * sk;
            rolled_down[i] = down[i] * ck - right[i] * sk;
        }
        [rolled_right, rolled_down, forward]
    }

    /// Which way the ship is pointing, in the camera's space.
    ///
    /// The nose is `(0, 0, 1)` in the hull's frame, so this is the third column
    /// of [`Self::basis`] and nothing more. Two things want it: the sky, which
    /// streams the other way along it, and the bubble, which is drawn out along
    /// it and has to know how much of that length the camera can still see.
    pub fn nose_in_camera(self) -> [f32; 3] {
        let b = self.basis();
        [b[0][2], b[1][2], b[2][2]]
    }

    /// Which way the sky runs, in the camera's space: the opposite of the nose,
    /// because the ship is what is moving.
    ///
    /// Exactly `(-1, 0, 0)` at [`Self::LEVEL`], which the star band was written
    /// under and took a fast path on. Nothing reads it that way any more — the
    /// sky is in the world and travels with the ship rather than against the
    /// camera — but the exactness is still the thing `Orbit::LEVEL` is for, and
    /// `the_level_orbit_is_the_quarter_turn_it_replaced` still holds it.
    pub fn sky_travel(self) -> [f32; 3] {
        let n = self.nose_in_camera();
        [-n[0], -n[1], -n[2]]
    }
}

/// The whole of where the outside camera is: which way it is pointed, and how
/// far off it is parked.
///
/// A bundle rather than two arguments threaded side by side through six
/// functions, and it is the honest unit anyway — a hull has no opinion about
/// zooms or orbits, only about where the eye is.
#[derive(Debug, Clone, Copy)]
pub struct Eye {
    /// Screen right, screen down, and the way it is looking, in the hull's own
    /// frame. From [`Orbit::basis`].
    pub basis: [[f32; 3]; 3],
    /// Range to the ship. From [`ship_distance`].
    pub distance: f32,
}

impl Eye {
    pub fn new(orbit: Orbit, zoom: f32) -> Self {
        Self {
            basis: orbit.held().basis(),
            distance: ship_distance(zoom),
        }
    }

    /// Turn a hull-space point into the camera's space.
    ///
    /// Three dot products and a standoff. The rotation is about the *ship's own
    /// centre*, so the range comes out in `[distance - HULL_REACH, distance +
    /// HULL_REACH]` whatever the orbit is doing — which is exactly how the
    /// `const` assertion beside the hull is worded, in [`crate::models`], and
    /// why swinging the camera all the way round does not disturb it. That used
    /// to be a pair, one at each end of the sandwich; the other answered to the
    /// star band's near wall and went with the band.
    ///
    /// At [`Orbit::LEVEL`] this is `(x, y, z) → (z, y, distance - x)`, to the
    /// bit. See [`Orbit::basis`] for why that matters more than it looks like
    /// it should.
    pub fn to_camera(&self, v: [f32; 3]) -> [f32; 3] {
        let [right, down, forward] = self.basis;
        [
            v[0] * right[0] + v[1] * right[1] + v[2] * right[2],
            v[0] * down[0] + v[1] * down[1] + v[2] * down[2],
            v[0] * forward[0] + v[1] * forward[1] + v[2] * forward[2] + self.distance,
        ]
    }
}

/// Which camera the flight is being flown behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// From the pilot's seat, looking down the nose. The original view, and
    /// the one everything that is not interactive still flies in.
    #[default]
    Cockpit,
    /// Off the ship's starboard beam: the hull in profile with the sky
    /// streaming astern, and — once the drive is lit — bending around it.
    Side,
}

impl ViewMode {
    /// Every view, in the order `C` walks them.
    pub const ALL: [ViewMode; 2] = [ViewMode::Cockpit, ViewMode::Side];

    /// The next one round. There are two today; the cycle is written so a
    /// third costs one line.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// A view's name, spelled once so everything that has to say one agrees.
    ///
    /// The panel does not print it — it gates on the variant, since the two
    /// views differ in what they draw rather than in what they are called —
    /// and `--view` parses into [`crate::cli::ViewArg`], whose spellings clap
    /// derives from its own variant names. That derivation is what makes this
    /// worth keeping rather than a name nothing reads: the two enums have to
    /// stay in step, and `the_command_line_takes_every_view_by_its_name` in
    /// `cli.rs` walks [`Self::ALL`] to say they do.
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Cockpit => "cockpit",
            ViewMode::Side => "side",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn the_cycle_comes_all_the_way_round() {
        let mut view = ViewMode::default();
        assert_eq!(view, ViewMode::Cockpit, "a flight starts in the cockpit");
        for _ in 0..ViewMode::ALL.len() {
            view = view.next();
        }
        assert_eq!(view, ViewMode::Cockpit, "pressing C enough times gets home");
        // And every view is reachable, or one of them is unflyable.
        let mut seen = vec![ViewMode::Cockpit];
        for _ in 1..ViewMode::ALL.len() {
            seen.push(seen[seen.len() - 1].next());
        }
        for view in ViewMode::ALL {
            assert!(seen.contains(&view), "{view:?} is not on the cycle");
        }
    }

    #[test]
    fn every_view_has_a_name_and_they_differ() {
        let names: Vec<&str> = ViewMode::ALL.iter().map(|v| v.label()).collect();
        assert!(names.iter().all(|n| n.is_ascii() && !n.is_empty()));
        for (i, a) in names.iter().enumerate() {
            assert!(!names[i + 1..].contains(a), "two views answer to {a}");
        }
    }

    #[test]
    fn the_ship_is_the_same_size_of_frame_on_every_terminal() {
        // The standoff is derived from the focal length rather than tuned, so
        // the hull is the same fraction of the picture on a phone-sized
        // terminal and on a wall — and the zoom scales that fraction rather
        // than replacing it, so the property holds all the way through the
        // range. Checked through the real camera, because that is the
        // arithmetic that would drift.
        use crate::render::Renderer;
        use crate::ship::Ship;
        use crate::term::ColorMode;

        for (cols, rows) in [(40, 12), (120, 36), (300, 90), (1, 1)] {
            let renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
            let cam = renderer.exterior_camera(&Ship::new(), 0.0);
            for zoom in [ZOOM_MIN, 0.8, ZOOM_DEFAULT, 1.7, ZOOM_MAX] {
                // The nose, at the end of the unit box, out along the track.
                let nose = cam
                    .project([1.0, 0.0, ship_distance(zoom)])
                    .expect("the ship is in front of its own camera");
                let half = nose.0 - cam.cx;
                let want = ship_half_on_screen(cam.height, zoom);
                assert!(
                    (half - want).abs() < 1e-3,
                    "at {cols}x{rows} zoomed {zoom} the ship is {half} rather than {want}"
                );
            }
        }
    }

    #[test]
    fn the_two_ways_of_asking_how_big_the_ship_is_agree() {
        // One is a distance to put the hull at and the other is the size it
        // comes out; they are the same sentence read from either end, and the
        // renderer uses both in the same frame — the standoff to place the
        // hull, the half-length to size the bubble round it. If they drifted
        // apart the bubble would sit off the ship it belongs to.
        for height in [2.0f32, 24.0, 72.0, 180.0] {
            let focal = height * SIDE_FOCAL;
            for zoom in [ZOOM_MIN, 0.75, ZOOM_DEFAULT, 2.0, ZOOM_MAX] {
                let from_distance = focal / ship_distance(zoom);
                let direct = ship_half_on_screen(height, zoom);
                assert!(
                    (from_distance - direct).abs() < 1e-3 * direct.max(1.0),
                    "at height {height} zoomed {zoom}: {from_distance} against {direct}"
                );
            }
        }
    }

    #[test]
    fn the_bubble_is_the_same_number_of_ships_across_at_every_zoom() {
        // What the lens's own constant used to claim in prose and could not
        // check: the Einstein radius is sized against the hull, so the bubble
        // has to come and go on screen exactly as the ship does. Measured
        // through both real arithmetics — the hull's nose through `project`,
        // the radius through `Lens::for_warp` — because it is the drift
        // *between* them that this is here to catch, and a comment tying two
        // constants together is precisely what drifted last time.
        use crate::lens::Lens;
        use crate::render::Renderer;
        use crate::ship::Ship;
        use crate::term::ColorMode;

        for (cols, rows) in [(40, 12), (120, 36), (300, 90)] {
            let renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
            let cam = renderer.exterior_camera(&Ship::new(), 0.0);
            let mut first = None;
            for zoom in [ZOOM_MIN, 0.9, ZOOM_DEFAULT, 1.6, ZOOM_MAX] {
                let nose = cam
                    .project([1.0, 0.0, ship_distance(zoom)])
                    .expect("the ship is in front of its own camera");
                let half = nose.0 - cam.cx;
                let lens = Lens::for_warp(
                    (cam.cx, cam.cy),
                    1.0,
                    ship_half_on_screen(cam.height, zoom),
                    Orbit::LEVEL.nose_in_camera(),
                );
                let ships_across = lens.radius / half;
                let want = *first.get_or_insert(ships_across);
                assert!(
                    (ships_across - want).abs() < 1e-3,
                    "at {cols}x{rows} the bubble is {ships_across} ships across \
                     zoomed {zoom}, against {want} at the other zooms"
                );
                // And it really does hold the ship, at every one of them.
                // Asked of the bubble rather than of its radius, because the
                // bubble is elongated and seated astern of the hull now, and a
                // scalar could not see either.
                assert!(
                    lens.shadowed(nose),
                    "the hull is poking out of its own bubble at zoom {zoom}"
                );
            }
        }
    }

    #[test]
    fn the_level_orbit_is_the_quarter_turn_it_replaced() {
        // The whole of why the reference frames did not move. `Eye::to_camera`
        // is three dot products where this used to be a hand-written swizzle,
        // and at the shot the flight opens on it has to be that swizzle to the
        // bit — not to a tolerance. Every sine in the basis is an exact zero
        // and every cosine an exact one, so every product has an exact zero or
        // an exact one in it and nothing rounds.
        //
        // Flown over the real vertex data rather than over a few made-up
        // points, because a hull is where a stray `-0.0` would come from, and
        // over every zoom because the standoff is the one term that is not a
        // literal.
        for zoom in [ZOOM_MIN, 0.77, ZOOM_DEFAULT, 1.9, ZOOM_MAX] {
            let eye = Eye::new(Orbit::LEVEL, zoom);
            let distance = ship_distance(zoom);
            for model in crate::models::models() {
                for v in &model.verts {
                    let got = eye.to_camera(*v);
                    let want = [v[2], v[1], distance - v[0]];
                    assert_eq!(
                        got, want,
                        "{} at zoom {zoom}: {v:?} placed at {got:?} rather than {want:?}",
                        model.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_camera_goes_all_the_way_round_on_every_axis() {
        // All three angles wrap and none of them stops. The elevation used to
        // clamp at the quarter turn, on the argument that a turn of azimuth
        // already reaches everything past it — which is a fact about the
        // reachable set and not about the control, and the control is what a
        // hand on the key is using. Holding `W` now carries the camera over the
        // top rather than parking it there.
        for n in [-1000, -7, 0, 7, 1000] {
            let o = Orbit {
                azimuth: n as f32 * 0.7,
                elevation: n as f32 * 0.5,
                roll: n as f32 * -0.9,
            }
            .held();
            for (name, angle) in [
                ("azimuth", o.azimuth),
                ("elevation", o.elevation),
                ("roll", o.roll),
            ] {
                assert!(
                    angle.abs() <= PI,
                    "a camera swung {n} steps kept {angle} on the {name}"
                );
            }
        }

        // Over the top is a *turn*, not a stop and not a jump: the basis is
        // periodic in the elevation, so a quarter turn past the pole and the
        // same angle short of it differ by a rotation rather than by a seam.
        // Stated as the thing that would break — a discontinuity here would
        // read as the sky tearing as the camera crossed the top.
        for step in 0..24 {
            let e = step as f32 * TAU / 24.0;
            let here = Orbit {
                azimuth: 0.3,
                elevation: e,
                roll: 0.0,
            }
            .held()
            .basis();
            let round = Orbit {
                azimuth: 0.3,
                elevation: e + TAU,
                roll: 0.0,
            }
            .held()
            .basis();
            for (a, b) in here.iter().flatten().zip(round.iter().flatten()) {
                assert!(
                    (a - b).abs() < 1e-5,
                    "a full turn of elevation did not come back: {here:?} against {round:?}"
                );
            }
        }
    }

    #[test]
    fn an_orbit_outside_the_range_is_held_rather_than_believed() {
        // The clamp lives with the geometry rather than at the keyboard, for
        // the same reason the zoom's does. A NaN is sent home outright rather
        // than through `clamp`, which passes one straight out the other side —
        // and a NaN angle is not a frame drawn slightly wrong, it is a frame
        // with no sky in it at all.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for which in 0..3 {
                let mut o = Orbit::LEVEL;
                [&mut o.azimuth, &mut o.elevation, &mut o.roll][which].clone_from(&bad);
                assert!(o.held().is_level(), "a {bad} on axis {which} was believed");
            }
        }
        // And every angle it does accept produces a basis worth projecting
        // through: orthonormal, right-handed, and finite.
        //
        // Swept past the quarter turn on the elevation as well, which is newly
        // reachable and is the reason this matters more than it reads.
        // `models::plates` decides which way a plate faces from the *sign* of
        // its projected area, so a basis that went left-handed anywhere in here
        // would invert every facing test in the hangar and go on looking lit.
        for az in [-3.0f32, -0.4, 0.0, 1.2, 3.0] {
            for el in [-3.0f32, -FRAC_PI_2, -0.3, 0.0, 1.1, FRAC_PI_2, 2.4, 3.0] {
                for roll in [-3.0f32, 0.0, 2.5] {
                    let b = Orbit {
                        azimuth: az,
                        elevation: el,
                        roll,
                    }
                    .held()
                    .basis();
                    let dot = |i: usize, j: usize| {
                        b[i][0] * b[j][0] + b[i][1] * b[j][1] + b[i][2] * b[j][2]
                    };
                    for i in 0..3 {
                        assert!((dot(i, i) - 1.0).abs() < 1e-5, "{b:?} is not unit");
                        for j in i + 1..3 {
                            assert!(dot(i, j).abs() < 1e-5, "{b:?} is not square");
                        }
                    }
                    // Right-handed: screen right crossed with screen down is
                    // the way the camera is looking. The mirror image of this
                    // looks almost identical and inverts every facing test in
                    // `models::plates`.
                    let cross = [
                        b[0][1] * b[1][2] - b[0][2] * b[1][1],
                        b[0][2] * b[1][0] - b[0][0] * b[1][2],
                        b[0][0] * b[1][1] - b[0][1] * b[1][0],
                    ];
                    for k in 0..3 {
                        assert!(
                            (cross[k] - b[2][k]).abs() < 1e-5,
                            "the camera basis has been mirrored: {b:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn elevating_the_camera_leaves_the_ship_the_same_size() {
        // Lifting the camera over the hull turns it about the ship's own
        // centre, and the standoff is the radius of that turn — so the range
        // does not change and neither does how big the ship comes out. This is
        // the claim the bubble's sizing rests on: `ship_half_on_screen` reads
        // the zoom and nothing else, and if an orbit could resize the hull it
        // would have to read that too.
        //
        // It also guards against the obvious wrong implementation, which is to
        // slide the eye rather than swing it.
        use crate::render::Renderer;
        use crate::ship::Ship;
        use crate::term::ColorMode;

        let renderer = Renderer::new(120, 36, ColorMode::Truecolor, 1.9);
        let cam = renderer.exterior_camera(&Ship::new(), 0.0);
        for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
            let want = ship_half_on_screen(cam.height, zoom);
            for elevation in [-3.0, -FRAC_PI_2, -0.9, 0.0, 0.4, FRAC_PI_2, 2.2, 3.0] {
                let orbit = Orbit {
                    azimuth: 0.0,
                    elevation,
                    roll: 0.0,
                };
                let eye = Eye::new(orbit, zoom);
                let nose = cam
                    .project(eye.to_camera([0.0, 0.0, 1.0]))
                    .expect("the ship is in front of its own camera");
                let half = ((nose.0 - cam.cx).powi(2) + (nose.1 - cam.cy).powi(2)).sqrt();
                assert!(
                    (half - want).abs() < 1e-3,
                    "elevated {elevation} the ship is {half} rather than {want}"
                );
            }
        }
    }

    #[test]
    fn the_sky_runs_the_way_the_ship_is_pointing() {
        // The travel direction is the nose reversed, and abeam it is exactly
        // `(-1, 0, 0)` — which is not merely close to what the star band was
        // written against, it *is* what the star band was written against, and
        // the fast path there tests it for equality.
        assert_eq!(Orbit::LEVEL.sky_travel()[0], -1.0);
        for elevation in [-FRAC_PI_2, -0.5, 0.0, 0.5, FRAC_PI_2] {
            for roll in [-2.0f32, 0.0, 2.0] {
                let travel = Orbit {
                    azimuth: 0.0,
                    elevation,
                    roll,
                }
                .sky_travel();
                assert_eq!(
                    travel[2], 0.0,
                    "the sky gained depth from a camera square to the track"
                );
            }
        }
        // Swung round toward the nose it does gain depth, which is the whole
        // reason the band had to learn to move in three dimensions.
        let ahead = Orbit {
            azimuth: 1.2,
            elevation: 0.0,
            roll: 0.0,
        }
        .sky_travel();
        assert!(
            ahead[2].abs() > 0.5,
            "swinging the camera ahead left the sky running flat: {ahead:?}"
        );
    }

    #[test]
    fn a_zoom_outside_the_range_is_held_rather_than_believed() {
        // The clamp lives here rather than only at the keyboard, because the
        // near planes either side of the hull are what it is protecting and
        // they do not care where a number came from.
        for zoom in [f32::NAN, -1.0, 0.0, 1e30, f32::INFINITY, f32::NEG_INFINITY] {
            let d = ship_distance(zoom);
            assert!(
                (MIN_SHIP_DISTANCE..=MAX_SHIP_DISTANCE).contains(&d),
                "a zoom of {zoom} put the ship at {d}"
            );
        }
        assert_eq!(ship_distance(ZOOM_MIN), MAX_SHIP_DISTANCE);
        assert_eq!(ship_distance(ZOOM_MAX), MIN_SHIP_DISTANCE);
        // Zooming in brings the ship closer and makes it bigger. Stated because
        // the two run opposite ways and a sign slip would read as sensible.
        assert!(ship_half_on_screen(72.0, ZOOM_MIN) < ship_half_on_screen(72.0, ZOOM_MAX));
    }
}
