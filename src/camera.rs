//! The lens the sky is seen through, and the segment a star leaves on it.
//!
//! One camera serves both views and the hull as well: [`crate::universe`]
//! projects stars through it from the seat and from off the beam, and
//! [`crate::models`] projects every plate of every hull through the same one.
//! That sharing is why [`Camera::project_beyond`] exists — a near plane that is
//! right for a plate is nine tenths of a light year wrong for a star.
//!
//! Sizes are in canvas *subpixels*, never in cells, and [`Camera::focal`] is
//! derived from height alone — so the vertical field of view is fixed and a
//! wide terminal simply sees more sky either side of the nose.

/// Nearest a *hull* may be projected from.
///
/// A ship's own scale: hulls are modelled inside the unit box, so this is nine
/// tenths of one of them. It used to be the star field's near plane as well,
/// which cost nothing while the sky was measured in the same units and would
/// now throw a star away at nine tenths of a light year — see
/// [`Camera::project_beyond`], which is what lets each caller name the plane it
/// means.
pub const Z_NEAR: f32 = 0.9;

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

    /// Perspective-project a camera-space point. `None` if it is at or behind
    /// the near plane, where the projection would blow up.
    pub fn project(&self, pos: [f32; 3]) -> Option<(f32, f32)> {
        self.project_beyond(pos, Z_NEAR)
    }

    /// The same, against a near plane the caller names.
    ///
    /// [`Z_NEAR`] is a hull number — nine tenths of a unit where the ship is
    /// about one — and it is the right one for a plate, which is what
    /// [`Self::project`] is for. It is emphatically not the right one for a
    /// star: [`crate::universe`] measures its sky in light years, so a star
    /// held to the same plane would be thrown away at nine tenths of a light
    /// year for a reason that has nothing to do with it. One projection, two
    /// near planes, and each caller says which it means.
    pub fn project_beyond(&self, pos: [f32; 3], near: f32) -> Option<(f32, f32)> {
        if pos[2] <= near {
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

/// One star's contribution to a frame: a coloured segment to add to the canvas.
#[derive(Debug, Clone, Copy)]
pub struct Streak {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub color: [f32; 3],
    pub intensity: f32,
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
        for z in [1.0, 10.0, 100.0, 260.0] {
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
}
