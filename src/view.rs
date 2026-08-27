//! Which camera the flight is being flown behind, and where the outside one is
//! parked.

use crate::ship::wrap_signed;
use std::f32::consts::{PI, TAU};

/// Focal length of the side camera, as a multiple of the canvas height. Longer
/// than the cockpit's 0.85: a long lens flattens the hull into a profile, which
/// is the shot, where a wide one would splay it into a three-quarter view.
pub const SIDE_FOCAL: f32 = 1.15;
/// Half the ship's length on screen at [`ZOOM_DEFAULT`], as a fraction of the
/// canvas height.
pub const SHIP_SCREEN_FRAC: f32 = 0.125;
/// The furthest any part of a hull reaches from the ship's own centre. Hulls
/// are modelled inside the unit box, so this is the box's diagonal, rounded up.
pub const HULL_REACH: f32 = 1.8;

/// How far in and out the side camera may be pushed, as a multiple of the
/// default framing.
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
pub const ORBIT_EASE: f32 = 9.0;

/// An angle folded onto a single turn, and left exactly alone if it is already
/// on one.
fn turn_of(angle: f32) -> f32 {
    if (-PI..PI).contains(&angle) {
        angle
    } else {
        wrap_signed(angle)
    }
}

/// Where the camera outside is pointed, as three angles about the ship.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Orbit {
    /// Round the ship, about its own down axis. Zero is dead abeam to starboard
    /// — the shot as it opens — and positive walks the camera forward toward
    /// the nose, so at a quarter turn it is ahead looking back down the throat
    /// and at three-quarters it is astern in the wake. Wraps.
    pub azimuth: f32,
    /// Up over the hull and down under it, about the track. Zero is level with
    /// it, positive lifts the camera, and a quarter turn is the plan view.
    /// Wraps, so half a turn is the view from the far beam, inverted.
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
    #[cfg(test)]
    pub fn is_level(self) -> bool {
        self == Self::LEVEL
    }

    /// This orbit, folded onto a single turn on all three axes, and sent home
    /// outright if any of them is not a number.
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
    pub fn nose_in_camera(self) -> [f32; 3] {
        let b = self.basis();
        [b[0][2], b[1][2], b[2][2]]
    }

    /// Which way the sky runs, in the camera's space: the opposite of the nose,
    /// because the ship is what is moving.
    pub fn sky_travel(self) -> [f32; 3] {
        let n = self.nose_in_camera();
        [-n[0], -n[1], -n[2]]
    }
}

/// The whole of where the outside camera is: which way it is pointed, and how
/// far off it is parked.
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
    /// From the pilot's seat, looking down the nose. The original view, and the
    /// one everything that is not interactive still flies in.
    #[default]
    Cockpit,
    /// Off the ship's starboard beam: the hull in profile with the sky
    /// streaming astern, and — once the drive is lit — bending around it.
    Side,
}

impl ViewMode {
    /// Every view, in the order `C` walks them.
    pub const ALL: [ViewMode; 2] = [ViewMode::Cockpit, ViewMode::Side];

    /// The next one round. There are two today; the cycle is written so a third
    /// costs one line.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// A view's name, spelled once so everything that has to say one agrees.
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
        // range.
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
        // hull, the half-length to size the bubble round it.
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
        // has to come and go on screen exactly as the ship does.
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
                assert!(
                    lens.shadowed(nose),
                    "the hull is poking out of its own bubble at zoom {zoom}"
                );
            }
        }
    }

    #[test]
    fn the_level_orbit_is_the_quarter_turn_it_replaced() {
        // The whole of why the reference frames did not move.
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
        // All three angles wrap and none of them stops.
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
        // the same reason the zoom's does.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for which in 0..3 {
                let mut o = Orbit::LEVEL;
                [&mut o.azimuth, &mut o.elevation, &mut o.roll][which].clone_from(&bad);
                assert!(o.held().is_level(), "a {bad} on axis {which} was believed");
            }
        }
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
                    // the way the camera is looking.
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
        // does not change and neither does how big the ship comes out.
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
        // Zooming in brings the ship closer and makes it bigger.
        assert!(ship_half_on_screen(72.0, ZOOM_MIN) < ship_half_on_screen(72.0, ZOOM_MAX));
    }
}
