//! Bending starlight around the ship.

/// Ceiling on how much a single image may be brightened. The magnification of a
/// point source diverges on the lens axis — a star dead behind the bubble is a
/// ring of infinite brightness — and nothing downstream survives an infinite
/// intensity, so the peak is clamped to something the tonemap can still show as
/// a highlight rather than as a hole.
const MAX_MAGNIFICATION: f32 = 8.0;

/// The range of offsets, in Einstein radii, over which the magnification is
/// worth evaluating honestly. Below the floor it has already passed
/// `MAX_MAGNIFICATION`; above the ceiling it has settled on its limit.
const U_FLOOR: f32 = 1e-3;
const U_CEILING: f32 = 1e6;

/// How many Einstein radii out the lens stops being worth evaluating. At ten
/// the deflection is a tenth of a radius and the counter-image is gone.
const REACH: f32 = 10.0;

/// The bubble's own shadow, as a fraction of the way out to the ring.
const SHADOW_FRAC: f32 = 0.72;

/// That last sentence, made a condition of the build rather than a claim in a
/// comment: the shadow has to sit inside the ring, or it starts taking primary
/// images and a streak sweeping past the bubble breaks into pieces.
const _: () = assert!(
    SHADOW_FRAC < 1.0,
    "the shadow is outside the ring, and would swallow primary images"
);

/// The Einstein radius at full warp, as a multiple of the ship's own half-
/// length on screen.
const RADIUS_IN_SHIPS: f32 = 2.0;

/// The bubble's semi-major axis, along the track, as a multiple of the Einstein
/// radius.
const RING_MAJOR: f32 = 1.4832;

/// And across it — the reciprocal, which is the whole of why this is a reshape
/// rather than an enlargement.
const RING_MINOR: f32 = 1.0 / RING_MAJOR;

/// How far astern of the ship the bubble sits, as a fraction of its own semi-
/// major axis.
const WAKE_SHIFT: f32 = 0.20;

/// At full warp the shadow has to reach past the ship's nose, or stars are
/// drawn over the hull and the swept-clear disc stops being what it is for. The
/// shadow reaches `RADIUS_IN_SHIPS · RING_MAJOR · SHADOW_FRAC` ahead of the
/// bubble's centre and the centre sits `RADIUS_IN_SHIPS · RING_MAJOR ·
/// WAKE_SHIFT` behind the ship, both in ship half-lengths, and the nose is at
/// one. Its opposite number is the runtime test over every hull in the hangar;
/// this is the arithmetic on its own, failing the build rather than a run.
const _: () = assert!(
    RADIUS_IN_SHIPS * RING_MAJOR * (SHADOW_FRAC - WAKE_SHIFT) > 1.0,
    "the ship's nose is outside its own bubble, and stars would be drawn over it"
);

/// A thin point-mass lens sitting on the canvas, drawn out along the track.
#[derive(Debug, Clone, Copy)]
pub struct Lens {
    /// Where the mass is, in canvas subpixels. Astern of the ship rather than
    /// on it — see [`Lens::for_warp`].
    pub center: (f32, f32),
    /// The Einstein radius, in subpixels: the scale the bubble is sized by and
    /// the unit the magnification is worked in. Zero is no lens at all, and is
    /// an exact identity rather than an approximation of one.
    pub radius: f32,
    /// The outline's two semi-axes in subpixels, the long one first.
    axes: (f32, f32),
    /// The reciprocals of those two, worked out once when the bubble is built.
    /// Nothing may set one without the other.
    inv_axes: (f32, f32),
    /// Which way the long axis lies on the canvas, as `(cos, sin)`.
    turn: (f32, f32),
}

/// Which of the two images a point-mass lens forms of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Image {
    /// Outside the ring, on the same side as the source. This is the bright
    /// one, and the only one at all when the lens is off.
    Primary,
    /// Inside the ring and on the opposite side. Always the fainter of the two,
    /// and it fades to nothing as the source moves away from the axis.
    Secondary,
}

