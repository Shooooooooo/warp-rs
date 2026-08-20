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
//! What is drawn here is that lens *being flown*. A round bubble says nothing
//! about which way the ship is going and reads as a collar painted on the
//! glass, so this one is drawn out along the track and seated astern of the
//! hull — the ship rides the leading third of its own disturbance, the way a
//! body sits in its wake.
//!
//! Neither of those is a new lens, and the first is not even new arithmetic.
//! Dividing an offset by the two semi-axes turns the ellipse into the circle
//! the textbook solves, and because the semi-axes are reciprocals of each other
//! that map has determinant one. Conjugating a lens by an invertible linear map
//! preserves everything a lens is made of — how many images there are, which
//! side of the ring each falls on, the product of the two roots, and the
//! magnification, which is a determinant and so comes through a conjugation
//! untouched. So this is still the standard thin point-mass lens, worked in
//! screen space because that is where the deflection is wanted, in canvas
//! subpixels throughout. The only difference is that distance is measured in an
//! elliptical metric rather than a round one. [`Lens::offset`] is that metric,
//! and everything below is the textbook once a quantity is expressed in it.
//!
//! The wake is the other half, and it is not in the outline at all. Skewing the
//! ring fore and aft is the obvious way to draw a teardrop and it is the wrong
//! one: each image then has to be solved on its own ray with its own radius,
//! which costs the cancellation that keeps the counter-image accurate, and at
//! any elongation worth having it pinches the waist into a peanut. Moving the
//! *centre* astern instead leaves the outline a clean convex ellipse and every
//! paired invariant exactly where it was, while relative to the hull the bubble
//! is strongly asymmetric: at full warp the ring reaches 2.4 ship-halves ahead
//! of the ship and 3.6 astern.

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

/// The bubble's own shadow, as a fraction of the way out to the ring.
///
/// A *transparent* point mass maps the entire sky into the disc inside its
/// Einstein ring as a demagnified counter-image, and lensing conserves surface
/// brightness, so that disc comes out about as bright as the sky it copies —
/// no bubble, just a shuffled middle. What makes a real compact object read as
/// one is that it swallows what passes closest, leaving a dark disc with the
/// counter-images crowded into a bright rim just outside it. A warp bubble is
/// opaque for the same reason a hull is, so anything imaged inside this is
/// simply not drawn. The primary image is always outside the ring, on every
/// ray, so this only ever swallows counter-images.
const SHADOW_FRAC: f32 = 0.72;

/// That last sentence, made a condition of the build rather than a claim in a
/// comment: the shadow has to sit inside the ring, or it starts taking primary
/// images and a streak sweeping past the bubble breaks into pieces.
const _: () = assert!(
    SHADOW_FRAC < 1.0,
    "the shadow is outside the ring, and would swallow primary images"
);

/// The Einstein radius at full warp, as a multiple of the ship's own half-length
/// on screen.
///
/// Sized against the hull rather than by eye, and now *said* that way. It used
/// to be a fraction of the canvas height — 0.48, which was twice the ship's
/// 0.24 by hand and by hand only, with a comment to hold the two together. That
/// was a fair way to write a constant while the framing was itself constant. It
/// stopped being one the moment the camera could be pushed in and out: a bubble
/// pinned to the canvas would have sat there at a fixed size while the hull
/// inside it grew and shrank, which is exactly the collar this number exists to
/// avoid.
///
/// Two ship-lengths from nose to ring is deliberately more than the geometry
/// demands. A bubble that merely clears the ship reads as a collar around it;
/// the sky wants somewhere to bend, and the swept-clear disc is most of what
/// makes the lensing legible.
const RADIUS_IN_SHIPS: f32 = 2.0;

/// The bubble's semi-major axis, along the track, as a multiple of the Einstein
/// radius.
///
/// Squared, it is the elongation: the bubble is 2.2 times as long as it is
/// wide. Pulled up by the whole point of the shape — a round bubble is the same
/// bubble at rest as at warp, and says nothing about which way the ship is
/// pointing. Pulled down by the waist, which narrows as the ends stretch: at
/// about three to one the swept-clear sky above and below a rolled hull is
/// thinner than the hull is, and the bubble stops reading as something the ship
/// is inside.
const RING_MAJOR: f32 = 1.4832;

