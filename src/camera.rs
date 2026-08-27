//! The lens the sky is seen through, and the segment a star leaves on it.

/// Nearest a *hull* may be projected from.
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
    pub fn project_beyond(&self, pos: [f32; 3], near: f32) -> Option<(f32, f32)> {
        if pos[2] <= near {
            return None;
        }
        let inv = self.focal / pos[2];
        Some((self.cx + pos[0] * inv, self.cy + pos[1] * inv))
    }

    /// Where a direction vanishes: the screen point the projection of a ray
    /// pointing this way converges on as the ray runs off to infinity.
    pub fn vanishing_point(&self, dir: [f32; 3]) -> Option<(f32, f32)> {
        if dir[2] <= 0.0 {
            return None;
        }
        let (x, y) = (dir[0] / dir[2] * self.focal, dir[1] / dir[2] * self.focal);
        // The guard above lets a NaN through — every comparison against one is
        // false — and a direction lying almost exactly in the image plane
        // overflows the divide rather than answering a very distant point.
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
        // The distinction the engine lance is clamped on.
        let cam = cam();
        assert!(cam.vanishing_point([0.3, -0.2, -1.0]).is_none());
        assert!(cam.vanishing_point([0.3, -0.2, 0.0]).is_none());
        assert!(cam.vanishing_point([0.3, -0.2, 1.0]).is_some());
        assert_eq!(cam.vanishing_point([0.0, 0.0, 1.0]), Some((cam.cx, cam.cy)));

        // The two ways the arithmetic fails rather than the direction, and they
        // are caught in different places.
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
        // plume drawn along the ray closes on it and never arrives.
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