/// Where a source appears, and what it does to its brightness.
#[derive(Debug, Clone, Copy)]
pub struct Lensed {
    pub at: (f32, f32),
    /// Magnification: multiply the source's intensity by this.
    pub gain: f32,
}

/// How much brighter a source at `u` Einstein radii comes out, per image.
fn magnification(u: f32, image: Image) -> f32 {
    let u = if u.is_nan() {
        U_FLOOR
    } else {
        u.clamp(U_FLOOR, U_CEILING)
    };
    let shared = ((u * u + 2.0) / (2.0 * u * (u * u + 4.0).sqrt())).min(MAX_MAGNIFICATION);
    match image {
        Image::Primary => shared + 0.5,
        Image::Secondary => (shared - 0.5).max(0.0),
    }
}

impl Lens {
    /// A lens that does nothing, and does it exactly rather than very nearly:
    /// the zero radius is what every path here checks, through [`Self::is_on`],
    /// so a sublight frame takes the same bytes it would if this module were
    /// not in the tree.
    pub const OFF: Lens = Lens {
        center: (0.0, 0.0),
        radius: 0.0,
        axes: (0.0, 0.0),
        inv_axes: (0.0, 0.0),
        turn: (1.0, 0.0),
    };

    /// The lens the drive is currently making, given the 0..=1 warp ramp and
    /// the ship's half-length on screen in subpixels. Quadratic in the ramp, so
    /// it opens up as the drive spools rather than snapping on the instant the
    /// light barrier is crossed, and it is exactly zero at sublight.
    pub fn for_warp(center: (f32, f32), warp: f32, ship_half: f32, nose: [f32; 3]) -> Self {
        let warp = warp.clamp(0.0, 1.0);
        let radius = ship_half.max(0.0) * RADIUS_IN_SHIPS * warp * warp;

        // How much of the track's length the camera can still see, and which
        // way that length lies on the canvas.
        let sin_sq = nose[0] * nose[0] + nose[1] * nose[1];
        let sin_phi = sin_sq.sqrt();
        // End-on there is no direction to be found and no need of one: the
        // outline is a circle, so any fixed axis will do and this picks the one
        // every other case reduces to.
        let turn = if sin_phi > f32::MIN_POSITIVE {
            (nose[0] / sin_phi, nose[1] / sin_phi)
        } else {
            (1.0, 0.0)
        };

        let across = radius * RING_MINOR;
        let along = if nose[2] == 0.0 {
            radius * RING_MAJOR
        } else {
            let cos_sq = nose[2] * nose[2];
            radius * (RING_MAJOR * RING_MAJOR * sin_sq + RING_MINOR * RING_MINOR * cos_sq).sqrt()
        };

        // The wake is a displacement along the track in three dimensions, so it
        // foreshortens with the track rather than with the outline it is a
        // fraction of — `along` has a floor at `W` and this does not.
        let wake = radius * RING_MAJOR * WAKE_SHIFT * sin_phi;
        Self {
            center: (center.0 - wake * turn.0, center.1 - wake * turn.1),
            radius,
            axes: (along, across),
            inv_axes: (1.0 / along, 1.0 / across),
            turn,
        }
    }

    /// Whether this lens bends anything.
    pub fn is_on(&self) -> bool {
        self.radius > 0.0 && self.radius.is_finite()
    }

    /// The ring's two semi-axes in subpixels: along the track first, across it
    /// second.
    pub fn semi_axes(&self) -> (f32, f32) {
        self.axes
    }

    /// Where `p` sits in the frame the bubble is round in: turned onto the
    /// outline's own axes, then divided by them.
    fn offsets(&self, p: (f32, f32)) -> (f32, f32) {
        let (dx, dy) = (p.0 - self.center.0, p.1 - self.center.1);
        let (cos, sin) = self.turn;
        let (along, across) = if sin == 0.0 {
            (dx * cos, dy * cos)
        } else {
            (dx * cos + dy * sin, dy * cos - dx * sin)
        };
        (along * self.inv_axes.0, across * self.inv_axes.1)
    }

