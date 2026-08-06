//! Which camera the flight is being flown behind, and where the outside one
//! is parked.
//!
//! A leaf module on purpose: the flight loop, the renderer, the panel and the
//! command line all need to name a view, and none of them should have to reach
//! through another to do it.

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
/// Both ends are held by a near plane rather than by taste, and both are
/// checked where the plane they answer to is declared: pulling back too far
/// would let a star pass between the eye and the hull, which is the whole of
/// the exterior view's depth sorting ([`crate::exterior`]), and pushing in too
/// far would put a rolled fin through the projection's own near plane and drop
/// the plate ([`crate::models`]). The numbers here leave both with room.
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
/// Zooming is a dolly and not a change of lens: the star band is laid out
/// against `Camera::focal` and cached against it, so a zoom that touched the
/// focal length would have to re-fold the whole field every notch — and
/// re-folding drops every trail. Moving the ship instead leaves the sky exactly
/// where it was, which is also the honest picture, since the band is out at
/// eighteen units and up and a few units of dolly is no parallax at all.
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
                let lens =
                    Lens::for_warp((cam.cx, cam.cy), 1.0, ship_half_on_screen(cam.height, zoom));
                let ships_across = lens.radius / half;
                let want = *first.get_or_insert(ships_across);
                assert!(
                    (ships_across - want).abs() < 1e-3,
                    "at {cols}x{rows} the bubble is {ships_across} ships across \
                     zoomed {zoom}, against {want} at the other zooms"
                );
                // And it really does hold the ship, at every one of them.
                assert!(
                    lens.shadow() > half,
                    "the hull is poking out of its own bubble at zoom {zoom}"
                );
            }
        }
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