/// And across it — the reciprocal, which is the whole of why this is a reshape
/// rather than an enlargement.
///
/// The ellipse encloses `π·radius²` whatever `RING_MAJOR` is set to, so the
/// bubble is the same lump of curved spacetime shaped by the flow rather than a
/// bigger one. That is worth more than tidiness: [`Lens::bends`] sweeps the same
/// ellipse scaled by [`REACH`], so the region of sky that takes the expensive
/// bent path has exactly the area it always had, and an exterior frame at warp
/// costs what it always cost. Derived rather than tuned, so the two cannot drift
/// apart.
const RING_MINOR: f32 = 1.0 / RING_MAJOR;

/// How far astern of the ship the bubble sits, as a fraction of its own
/// semi-major axis.
///
/// This is the entire wake. The outline is a symmetric ellipse; what makes it
/// read as something trailing the hull is that the hull is not in the middle of
/// it. Pulled up by the asymmetry — at zero this is a collar again — and pulled
/// down hard by the nose, which walks out of the shadow altogether at 0.383 and
/// starts taking stars across the ship with it. The assertion below is what
/// holds that end.
const WAKE_SHIFT: f32 = 0.20;

/// At full warp the shadow has to reach past the ship's nose, or stars are
/// drawn over the hull and the swept-clear disc stops being what it is for.
/// The shadow reaches `RADIUS_IN_SHIPS · RING_MAJOR · SHADOW_FRAC` ahead of the
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
    ///
    /// Stored rather than derived from [`RING_MAJOR`] on demand, because they
    /// depend on where the camera is: the bubble is a solid drawn out along the
    /// track, and a camera swung round toward the nose sees that length
    /// foreshortened. [`Lens::for_warp`] is where they come from.
    axes: (f32, f32),
    /// The reciprocals of those two, worked out once when the bubble is built.
    ///
    /// [`Lens::offsets`] ended on a divide per axis, and it is the floor every
    /// gate in this module stands on — `bends` and `shadowed` run it on the
    /// whole pool, `map` and `crosses_the_ring` on every sample of every bent
    /// streak. Two numbers fixed for the frame were being divided by a dozen
    /// times per sample. Derived rather than stored independently: nothing may
    /// set one without the other.
    inv_axes: (f32, f32),
    /// Which way the long axis lies on the canvas, as `(cos, sin)`.
    ///
    /// Exactly `(1.0, 0.0)` whenever the camera is abeam of the track — which
    /// is the shot as it opens, and every elevation and camera roll of it — so
    /// the rotation in [`Lens::offsets`] costs a multiply by one and an
    /// addition of zero on the path the reference frames are recorded from.
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
///
/// Split out because the axis case below cannot reach it through the ordinary
/// path and must not grow a second copy of it: `μ± = (u² + 2) / (2u√(u² + 4)) ±
/// ½`. `u` is held inside the range where that closed form is worth evaluating.
/// The magnification is monotonic in `u`: below the floor it is already past
/// `MAX_MAGNIFICATION`, and above the ceiling it has settled onto its limit of
/// one image at unit brightness. A NaN — which arrives when the caller hands
/// over a NaN centre — is sent to the floor rather than through `clamp`, which
/// passes one straight out the other side.
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
    ///
    /// The centre is not a second line of defence, whatever it looks like.
    /// This once claimed to be "placed off-canvas as well as zero-sized", and
    /// `(0.0, 0.0)` is the top-left subpixel — as on the canvas as a point can
    /// be. The radius is the whole of it.
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
    ///
    /// The bubble is measured in ships rather than in frames because it belongs
    /// to the ship: it is the same lump of curved spacetime whether the camera
    /// is looking from close to or far off, so it has to come and go on screen
    /// exactly as the hull does. [`crate::view::ship_half_on_screen`] is where
    /// that half-length comes from.
    ///
    /// The centre it is handed is where the *ship* is; the bubble is seated
    /// astern of that, which is the one place in this module that knows which
    /// way the ship is pointing. `nose` is that direction, in the camera's own
    /// space, from [`crate::view::Orbit::nose_in_camera`] — a unit vector, and
    /// the only thing here that changes when the camera is swung round. The
    /// shift is a fraction of the semi-major axis rather than of the hull, so
    /// the wake opens up with the bubble instead of sliding out of it.
    ///
    /// The bubble is a *solid* — a spheroid drawn out along the track — and
    /// what goes on the canvas is its outline. Support functions do the rest:
    /// for an ellipsoid the extent in a screen direction `ŝ` is
    /// `√(L²(ŝ·t̂)² + W²(1 − (ŝ·t̂)²))`, and the track's own screen direction
    /// has `ŝ·t̂ = sin φ`, while the one across it has `ŝ·t̂ = 0`. So the long
    /// axis runs from `L` broadside down to `W` end-on and the short one never
    /// moves — which is why a ship seen head-on sits in a circle rather than in
    /// a line, and why nothing has to bound how far the camera may be swung.
    ///
    /// Broadside is branched rather than computed, and `nose[2] == 0.0` is
    /// exactly that case: the camera abeam of the track, at any elevation and
    /// any roll of its own. `√(RING_MAJOR²)` is `RING_MAJOR` under IEEE
    /// arithmetic and would very probably survive, but "very probably" is not
    /// what the reference frames are pinned with.
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
        // fraction of — `along` has a floor at `W` and this does not. `radius`
        // already carries the range correction, so this is the honest
        // projection of that displacement and not an approximation of one.
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
    ///
    /// Their product is `radius²` broadside, which is the area promise — and it
    /// is deliberately *not* extended to every angle, because a bubble seen
    /// end-on genuinely covers less sky than one seen side-on and pretending
    /// otherwise would inflate it as the camera came round. What
    /// [`RING_MINOR`] being a reciprocal buys is that the promise holds however
    /// the bubble is *shaped*; how much of it the camera can see is a different
    /// question with a different answer.
    pub fn semi_axes(&self) -> (f32, f32) {
        self.axes
    }

    /// Where `p` sits in the frame the bubble is round in: turned onto the
    /// outline's own axes, then divided by them.
    ///
    /// The one place the ellipse is actually spelled out, so the four questions
    /// below cannot drift apart about what shape it is. No `is_on` guard —
    /// every caller has already asked, and this sits under the inner loop.
    ///
    /// The square case is branched, and this one is for speed rather than for
    /// exactness: `dx * 1.0 + dy * 0.0` is already `dx` to the bit, but this is
    /// two of the three hottest gates in the program and paying four multiplies
    /// and two adds per sample for a turn of zero cost five percent of the
    /// drawing time in a warp frame at twenty thousand stars — measured, 21.3 ms
    /// against 22.3. The branch is free: `turn` is fixed for the whole frame, so
    /// it predicts perfectly.
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
    ///
    /// This is the elliptical metric the whole module is worked in, and every
    /// question it used to answer with a distance and a radius it now answers
    /// with this and a pure number: the shadow is `SHADOW_FRAC`, the reach is
    /// `REACH`, the ring is one. A lens that is not there has nothing inside
    /// it, so it reports everything infinitely far out.
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
    ///
    /// The deflection falls off as `e²/r`, so ten Einstein radii out it is a
    /// tenth of a radius — a subpixel or two at the sizes this runs at, against
    /// a streak that is already several — and the counter-image has long since
    /// faded. The overwhelming majority of a star pool is out there, and this
    /// is what lets it cost exactly what it costs with the drive shut down.
    ///
    /// The reach is the ring scaled up, so it is elongated too, and its area is
    /// `π(REACH·radius)²` — the same disc's worth of sky as when the bubble was
    /// round. That is not a coincidence and it is not free: it is what
    /// [`RING_MINOR`] being a reciprocal buys.
    /// Asked of every point rather than of the two ends, because an exposure
    /// the ship turned through is a curve and its middle can pass the bubble
    /// while neither end does. At two points it is the two comparisons it
    /// always was, in the order it always made them.
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
    ///
    /// This is the *gradient* of the deflection, not the deflection: what
    /// matters for chopping a streak up is how much the bend changes along it,
    /// and that falls off as the inverse square of the distance where the
    /// deflection itself only falls off as `e²/r`. So a streak out at the edge
    /// of the frame is displaced almost uniformly along its length — which is
    /// to say not curved at all — and can be laid down straight. That is what
    /// keeps a lensed frame close to the price of an unlensed one.
    ///
    /// `e²/(r² + e²)` said in offsets is `1/(1 + m²)`, which is the same number
    /// where the bubble is round and the right one where it is not: a streak
    /// passing over the narrow waist is bent harder, at the same distance, than
    /// one running out along the wake, and gets subdivided to match.
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
    ///
    /// With the source `m` Einstein radii off the centre, the two images sit at
    /// the roots of `θ − 1/θ = m`, in the same units:
    ///
    /// ```text
    /// θ₊ = (m + √(m² + 4)) / 2      outside the ring, same side
    /// θ₋ = (m − √(m² + 4)) / 2      inside it, and negative: the far side
    /// ```
    ///
    /// Both come out as a multiple of the source's own offset, so the answer is
    /// the offset scaled — and since the scaling happens in the round frame and
    /// the frame is reached by a linear map, scaling the *canvas* offset by the
    /// same number lands in the same place. That is why nothing here has to
    /// normalise anything or take an angle: the image is on the source's own
    /// ray whatever shape the ring is, and only the length of the ray changes.
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
        // point cannot be drawn as. Pick a direction — any fixed one will do,
        // and this is a case of measure zero — so the ring at least gets a
        // point on it instead of a NaN. A NaN centre is sent down here too and
        // for the same reason: there is no ray to put an image on, so it takes
        // the one this picks and comes back with the NaN it arrived with.
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
        // Written this way the overflow is harmless — `4/∞` is zero, the root
        // is one, and the primary comes back exactly unbent, which is what a
        // source that far out should do.
        let root = (1.0 + 4.0 / (m * m)).sqrt();
        let scale = match image {
            Image::Primary => (1.0 + root) * 0.5,
            // `θ₋` is `(m − √(m² + 4))/2`, which is two nearly equal numbers
            // subtracted once `m` is large. The two roots multiply to one, so
            // taking it from the primary is the same answer without the
            // cancellation — and it is exactly the ring when the source is on
            // axis. As a multiple of the source's own offset that is
            // `1/(m²·θ₊)`, negative because the counter-image is on the far
            // side.
            Image::Secondary => -2.0 / (m * m * (1.0 + root)),
        };

        Lensed {
            at: (self.center.0 + dx * scale, self.center.1 + dy * scale),
            gain: magnification(m, image),
        }
    }
}

