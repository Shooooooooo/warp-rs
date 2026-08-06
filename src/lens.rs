//! Bending starlight around the ship.
//!
//! A warp bubble is a lump of curved spacetime, and light that passes one is
//! deflected the way it is by any other mass: the sky behind the ship is pushed
//! outward, away from the bubble, and piles up into a bright ring at the radius
//! where the deflection exactly cancels the offset. Inside that ring the same
//! source shows again, fainter and on the opposite side — the counter-image
//! every gravitational lens produces and the thing that makes one legible as a
//! lens rather than as a smudge.
//!
//! This is the standard thin point-mass lens, worked in screen space because
//! that is where the deflection is wanted: everything here is in canvas
//! subpixels, and the Einstein radius is the only parameter.

/// Ceiling on how much a single image may be brightened. The magnification of
/// a point source diverges on the lens axis — a star dead behind the bubble is
/// a ring of infinite brightness — and nothing downstream survives an infinite
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

/// The bubble's own shadow, as a fraction of the Einstein radius.
///
/// A *transparent* point mass maps the entire sky into the disc inside its
/// Einstein ring as a demagnified counter-image, and lensing conserves surface
/// brightness, so that disc comes out about as bright as the sky it copies —
/// no bubble, just a shuffled middle. What makes a real compact object read as
/// one is that it swallows what passes closest, leaving a dark disc with the
/// counter-images crowded into a bright rim just outside it. A warp bubble is
/// opaque for the same reason a hull is, so anything imaged inside this radius
/// is simply not drawn. The primary image is always outside the Einstein ring,
/// so this only ever swallows counter-images.
const SHADOW_FRAC: f32 = 0.72;

/// Fraction of the canvas height the Einstein radius reaches at full warp.
///
/// Sized against the hull rather than by eye. The ship is 0.24 of the canvas
/// height from nose to centre, so this holds it with a good deal of room to
/// spare — deliberately more than the geometry demands. A bubble that merely
/// clears the ship reads as a collar around it; the sky wants somewhere to
/// bend, and the swept-clear disc is most of what makes the lensing legible.
const RADIUS_AT_WARP: f32 = 0.48;

/// A thin point-mass lens sitting on the canvas.
#[derive(Debug, Clone, Copy)]
pub struct Lens {
    /// Where the mass is, in canvas subpixels.
    pub center: (f32, f32),
    /// The Einstein radius, in subpixels. Zero is no lens at all, and is an
    /// exact identity rather than an approximation of one.
    pub radius: f32,
}

/// Which of the two images a point-mass lens forms of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Image {
    /// Outside the Einstein ring, on the same side as the source. This is the
    /// bright one, and the only one at all when the lens is off.
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

impl Lens {
    /// A lens that does nothing, and does it exactly rather than very nearly:
    /// the zero radius is what every path here checks, through [`Self::is_on`],
    /// so a sublight frame takes the same bytes it would if this module were
    /// not in the tree.
    ///
    /// The centre is not a second line of defence, whatever it looks like.
    /// This once claimed to be "placed off-canvas as well as zero-sized", and
    /// `(0.0, 0.0)` is the top-left subpixel — as on the canvas as a point can
    /// be. The radius is the whole of it.
    pub const OFF: Lens = Lens {
        center: (0.0, 0.0),
        radius: 0.0,
    };

    /// The lens the drive is currently making, given the 0..=1 warp ramp and
    /// the canvas height. Quadratic in the ramp, so it opens up as the drive
    /// spools rather than snapping on the instant the light barrier is crossed,
    /// and it is exactly zero at sublight.
    pub fn for_warp(center: (f32, f32), warp: f32, height: f32) -> Self {
        let warp = warp.clamp(0.0, 1.0);
        Self {
            center,
            radius: height.max(0.0) * RADIUS_AT_WARP * warp * warp,
        }
    }

    /// Whether this lens bends anything.
    pub fn is_on(&self) -> bool {
        self.radius > 0.0 && self.radius.is_finite()
    }

    /// Radius of the bubble's shadow. Nothing imaged inside this is drawn.
    pub fn shadow(&self) -> f32 {
        self.radius * SHADOW_FRAC
    }

    /// Whether a point is one the bubble has swallowed.
    pub fn shadowed(&self, p: (f32, f32)) -> bool {
        self.is_on() && (p.0 - self.center.0).hypot(p.1 - self.center.1) < self.shadow()
    }

    /// Whether a segment passes close enough for the bend to be worth the work
    /// of chopping it up and curving it.
    ///
    /// The deflection falls off as `e²/r`, so ten Einstein radii out it is a
    /// tenth of a radius — well under a subpixel at any size this runs at — and
    /// the counter-image has long since faded. The overwhelming majority of a
    /// star pool is out there, and this is what lets it cost exactly what it
    /// costs with the drive shut down.
    pub fn bends(&self, from: (f32, f32), to: (f32, f32)) -> bool {
        if !self.is_on() {
            return false;
        }
        let reach = self.radius * REACH;
        let near = |p: (f32, f32)| (p.0 - self.center.0).hypot(p.1 - self.center.1) <= reach;
        near(from) || near(to)
    }

