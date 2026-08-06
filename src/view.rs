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
/// Half the ship's length on screen, as a fraction of the canvas height.
///
/// Pulled between two things. Too small and the detail goes: a saucer thin
/// enough to *be* a saucer comes out about a subpixel thick on a thirty-row
/// terminal, which is a line rather than a shape, and that is what this was
/// raised from 0.20 to fix. Too large and the ship crowds the frame it is
/// meant to be flying through, and fills the warp bubble instead of sitting
/// inside it. This is a little to the roomy side of the middle.
pub const SHIP_SCREEN_FRAC: f32 = 0.24;
/// Range from the side camera to the ship. This falls out as a constant
/// because the focal length is proportional to the canvas height, so the ship
/// is the same fraction of the frame on a phone-sized terminal and a wall.
pub const SHIP_DISTANCE: f32 = SIDE_FOCAL / SHIP_SCREEN_FRAC;
/// The furthest any part of a hull reaches from the ship's own centre. Hulls
/// are modelled inside the unit box, so this is the box's diagonal, rounded up.
pub const HULL_REACH: f32 = 1.8;

/// Which camera the flight is being flown behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// From the pilot's seat, looking down the nose. The original view, and
    /// the one everything that is not interactive still flies in.
    #[default]
    Cockpit,
    /// Off the ship's port beam: the hull in profile with the sky streaming
    /// astern, and — once the drive is lit — bending around it.
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

    /// What the command line takes and the panel prints.
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
        // terminal and on a wall. Checked through the real camera, because
        // that is the arithmetic that would drift.
        use crate::render::Renderer;
        use crate::ship::Ship;
        use crate::term::ColorMode;

        for (cols, rows) in [(40, 12), (120, 36), (300, 90), (1, 1)] {
            let renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
            let cam = renderer.exterior_camera(&Ship::new(), 0.0);
            // The nose, at the end of the unit box, out along the track.
            let nose = cam
                .project([1.0, 0.0, SHIP_DISTANCE])
                .expect("the ship is in front of its own camera");
            let half = nose.0 - cam.cx;
            assert!(
                (half - cam.height * SHIP_SCREEN_FRAC).abs() < 1e-3,
                "at {cols}x{rows} the ship is {half} of a {} frame",
                cam.height
            );
        }
    }
}