    /// Which way the long axis lies on the canvas, as `(cos, sin)`. What the
    /// wash inside the bubble has to be turned by, since the hole it sits in
    /// stops being square to the frame the moment the camera is swung both
    /// round the ship and over it.
    pub fn turn(&self) -> (f32, f32) {
        self.turn
    }

    /// The same two, for the shadow. What the wash inside the bubble is drawn
    /// to, since a round wash in an elongated hole gives the shape away.
    pub fn shadow_axes(&self) -> (f32, f32) {
        let (a, b) = self.semi_axes();
        (a * SHADOW_FRAC, b * SHADOW_FRAC)
    }

    /// How far `p` sits from the centre, in Einstein radii — exactly 1.0 on the
    /// ring, whichever way round the bubble it is measured.
    pub fn offset(&self, p: (f32, f32)) -> f32 {
        if !self.is_on() {
            return f32::INFINITY;
        }
        self.offset_sq(p).sqrt()
    }

    /// The square of it, which is what the gates actually want. Being an
    /// ellipse rather than an angle-dependent radius is what makes this
    /// possible: membership of an ellipse is a closed form, so the three
    /// hottest questions in the module lost the square root they would
    /// otherwise have grown.
    fn offset_sq(&self, p: (f32, f32)) -> f32 {
        let (ex, ey) = self.offsets(p);
        ex * ex + ey * ey
    }

    /// Whether a point is one the bubble has swallowed.
    pub fn shadowed(&self, p: (f32, f32)) -> bool {
        self.is_on() && self.offset_sq(p) < SHADOW_FRAC * SHADOW_FRAC
    }

    /// Whether a segment passes close enough for the bend to be worth the work
    /// of chopping it up and curving it.
    pub fn bends(&self, points: &[crate::canvas::Trace]) -> bool {
        if !self.is_on() {
            return false;
        }
        let reach = REACH * REACH;
        points.iter().any(|p| self.offset_sq((p.0, p.1)) <= reach)
    }