    /// Whether the straight line between two points passes inside the Einstein
    /// ring. Point-to-segment distance, and no trigonometry: this is asked once
    /// per sample of every streak in the pool.
    fn crosses_the_ring(&self, from: (f32, f32), to: (f32, f32)) -> bool {
        let (ax, ay) = (from.0 - self.center.0, from.1 - self.center.1);
        let (bx, by) = (to.0 - self.center.0, to.1 - self.center.1);
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
        nx * nx + ny * ny < self.radius * self.radius
    }

    /// How sharply the lens is bending things at `p`, as a 0..=1 ramp.
    ///
    /// This is the *gradient* of the deflection, not the deflection: what
    /// matters for chopping a streak up is how much the bend changes along it,
    /// and that falls off as the inverse square of the distance where the
    /// deflection itself only falls off as `e²/r`. So a streak out at the edge
    /// of the frame is displaced almost uniformly along its length — which is
    /// to say not curved at all — and can be laid down straight. That is what
    /// keeps a lensed frame close to the price of an unlensed one.
    ///
    /// (Said here as "the deflection falls off as the square of the distance",
    /// which is the wrong quantity and the wrong power, and disagreed with
    /// [`Self::bends`] thirty lines above.)
    pub fn curvature(&self, p: (f32, f32)) -> f32 {
        if !self.is_on() {
            return 0.0;
        }
        let r_sq = (p.0 - self.center.0).powi(2) + (p.1 - self.center.1).powi(2);
        let e_sq = self.radius * self.radius;
        if !r_sq.is_finite() {
            return 0.0;
        }
        e_sq / (r_sq + e_sq)
    }

    /// Where `p` appears once the lens has had it, and by how much its
    /// brightness changes.
    ///
    /// With the source at offset `r` from the centre and an Einstein radius
    /// `e`, the images sit at the two roots of `θ − e²/θ = r`:
    ///
    /// ```text
    /// θ₊ = (r + √(r² + 4e²)) / 2      outside the ring, same side
    /// θ₋ = (r − √(r² + 4e²)) / 2      inside it, and negative: the far side
    /// ```
    ///
    /// with magnifications `μ± = (u² + 2) / (2u√(u² + 4)) ± ½`, `u = r/e`.
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
        let r = dx.hypot(dy);
        let e = self.radius;
        // On the axis the source images as a complete ring, which a single
        // point cannot be drawn as. Pick a direction — any fixed one will do,
        // and this is a case of measure zero — so the ring at least gets a
        // point on it instead of a NaN.
        let (ux, uy) = if r > f32::EPSILON {
            (dx / r, dy / r)
        } else {
            (1.0, 0.0)
        };

        // `√(r² + 4e²)`, factored so it survives the range a projection can
        // produce: a star a hair past the near plane lands billions of
        // subpixels off the canvas, and squaring that overflows an `f32` into
        // an infinity that then propagates into the position.
        let root = if r > f32::EPSILON {
            r * (1.0 + (2.0 * e / r).powi(2)).sqrt()
        } else {
            2.0 * e
        };
        let primary = (r + root) * 0.5;
        let (offset, sign) = match image {
            Image::Primary => (primary, 1.0),
            // |θ₋| is `(root − r)/2`, but that is two nearly equal numbers
            // subtracted once `r` is large. The two roots multiply to `e²`, so
            // taking it from the primary is the same answer without the
            // cancellation — and it is exactly `e` when the source is on axis.
            Image::Secondary => (
                if primary > f32::MIN_POSITIVE {
                    e * e / primary
                } else {
                    e
                },
                -1.0,
            ),
        };

        // `u` is the offset in Einstein radii, held inside the range where the
        // closed form below is worth evaluating. The magnification is monotonic
        // in `u`: below the floor it is already past `MAX_MAGNIFICATION`, and
        // above the ceiling it has settled onto its limit of one image at unit
        // brightness. A NaN — which arrives when the caller hands over a NaN
        // centre — is sent to the floor rather than through `clamp`, which
        // passes one straight out the other side.
        let u = r / e;
        let u = if u.is_nan() {
            U_FLOOR
        } else {
            u.clamp(U_FLOOR, U_CEILING)
        };
        let shared = ((u * u + 2.0) / (2.0 * u * (u * u + 4.0).sqrt())).min(MAX_MAGNIFICATION);
        let gain = match image {
            Image::Primary => shared + 0.5,
            Image::Secondary => (shared - 0.5).max(0.0),
        };