/// Largest angular step, in radians, allowed between two points of a bent
/// path before the arc between them is filled in. Measured about the round
/// frame rather than the canvas, so a step is the same fraction of the way
/// round the ring at the waist as out along the wake.
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
    /// mass its image sweeps *around* the ring, and a chord between two samples
    /// on opposite sides of that sweep cuts straight through the middle of the
    /// bubble the lens is supposed to have emptied. Interpolating in polar
    /// coordinates instead follows the sweep. Away from the lens the angle
    /// barely moves and this adds nothing at all.
    ///
    /// The polar coordinates are the round frame's, so what gets laid down is
    /// an arc of the ellipse rather than of some circle drawn through it — the
    /// turned ellipse, since the way back out of that frame has to undo the
    /// same turn [`Self::offsets`] applied on the way in. Two points on the
    /// ring stay on the ring the whole way round.
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
        // What has to be caught is the pair that *straddles* the sweep, and
        // that shows up as a chord cutting inside the ring — where no primary
        // image can be, so nothing legitimate is ever there to be cut.
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
            // The pace is carried across the points the sweep fills in: a
            // bend moves where a star's light lands, never how fast it got
            // there.
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

    /// The point `m` Einstein radii from the centre along `(ux, uy)`.
    fn at(lens: &Lens, dir: (f32, f32), m: f32) -> (f32, f32) {
        let (a, b) = lens.semi_axes();
        let scale = m / (dir.0 / a).hypot(dir.1 / b);
        (lens.center.0 + dir.0 * scale, lens.center.1 + dir.1 * scale)
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
        // the bubble around the ship swept clear of stars. Said in offsets, so
        // it is a statement about the ellipse and not about a circle that
        // happens to be inscribed in it.
        let lens = lens();
        for dir in directions() {
            for i in 0..80 {
                let m = i as f32 * 0.1;
                let p = at(&lens, dir, m);
                let out = lens.map(p, Image::Primary);
                let imaged = lens.offset(out.at);
                assert!(
                    imaged >= 1.0 - 1e-3,
                    "a source {m} rings out along {dir:?} imaged at {imaged}"
                );
                // And it only ever moves outward, never back toward the centre.
                assert!(imaged >= m - 1e-3);
            }
        }
    }

    #[test]
    fn the_counter_image_is_inside_the_ring_and_on_the_far_side() {
        let lens = lens();
        for dir in directions() {
            for m in [0.4, 1.0, 2.5, 7.0] {
                let p = at(&lens, dir, m);
                let inner = lens.map(p, Image::Secondary);
                assert!(
                    lens.offset(inner.at) <= 1.0 + 1e-3,
                    "the counter-image escaped the ring: {:?}",
                    inner.at
                );
                // Opposite side means the two offsets point against each other.
                let (dx, dy) = (p.0 - lens.center.0, p.1 - lens.center.1);
                let (ix, iy) = (inner.at.0 - lens.center.0, inner.at.1 - lens.center.1);
                assert!(
                    ix * dx + iy * dy < 0.0,
                    "{dir:?} at {m} imaged to {ix},{iy}"
                );
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
            // And the counter-image has faded to nothing rather than lingering.
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
        // And a bubble that is not there is not seated anywhere either.
        assert_eq!(Lens::for_warp(c, 0.0, ship, ABEAM).center, c);
    }

    #[test]
    fn the_bubble_grows_and_shrinks_with_the_ship_inside_it() {
        // The whole of what makes the bubble a bubble rather than a collar
        // painted on the glass: it is measured in ships, so twice the ship is
        // twice the bubble, and everything derived from the radius follows
        // without being asked to. Those are asserted rather than assumed
        // exactly because deriving them is what makes them easy to forget.
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
        // And the reach, which is what decides whether a streak is bent at all.
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
        // filled with a demagnified copy of the sky. It has to swallow only
        // counter-images: if it could take a primary, a streak sweeping past
        // would break into pieces, and the sky would flicker as stars crossed.
        // True on every ray for the same reason it is true on one — the shadow
        // is a fraction of the way out to the ring and the primary is never
        // inside it — which is exactly what an elongation that pinched fore and
        // aft would have thrown away.
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
        // against the two constants that are supposed to make it hold. It is
        // load-bearing twice over: it is why elongating the bubble is a reshape
        // rather than an enlargement, and it is why `bends` still routes the
        // same amount of sky down the expensive path, so an exterior frame at
        // warp costs what it always cost.
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
        // The shape itself, and which way round it lies. The nose is to screen
        // right out here, so the long axis is the screen horizontal; a bubble
        // elongated the other way would be a ship flying sideways.
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
        // is the whole of why it reads as something the ship is dragging. The
        // nose still has to be inside the shadow, though — the compile-time
        // assertion above says so in constants, and this says it in subpixels,
        // through the arithmetic a frame actually runs.
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
        // ring. Joining two samples of that sweep with a straight line draws a
        // chord through the middle of the bubble — the one place nothing is
        // supposed to be.
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
        // And the moment travels with the place: the sweep fills in time as
        // well as space, so the pace a leg was flown at survives being bent.
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
            // And one built the way the renderer builds them, from a nose
            // direction that has gone wrong: `for_warp` divides by the track's
            // on-screen length, and a zero-length one is the end-on case.
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