    /// Whether the straight line between two points passes inside the ring.
    /// Point-to-segment distance, worked on the offsets rather than the points,
    /// and no trigonometry: this is asked once per sample of every streak the
    /// lens reaches. Straight lines survive the scaling, so the nearest point
    /// of the scaled segment is the nearest point of the real one.
    fn crosses_the_ring(&self, from: (f32, f32), to: (f32, f32)) -> bool {
        let (ax, ay) = self.offsets(from);
        let (bx, by) = self.offsets(to);
        let (dx, dy) = (bx - ax, by - ay);
        let len_sq = dx * dx + dy * dy;
        // Where along the segment the nearest point to the centre falls, held
        // to the segment itself rather than the whole line it lies on.
        let t = if len_sq > f32::MIN_POSITIVE {
            (-(ax * dx + ay * dy) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (nx, ny) = (ax + dx * t, ay + dy * t);
        nx * nx + ny * ny < 1.0
    }

    /// How sharply the lens is bending things at `p`, as a 0..=1 ramp.
    pub fn curvature(&self, p: (f32, f32)) -> f32 {
        if !self.is_on() {
            return 0.0;
        }
        let m_sq = self.offset_sq(p);
        if !m_sq.is_finite() {
            return 0.0;
        }
        1.0 / (1.0 + m_sq)
    }

    /// Where `p` appears once the lens has had it, and by how much its
    /// brightness changes.
    pub fn map(&self, p: (f32, f32), image: Image) -> Lensed {
        let unbent = Lensed { at: p, gain: 1.0 };
        if !self.is_on() || !p.0.is_finite() || !p.1.is_finite() {
            // The secondary image of a source no lens is bending does not
            // exist, rather than existing on top of the primary.
            return match image {
                Image::Primary => unbent,
                Image::Secondary => Lensed { at: p, gain: 0.0 },
            };
        }

        let (dx, dy) = (p.0 - self.center.0, p.1 - self.center.1);
        let (ex, ey) = self.offsets(p);
        let m = crate::canvas::length_of(ex, ey);

        // On the axis the source images as the complete ring, which a single
        // point cannot be drawn as.
        if m.is_nan() || m <= f32::EPSILON {
            let along = match image {
                Image::Primary => self.axes.0,
                Image::Secondary => -self.axes.0,
            };
            return Lensed {
                at: (
                    self.center.0 + along * self.turn.0,
                    self.center.1 + along * self.turn.1,
                ),
                gain: magnification(U_FLOOR, image),
            };
        }

        // `√(m² + 4)/m`, factored rather than squared so it survives the range
        // a projection can produce: a star a hair past the near plane lands
        // billions of subpixels off the canvas, and squaring that overflows an
        // `f32` into an infinity that would then propagate into the position.
        let root = (1.0 + 4.0 / (m * m)).sqrt();
        let scale = match image {
            Image::Primary => (1.0 + root) * 0.5,
            // `θ₋` is `(m − √(m² + 4))/2`, which is two nearly equal numbers
            // subtracted once `m` is large.
            Image::Secondary => -2.0 / (m * m * (1.0 + root)),
        };

        Lensed {
            at: (self.center.0 + dx * scale, self.center.1 + dy * scale),
            gain: magnification(m, image),
        }
    }
}

/// Largest angular step, in radians, allowed between two points of a bent path
/// before the arc between them is filled in. Measured about the round frame
/// rather than the canvas, so a step is the same fraction of the way round the
/// ring at the waist as out along the wake.
const MAX_ARC_STEP: f32 = 0.25;
/// Ceiling on that filling-in, so a source sweeping right past the axis cannot
/// ask for an unbounded number of points.
const MAX_ARC_FILL: usize = 24;

impl Lens {
    /// Append the arc from `from` to `to`, as seen about the lens, to `out`.
    /// `from` is assumed to be there already; `to` always ends up there.
    pub fn arc_to(
        &self,
        from: crate::canvas::Trace,
        to: crate::canvas::Trace,
        out: &mut Vec<crate::canvas::Trace>,
    ) {
        // The expensive part of this is two `atan2`s and a `sin_cos` per point,
        // and the great majority of pairs do not need it: two samples of a
        // streak that is merely passing by are a fraction of a radian apart and
        // a straight line between them is the arc to well under a subpixel.
        if !self.crosses_the_ring((from.0, from.1), (to.0, to.1)) {
            out.push(to);
            return;
        }
        let (a, b) = self.semi_axes();
        let polar = |p: (f32, f32)| {
            let (ex, ey) = self.offsets(p);
            (crate::canvas::length_of(ex, ey), ey.atan2(ex))
        };
        let (r0, th0) = polar((from.0, from.1));
        let (r1, th1) = polar((to.0, to.1));
        // The short way round: the image sweeps, it does not jump.
        let mut sweep = th1 - th0;
        while sweep > std::f32::consts::PI {
            sweep -= std::f32::consts::TAU;
        }
        while sweep < -std::f32::consts::PI {
            sweep += std::f32::consts::TAU;
        }

        let steps = if sweep.is_finite() {
            ((sweep.abs() / MAX_ARC_STEP).ceil() as usize).clamp(1, MAX_ARC_FILL)
        } else {
            1
        };
        for i in 1..steps {
            let s = i as f32 / steps as f32;
            let (r, th) = (r0 + (r1 - r0) * s, th0 + sweep * s);
            let (along, across) = (a * r * th.cos(), b * r * th.sin());
            // The pace is carried across the points the sweep fills in: a bend
            // moves where a star's light lands, never how fast it got there.
            out.push((
                self.center.0 + along * self.turn.0 - across * self.turn.1,
                self.center.1 + across * self.turn.0 + along * self.turn.1,
                from.2,
            ));
        }
        out.push(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nose as [`crate::view::Orbit::LEVEL`] hands it over: the camera
    /// abeam of the track, which is the shot every one of these was written
    /// against.
    const ABEAM: [f32; 3] = [1.0, 0.0, 0.0];

    fn lens() -> Lens {
        Lens::for_warp((100.0, 50.0), 1.0, 10.0, ABEAM)
    }

    /// A lens assembled by hand rather than by [`Lens::for_warp`], so a test
    /// can hand this module a centre and a radius it would never produce
    /// itself. The outline is the level one, which is what every caller that
    /// does go through `for_warp` gets abeam.
    fn raw(center: (f32, f32), radius: f32) -> Lens {
        Lens {
            center,
            radius,
            axes: (radius * RING_MAJOR, radius * RING_MINOR),
            inv_axes: (1.0 / (radius * RING_MAJOR), 1.0 / (radius * RING_MINOR)),
            turn: (1.0, 0.0),
        }
    }

    /// The noses this module's properties are asked at, which is more than the
    /// one they were all written against.
    fn noses() -> impl Iterator<Item = [f32; 3]> {
        [
            [1.0, 0.0, 0.0],
            [0.94, 0.0, -0.34],
            [0.5, 0.0, -0.87],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
            [0.61, 0.5, -0.61],
            [-0.61, -0.5, -0.61],
        ]
        .into_iter()
    }

    /// A ring's worth of directions to sweep a property over, so nothing is
    /// asserted about the long axis that is not also asserted about the waist.
    /// The bubble is not round any more, and a test that only ever probes along
    /// `+x` would say nothing at all about the shape.
    fn directions() -> impl Iterator<Item = (f32, f32)> {
        (0..24).map(|i| {
            let th = i as f32 * std::f32::consts::TAU / 24.0;
            (th.cos(), th.sin())
        })
    }

    /// The point `m` Einstein radii from the centre along `(ux, uy)`, where the
    /// direction is read in the *bubble's* own frame.
    fn at(lens: &Lens, dir: (f32, f32), m: f32) -> (f32, f32) {
        let (a, b) = lens.semi_axes();
        let scale = m / (dir.0 / a).hypot(dir.1 / b);
        let (along, across) = (dir.0 * scale, dir.1 * scale);
        let (cos, sin) = lens.turn();
        (
            lens.center.0 + along * cos - across * sin,
            lens.center.1 + along * sin + across * cos,
        )
    }

    #[test]
    fn a_lens_that_is_off_is_an_exact_identity() {
        // Not approximately: the side view at sublight draws the same sky it
        // would with no lens code at all, so this must not be a near-miss that
        // shifts every star by an ulp.
        for p in [(0.0, 0.0), (37.5, -12.25), (1e6, 4.0)] {
            let got = Lens::OFF.map(p, Image::Primary);
            assert_eq!(got.at, p);
            assert_eq!(got.gain, 1.0);
            assert_eq!(Lens::OFF.map(p, Image::Secondary).gain, 0.0);
        }
    }

    #[test]
    fn the_sky_is_pushed_outside_the_einstein_ring() {
        // The primary image never lands inside the ring, which is what leaves
        // the bubble around the ship swept clear of stars.
        for nose in noses() {
            let lens = Lens::for_warp((100.0, 50.0), 1.0, 10.0, nose);
            for dir in directions() {
                for i in 0..80 {
                    let m = i as f32 * 0.1;
                    let p = at(&lens, dir, m);
                    let out = lens.map(p, Image::Primary);
                    let imaged = lens.offset(out.at);
                    assert!(
                        imaged >= 1.0 - 1e-3,
                        "at nose {nose:?}, a source {m} rings out along {dir:?} imaged at {imaged}"
                    );
                    assert!(
                        imaged >= m - 1e-3,
                        "at nose {nose:?}, {m} imaged at {imaged}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_counter_image_is_inside_the_ring_and_on_the_far_side() {
        // At every nose, for the reason the test above sweeps them: abeam is
        // the one angle whose arithmetic skips the rotation.
        for nose in noses() {
            let lens = Lens::for_warp((100.0, 50.0), 1.0, 10.0, nose);
            for dir in directions() {
                for m in [0.4, 1.0, 2.5, 7.0] {
                    let p = at(&lens, dir, m);
                    let inner = lens.map(p, Image::Secondary);
                    assert!(
                        lens.offset(inner.at) <= 1.0 + 1e-3,
                        "at nose {nose:?} the counter-image escaped the ring: {:?}",
                        inner.at
                    );
                    // Opposite side means the two offsets point against each
                    // other.
                    let (dx, dy) = (p.0 - lens.center.0, p.1 - lens.center.1);
                    let (ix, iy) = (inner.at.0 - lens.center.0, inner.at.1 - lens.center.1);
                    assert!(
                        ix * dx + iy * dy < 0.0,
                        "at nose {nose:?}, {dir:?} at {m} imaged to {ix},{iy}"
                    );
                }
            }
        }
    }

    #[test]
    fn both_images_converge_on_the_ring_for_a_source_on_the_axis() {
        // A source dead behind the lens is the ring itself, so both images sit
        // on it — from opposite directions.
        let lens = lens();
        let p = lens.center;
        for image in [Image::Primary, Image::Secondary] {
            let out = lens.map(p, image);
            assert!(
                (lens.offset(out.at) - 1.0).abs() < 1e-3,
                "{image:?} landed at {:?}",
                out.at
            );
            assert!(out.gain.is_finite(), "{image:?} came out infinitely bright");
        }
    }

    #[test]
    fn magnification_peaks_near_the_ring_and_settles_to_nothing_far_out() {
        let lens = lens();
        let total = |dir: (f32, f32), m: f32| {
            let p = at(&lens, dir, m);
            lens.map(p, Image::Primary).gain + lens.map(p, Image::Secondary).gain
        };
        // A lens conserves surface brightness and adds an image, so the pair is
        // never dimmer than the source was — and the brightening depends only
        // on how many rings out the source is, never on which way round.
        for dir in directions() {
            for i in 0..160 {
                let m = i as f32 * 0.1875;
                let got = total(dir, m);
                assert!(got >= 1.0 - 1e-4, "{m} rings out came out dimmer: {got}");
                assert!(
                    (got - total((1.0, 0.0), m)).abs() < 1e-3,
                    "the shape leaked into the brightness at {m} rings along {dir:?}"
                );
            }
        }
        // The textbook value on the ring, and effectively nothing at 20 radii.
        assert!(
            (total((1.0, 0.0), 1.0) - 1.34).abs() < 0.01,
            "{}",
            total((1.0, 0.0), 1.0)
        );
        assert!(total((1.0, 0.0), 20.0) < 1.01);
        assert!(total((1.0, 0.0), 0.05) > total((1.0, 0.0), 1.0));
    }

    #[test]
    fn the_primary_image_fades_back_into_the_unbent_sky() {
        // Far from the lens the sky must look untouched, or the whole frame
        // reads as though it is being pulled about.
        let lens = lens();
        for dir in directions() {
            let p = at(&lens, dir, 200.0);
            let out = lens.map(p, Image::Primary);
            assert!(
                (out.at.0 - p.0).abs() < 0.4 && (out.at.1 - p.1).abs() < 0.4,
                "{dir:?} moved to {:?} from {p:?}",
                out.at
            );
            assert!((out.gain - 1.0).abs() < 0.01);
            assert!(lens.map(p, Image::Secondary).gain < 0.01);
        }
    }

    #[test]
    fn the_ramp_opens_with_the_drive_and_is_shut_at_sublight() {
        // A ship nine subpixels from nose to centre, which is what a hull comes
        // out at on a thirty-six-row terminal at the framing a flight opens on.
        let c = (60.0, 30.0);
        let ship = 9.0;
        assert!(
            !Lens::for_warp(c, 0.0, ship, ABEAM).is_on(),
            "sublight must not bend"
        );
        let half = Lens::for_warp(c, 0.5, ship, ABEAM).radius;
        let full = Lens::for_warp(c, 1.0, ship, ABEAM).radius;
        assert!(half > 0.0 && full > half * 3.0, "{half} then {full}");
        // Out of range in either direction is clamped, not extrapolated.
        assert_eq!(Lens::for_warp(c, -3.0, ship, ABEAM).radius, 0.0);
        assert_eq!(Lens::for_warp(c, 9.0, ship, ABEAM).radius, full);
        assert_eq!(Lens::for_warp(c, 0.0, ship, ABEAM).center, c);
    }

    #[test]
    fn the_bubble_grows_and_shrinks_with_the_ship_inside_it() {
        // The whole of what makes the bubble a bubble rather than a collar
        // painted on the glass: it is measured in ships, so twice the ship is
        // twice the bubble, and everything derived from the radius follows
        // without being asked to.
        let c = (60.0, 30.0);
        let small = Lens::for_warp(c, 1.0, 9.0, ABEAM);
        let large = Lens::for_warp(c, 1.0, 18.0, ABEAM);
        assert_eq!(large.radius, small.radius * 2.0, "the ring did not follow");
        assert_eq!(
            large.shadow_axes().0,
            small.shadow_axes().0 * 2.0,
            "the shadow did not"
        );
        assert_eq!(
            large.shadow_axes().1,
            small.shadow_axes().1 * 2.0,
            "and not across, either"
        );
        // Nor did the wake: the ship sits the same way into a bubble of any
        // size, which is what stops the hull sliding out of it as the drive
        // spools.
        assert_eq!(
            (c.0 - large.center.0) / large.radius,
            (c.0 - small.center.0) / small.radius,
            "the ship moved inside its own wake"
        );
        let far = (c.0 + small.radius * 9.5, c.1);
        assert!(
            small.bends(&[(far.0, far.1, 0.0), (far.0, far.1, 1.0)]),
            "the reach must scale with the ring"
        );
        let further = (c.0 + small.radius * 19.0, c.1);
        assert!(!small.bends(&[(further.0, further.1, 0.0), (further.0, further.1, 1.0)]));
        assert!(
            large.bends(&[(further.0, further.1, 0.0), (further.0, further.1, 1.0)]),
            "the larger reach did not"
        );
    }

    #[test]
    fn the_bubble_never_swallows_the_primary_image() {
        // The shadow is what makes the middle of the frame dark instead of
        // filled with a demagnified copy of the sky.
        let lens = lens();
        for dir in directions() {
            for i in 0..200 {
                let p = at(&lens, dir, i as f32 * 0.1);
                assert!(
                    !lens.shadowed(lens.map(p, Image::Primary).at),
                    "the bubble swallowed a primary along {dir:?}"
                );
            }
        }
        assert!(
            !Lens::OFF.shadowed((0.0, 0.0)),
            "an unlit drive shadows nothing"
        );
    }

    #[test]
    fn the_bubble_sweeps_the_same_sky_clear_however_it_is_shaped() {
        // The area promise, taken through the real arithmetic rather than
        // against the two constants that are supposed to make it hold.
        let lens = lens();
        let steps = 4096;
        let mut area = 0.0f64;
        for i in 0..steps {
            let th = i as f32 * std::f32::consts::TAU / steps as f32;
            // Where the ring crosses this ray, found by asking `offset` where
            // it reports one rather than by evaluating the ellipse again.
            let on_ring = at(&lens, (th.cos(), th.sin()), 1.0);
            let r = (on_ring.0 - lens.center.0).hypot(on_ring.1 - lens.center.1);
            area += 0.5 * (r as f64).powi(2) * (std::f64::consts::TAU / steps as f64);
        }
        let want = std::f64::consts::PI * (lens.radius as f64).powi(2);
        assert!(
            (area - want).abs() < want * 1e-3,
            "the ring encloses {area}, against the {want} a round one would"
        );
    }

    #[test]
    fn the_bubble_is_longer_along_the_track_than_across_it() {
        // The shape itself, and which way round it lies.
        let lens = lens();
        let along = at(&lens, (1.0, 0.0), 1.0).0 - lens.center.0;
        let across = at(&lens, (0.0, 1.0), 1.0).1 - lens.center.1;
        assert!(
            along > across * 2.0,
            "the bubble is only {along} by {across}"
        );
        let (a, b) = lens.semi_axes();
        assert!((a - along).abs() < 1e-3 && (b - across).abs() < 1e-3);
    }

    #[test]
    fn the_ship_rides_in_the_front_of_its_own_wake() {
        // The bubble is seated astern of the hull rather than around it, which
        // is the whole of why it reads as something the ship is dragging.
        let c = (100.0, 50.0);
        let ship = 9.0;
        let lens = Lens::for_warp(c, 1.0, ship, ABEAM);
        assert!(lens.center.0 < c.0, "the bubble is not astern of anything");
        assert!(
            lens.shadowed((c.0 + ship, c.1)),
            "the ship's nose is outside its own bubble"
        );
        assert!(
            lens.shadowed((c.0 - ship, c.1)),
            "and its tail has come out of the back"
        );
        // There is more bubble behind the ship than in front of it, which is
        // what a wake is.
        let (a, _) = lens.semi_axes();
        let ahead = lens.center.0 + a - c.0;
        let astern = c.0 - (lens.center.0 - a);
        assert!(
            astern > ahead * 1.3,
            "{astern} astern against {ahead} ahead is not a wake"
        );
    }

    #[test]
    fn an_arc_goes_round_the_ring_rather_than_across_it() {
        // A source passing behind the mass has its image sweep right round the
        // ring.
        let lens = lens();
        let (from, to) = (at(&lens, (1.0, 0.0), 1.0), at(&lens, (-1.0, 0.0), 1.0));
        let (from, to) = ((from.0, from.1, 0.0), (to.0, to.1, 1.0));
        let mut path = vec![from];
        lens.arc_to(from, to, &mut path);
        assert!(
            path.len() > 4,
            "half a turn came out as {} points",
            path.len()
        );
        for p in &path {
            let m = lens.offset((p.0, p.1));
            assert!(
                (m - 1.0).abs() < 1e-3,
                "the arc left the ring, at {m} rings out"
            );
        }
        for pair in path.windows(2) {
            assert!(
                pair[1].2 >= pair[0].2,
                "the arc ran backwards through the exposure"
            );
        }
        assert_eq!(*path.last().unwrap(), to, "the arc has to arrive");

        // Two points a hair apart, out beyond the ring where a streak passing
        // by actually is: no filling in at all.
        let outside = at(&lens, (1.0, 0.0), 1.05);
        let outside = (outside.0, outside.1, 0.0);
        let mut path = vec![outside];
        lens.arc_to(outside, (outside.0 + 0.2, outside.1 + 0.1, 1.0), &mut path);
        assert_eq!(path.len(), 2, "a straight run should not be subdivided");
    }

    #[test]
    fn nothing_the_renderer_can_hand_it_produces_a_nan() {
        // A star can be projected anywhere at all, including off to infinity
        // when it is a hair past the near plane, and one NaN spreads across the
        // canvas for the rest of the frame.
        let lenses = [
            lens(),
            Lens::OFF,
            raw((0.0, 0.0), f32::MIN_POSITIVE),
            raw((f32::NAN, 0.0), 10.0),
            raw((0.0, 0.0), f32::INFINITY),
            Lens::for_warp((100.0, 50.0), 1.0, 10.0, [0.0, 0.0, 1.0]),
            Lens::for_warp((100.0, 50.0), 1.0, 10.0, [0.0, 0.0, 0.0]),
            Lens::for_warp((100.0, 50.0), 1.0, 10.0, [f32::NAN, 0.0, 0.0]),
        ];
        let points = [
            (0.0, 0.0),
            (100.0, 50.0),
            (f32::NAN, 1.0),
            (1.0, f32::INFINITY),
            (-1e30, 1e30),
            (f32::MIN_POSITIVE, -f32::MIN_POSITIVE),
        ];
        for lens in lenses {
            for p in points {
                for image in [Image::Primary, Image::Secondary] {
                    let out = lens.map(p, image);
                    assert!(
                        out.gain.is_finite() && out.gain >= 0.0,
                        "{lens:?} on {p:?} gave gain {}",
                        out.gain
                    );
                    // A non-finite *position* is allowed straight back out —
                    // the canvas drops those — but it must not be invented from
                    // finite inputs.
                    if p.0.is_finite() && p.1.is_finite() && lens.center.0.is_finite() {
                        assert!(
                            out.at.0.is_finite() && out.at.1.is_finite(),
                            "{lens:?} on {p:?} gave {:?}",
                            out.at
                        );
                    }
                }
            }
        }
    }
}