        Lensed {
            at: (
                self.center.0 + ux * offset * sign,
                self.center.1 + uy * offset * sign,
            ),
            gain,
        }
    }
}

/// Largest angular step, in radians, allowed between two points of a bent
/// path before the arc between them is filled in.
const MAX_ARC_STEP: f32 = 0.25;
/// Ceiling on that filling-in, so a source sweeping right past the axis cannot
/// ask for an unbounded number of points.
const MAX_ARC_FILL: usize = 24;

impl Lens {
    /// Append the arc from `from` to `to`, as seen about the lens, to `out`.
    /// `from` is assumed to be there already; `to` always ends up there.
    ///
    /// Two points of a bent streak are joined by a straight line, and near the
    /// lens that is wrong in a way that shows: as a source passes behind the
    /// mass its image sweeps *around* the Einstein ring, and a chord between
    /// two samples on opposite sides of that sweep cuts straight through the
    /// middle of the bubble the lens is supposed to have emptied. Interpolating
    /// in polar coordinates instead follows the sweep. Away from the lens the
    /// angle barely moves and this adds nothing at all.
    pub fn arc_to(&self, from: (f32, f32), to: (f32, f32), out: &mut Vec<(f32, f32)>) {
        // The expensive part of this is two `atan2`s and a `sin_cos` per point,
        // and the great majority of pairs do not need it: two samples of a
        // streak that is merely passing by are a fraction of a radian apart and
        // a straight line between them is the arc to well under a subpixel.
        // What has to be caught is the pair that *straddles* the sweep, and
        // that shows up as a chord cutting inside the ring — where no primary
        // image can be, so nothing legitimate is ever there to be cut.
        if !self.crosses_the_ring(from, to) {
            out.push(to);
            return;
        }
        let polar = |p: (f32, f32)| {
            let (dx, dy) = (p.0 - self.center.0, p.1 - self.center.1);
            (dx.hypot(dy), dy.atan2(dx))
        };
        let (r0, a0) = polar(from);
        let (r1, a1) = polar(to);
        // The short way round: the image sweeps, it does not jump.
        let mut sweep = a1 - a0;
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
            let t = i as f32 / steps as f32;
            let (r, a) = (r0 + (r1 - r0) * t, a0 + sweep * t);
            out.push((self.center.0 + r * a.cos(), self.center.1 + r * a.sin()));
        }
        out.push(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lens() -> Lens {
        Lens {
            center: (100.0, 50.0),
            radius: 20.0,
        }
    }

    fn radius(lens: &Lens, p: (f32, f32)) -> f32 {
        (p.0 - lens.center.0).hypot(p.1 - lens.center.1)
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
            // And there is no counter-image to draw.
            assert_eq!(Lens::OFF.map(p, Image::Secondary).gain, 0.0);
        }
    }

    #[test]
    fn the_sky_is_pushed_outside_the_einstein_ring() {
        // The primary image never lands inside the ring, which is what leaves
        // the bubble around the ship swept clear of stars.
        let lens = lens();
        for i in 0..200 {
            let r = i as f32 * 0.5;
            let p = (lens.center.0 + r, lens.center.1);
            let out = lens.map(p, Image::Primary);
            assert!(
                radius(&lens, out.at) >= lens.radius - 1e-3,
                "a source at {r} imaged inside the ring at {}",
                radius(&lens, out.at)
            );
            // And it only ever moves outward, never back toward the centre.
            assert!(radius(&lens, out.at) >= r - 1e-3);
        }
    }

    #[test]
    fn the_counter_image_is_inside_the_ring_and_on_the_far_side() {
        let lens = lens();
        for (dx, dy) in [(30.0, 0.0), (0.0, -45.0), (12.0, 9.0), (-70.0, 5.0)] {
            let p = (lens.center.0 + dx, lens.center.1 + dy);
            let inner = lens.map(p, Image::Secondary);
            assert!(
                radius(&lens, inner.at) <= lens.radius + 1e-3,
                "the counter-image escaped the ring: {:?}",
                inner.at
            );
            // Opposite side means the two offsets point against each other.
            let (ix, iy) = (inner.at.0 - lens.center.0, inner.at.1 - lens.center.1);
            assert!(ix * dx + iy * dy < 0.0, "{dx},{dy} imaged to {ix},{iy}");
        }
    }

    #[test]
    fn both_images_converge_on_the_ring_for_a_source_on_the_axis() {
        // A source dead behind the lens is the Einstein ring itself, so both
        // images sit on it — from opposite directions.
        let lens = lens();
        let p = lens.center;
        for image in [Image::Primary, Image::Secondary] {
            let out = lens.map(p, image);
            assert!(
                (radius(&lens, out.at) - lens.radius).abs() < 1e-3,
                "{image:?} landed at {:?}",
                out.at
            );
            assert!(out.gain.is_finite(), "{image:?} came out infinitely bright");
        }
    }

    #[test]
    fn magnification_peaks_near_the_ring_and_settles_to_nothing_far_out() {
        let lens = lens();
        let total = |r: f32| {
            let p = (lens.center.0 + r, lens.center.1);
            lens.map(p, Image::Primary).gain + lens.map(p, Image::Secondary).gain
        };
        // A lens conserves surface brightness and adds an image, so the pair is
        // never dimmer than the source was.
        for i in 0..400 {
            let r = i as f32 * 0.75;
            assert!(total(r) >= 1.0 - 1e-4, "{r} came out dimmer: {}", total(r));
        }
        // The textbook value on the ring, and effectively nothing at 20 radii.
        assert!(
            (total(lens.radius) - 1.34).abs() < 0.01,
            "{}",
            total(lens.radius)
        );
        assert!(total(lens.radius * 20.0) < 1.01);
        assert!(total(lens.radius * 0.05) > total(lens.radius));
    }

    #[test]
    fn the_primary_image_fades_back_into_the_unbent_sky() {
        // Far from the lens the sky must look untouched, or the whole frame
        // reads as though it is being pulled about.
        let lens = lens();
        let p = (lens.center.0 + 4000.0, lens.center.1);
        let out = lens.map(p, Image::Primary);
        assert!((out.at.0 - p.0).abs() < 0.2 && (out.at.1 - p.1).abs() < 0.2);
        assert!((out.gain - 1.0).abs() < 0.01);
        // And the counter-image has faded to nothing rather than lingering.
        assert!(lens.map(p, Image::Secondary).gain < 0.01);
    }

    #[test]
    fn the_ramp_opens_with_the_drive_and_is_shut_at_sublight() {
        let c = (60.0, 30.0);
        assert!(
            !Lens::for_warp(c, 0.0, 72.0).is_on(),
            "sublight must not bend"
        );
        let half = Lens::for_warp(c, 0.5, 72.0).radius;
        let full = Lens::for_warp(c, 1.0, 72.0).radius;
        assert!(half > 0.0 && full > half * 3.0, "{half} then {full}");
        // Out of range in either direction is clamped, not extrapolated.
        assert_eq!(Lens::for_warp(c, -3.0, 72.0).radius, 0.0);
        assert_eq!(Lens::for_warp(c, 9.0, 72.0).radius, full);
    }

    #[test]
    fn the_bubble_never_swallows_the_primary_image() {
        // The shadow is what makes the middle of the frame dark instead of
        // filled with a demagnified copy of the sky. It has to swallow only
        // counter-images: if it could take a primary, a streak sweeping past
        // would break into pieces, and the sky would flicker as stars crossed.
        let lens = lens();
        for i in 0..500 {
            let r = i as f32 * 0.4;
            let p = (lens.center.0 + r, lens.center.1 - r * 0.3);
            assert!(
                !lens.shadowed(lens.map(p, Image::Primary).at),
                "the bubble swallowed a primary at r = {r}"
            );
        }
        assert!(
            lens.shadow() < lens.radius,
            "the shadow is not inside the ring"
        );
        assert!(
            !Lens::OFF.shadowed((0.0, 0.0)),
            "an unlit drive shadows nothing"
        );
    }

    #[test]
    fn an_arc_goes_round_the_ring_rather_than_across_it() {
        // A source passing behind the mass has its image sweep right round the
        // ring. Joining two samples of that sweep with a straight line draws a
        // chord through the middle of the bubble — the one place nothing is
        // supposed to be.
        let lens = lens();
        let (from, to) = (
            (lens.center.0 + lens.radius, lens.center.1),
            (lens.center.0 - lens.radius, lens.center.1),
        );
        let mut path = vec![from];
        lens.arc_to(from, to, &mut path);
        assert!(
            path.len() > 4,
            "half a turn came out as {} points",
            path.len()
        );
        for p in &path {
            let r = (p.0 - lens.center.0).hypot(p.1 - lens.center.1);
            assert!(
                (r - lens.radius).abs() < 1e-3,
                "the arc left the ring, at radius {r}"
            );
        }
        assert_eq!(*path.last().unwrap(), to, "the arc has to arrive");

        // Two points a hair apart cost nothing: no filling in at all.
        let mut path = vec![from];
        lens.arc_to(from, (from.0 + 0.2, from.1 + 0.1), &mut path);
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
            Lens {
                center: (0.0, 0.0),
                radius: f32::MIN_POSITIVE,
            },
            Lens {
                center: (f32::NAN, 0.0),
                radius: 10.0,
            },
            Lens {
                center: (0.0, 0.0),
                radius: f32::INFINITY,
            },
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
                    // the canvas drops those — but it must not be invented
                    // from finite inputs.
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
