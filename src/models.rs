//! The ships, and how to draw one.
//!
//! A hull is a closed solid in its own coordinates — `+z` out the nose, `+x` to
//! starboard, `+y` down, the same axes the flight model steers about — sized to
//! fit the unit box, so a model is a *shape* and the renderer alone decides how
//! big it is on screen.
//!
//! Two decisions carry most of the weight here.
//!
//! The first is that hulls are assembled from primitives rather than typed out
//! as vertex and index lists. A shell through a few cross-sections builds a
//! fuselage, a fin, a wing or a cargo block, and it gets the winding right by
//! construction; thirty hand-entered faces per ship would not, and the failure
//! mode of one wrong index is a plate that quietly disappears at some
//! particular angle.
//!
//! The second is that every model is *closed*, with no zero-thickness plates.
//! That is what makes the sign of a face's projected area a complete answer to
//! which way it points, and it is why a fin here is a thin box rather than a
//! single quad. A quad has no outside, so it would wink out of existence every
//! time the ship rolled it edge-on.
//!
//! Hulls are drawn opaque, which means the plates cover what is behind them
//! instead of adding to it — the one place in the renderer where light is not
//! accumulated. There is still no depth buffer. Four things stand in for one:
//! the star band starts well beyond the ship, so nothing can be in front of it;
//! plates facing away are culled; the rest are painted far to near, which is
//! what sorts a nacelle against the engineering hull behind it; and the drive,
//! which is the only light in the frame that can be on either side of a plate,
//! is drawn on the side its exhaust is pointed toward.

use crate::camera::{self, Camera, Streak};
use crate::canvas::{Canvas, Facet};
use crate::ship::{Ship, MAX_PITCH_RATE, MAX_YAW_RATE};
use crate::view::{Eye, HULL_REACH, MIN_SHIP_DISTANCE};
use std::sync::OnceLock;

/// The zoom may push the camera in, and pushing it in far enough would put the
/// near corner of a rolled hull through the plane the projection gives up at —
/// where `Camera::project` answers `None` and [`plates`] drops the face. The
/// hull would come apart a plate at a time rather than fail outright, which is
/// the sort of thing that goes unnoticed for a while, so the closest the zoom
/// can get is a compile-time fact.
///
/// It used to have an opposite number, beside the star band's near wall: the
/// ship had to stay inside it, or a star could be drawn over a hull it was
/// supposed to be behind. [`crate::universe`] holds its nearest star light
/// years off, so that end of the sandwich is now twelve orders of magnitude
/// clear and there is nothing left to assert.
const _: () = assert!(
    MIN_SHIP_DISTANCE - HULL_REACH > camera::Z_NEAR,
    "the zoom can push a hull through the near plane, and a plate that cannot \
     be projected is simply not drawn"
);

/// How much nearer plates outshine further ones.
///
/// This carries more weight than it looks like it should. One lamp says which
/// *way* a plate faces, and says nothing at all about two plates that face the
/// same way — a slung container against the flank it is slung from, both of
/// them flat to the camera — so without depth in the shading a boxy hull comes
/// out as one undifferentiated mass. Range is the only thing left that tells
/// them apart.
const DEPTH_SHADE: f32 = 1.2;

/// The direction of the light *from* the hull, in the camera's space: over the
/// viewer's shoulder and a good way above. Unit length; `x` is to screen right,
/// `y` is down, `z` is away from the eye.
///
/// One lamp and a Lambert term is the whole lighting model. At this size a
/// plate is a handful of subpixels, and anything subtler than "which way is
/// this pointing" is spent where nobody can see it.
const LIGHT: [f32; 3] = [0.26, -0.90, -0.35];
/// What a plate facing away from the lamp still shows. Not zero: unlit is not
/// the same as absent, and a hull against a black sky needs its dark side to
/// still read as hull.
const AMBIENT: f32 = 0.13;
const DIFFUSE: f32 = 0.95;
/// How much the warp bubble lights the ship it is wrapped around, at full
/// warp. The glow inside the bubble is drawn behind the hull, so without this
/// the ship gets steadily darker than its own backdrop as the drive spools and
/// ends up a murky silhouette against it — which is the wrong way round for
/// something sitting *inside* the light.
const BUBBLE_LIGHT: f32 = 0.45;
/// Colour of a lit engine at impulse, and at warp.
///
/// Both ends are blue, and the drive whitens rather than cooling as it spools.
/// It used to run amber at impulse, which put the hottest-looking colour of the
/// two on the *colder* setting — and read as a chemical rocket bolted to
/// something that crosses light years. A drive gets whiter as it gets hotter,
/// so the ramp runs from a saturated blue to very nearly white, and lighting
/// the warp drive still changes the colour and not only the length.
const IMPULSE_FLAME: [f32; 3] = [0.24, 0.55, 1.00];
const WARP_FLAME: [f32; 3] = [0.82, 0.92, 1.00];

/// How far aft a bell throws its exhaust, in hull units per unit of bell
/// radius, at the top of the sublight range.
///
/// Measured off the bell rather than off the ship so a bigger drive throws a
/// longer flame, which hands the fleet its variety for nothing: the Normandy's
/// single 0.15 bell trails the furthest, the Enterprise's 0.07 impulse engine
/// leaves a stub above two nacelle lances. At 12.0 a nacelle plume is 1.3 hull
/// units at full impulse — a little under the hull's own length, enough to read
/// as something being left behind and not so much that a ship at cruise looks
/// like a ship at warp. Shorter than about 8.0 it stops being a trail and
/// becomes a brick on the tail: the fan is as wide as the bell, so a plume that
/// is not several times longer than it is wide has no shape to show.
const TRAIL_PER_RADIUS: f32 = 12.0;
/// What full warp multiplies that reach by, quadratic in the warp ramp.
///
/// This looks unreachable and is not. A drive that is *lit* throws its lance at
/// the frame edge and has no use for a multiple of the hull — but `warp_engaged`
/// goes false the instant the drive is switched off, while the ship is still
/// superluminal and the warp ramp is still most of the way up. So this is what
/// shapes the plume through the spin-down, and only through the spin-down.
///
/// The same *shape* as the ramp the sky's own exposure takes in
/// [`crate::universe`], and deliberately not the same number. Taken literally —
/// exhaust as a parcel of a certain age, streaming astern at the ship's own
/// speed, the way a star's streak is one step of its own motion — a quarter
/// second of it at full warp is 195 world units, some fourteen screen widths,
/// by which point `draw_streak`'s length falloff has divided the light by two
/// hundred and the drive goes *dark* exactly when it is hottest. The warp ramp
/// is the compressed handle on that 42-to-780 range every other effect here
/// already takes, and it is taken here for the same reason.
const TRAIL_STRETCH: f32 = 2.8;
/// Per-lane brightness of a plume, before the length falloff spreads it.
///
/// Tuned against the hottest case rather than the average one, and it is a good
/// deal hotter than it looks. The camera is on the beam, so hull `x` is almost
/// pure camera *depth*: a symmetric pair of bells projects onto the same
/// subpixels and the two plumes add. So do neighbouring lanes of one fan, since
/// five of them cross two subpixels. Between them the common case is four times
/// nominal, and the first value tried here — set against one lane of one bell —
/// put the Enterprise at impulse through the tonemap as a saturated brick
/// brighter than the hull it came out of.
///
/// It is now the brightness at the *nozzle*, because `draw_trail` divides
/// `Canvas::streak_spread` back out — which is why the figure is so much
/// smaller than it reads: the value it replaced was per lane before that
/// division, and at the reference framing the spread is a factor of eleven.
const TRAIL_INTENSITY: f32 = 0.045;
/// How much brighter a lit warp drive burns than a lit impulse one.
///
/// Without it the drive reads *dimmer* the faster it goes: `draw_streak`
/// spreads a streak's light over its length, so the same exhaust stretched
/// three times as far is a third as bright per subpixel. Total light still
/// climbs, but the eye reads the head of the plume, not the integral of it.
const TRAIL_WARP_LIFT: f32 = 1.5;
/// The middle of the fleet's range of bell radii. Brightness is scaled by the
/// ratio to it, so the Enterprise's impulse engine — "much the smaller", by its
/// own comment — does not throw the same plume as its nacelles.
const NOMINAL_BELL: f32 = 0.12;
/// The most streaks one plume is drawn from.
///
/// The count itself is not a constant: it follows the fan's width in subpixels,
/// so the lanes are always about one apart. A fixed count cannot do that at
/// both ends of the dolly, and getting it wrong is worse than it sounds — five
/// lanes over a fan thirteen subpixels wide came out as a *broom*, five
/// distinct diverging lines with black between them, while the same five over
/// two subpixels is four wasted streaks laid on top of each other. Spacing
/// them at a subpixel also makes the brightness self-normalising, since each
/// column of the plume's cross-section then gets light from about one lane
/// however many there are.
///
/// The cap is the only part that is arbitrary, and it only binds on a terminal
/// large enough that nine already look continuous.
const MAX_PLUME_LANES: usize = 9;
/// Half-width of the fan where it leaves the bell, and where it is widest, both
/// as multiples of the bell's own radius on screen. Exhaust leaves a nozzle
/// narrower than the nozzle and expands into vacuum, and lanes that all
/// converged on one point would pile their whole light onto the single subpixel
/// the bell's glow already owns.
const PLUME_THROAT: f32 = 0.7;
const PLUME_FLARE: f32 = 1.6;
/// How much shorter the outermost lane of the fan is than the centre one.
///
/// This is what gives the plume a silhouette instead of an outline. Lanes of
/// equal length draw a *rectangle*: every tail lands on one line straight
/// across the flow and the sides stop dead at the outermost lane, so the whole
/// thing reads as a block bolted to the tail. Shortening the outer lanes
/// carries the tails round in a curve from the widest point to a tip, which is
/// the shape a flame has.
///
/// It used to be worse, and the argument here used to lean on why: a streak ran
/// out at a floor of a third rather than at nothing, so the flat far end was
/// laid down bright as well as flat. The floor is gone — see
/// [`Canvas::draw_streak`], where a star's tail is now where the shutter closes
/// rather than where it was a frame ago — so the end fades whatever the taper
/// does, and what is left of the argument is the outline itself.
///
/// A lance does not want one, so the figure eases toward `PLUME_TAPER_AT_WARP`
/// as the drive spools: at frame length the flame's taper puts the widest point
/// somewhere in the middle of the view and leaves a single thin lane carrying
/// on past it.
const PLUME_TAPER: f32 = 0.55;
const PLUME_TAPER_AT_WARP: f32 = 0.15;
/// How much dimmer the outermost lane is than the centre one, quadratically.
/// The skirt of a plume is not its core, and an edge that ends at full value
/// draws a line down each side of it.
const PLUME_EDGE_FADE: f32 = 0.8;
/// Where a plume is cut in the camera's space.
///
/// A hair beyond the plane [`Camera::project`] gives up at, because it *drops*
/// a point it cannot see rather than clipping it — so an uncut plume does not
/// shorten as it swings toward the eye, it vanishes whole, and a drive that
/// blinks out under hard yaw reads as a fault rather than as a lean.
const PLUME_NEAR: f32 = camera::Z_NEAR * 1.05;
/// How hard the flame gutters at impulse, and at warp.
///
/// Not the same number, and the difference is the point: a flame burning in a
/// bell is an unsteady thing, and a warp field is a continuum. Killing it
/// entirely at warp was tried and reads as a decal; leaving the impulse figure
/// on reads as a fault in the drive.
const FLICKER_AT_IMPULSE: f32 = 0.30;
const FLICKER_AT_WARP: f32 = 0.10;
/// How fast it gutters, in radians per second, and the two incommensurate rates
/// it is beaten between so it never comes out a clean sine — the same trick the
/// camera shake uses, and for the same reason.
const FLICKER_RATE: f64 = 11.0;
const FLICKER_BEAT: f64 = 0.61;
/// Phase offsets, per bell and per lane of the fan. The first stops a hull with
/// several bells pulsing in lockstep; the second is small on purpose, because a
/// fan whose lanes disagree strongly reads as static rather than as fire.
const FLICKER_PER_BELL: f64 = 2.1;
const FLICKER_PER_LANE: f64 = 0.37;
/// How far the drive catching throws the plume, on top of `Ship::flash`: how
/// much brighter it burns, and how much further it reaches.
///
/// The screen-wide white-out covers the first instant of an engage; this is
/// what is left as that fades, and it is the only part of the moment the ship
/// itself does rather than the frame.
///
/// Mostly brightness, and the split is not cosmetic. A surge that only
/// lengthened the plume would make it *dimmer*: `draw_streak` spreads a
/// streak's light over its length, so stretching the flame two and a half times
/// while putting no more light into it divides what every subpixel of it gets
/// by very nearly the same figure. The first version of this did exactly that,
/// and a drive catching came out as a flame that went thin and grey.
const TRAIL_SURGE: f32 = 1.6;
const TRAIL_SURGE_REACH: f32 = 0.6;
/// What is left of a plume while the drive is spinning down. Speed alone cannot
/// say this: from out here a dropout and a throttle eased back are the same
/// falling number, and only one of them should put the flame out.
const TRAIL_DROPOUT: f32 = 0.35;

/// How far the track has to be turned out of the image plane before the hull
/// counts as wholly in front of the drive, as the sine of that angle. About six
/// degrees.
///
/// The hull hides the drive or it does not, and [`drive_behind_hull`] would be a
/// `bool` if the beam were not exactly where the answer runs out. Square to the
/// track the bells sit *on* the silhouette's edge, half in and half out, and
/// neither answer is the right one — so a hard swap there is a step, and it was
/// measured before this ramp went in: crossing the beam moved a subpixel by 137
/// of 255 and shifted thirty of them at once, on a ship the pilot had not
/// touched. Sixty times a second, a weave of under a degree either side of
/// square is enough to sit on that edge and blink.
///
/// Both ends are held. Narrower and the step comes back; wider and the drive
/// goes on shining through a hull that is plainly in front of it. Six degrees is
/// one press of a camera key ([`crate::view::ORBIT_STEP`]) and about nine times
/// the lean the autopilot's weave holds, which is the pair of scales that matter
/// — a control that swings the shot cannot outrun the swap, and a ship nobody is
/// flying cannot cross it. Both are pinned rather than left as prose:
/// `the_swap_takes_a_whole_press_of_the_key_that_crosses_it` and
/// `a_ship_nobody_is_flying_never_crosses_the_swap`, which flies the real
/// autopilot and finds it a ninth of the way over.
const OCCLUSION_BAND: f32 = 0.1045;

/// Lean the hull takes from a turn, in radians at full deflection. The
/// camera rides with the ship, so this is the only thing that says a turn
/// is happening.
const YAW_LEAN: f32 = 0.35;
/// Lean the hull takes from a pull on the stick, in radians at full deflection.
/// A little under the yaw's, because the two are not equally visible from here:
/// pitch turns the hull in the plane of the screen, where every degree of it
/// shows, while a yaw mostly swings the nose toward the camera and needs the
/// wider angle to read as anything at all.
const PITCH_LEAN: f32 = 0.30;

/// A glowing engine bell.
#[derive(Debug, Clone, Copy)]
pub struct Engine {
    pub at: [f32; 3],
    /// In hull units, scaled to the screen along with everything else.
    pub radius: f32,
}

/// One ship.
pub struct ShipModel {
    /// Lowercase ASCII: what `--ship` takes.
    pub name: &'static str,
    /// One line about it, for the picker.
    pub blurb: &'static str,
    pub verts: Vec<[f32; 3]>,
    /// Convex faces, wound anticlockwise seen from *outside* the hull.
    pub faces: Vec<Vec<u16>>,
    pub engines: Vec<Engine>,
    /// What the plates are painted, before the light gets to them.
    pub hull: [f32; 3],
}

/// A cross-section of a hull: a rectangle on a plane across the ship.
#[derive(Debug, Clone, Copy)]
struct Section {
    z: f32,
    /// Where the rectangle's centre sits across and up the ship.
    cx: f32,
    cy: f32,
    /// Half-width and half-height.
    hx: f32,
    hy: f32,
}

impl Section {
    const fn at(z: f32, hx: f32, hy: f32) -> Self {
        Section {
            z,
            cx: 0.0,
            cy: 0.0,
            hx,
            hy,
        }
    }

    const fn offset(z: f32, cx: f32, cy: f32, hx: f32, hy: f32) -> Self {
        Section { z, cx, cy, hx, hy }
    }

    /// The four corners, port-up first and going round the way that leaves the
    /// side faces and the caps below wound outward.
    fn corners(&self) -> [[f32; 3]; 4] {
        [
            [self.cx - self.hx, self.cy - self.hy, self.z],
            [self.cx + self.hx, self.cy - self.hy, self.z],
            [self.cx + self.hx, self.cy + self.hy, self.z],
            [self.cx - self.hx, self.cy + self.hy, self.z],
        ]
    }

    /// The same outline cut into `sides` points instead of four, going round
    /// the same way the corners do.
    ///
    /// At four this *is* [`Self::corners`], handed back untouched rather than
    /// recomputed. That is not an optimisation: the general form below would
    /// return a right angle a fraction of an ulp off square, and both ships in
    /// the hangar are lofted through this — moving either by an ulp repaints a
    /// sky nothing asked to have repainted.
    ///
    /// Past four, the outline is the polygon that *circumscribes* the ellipse
    /// the rectangle encloses: vertex `k` at `π + π/n + k·2π/n`, pushed out by
    /// `1/cos(π/n)` so the edges touch the ellipse rather than the corners
    /// sitting on it. That is exactly what makes the four-sided case the
    /// rectangle and not the diamond inscribed in it, which is what lets one
    /// spelling serve both.
    ///
    /// Keep `sides` a multiple of four. `hx` and `hy` are half-extents at 4, 8
    /// and 12 because a vertex lands on each axis exactly; at ten the top and
    /// bottom of the ring overshoot `hy` by five percent and a section stops
    /// meaning what its fields are named.
    fn ring(&self, sides: usize) -> Vec<[f32; 3]> {
        if sides == 4 {
            return self.corners().to_vec();
        }
        let step = std::f32::consts::TAU / sides as f32;
        let out = 1.0 / (step * 0.5).cos();
        (0..sides)
            .map(|k| {
                let theta = std::f32::consts::PI + step * 0.5 + k as f32 * step;
                [
                    self.cx + self.hx * theta.cos() * out,
                    self.cy + self.hy * theta.sin() * out,
                    self.z,
                ]
            })
            .collect()
    }
}

/// Somewhere to accumulate a hull while it is being assembled.
#[derive(Default)]
struct Builder {
    verts: Vec<[f32; 3]>,
    faces: Vec<Vec<u16>>,
}

impl Builder {
    fn push_ring(&mut self, points: impl IntoIterator<Item = [f32; 3]>) -> u16 {
        let base = self.verts.len() as u16;
        self.verts.extend(points);
        base
    }

    /// A closed solid lofted through a run of cross-sections, aft to fore.
    ///
    /// This is the workhorse: a fuselage is a shell, and so is a wing, a fin, a
    /// cargo block and an engine housing. Anything that would be a flat plate
    /// is given a little thickness instead, so it stays a solid.
    fn shell(&mut self, sections: &[Section]) {
        self.loft(sections, 4);
    }

    /// The same, with each section's rectangle cut into `sides` points.
    ///
    /// A rectangle lofted along the track is a box, and a box is the wrong
    /// shape for the two things this ship is mostly made of: a saucer is a disc
    /// seen from above and a nacelle is a tube seen from anywhere. The count is
    /// per solid rather than per hull, so a flat blade and a thin strut go on
    /// costing four points each while the shapes that need to be round pay for
    /// being round.
    ///
    /// [`Self::shell`] is this at four, and [`Section::ring`] is written so it
    /// is that *bit for bit* — everything below is the arithmetic that was
    /// already here, over a ring of `n` points instead of a ring of 4.
    fn loft(&mut self, sections: &[Section], sides: usize) {
        assert!(sections.len() >= 2, "a shell needs two sections to loft");
        assert!(sides >= 3, "a ring needs three points to enclose anything");
        let rings: Vec<u16> = sections
            .iter()
            .map(|s| self.push_ring(s.ring(sides)))
            .collect();
        self.skin(&rings, sides);
    }

    /// The faces of a run of rings already pushed: a band of quads between each
    /// neighbouring pair, and a cap at each end.
    ///
    /// Split out of [`Self::loft`] so [`Self::leaned_shell`] can lay its own
    /// points down and still be wired up by the one piece of code that knows
    /// which way round a ring goes. Nothing here is arithmetic — every value in
    /// it is a vertex index — so the split cannot move a hull by a bit, which
    /// matters because the enterprise is lofted through it.
    fn skin(&mut self, rings: &[u16], sides: usize) {
        let n = sides as u16;
        for pair in rings.windows(2) {
            let (aft, fore) = (pair[0], pair[1]);
            for i in 0..n {
                let j = (i + 1) % n;
                self.faces.push(vec![aft + i, aft + j, fore + j, fore + i]);
            }
        }
        // The nose cap faces forward; the tail cap is the same ring the other
        // way about, so its normal points aft.
        let fore = rings[rings.len() - 1];
        self.faces.push((0..n).map(|i| fore + i).collect());
        let aft = rings[0];
        self.faces.push((0..n).rev().map(|i| aft + i).collect());
    }

    /// A thin box, for a wing or a fin: the same solid a plate would be if a
    /// plate had an outside.
    fn plate(&mut self, aft: Section, fore: Section) {
        self.shell(&[aft, fore]);
    }

    /// A shell leaned about the ship's own axis, pivoted on a point in the
    /// plane across the track.
    ///
    /// [`Section`]'s rectangle is axis-aligned in `x` and `y`, so one `cy`
    /// covers a whole span and the only lean a run of them can describe is a
    /// lean toward the *rear* — each section dropping a little as the sections
    /// walk aft. A wing that hangs down toward its tip while staying level fore
    /// and aft is the other rotation entirely, and no arrangement of `cy` will
    /// spell it.
    ///
    /// This is that rotation and nothing else: the same [`Section::ring`]
    /// points, turned about `z` around `pivot` before they are pushed, then
    /// handed to the same [`Self::skin`] every other solid here goes through.
    /// The winding still comes out right *by construction*, which is the whole
    /// reason this is a primitive rather than a wing stepped outboard in three
    /// separate plates: a turn about the axis a ring lies across is a turn
    /// within the ring's own plane and of determinant one, so it moves the four
    /// points and cannot reverse the order they go round in.
    ///
    /// The pivot is the wing's own root rather than the ship's centreline, so
    /// the lean swings the tip down and leaves the root where it was put.
    /// Turning about the centreline would drag the root out of the flank it is
    /// meant to be buried in, and the wing would hang off the hull instead of
    /// out of it.
    fn leaned_shell(&mut self, sections: &[Section], lean: f32, pivot: (f32, f32)) {
        assert!(sections.len() >= 2, "a shell needs two sections to loft");
        const SIDES: usize = 4;
        let (sin, cos) = lean.sin_cos();
        let rings: Vec<u16> = sections
            .iter()
            .map(|s| {
                let turned = s.ring(SIDES).into_iter().map(|p| {
                    let (dx, dy) = (p[0] - pivot.0, p[1] - pivot.1);
                    [
                        pivot.0 + dx * cos - dy * sin,
                        pivot.1 + dx * sin + dy * cos,
                        p[2],
                    ]
                });
                self.push_ring(turned)
            })
            .collect();
        self.skin(&rings, SIDES);
    }

    fn finish(
        self,
        name: &'static str,
        blurb: &'static str,
        hull: [f32; 3],
        engines: Vec<Engine>,
    ) -> ShipModel {
        ShipModel {
            name,
            blurb,
            verts: self.verts,
            faces: self.faces,
            engines,
            hull,
        }
    }
}

fn engine(at: [f32; 3], radius: f32) -> Engine {
    Engine { at, radius }
}

/// Every ship, in the order the picker lists them.
pub fn models() -> &'static [ShipModel] {
    static MODELS: OnceLock<Vec<ShipModel>> = OnceLock::new();
    MODELS.get_or_init(|| vec![enterprise(), normandy()])
}

/// The one flown when nothing has said otherwise: the first in the list.
///
/// It does not *supply* that default — `--ship` carries the name as a string,
/// because clap wants one and an index would be a poor thing to type. Nothing
/// in a running program reads this at all; it exists so that
/// `the_view_and_the_ship_can_be_chosen_at_the_command_line` in `cli.rs` can
/// hold clap's `default_value` and the head of this list to each other, which
/// is a thing no code path would otherwise notice going wrong. Hence the gate:
/// an agreement between two spellings is worth pinning and worth not shipping.
#[cfg(test)]
pub const DEFAULT_MODEL: usize = 0;

/// Look a ship up by the name `--ship` and the picker use.
pub fn by_name(name: &str) -> Option<usize> {
    let name = name.trim().to_ascii_lowercase();
    models().iter().position(|m| m.name == name)
}

/// The saucer's radius, in hull units.
///
/// The one length the rest of this ship is measured against, and the only place
/// the unit box is spent by hand: the ship is `4.523` saucer radii from the
/// front of the disc to the back of a nacelle, and pinning the nose at `+1.0`
/// and the tail where it has always been leaves the disc this big.
const SAUCER_R: f32 = 0.415;
/// Where the disc's centre sits along the track, which puts its front edge on
/// the nose.
const SAUCER_Z: f32 = 1.0 - SAUCER_R;
/// And where its plane sits across the track: far enough above centre that the
/// engineering hull hanging under it and the nacelles standing over it come out
/// either side of the axis the ship rolls about.
const SAUCER_Y: f32 = -0.172;

/// How much taller than scale this hull is drawn.
///
/// At true proportion the ship is 0.44 units from the top of a nacelle to the
/// bottom of the engineering hull against 1.88 along the track, and the shot
/// opens with one unit about seven subpixels — so the neck, the pylons and the
/// gap under the nacelles would each come out thinner than a subpixel, which is
/// nothing at all rather than a thin thing. Every offset from the saucer's
/// plane and every girth is stretched by this; the saucer's own radius and
/// every length along the track are left honest, so the plan view is the ship's
/// and only the profile is flattered.
///
/// It stood at about 2.15 before, unwritten and spread across two dozen
/// literals, which read as a ship built out of pipes. Pulled up by the framing
/// the shot opens on and pulled down by every framing past it, since the zoom
/// goes to 2.8 and a terminal can be far taller than thirty rows.
const STOUT: f32 = 1.4;

/// Saucer, neck, engineering hull, two nacelles on pylons — a bow to the ship
/// every warp drive since has been drawn against.
///
/// Its proportions are measured rather than judged, off a reference mesh, and
/// they are written here in units of the saucer's own radius so that what is on
/// the page is the ratio and not somebody's arithmetic. The five that carry the
/// ship: the disc is a fifth of the length across and centred a radius back from
/// the nose; the engineering hull is `0.61` radii below its plane and only
/// `1.67` long; the nacelles are `0.65` radii out — *inside* the disc's rim, not
/// level with it — and `0.12` radii **above** the plane, tops square with the
/// bridge; and the neck leans forward as it climbs, because on this ship it
/// does.
///
/// That the nacelles stand above the saucer and not below it is the whole of
/// what makes the profile this ship's rather than a flying saucer with engines
/// bolted under it. It is worth saying because the opposite was asserted here
/// for a long time and drawn that way, and the silhouette that came out was
/// wrong in a manner nothing in the tree could notice: every line in the right
/// place and the stack upside down.
///
/// The one deliberate lie is [`STOUT`], and it is only ever told across the
/// track. Seen from the beam the two nacelles line up into one, which is the
/// silhouette everybody already has in mind; seen from above — which the camera
/// can now do — the saucer has to be a disc, and that is why it is lofted
/// through a circle's own chords instead of being a box with the corners
/// knocked off.
///
/// [`SHIP_SCREEN_FRAC`]: crate::view::SHIP_SCREEN_FRAC
fn enterprise() -> ShipModel {
    let mut b = Builder::default();

    // Reference units into hull units. Along and across the track the ship is
    // drawn at scale; up and down it is drawn `STOUT` times taller, and so is
    // every girth, which is the same lie told about the same axis twice.
    let z_of = |z: f32| SAUCER_Z + z * SAUCER_R;
    let x_of = |x: f32| x * SAUCER_R;
    let y_of = |up: f32| SAUCER_Y - up * SAUCER_R * STOUT;
    let girth = |g: f32| g * SAUCER_R * STOUT;

    // The saucer, lofted along the track through the disc's own chords: the
    // half-width at each station is the circle's, so the plan outline is round
    // by construction rather than by choosing flattering numbers. Seven
    // stations, bunched toward the rim where the chord falls away fastest —
    // evenly spaced ones cut the front and back off the disc.
    //
    // The profile is the reference's: a crown rising to `0.155` over the middle
    // and an underside dropping to `0.107` under the rim, both closing on the
    // rim as a station runs out of disc to be. It is one solid; the bridge and
    // the sensor dome are their own, because a lens cannot grow a lump.
    let saucer: Vec<Section> = [-0.97f32, -0.78, -0.45, 0.0, 0.45, 0.78, 0.97]
        .iter()
        .map(|&u| {
            let chord = (1.0 - u * u).sqrt();
            let (top, bottom) = (y_of(0.021 + 0.134 * chord * chord), y_of(-0.107 * chord));
            Section::offset(
                z_of(u),
                0.0,
                (top + bottom) * 0.5,
                x_of(chord),
                (bottom - top) * 0.5,
            )
        })
        .collect();
    b.loft(&saucer, 8);
    // The bridge, and the deck that swells up to meet it: the thing that says
    // which way up the ship goes, and the only part of the disc the eye can
    // find the middle of.
    b.loft(
        &[
            Section::offset(z_of(-0.22), 0.0, y_of(0.150), x_of(0.14), girth(0.040)),
            Section::offset(z_of(0.00), 0.0, y_of(0.178), x_of(0.22), girth(0.048)),
            Section::offset(z_of(0.22), 0.0, y_of(0.150), x_of(0.14), girth(0.040)),
        ],
        8,
    );
    // And the sensor dome hanging under the middle of it.
    b.loft(
        &[
            Section::offset(z_of(-0.20), 0.0, y_of(-0.150), x_of(0.12), girth(0.055)),
            Section::offset(z_of(0.00), 0.0, y_of(-0.180), x_of(0.18), girth(0.110)),
            Section::offset(z_of(0.20), 0.0, y_of(-0.150), x_of(0.12), girth(0.055)),
        ],
        8,
    );
    // The impulse deck, across the back of the saucer and standing a little
    // proud of it — which is what gives the bell aft of it something to sit on.
    b.shell(&[
        Section::offset(z_of(-1.021), 0.0, y_of(-0.023), x_of(0.200), girth(0.044)),
        Section::offset(z_of(-0.572), 0.0, y_of(-0.023), x_of(0.264), girth(0.044)),
    ]);
    // The neck, leaning forward as it climbs from the engineering hull. Four
    // points: it is a blade, and a blade seen edge-on is the one shape a round
    // section would spend its points hiding.
    b.shell(&[
        Section::offset(z_of(-1.313), 0.0, y_of(-0.409), girth(0.048), girth(0.100)),
        Section::offset(z_of(-0.373), 0.0, y_of(-0.055), girth(0.048), girth(0.100)),
    ]);
    // The engineering hull: a tube slung below and aft, fattest a third of the
    // way along and tapering both ways. Short — it ends well forward of the
    // nacelles, which is most of what stops this reading as a ship with three
    // engines.
    b.loft(
        &[
            Section::offset(z_of(-2.244), 0.0, y_of(-0.608), girth(0.145), girth(0.145)),
            Section::offset(z_of(-1.900), 0.0, y_of(-0.608), girth(0.205), girth(0.205)),
            Section::offset(z_of(-1.300), 0.0, y_of(-0.608), girth(0.233), girth(0.233)),
            Section::offset(z_of(-0.800), 0.0, y_of(-0.608), girth(0.215), girth(0.215)),
            Section::offset(z_of(-0.579), 0.0, y_of(-0.605), girth(0.171), girth(0.171)),
        ],
        8,
    );
    // The deflector, capping it: a shallow dish standing off the hull's forward
    // end, its aft ring inside the hull's so the two read as one casting.
    b.loft(
        &[
            Section::offset(z_of(-0.579), 0.0, y_of(-0.605), girth(0.127), girth(0.127)),
            Section::offset(z_of(-0.500), 0.0, y_of(-0.605), girth(0.120), girth(0.120)),
            Section::offset(z_of(-0.426), 0.0, y_of(-0.605), girth(0.055), girth(0.055)),
        ],
        8,
    );
    for side in [-1.0f32, 1.0] {
        // A pylon, out of the hull's upper flank and up to the nacelle's
        // underside. On the reference it is a slab standing at one station of
        // the track, which a loft along the track cannot make thin; so the two
        // sections are the two faces of that slab, and the strut leans aft as it
        // climbs by exactly its own thickness. It occupies the z the reference
        // gives it and reads as a strut rather than as the wall a single
        // upright section would have swept.
        b.plate(
            Section::offset(
                z_of(-1.759),
                side * x_of(0.553),
                y_of(-0.039),
                girth(0.060),
                girth(0.045),
            ),
            Section::offset(
                z_of(-1.684),
                side * x_of(0.240),
                y_of(-0.363),
                girth(0.060),
                girth(0.045),
            ),
        );
        // A nacelle: a tube standing *above* the saucer's plane and well aft of
        // it, closer in than the disc's rim, with the bussard rounded off the
        // front. Its aft cap is the tail of the whole ship.
        let (cx, cy) = (side * x_of(0.649), y_of(0.121));
        b.loft(
            &[
                Section::offset(z_of(-3.523), cx, cy, girth(0.102), girth(0.102)),
                Section::offset(z_of(-3.300), cx, cy, girth(0.118), girth(0.118)),
                Section::offset(z_of(-2.400), cx, cy, girth(0.117), girth(0.117)),
                Section::offset(z_of(-1.550), cx, cy, girth(0.129), girth(0.129)),
                Section::offset(z_of(-1.332), cx, cy, girth(0.070), girth(0.070)),
            ],
            8,
        );
        // The intercooler down its inboard flank. Small, and it earns its eight
        // faces from above and from head-on, where two bare tubes otherwise read
        // as two bare tubes.
        b.plate(
            Section::offset(
                z_of(-3.100),
                side * x_of(0.530),
                cy,
                girth(0.020),
                girth(0.042),
            ),
            Section::offset(
                z_of(-1.500),
                side * x_of(0.530),
                cy,
                girth(0.020),
                girth(0.042),
            ),
        );
    }
    b.finish(
        "enterprise",
        "Heavy cruiser. Saucer, neck, and two nacelles.",
        [0.21, 0.24, 0.31],
        vec![
            // The bells keep the radii they had, and are the one thing here not
            // taken off the reference. A bell is a light rather than a plate, so
            // it is allowed to be wider than the cap it sits on — it was 2.2
            // times the old nacelle's and is 1.85 times this one's, so it has
            // come *closer* to the geometry rather than further from it while
            // the tube around it got slimmer.
            //
            // Scaling them down with the hull was tried and put the nacelles at
            // 0.095, which reads the same and measures quite differently: the
            // plume's lane count follows the fan's width in subpixels, and
            // between 0.100 and 0.105 it steps from three lanes to five at a
            // 120x36 terminal and the lance's brightness there very nearly
            // doubles. Below the step the drive burns *brighter* the wider the
            // window, which is the one thing
            // `the_lance_burns_as_brightly_on_any_terminal` exists to catch, and
            // it caught it. The rest of the fleet sits at 0.11 to 0.17 for the
            // same reason, whether or not anyone knew it.
            engine([-x_of(0.649), y_of(0.121), z_of(-3.58)], 0.11),
            engine([x_of(0.649), y_of(0.121), z_of(-3.58)], 0.11),
            // Impulse, out of the back of the saucer, and much the smaller. It
            // fires between the nacelles rather than under them now, and washes
            // their inboard flanks on the way past.
            engine([0.0, y_of(-0.023), z_of(-1.05)], 0.07),
        ],
    )
}

/// The Normandy's reference units into hull units.
///
/// Unlike the enterprise there is no one length here worth measuring the rest of
/// the ship against — no saucer, no radius everything else is a multiple of — so
/// the numbers below stay in the reference mesh's own units and these three do
/// the whole conversion. The effect is the enterprise's: what is on the page is
/// the measurement, and a later reader can hold a station against the mesh
/// without redoing anybody's division.
///
/// **The reference's axes are not this crate's.** There `+x` runs out the nose,
/// `+y` is up and `+z` is to starboard; here `+z` is the nose, `+x` starboard
/// and `+y` *down*. So `z_of` below takes a reference `x`, `x_of` takes a
/// reference `z`, and `y_of` takes a reference `y` and turns it over.
///
/// The hull is 461.61 reference units from the tail cap to the tip of the nose,
/// and it is mapped onto 1.96 rather than the whole 2.0 the box allows. That
/// margin is not timidity: `every_model_fits_in_the_unit_box` asks for
/// `|c| <= 1.0`, and a scale that lands the nose exactly on the wall is one ulp
/// of rounding away from a red test for nothing gained. The enterprise's nose
/// sits at 0.9876 for the same reason.
const NORMANDY_SCALE: f32 = 1.96 / 461.61;
/// The station along the reference that lands on `z = 0`: halfway between the
/// tail cap at `-121.58` and the nose at `+340.03`.
const NORMANDY_MID: f32 = 109.225;
/// The height that lands on `y = 0`, and it is the *tail cap's own centre*
/// rather than the mesh's mid-height or the hull's.
///
/// That is a choice and it earns itself twice: it is what the hull rolls about,
/// and it is where the one bell sits — which is why
/// `the_plume_keeps_its_width_when_the_ship_rolls` is flown on this ship. A
/// plume on the axis lands in the same place at every roll, so the only thing
/// left that can move is the width of the fan around it.
const NORMANDY_DATUM: f32 = 55.91;

/// How much taller than scale this hull is drawn. [`STOUT`], told about the same
/// axis and for very nearly the same reason.
///
/// The Normandy is a flat ship: 40 reference units from its spine to its belly
/// against 462 along the track, where the enterprise is a quarter as long again
/// in proportion. At true depth the fuselage comes out under three subpixels
/// deep at the framing the shot opens on, and
/// `the_drive_does_not_shine_through_the_hull` wants a hundred subpixels the
/// hull covers *whole* from angles that foreshorten its length to a quarter.
///
/// Only the depth is flattered — lengths along the track and spans across it are
/// the mesh's, so the plan view is the ship's. What it costs is that the wings
/// swing a little further from the roll axis, which is what
/// `every_hull_stays_inside_its_own_bubble` is about; at this value the worst
/// vertex sits at 0.62 of the bubble's across-track reach.
const NORMANDY_STOUT: f32 = 1.3;

/// How far the wings hang, in radians, measured down from level at the tip.
///
/// The reference hangs four drive arms under the hull on struts, which is more
/// structure than a ship twenty subpixels long can show — two wings say the same
/// thing and say it at any size. What has to survive the simplification is the
/// Λ the ship makes head-on, and that is this angle and nothing else: flat, and
/// the pair reads as one plank; much past a third of a turn and the ship reads
/// as a dart rather than a frigate.
const WING_LEAN: f32 = 0.42;

/// A frigate: a long flat back, a nose drawn out to a blade, and two wings hung
/// low and leaning down to their tips.
///
/// Measured off a reference mesh the way the enterprise is, in that mesh's own
/// units — see [`NORMANDY_SCALE`] for the conversion and for the axis swap,
/// which is the one thing about the numbers below that cannot be guessed. Eight
/// of the mesh's twenty-one measured frames are kept; the fuselage's outline
/// between them is close enough to linear that the rest were paying for nothing.
///
/// **The two dorsal tail fins are deliberately not here.** The reference carries
/// them — a pair of swept blades over the tail, splaying outboard as they rise
/// to 90.9 against a spine of 78.1 beneath them, each capped by a thin rod, and
/// 2,077 faces of a 691,310-face hull. They are left off and the back is closed
/// as though they never were. That is what makes this the only hull in the
/// hangar with nothing at all standing above its spine, which is most of what
/// separates it from the enterprise's stack at the size either is drawn.
/// Nothing tests it, because nothing honest can — a bound on how high a vertex
/// may go restates the station table, and a rule about the tallest point passes
/// just as well with the blades put back, since the wings hang further below the
/// axis than the fins ever stood above it. So this paragraph is the guard: they
/// are absent by decision, not by oversight, and putting them back is a change
/// rather than a fix.
///
/// What is measured and what is drawn in, since "measured" is worth nothing
/// without the other half: the eight stations, the two shoulder plates and how
/// far out and how far down the wings reach are the mesh's. The wings'
/// planform, the lean, the nose cap and the bell are not.
///
/// One correction the mesh will not volunteer. Its widest frames amidships read
/// 38 units out, and that is the *shoulder plates* — two thin panels standing
/// proud of the flank between stations `-32` and `+62` — not the fuselage, which
/// is 26 there and is measured with them cut away. Anyone re-measuring off the
/// bounding box will "fix" the fuselage back out to 38 and lose the panels'
/// shadow line with it.
fn normandy() -> ShipModel {
    let mut b = Builder::default();

    // Reference units into hull units. Along and across the track the ship is
    // drawn at scale; up and down it is drawn `NORMANDY_STOUT` times deeper,
    // and so is every girth, which is the same lie told about the same axis
    // twice.
    let z_of = |along: f32| (along - NORMANDY_MID) * NORMANDY_SCALE;
    let x_of = |across: f32| across * NORMANDY_SCALE;
    let y_of = |up: f32| (NORMANDY_DATUM - up) * NORMANDY_SCALE * NORMANDY_STOUT;
    let girth = |g: f32| g * NORMANDY_SCALE * NORMANDY_STOUT;
    // One frame of the fuselage, from the four numbers a frame is actually
    // measured in: how far along, how far out, and where its spine and its
    // belly sit. Section::offset wants a centre and two half-extents, which is
    // the same frame after two subtractions nobody should have to check.
    let station = |along: f32, half: f32, top: f32, bottom: f32| {
        Section::offset(
            z_of(along),
            0.0,
            y_of((top + bottom) * 0.5),
            x_of(half),
            girth((top - bottom) * 0.5),
        )
    };

    // The fuselage. Eight sides rather than four because the camera goes over
    // the top and round the front out here, and a four-sided loft would show a
    // rectangle head-on and a slab from above; this ship is a rounded lens in
    // section for its whole length.
    b.loft(
        &[
            // The tail cap, and the only frame written out rather than passed
            // through `station`: `y = 0` is this cap's own centre, so through
            // `station` it would come back a hundred-millionth off zero — a
            // subtraction pretending to be a fact about where the drive sits.
            Section::at(z_of(-121.6), x_of(16.5), girth(5.2)),
            station(-95.0, 19.3, 69.7, 43.2),
            station(-65.0, 22.2, 73.2, 40.0),
            station(-35.0, 23.0, 75.7, 38.5),
            station(-5.0, 24.3, 77.6, 38.5),
            station(40.0, 26.0, 78.1, 38.7),
            station(90.0, 26.2, 77.6, 44.9),
            station(160.0, 23.5, 74.8, 50.9),
            station(235.0, 18.5, 68.4, 43.5),
            station(300.0, 14.0, 58.0, 43.4),
            station(340.0, 12.0, 47.1, 42.4),
        ],
        8,
    );

    for side in [-1.0f32, 1.0] {
        // The shoulder panel down the flank: a thin plate standing proud of the
        // skin over the middle third of the ship. It is what the mesh's widest
        // frames are, and it is a `plate` because that is what it is — a slab
        // at one stretch of the track, which a loft *along* the track cannot
        // make thin.
        b.plate(
            Section::offset(
                z_of(-32.0),
                side * x_of(31.5),
                y_of(52.6),
                x_of(7.0),
                girth(3.4),
            ),
            Section::offset(
                z_of(61.0),
                side * x_of(30.5),
                y_of(52.4),
                x_of(7.5),
                girth(4.9),
            ),
        );

        // The wing. Four frames, and each one is doing a job that shows.
        //
        // Every frame carries the same `cy`, so the wing has no pitch at all —
        // the whole of its droop is `WING_LEAN`, turned about the ship's own
        // axis on the root. A wing leaned by walking `cy` aft instead comes out
        // drooping toward the *tail*, which reads as a broken plank rather than
        // as anhedral, and makes no Λ at all from head-on.
        //
        // Aft to fore: the trailing tip pulled back inboard so the tip rakes
        // rather than ending square on a full chord; the widest frame, which is
        // the tip proper; the root; and a narrow frame tucked *inside* the
        // skin, which is what draws the leading edge out to a point and runs
        // the wing into the flank instead of stopping it against one.
        b.leaned_shell(
            &[
                Section::offset(
                    z_of(-112.0),
                    side * x_of(45.0),
                    y_of(44.0),
                    x_of(33.0),
                    girth(3.5),
                ),
                Section::offset(
                    z_of(-45.0),
                    side * x_of(62.0),
                    y_of(44.0),
                    x_of(50.0),
                    girth(4.5),
                ),
                Section::offset(
                    z_of(30.0),
                    side * x_of(40.0),
                    y_of(44.0),
                    x_of(30.0),
                    girth(5.5),
                ),
                Section::offset(
                    z_of(170.0),
                    side * x_of(11.0),
                    y_of(44.0),
                    x_of(8.0),
                    girth(4.5),
                ),
            ],
            side * WING_LEAN,
            (side * x_of(14.0), y_of(44.0)),
        );
    }

    b.finish(
        "normandy",
        "Frigate. Flat back, wings hung low.",
        // Near-neutral and the palest hull in the hangar, against the
        // enterprise's blue-grey. There is a ceiling on this and it is not
        // obvious: `light_at` sums the three channels, and
        // `a_hull_turned_by_less_than_a_subpixel_moves_by_less_than_a_subpixel`
        // holds a subpixel's step under 0.3 — at three samples a coverage step
        // is a ninth of a plate's light, so the sum wants to stay under about
        // one. This comes to 0.93.
        [0.30, 0.30, 0.33],
        vec![
            // One bell, on the ship's own axis, out of the middle of the tail
            // cap. The reference puts its drive there and nowhere else, and it
            // is the only hull here with a single bell — which is what makes it
            // the ship the roll test can ask its question of.
            //
            // 0.15 because this one bell carries the whole drive, and because
            // below about 0.10 the plume's lane count starts stepping between
            // terminal sizes and `the_lance_burns_as_brightly_on_any_terminal`
            // catches the jump.
            engine([0.0, 0.0, -1.0], 0.15),
        ],
    )
}

/// The attitude the hull is holding, as roll, pitch and yaw in radians.
///
/// Roll is taken as flown: it turns the ship about the very axis it is flying
/// along, so it moves the profile without moving the nose, and the camera does
/// not roll with it — `Q` and `E` read as a barrel roll against a level sky,
/// which is the best thing in this view.
///
/// Pitch and yaw are leans out of the *rates*, and deliberately not out of the
/// accumulated attitude the panel reads out. Out here the direction of travel
/// is the one thing that cannot move: the ship flies where its nose points, the
/// band of sky streams along that track, and there is no horizon for an
/// accumulated angle to be measured against — `heading` and `pitch` are a
/// compass, not a bearing off some fixed frame. Posing the hull from one tips
/// it off the track it is visibly flying along and then leaves it there, which
/// is what pitching about in the cockpit and stepping outside used to look
/// like: nose permanently high, stars still streaming dead level past it. A
/// rate, being a transient, says the pilot is on the stick *now* and hands the
/// ship back to its track the moment it is let go.
fn attitude(ship: &Ship) -> (f32, f32, f32) {
    let roll = ship.roll + ship.bank;
    let pitch = (ship.pitch_rate / MAX_PITCH_RATE).clamp(-1.0, 1.0) * PITCH_LEAN;
    let yaw = (ship.yaw_rate / MAX_YAW_RATE).clamp(-1.0, 1.0) * YAW_LEAN;
    (roll, pitch, yaw)
}

/// Place a hull-space point: roll about the nose, then pitch, then yaw, then
/// out into the camera's space.
///
/// The attitude is the ship's and the eye is the pilot's, and the two are
/// applied in that order for the reason the whole view is built on: the hull
/// leans about *its* axes wherever the camera happens to be watching from, and
/// swinging the camera round must not tip the ship off the track it is
/// visibly flying along.
fn place(v: [f32; 3], (roll, pitch, yaw): (f32, f32, f32), eye: &Eye) -> [f32; 3] {
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();

    // Roll drops the starboard wing, so `+x` turns toward `+y`, which is down.
    let (x, y) = (v[0] * cr - v[1] * sr, v[0] * sr + v[1] * cr);
    // Positive pitch is nose down, and the nose is `+z`, so `+z` turns toward
    // `+y` as well.
    let (y, z) = (y * cp + v[2] * sp, -y * sp + v[2] * cp);
    // Positive yaw is nose to starboard: `+z` turns toward `+x`.
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    eye.to_camera([x, y, z])
}

/// One plate of a hull, ready to draw: where it is on screen, how far off it
/// is, and how much light it is catching.
struct Plate {
    points: Vec<(f32, f32)>,
    depth: f32,
    shade: f32,
}

/// Work out which plates of a hull are facing the camera, and in what order
/// they have to be painted.
///
/// Two things are settled here. Facing comes from the sign of a plate's
/// projected area: the screen's `y` runs downward, which flips the usual sign,
/// so a plate whose outward normal points away from the camera comes out
/// positive — and those are the far side of the hull. That works only because
/// every model is a closed, consistently wound solid, which is what the tests
/// below are for.
///
/// Order comes from the depth sort. Culling alone is enough for a *single*
/// convex solid, whose front faces cannot overlap each other; these hulls are
/// assemblies of half a dozen separate solids, and a nacelle passing in front
/// of an engineering hull is two front-facing plates fighting over the same
/// subpixels. Painting far to near settles it without a depth buffer.
fn plates(model: &ShipModel, cam: &Camera, pose: (f32, f32, f32), eye: &Eye) -> Vec<Plate> {
    let placed: Vec<[f32; 3]> = model.verts.iter().map(|v| place(*v, pose, eye)).collect();
    let screen: Vec<Option<(f32, f32)>> = placed.iter().map(|v| cam.project(*v)).collect();

    let mut plates: Vec<Plate> = Vec::with_capacity(model.faces.len());
    for face in &model.faces {
        // A plate with a vertex behind the near plane cannot be measured, let
        // alone drawn. Nothing should reach that — the whole hull sits clear of
        // it at every zoom, at `MIN_SHIP_DISTANCE` less at most `HULL_REACH`
        // against a near plane of `camera::Z_NEAR`, which is the `const`
        // assertion at the top of this module — but `project` answers with an
        // `Option`, and a rolled fin is exactly the thing that would find out.
        // Stated as the constants rather than as the gap between them, which
        // is what it was: that gap has moved every time `SHIP_SCREEN_FRAC`
        // did, and the number written here had stopped following it. Now that
        // the zoom moves it every frame, an assertion is the only form of it
        // that can keep up.
        let Some(points) = face
            .iter()
            .map(|i| screen[*i as usize])
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if signed_area(&points) >= 0.0 {
            continue;
        }

        let depth = face
            .iter()
            .map(|i| placed[*i as usize][2])
            .sum::<f32>()
            .max(f32::MIN_POSITIVE)
            / face.len() as f32;
        let normal = normal_of(&placed, face);
        // Lambert, and nothing more: one light, no falloff, no specular. The
        // job is to say which way a plate is pointing, and at this resolution
        // anything subtler is spent on a subpixel nobody can see.
        let facing = normal[0] * LIGHT[0] + normal[1] * LIGHT[1] + normal[2] * LIGHT[2];
        plates.push(Plate {
            points,
            depth,
            shade: AMBIENT + DIFFUSE * facing.clamp(0.0, 1.0),
        });
    }
    plates.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    plates
}

/// Draw the hull over whatever is already on the canvas.
///
/// The plates are opaque: they cover the sky rather than adding to it, which is
/// what makes the ship a ship and not a hologram of one. There is still no
/// depth buffer and there still does not need to be one — the star band starts
/// well beyond the hull at every zoom, so nothing can come between it and the
/// camera, and the hull sorts against itself.
///
/// The whole hull goes to the canvas in one call, still in the order it was
/// sorted into. Handing the plates over one at a time is what a painter's
/// algorithm suggests and it is wrong here: an outline measured finer than a
/// subpixel has to compose its coverage once per sample, and a plate blended on
/// its own leaves a share of the sky along every edge it holds in common with
/// its neighbour — a line of it, down the middle of the ship. [`Canvas::fill_hull`]
/// carries the argument in full.
///
/// `time` is here for the flame's gutter and nothing else. It is `f64` because
/// a screensaver is left up for days and an `f32` phase goes coarse enough to
/// stop advancing after about six of them, which would freeze the trails.
pub fn draw(
    canvas: &mut Canvas,
    cam: &Camera,
    ship: &Ship,
    model: &ShipModel,
    eye: &Eye,
    time: f64,
) {
    let pose = attitude(ship);
    let bubble = ship.warp_intensity() * BUBBLE_LIGHT;
    let plates = plates(model, cam, pose, eye);
    let faces: Vec<Facet<'_>> = plates
        .iter()
        .map(|plate| {
            // Nearer plates read a shade brighter. It is a small effect on
            // purpose: the lighting says which way a plate faces, and this only
            // says which of two plates facing the same way is the closer.
            // Measured against the standoff in force rather than a fixed one,
            // so the ratio stays centred on the hull's own middle — and
            // flattens as the camera pulls back, which is what a longer
            // standoff really does to a subject.
            let near = (eye.distance / plate.depth).powf(DEPTH_SHADE);
            let paint = (plate.shade + bubble) * near;
            let mut lit = [0.0; 3];
            for (channel, base) in lit.iter_mut().zip(model.hull) {
                *channel = base * paint;
            }
            Facet {
                points: &plate.points,
                color: lit,
            }
        })
        .collect();

    // Which of the two goes down first is the whole of the occlusion between
    // them: the plates are opaque and the drive is light, so a drive laid down
    // first is covered by whatever hull stands in front of it and shows
    // wherever none does. That is a depth test, at the only granularity a
    // renderer with one opaque write per frame can offer.
    //
    // Both passes rather than one or the other, because the answer at the beam
    // is neither. The drive is drawn twice, sharing its light between the two
    // sides of the hull, and each side is skipped when its share is nothing at
    // all — so square to the track this is exactly the single call after the
    // plates that it has always been, and from ahead it is exactly the single
    // call before them. Everything the drive draws is linear in its intensity,
    // so what the two passes lay down where the hull does not stand is the one
    // pass they were split from.
    let behind = drive_behind_hull(pose, eye);
    let pass = |share| Drive {
        ship,
        model,
        pose,
        eye,
        time,
        share,
    };
    if behind > 0.0 {
        draw_engines(canvas, cam, pass(behind));
    }
    canvas.fill_hull(&faces);
    if behind < 1.0 {
        draw_engines(canvas, cam, pass(1.0 - behind));
    }
}

/// How much of the drive the hull stands in front of: none of it while the
/// bells are still pointed at the camera, all of it once they are turned
/// [`OCCLUSION_BAND`] past square.
///
/// Every bell on every hull in the hangar fires along the ship's own `-z`, so
/// this is one question rather than one per bell: once the exhaust is leaving
/// the ship away from the eye, every nozzle is pointed into the far side of the
/// hull and everything the drive throws is behind it. That is exactly the shot
/// the bug was reported from — from ahead, the enterprise's nacelle bells sat
/// as two blue lamps in the middle of a saucer that should have been hiding
/// them, because [`draw_engines`] ran after [`Canvas::fill_hull`] and
/// everything in it adds.
///
/// The measure is the depth the ship's own axis gains over a unit of its
/// length, which is the sine of how far the track is turned out of the image
/// plane. Asked of the *posed* axis rather than of the camera's azimuth, so the
/// lean the hull is holding is in the answer: a ship yawed toward the eye
/// really has swung its bells round into sight, and [`attitude`] is where that
/// lean lives. Both points go through [`place`], so the standoff sits on both
/// sides of the subtraction and cancels exactly — and at
/// [`crate::view::Orbit::LEVEL`] the ship's axis lies flat in the image plane,
/// the two depths are the same float, and this is a hard zero rather than
/// something very close to one.
fn drive_behind_hull(pose: (f32, f32, f32), eye: &Eye) -> f32 {
    let root = place([0.0, 0.0, 0.0], pose, eye);
    let aft = place([0.0, 0.0, -1.0], pose, eye);
    ((aft[2] - root[2]) / OCCLUSION_BAND).clamp(0.0, 1.0)
}

/// A face's outward normal, unit length, in the camera's space.
fn normal_of(placed: &[[f32; 3]], face: &[u16]) -> [f32; 3] {
    let v = |i: usize| placed[face[i] as usize];
    let (o, a, b) = (v(0), v(1), v(face.len() - 1));
    let (u, w) = (
        [a[0] - o[0], a[1] - o[1], a[2] - o[2]],
        [b[0] - o[0], b[1] - o[1], b[2] - o[2]],
    );
    // `u × w`, with `w` running back around the face rather than on to the
    // next vertex. Faces are wound anticlockwise seen from outside, so this
    // points out of the hull — which the test below pins, because getting it
    // backwards inverts every light in the scene and still looks lit.
    let n = [
        u[1] * w[2] - u[2] * w[1],
        u[2] * w[0] - u[0] * w[2],
        u[0] * w[1] - u[1] * w[0],
    ];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if length > f32::MIN_POSITIVE {
        [n[0] / length, n[1] / length, n[2] / length]
    } else {
        [0.0, 0.0, -1.0]
    }
}

/// One pass of the drive over the canvas: the ship, the pose and the eye it is
/// drawn from, and how much of the flame this pass is carrying.
///
/// A bundle for the same reason [`Flame`] below is one and `render::Exterior` is
/// another — clippy stops at seven arguments and CI runs it as an error — and it
/// is the honest unit besides: [`draw`] makes this call twice with everything
/// the same but the last field.
struct Drive<'a> {
    ship: &'a Ship,
    model: &'a ShipModel,
    pose: (f32, f32, f32),
    eye: &'a Eye,
    time: f64,
    /// How much of the drive this pass is drawing, which is how [`draw`] splits
    /// one flame across both sides of the hull. It multiplies the light and
    /// nothing else — not the reach, not the fan, not the gutter — so the two
    /// passes put the same flame on the canvas that one pass would have, and
    /// the only difference between them is which of the two the plates got to
    /// cover.
    share: f32,
}

/// The drive: a glow out of each bell — blue on impulse, whitening at warp, and
/// out with the throttle — with the exhaust it throws behind it.
///
/// Where this runs relative to the plates is [`draw`]'s decision and not this
/// function's, and it is worth reading the two together. Everything in `Canvas`
/// accumulates except `fill_hull`, so the order is the whole of the occlusion:
/// after the plates the drive shines through the hull, before them the hull
/// covers it.
///
/// Neither answer is right at every angle, which is why it is a question rather
/// than a rule. Once the bells are pointed away from the eye the hull is
/// between the two and the drive belongs underneath — from ahead it is behind
/// the whole ship. Square to the track and behind it the plume is genuinely in
/// front, and it costs something to say so: the Enterprise's impulse engine is
/// the one bell in the fleet that is not on the tail, and its exhaust runs
/// *between* the nacelles rather than clear of them — 0.19 hull units inboard
/// of the nearer flank, 0.009 under their lower edge, and thrown in a fan
/// 0.11 wide. So it misses them in the round and lies straight across them from
/// the beam, where hull `x` is nearly pure camera depth. Drawn under the plates
/// it would be chopped by a silhouette it never touches; drawn over them it
/// shines through as the wash a hot plume genuinely puts on structure it plays
/// over. That is still the cheaper mistake, and it is still the one made — but
/// only on the side of the beam where the plume really is the nearer of the
/// two.
///
/// That same bell is where the answer stops being clean, and it is worth saying
/// rather than discovering. Forward of the beam its own nozzle is behind the
/// saucer and belongs under the plates, while the plume it throws runs aft past
/// two nacelles it can be nearer than — so the bell and its own exhaust want
/// opposite sides of the same frame, and a per-*bell* question could not answer
/// that either, since the plume is in front of the port nacelle and behind the
/// starboard one at the same instant. One order for the drive is the honest
/// limit of a renderer with a single opaque write, and the side it takes is the
/// one the saucer argues for, because the saucer is the thing actually standing
/// in front. Measured at `--orbit 55,35,20` and `--orbit 65,0,0`: what goes is
/// the bell, and the plume runs on unbroken out of the silhouette.
fn draw_engines(canvas: &mut Canvas, cam: &Camera, drive: Drive<'_>) {
    let Drive {
        ship,
        model,
        pose,
        eye,
        time,
        share,
    } = drive;
    let warp = ship.warp_intensity();
    let lit = (ship.speed / crate::ship::CRUISE_MAX)
        .clamp(0.0, 1.0)
        .max(warp);
    if lit <= 0.0 {
        return;
    }
    let mut color = [0.0; 3];
    for i in 0..3 {
        color[i] = IMPULSE_FLAME[i] + (WARP_FLAME[i] - IMPULSE_FLAME[i]) * warp;
    }
    // The drive catching throws the flame; the drive quitting puts it out. The
    // first rides `flash`, which the frame is already using to white itself
    // out, so this is what is left of that moment once the white-out has gone.
    let surge = 1.0 + ship.flash * TRAIL_SURGE;
    let surge_reach = 1.0 + ship.flash * TRAIL_SURGE_REACH;
    let gutter = if ship.dropping_out() {
        TRAIL_DROPOUT
    } else {
        1.0
    };
    let flicker_amt = FLICKER_AT_IMPULSE + (FLICKER_AT_WARP - FLICKER_AT_IMPULSE) * warp;

    for (i, bell) in model.engines.iter().enumerate() {
        let at = place(bell.at, pose, eye);
        let Some(screen) = cam.project(at) else {
            continue;
        };
        let radius = bell.radius * cam.focal / at[2].max(f32::MIN_POSITIVE);
        draw_trail(
            canvas,
            cam,
            Flame {
                bell,
                index: i,
                pose,
                eye,
                root: at,
                head: screen,
                radius,
                color,
                lit,
                warp,
                engaged: ship.warp_engaged,
                surge,
                surge_reach,
                gutter,
                flicker_amt,
                time,
                share,
            },
        );
        canvas.add_glow(
            screen.0,
            screen.1,
            radius * (1.0 + 0.9 * lit),
            color,
            (0.10 + 0.75 * lit) * share,
        );
    }
}

/// One bell's worth of exhaust, and everything the frame already worked out
/// about it.
///
/// A bundle rather than a long argument list: [`draw_trail`] wants fifteen of
/// these and clippy's limit is seven, and the alternative — working the
/// per-frame ones out again inside the loop — would leave the colour and the
/// ramps derived in two places.
struct Flame<'a> {
    bell: &'a Engine,
    /// Which bell this is, used only to stagger the gutter.
    index: usize,
    pose: (f32, f32, f32),
    eye: &'a Eye,
    /// Where the bell sits in the camera's space, and where that projects to.
    root: [f32; 3],
    head: (f32, f32),
    /// The bell's radius on screen, in subpixels.
    radius: f32,
    color: [f32; 3],
    lit: f32,
    warp: f32,
    /// Whether the warp drive is lit, which is what throws the lance at the
    /// frame edge — or at the point the plume vanishes at, whichever the camera
    /// angle puts nearer. Not the same question as `warp > 0.0`: a drive
    /// spinning down is superluminal and switched off at once.
    engaged: bool,
    surge: f32,
    surge_reach: f32,
    gutter: f32,
    flicker_amt: f32,
    time: f64,
    /// How much of this flame the pass being drawn is carrying — see
    /// [`draw_engines`]. One when the drive is drawn on one side of the hull
    /// only, which is every camera angle but the few degrees either side of
    /// square to the track.
    share: f32,
}

/// The exhaust behind one bell: a short fan of streaks laid down the ship's
/// own axis, brightest where it leaves the nozzle.
///
/// Drawn with [`Canvas::draw_streak`] — the primitive the whole sky is made of
/// — because three of the four things a plume wants fall out of it for nothing:
/// the ramp already runs from nothing at `from` to full at `to`, so putting
/// the head at the nozzle brightens the exhaust where it is hottest and runs it
/// out where the plume ends;
/// the length falloff already spreads a long lance instead of letting it burn a
/// solid bar; and it accumulates into the same buffer, tonemapped once with
/// everything else. A chain of glows was the other candidate and the arithmetic
/// killed it: at the reference framing a bell's radius comes to 0.99 subpixels,
/// `add_glow` skips any sample at or past its rim, and it measures the falloff
/// from *integer* subpixel centres — so the chain would need one glow per
/// subpixel of length not to bead into dots, and would crawl as the ship's
/// projected position moved a fraction of a subpixel between frames.
///
/// The fourth thing is width, and that is the fan. Its spread is taken
/// perpendicular in *screen* space rather than in the hull's: hull `x` is
/// almost pure camera depth from this beam and contributes nothing to screen
/// width, while hull `y` alone would collapse the plume to a hairline every
/// time the ship rolled ninety degrees. An axisymmetric plume must not narrow
/// with a roll, and the screen perpendicular cannot.
fn draw_trail(canvas: &mut Canvas, cam: &Camera, flame: Flame<'_>) {
    // Two incommensurate rates, beaten together, so the flame never gutters on
    // a clean sine — and staggered per bell so a hull with several of them does
    // not pulse in lockstep. Evaluated in `f64`, like the camera shake: there are
    // only a handful of these a frame, and it keeps the argument reduction
    // exact however long the process has been up.
    let phase = flame.time * FLICKER_RATE + flame.index as f64 * FLICKER_PER_BELL;
    let gutter_of = |lane: usize| {
        let p = phase + lane as f64 * FLICKER_PER_LANE;
        let beat = (p.sin() * 0.6 + (p * FLICKER_BEAT).sin() * 0.4) as f32;
        1.0 + flame.flicker_amt * beat
    };

    let stretch = 1.0 + flame.warp * flame.warp * TRAIL_STRETCH;
    let reach = flame.bell.radius * TRAIL_PER_RADIUS * flame.lit * stretch * flame.surge_reach;
    if reach <= 0.0 {
        return;
    }

    // Aft along the hull's own axis, posed by the same stack the plates go
    // through, so the plume rolls and leans with the ship and follows the
    // dolly without a constant of its own.
    let tail = [flame.bell.at[0], flame.bell.at[1], flame.bell.at[2] - reach];
    let mut end = place(tail, flame.pose, flame.eye);
    // `place` is a rotation and a translation, so the plume is still a straight
    // segment out here and can be cut against the near plane in closed form.
    // The root can never reach it — that is the `const` assertion at the top of
    // this module — so this only ever shortens.
    if end[2] < PLUME_NEAR {
        let span = flame.root[2] - end[2];
        let t = if span > f32::MIN_POSITIVE {
            ((flame.root[2] - PLUME_NEAR) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        for (axis, root) in end.iter_mut().zip(flame.root) {
            *axis = root + (*axis - root) * t;
        }
    }
    let Some(foot) = cam.project(end) else {
        return;
    };

    // The screen perpendicular. A plume seen exactly end-on has no length to
    // take one from, and is a bell rather than a trail; leave it to the glow.
    let (mut dx, mut dy) = (foot.0 - flame.head.0, foot.1 - flame.head.1);
    let span = dx.hypot(dy);
    if !span.is_finite() || span < 1.0 {
        return;
    }
    let (px, py) = (-dy / span, dx / span);

    // Where this plume's own direction vanishes, if it has such a point, and
    // how far off that is in multiples of the plume's own projected length.
    // The near-plane cut above only shortens `end` toward `root` along the same
    // ray, so this is the plume's direction whatever the cut did to it.
    //
    // Taken as a dot product against the plume's own screen direction and
    // divided by the square of it, so what comes back is the point's range in
    // multiples of the plume — the unit the stretch below is already in — and
    // the sign arrives with it.
    //
    // Strictly greater than one is where it has to land: the foot is a point on
    // the ray and the vanishing point is that ray's limit, so the foot always
    // lies between the nozzle and it. Saying so rather than `> 0.0` is what
    // holds `gone` inside `[0, 1)`, and a value under one would turn the fan
    // inside out on the sublight branch below. It is the arithmetic's backstop
    // rather than a case that arises.
    let horizon = cam
        .vanishing_point([
            end[0] - flame.root[0],
            end[1] - flame.root[1],
            end[2] - flame.root[2],
        ])
        .map(|v| ((v.0 - flame.head.0) * dx + (v.1 - flame.head.1) * dy) / (span * span))
        .filter(|h| h.is_finite() && *h > 1.0);

    // How far along the way to that point the tip finished up. It is also,
    // exactly, one minus the ratio of the tip's depth to the nozzle's — a ray
    // projects so that `P(s) - V` is `(H - V)` times that ratio — which is the
    // factor the far end of the fan is narrower by. So one division answers
    // both questions, and neither needs the depth it is really about.
    //
    // Zero where there is no such point, which is a multiply by one below and
    // the byte-for-byte abeam frame the reference flights are recorded from.
    let mut gone = 0.0f32;

    // A lit warp drive does not trail, it tears: from the beam the lance runs
    // clean off the side of the frame, and it gets there the moment the drive
    // catches rather than growing into it over the warp range. The white-out on
    // `Ship::flash` covers that instant, so the jump arrives under cover.
    //
    // Stretched here, in screen space, on a direction that has already been
    // through the projection and the near-plane cut — so the lean is in it
    // exactly and the cut does not have to be re-derived against a segment
    // several times longer. Solving for the hull-unit length that reaches the
    // edge would need both again and would answer differently at every zoom.
    //
    // Gated on `warp_engaged` and not on the warp ramp, so a drive spinning
    // down loses its lance in the frame it is switched off, while the ship is
    // still doing most of its old speed. That is the read: the drive quits, the
    // ship coasts.
    if flame.engaged {
        // The diagonal reaches the frame edge from anywhere inside it whatever
        // direction the plume is pointing, and `draw_streak` clips, so there is
        // nothing to be gained by working out which edge it leaves by.
        let mut lance = cam.width.hypot(cam.height) / span;
        // The frame is not the only end a lance can have, and where the two
        // disagree the frame is wrong. A plume pointed away from the eye —
        // which is every camera angle forward of the beam — projects onto the
        // segment between the nozzle and the point its direction vanishes at,
        // and it never arrives: past that point there is no exhaust left to
        // draw. Stretched to the frame edge regardless, each bell's lance goes
        // clean through and out the far side, where a symmetric pair swap over
        // and cross, which reads as two drives firing at each other rather than
        // as one ship under way.
        //
        // To the point itself, and not a fraction short of it. That fraction
        // used to be here, and it was covering for a tail floor in
        // `draw_streak` rather than for anything about the geometry: every bell
        // shares this one point, so a lance ending *on* it left a third of full
        // brightness from every lane of every bell on a single subpixel — the
        // artefact relocated rather than removed. Taking the floor away is what
        // paid for the margin: the sample that lands on the point carries a
        // ramp of exactly zero, and there is nothing left for a fraction short
        // of it to buy.
        if let Some(horizon) = horizon {
            lance = lance.min(horizon);
            gone = lance / horizon;
        }
        dx *= lance;
        dy *= lance;
    } else if let Some(horizon) = horizon {
        // A plume the drive is not tearing into a lance is already the segment
        // the projection handed it, so how far it has receded is simply how far
        // along it went.
        gone = 1.0 / horizon;
    }

    let brightness = TRAIL_INTENSITY
        * flame.lit
        * flame.gutter
        * flame.surge
        * (1.0 + flame.warp * TRAIL_WARP_LIFT)
        * (flame.bell.radius / NOMINAL_BELL)
        * flame.share;
    // A flame comes to a tip; a lance runs off the frame in every lane. Left at
    // the flame's figure, a full-length plume draws a wide wedge that peaks
    // somewhere in the middle of the frame with a single thin whisker carrying
    // on past it, which reads as a flame with a hair growing out of it.
    let taper = PLUME_TAPER + (PLUME_TAPER_AT_WARP - PLUME_TAPER) * flame.warp;
    // Enough lanes that they land about a subpixel apart across the widest end
    // of the fan, and no more. Forced odd so there is a centre lane, which is
    // the whole of the plume on a terminal too small for a second.
    let throat = flame.radius * PLUME_THROAT;
    let widest = flame.radius * PLUME_FLARE;
    let count = (((widest * 2.0).ceil() as usize).clamp(1, MAX_PLUME_LANES)) | 1;
    let half = (count - 1) as f32 * 0.5;
    for lane in 0..count {
        // Symmetric about the centre lane, which sits at zero.
        let offset = if half > 0.0 {
            (lane as f32 - half) / half
        } else {
            0.0
        };
        let out = offset * offset;
        // Outer lanes stop short, which is what carries the tails round in a
        // curve from the widest point to a tip. Lanes of equal length draw a
        // rectangle instead — every tail on one line across the flow, and the
        // sides squared off at the outermost lane.
        let shorten = 1.0 - taper * out;
        let nozzle = (
            flame.head.0 + px * offset * throat,
            flame.head.1 + py * offset * throat,
        );
        // The far end of the fan stands further off than the nozzle does, so it
        // is smaller, and `1 - gone` is the ratio of those two depths exactly.
        // Taken against this lane's own length rather than the centre one's: an
        // outer lane stops short, so it has receded less far and narrows less,
        // which is what keeps the fan a fan all the way to the tip instead of
        // pinching every lane onto the same point.
        let flare = widest * (1.0 - gone * shorten);
        let tip = (
            nozzle.0 + (dx + px * offset * flare) * shorten,
            nozzle.1 + (dy + py * offset * flare) * shorten,
        );
        // Divided back out, so what the constants above name is the brightness
        // at the nozzle rather than the brightness of a lane of some particular
        // length. Without it the lance — whose length is the frame's, not the
        // drive's — would burn dimmer the wider the terminal, and the same
        // flight would not look the same on two machines.
        let held = canvas.streak_spread(tip, nozzle);
        canvas.draw_streak(&Streak {
            from: tip,
            to: nozzle,
            color: flame.color,
            intensity: brightness * held * gutter_of(lane) * (1.0 - PLUME_EDGE_FADE * out),
        });
    }
}

/// Twice the area a polygon encloses on screen, signed by its winding.
fn signed_area(points: &[(f32, f32)]) -> f32 {
    let mut total = 0.0;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        total += a.0 * b.1 - b.0 * a.1;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Renderer;
    use crate::term::ColorMode;
    use crate::view::{Orbit, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};
    use std::collections::HashMap;

    /// The standoff at the framing a flight opens on — what these tests mean
    /// when they say "the camera", unless they are about the zoom.
    const STANDOFF: f32 = crate::view::SIDE_FOCAL / crate::view::SHIP_SCREEN_FRAC;

    /// And the eye that goes with it: that standoff, from off the starboard
    /// beam. What "the camera" means here unless a test says otherwise.
    fn abeam() -> Eye {
        Eye::new(Orbit::LEVEL, ZOOM_DEFAULT)
    }

    /// The same, from wherever the camera has been swung to.
    fn eye_at(orbit: Orbit, zoom: f32) -> Eye {
        Eye::new(orbit, zoom)
    }

    fn orbit(azimuth: f32, elevation: f32, roll: f32) -> Orbit {
        Orbit {
            azimuth: azimuth.to_radians(),
            elevation: elevation.to_radians(),
            roll: roll.to_radians(),
        }
        .held()
    }

    /// A spread of camera angles to fly a property through, in degrees: the
    /// shot as it opens, both poles and past them, head-on and dead astern, the
    /// view from port, and corners with all three angles off zero at once.
    ///
    /// The ends are the point. Head-on is where the bubble stops being
    /// elongated and the hull presents its cross-section instead of its
    /// profile; the poles are where the hull's beam swings into the frame's
    /// vertical; and the corners are the only place the outline is turned off
    /// the horizontal at all.
    ///
    /// Past the poles as well, since the elevation stopped being clamped. Those
    /// angles are the same *views* as ones already here — an elevation past the
    /// quarter turn is the far beam inverted — but they reach them by a
    /// different path through `basis`, and it is the path that would break.
    fn orbits() -> Vec<Orbit> {
        vec![
            Orbit::LEVEL,
            orbit(90.0, 0.0, 0.0),
            orbit(-90.0, 0.0, 0.0),
            orbit(180.0, 0.0, 0.0),
            orbit(0.0, 90.0, 0.0),
            orbit(0.0, -90.0, 0.0),
            orbit(0.0, 135.0, 0.0),
            orbit(0.0, -135.0, 0.0),
            orbit(0.0, 180.0, 0.0),
            orbit(0.0, 0.0, 137.0),
            orbit(55.0, 35.0, 20.0),
            orbit(-140.0, -62.0, -100.0),
            orbit(35.0, 89.0, 45.0),
            orbit(115.0, 160.0, -70.0),
        ]
    }

    /// Camera angles that put the ship's own vanishing point on the canvas
    /// *with the hull clear of it*.
    ///
    /// Kept apart from [`orbits()`] rather than folded into it, because that
    /// spread is chosen for coverage of the camera basis — the poles, past
    /// them, the corners — and not one of its angles meets both halves of that.
    /// At `--orbit 90,0,0` the point lands dead centre and the ship is sitting
    /// on top of it; at `--orbit 55,35,20` and the rest of the corners it is off
    /// the top or the side, where a lance overshooting it is clipped away
    /// unseen. Which is how a lance several frame widths too long went
    /// unnoticed for as long as it did: every sweep there was flew straight past
    /// the one question.
    ///
    /// A plume recedes exactly when `cos(elevation) * sin(azimuth)` is positive,
    /// so the whole aft half of the azimuth has no forward vanishing point at
    /// all and an angle taken from it would be a line that never runs. The
    /// mirrors here go over the top instead: a negative azimuth with the
    /// elevation past the pole flips the cosine and puts the same shot back on
    /// the forward side, inverted — which is the only way to fly the other sign
    /// of the screen direction and still have something to measure.
    fn forward_quarter() -> Vec<Orbit> {
        vec![
            orbit(65.0, 0.0, 0.0),
            orbit(75.0, 12.0, 0.0),
            orbit(-70.0, 180.0, 0.0),
            orbit(110.0, -8.0, 25.0),
            orbit(-115.0, 180.0, -25.0),
        ]
    }

    fn cam(cols: usize, rows: usize, ship: &Ship) -> (Renderer, Camera) {
        let renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
        let cam = renderer.exterior_camera(ship, 0.0);
        (renderer, cam)
    }

    #[test]
    fn every_model_is_a_closed_solid() {
        // Every directed edge appears exactly once, and its reverse exactly
        // once — a closed, consistently oriented surface. This is what catches
        // a mistyped index, and it is why the hulls are assembled from
        // primitives rather than entered by hand.
        for model in models() {
            let mut edges: HashMap<(u16, u16), i32> = HashMap::new();
            for face in &model.faces {
                for i in 0..face.len() {
                    let (a, b) = (face[i], face[(i + 1) % face.len()]);
                    assert_ne!(a, b, "{}: a face repeats a vertex", model.name);
                    *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
                    assert!(
                        (a as usize) < model.verts.len(),
                        "{}: a face names vertex {a}, and there are {}",
                        model.name,
                        model.verts.len()
                    );
                }
            }
            for (edge, balance) in edges {
                assert_eq!(
                    balance, 0,
                    "{}: edge {edge:?} is not shared by two faces facing opposite ways",
                    model.name
                );
            }
        }
    }

    #[test]
    fn every_model_is_wound_outward() {
        // A closed surface can be perfectly closed and entirely inside out, and
        // the difference is invisible until half the hull disappears at some
        // particular angle. The divergence theorem settles it: summing the flux
        // of the position field through the faces gives the enclosed volume,
        // which is positive exactly when the winding points outward.
        for model in models() {
            let mut volume = 0.0f32;
            for face in &model.faces {
                let v0 = model.verts[face[0] as usize];
                for i in 1..face.len() - 1 {
                    let a = model.verts[face[i] as usize];
                    let b = model.verts[face[i + 1] as usize];
                    let cross = [
                        a[1] * b[2] - a[2] * b[1],
                        a[2] * b[0] - a[0] * b[2],
                        a[0] * b[1] - a[1] * b[0],
                    ];
                    volume += v0[0] * cross[0] + v0[1] * cross[1] + v0[2] * cross[2];
                }
            }
            assert!(
                volume > 0.05,
                "{} encloses {volume}, so it is inside out or flat",
                model.name
            );
        }
    }

    #[test]
    fn every_model_fits_in_the_unit_box() {
        // The camera's standoff assumes it, and so does the star band's near
        // wall: a hull that reached further could poke through the sky.
        for model in models() {
            for v in &model.verts {
                assert!(
                    v.iter().all(|c| c.is_finite() && c.abs() <= 1.0),
                    "{} has a vertex outside the box: {v:?}",
                    model.name
                );
            }
            for bell in &model.engines {
                assert!(bell.radius > 0.0 && bell.at.iter().all(|c| c.abs() <= 1.1));
            }
        }
    }

    #[test]
    fn every_model_has_a_plain_name_and_they_all_differ() {
        for model in models() {
            assert!(
                model.name.is_ascii() && model.name == model.name.to_lowercase(),
                "{:?} is not a name --ship can take",
                model.name
            );
            assert!(model.blurb.is_ascii() && !model.blurb.is_empty());
            // Only the folded spellings are worth asking about. `by_name` on
            // the name as written is `index_of` with a trim and a lowercase in
            // front of it, and the assertion above has just established that
            // both are no-ops here — so comparing the two would have been an
            // expression against a copy of itself.
            assert_eq!(
                by_name(&model.name.to_uppercase()),
                Some(index_of(model.name))
            );
            assert_eq!(
                by_name(&format!("  {}  ", model.name)),
                Some(index_of(model.name))
            );
        }
        // The half of the name that was never checked: `by_name` and `index_of`
        // both return the *first* match, so two ships sharing a name would have
        // agreed with each other all the way down and passed.
        let mut names: Vec<&str> = models().iter().map(|m| m.name).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "two ships answer to the same name");
        assert_eq!(by_name("no such ship"), None);
        assert!(DEFAULT_MODEL < models().len());
    }

    fn index_of(name: &str) -> usize {
        models().iter().position(|m| m.name == name).unwrap()
    }

    /// How much of the hull is drawn, and where, at a given attitude.
    fn footprint(model: &ShipModel, ship: &Ship, cols: usize, rows: usize) -> (usize, f32) {
        footprint_at(model, ship, cols, rows, 0.0)
    }

    /// The same, at a chosen instant. Only the flame's gutter reads the clock,
    /// so everything that is not about the flicker goes through `footprint` and
    /// gets a fixed one.
    fn footprint_at(
        model: &ShipModel,
        ship: &Ship,
        cols: usize,
        rows: usize,
        time: f64,
    ) -> (usize, f32) {
        let (renderer, cam) = cam(cols, rows, ship);
        let (w, h) = renderer.canvas_dims();
        let mut canvas = Canvas::new(w, h);
        draw(&mut canvas, &cam, ship, model, &abeam(), time);
        let mut lit = 0;
        let mut total = 0.0;
        for y in 0..h {
            for x in 0..w {
                let light = canvas.light_at(x, y);
                assert!(light.is_finite(), "{} drew a NaN", model.name);
                if light > 0.02 {
                    lit += 1;
                    total += light;
                }
            }
        }
        (lit, total)
    }

    #[test]
    fn every_ship_shows_itself_at_every_attitude() {
        // The test a zero-thickness plate fails: rolled edge-on, a single quad
        // has no side facing anywhere and simply stops being drawn.
        //
        // Flown at twice the rows the subpixel counts here were chosen against,
        // because the shot has since been pulled back to half the framing it
        // opened on. Doubling the terminal puts the hull back at the size the
        // number describes, which is the honest fix — a threshold quietly
        // lowered to suit a smaller ship would go on passing for a ship that
        // really had vanished.
        for model in models() {
            for step in 0..12 {
                let mut ship = Ship::new();
                ship.roll = step as f32 / 12.0 * std::f32::consts::TAU - std::f32::consts::PI;
                ship.pitch_rate = if step % 2 == 0 {
                    MAX_PITCH_RATE
                } else {
                    -MAX_PITCH_RATE
                };
                ship.yaw_rate = if step % 3 == 0 { MAX_YAW_RATE } else { 0.0 };
                let (lit, _) = footprint(model, &ship, 120, 72);
                assert!(
                    lit > 60,
                    "{} nearly vanished at step {step}: {lit} subpixels",
                    model.name
                );
            }
        }
    }

    #[test]
    fn every_hull_stays_whole_across_the_whole_zoom_range() {
        // A plate with one vertex behind the projection's near plane is not
        // clipped, it is dropped — so a hull pushed too close comes apart a
        // face at a time and goes on looking like a ship while it does. The
        // `const` assertion at the top of the module says the geometry cannot
        // reach that; this says it through the real projection, at attitudes
        // that swing the corners of the box out toward the eye, and over every
        // ship so a new one is covered by being added.
        for model in models() {
            for step in 0..8 {
                let mut ship = Ship::new();
                ship.roll = step as f32 / 8.0 * std::f32::consts::TAU;
                ship.pitch_rate = MAX_PITCH_RATE;
                ship.yaw_rate = MAX_YAW_RATE;
                let pose = attitude(&ship);
                let (_, cam) = cam(120, 36, &ship);

                for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
                    // Swung round the ship as well as pushed in and out. An
                    // orbit turns the hull about its own centre, so the range
                    // to a vertex still cannot leave `distance ± HULL_REACH` —
                    // which is what both assertions are really about, and the
                    // reason neither `const` assertion needed touching. Flown
                    // rather than argued, because that is what would go quiet.
                    for eye in orbits().into_iter().map(|o| eye_at(o, zoom)) {
                        for v in &model.verts {
                            let at = place(*v, pose, &eye);
                            assert!(
                                cam.project(at).is_some(),
                                "{} lost a vertex through the near plane at zoom {zoom}: {at:?}",
                                model.name
                            );
                            // The far end used to be checked here too: the
                            // hull had to stay inside the wall the old star
                            // band began at, eighteen units out, or a star
                            // could be drawn over it. There is no wall now and
                            // no arithmetic left to check — `crate::universe`
                            // holds its nearest star light years off, against a
                            // hull that reaches seventeen units, so the two are
                            // twelve orders of magnitude apart and no zoom this
                            // sweep can ask for brings them within sight of
                            // each other.
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_far_side_of_a_hull_is_not_drawn() {
        // Back-face culling is half of the hidden-surface removal here, and
        // the cheap half: a plate pointing away from the camera is dropped on
        // the sign of its projected area, before anything is painted.
        //
        // Measured off the beam rather than square on it. A symmetric ship in
        // exact profile hides its own far side for free — port and starboard
        // project onto the same outline — so it is the moment the ship rolls or
        // crabs out of profile that the cull earns its keep.
        let mut ship = Ship::new();
        ship.roll = 0.8;
        ship.yaw_rate = MAX_YAW_RATE * 0.8;
        let (_, cam) = cam(120, 36, &ship);

        for model in models() {
            let facing = plates(model, &cam, attitude(&ship), &abeam());
            assert!(
                facing.len() < model.faces.len() * 3 / 4,
                "{}: only {} of {} plates were culled",
                model.name,
                model.faces.len() - facing.len(),
                model.faces.len()
            );
            for plate in &facing {
                assert!(
                    signed_area(&plate.points) < 0.0,
                    "{}: a plate pointing away survived the cull",
                    model.name
                );
                assert!(plate.shade.is_finite() && plate.shade >= AMBIENT - 1e-6);
            }
        }
    }

    #[test]
    fn a_faces_normal_points_out_of_the_hull() {
        // Every lamp in the scene hangs off this, and getting it backwards is
        // invisible: the ship comes out lit either way, just lit from the wrong
        // side, with the plates facing the light dark and the ones facing away
        // bright. Checked on a plain box, where which way is out is not a
        // matter of opinion.
        let mut b = Builder::default();
        b.shell(&[Section::at(-0.5, 0.3, 0.2), Section::at(0.5, 0.3, 0.2)]);
        let box_ = b.finish("box", "a box", [1.0; 3], vec![]);

        let mut checked = 0;
        for face in &box_.faces {
            let corners: Vec<[f32; 3]> = face.iter().map(|i| box_.verts[*i as usize]).collect();
            let mut centre = [0.0f32; 3];
            for c in &corners {
                for i in 0..3 {
                    centre[i] += c[i] / corners.len() as f32;
                }
            }
            let normal = normal_of(&box_.verts, face);
            // A box is centred on the origin, so out is simply away from it.
            let outward: f32 = (0..3).map(|i| centre[i] * normal[i]).sum();
            assert!(
                outward > 0.0,
                "a face at {centre:?} claims a normal of {normal:?}, which points inward"
            );
            checked += 1;
        }
        assert_eq!(checked, 6, "a box has six sides");
    }

    #[test]
    fn four_sided_a_loft_is_the_shell_it_replaced() {
        // `shell` is `loft` at four and has to *be* it rather than agree with
        // it: every ship in the hangar is built through this, and a ring
        // rebuilt from sines and cosines would come back a fraction of an ulp
        // off square. That would move hulls nothing had asked to move, and
        // through them the reference frames. `Section::ring` hands the corners
        // back untouched at four, and this is what says so.
        //
        // Flown over sections that use every field — offset centres, unequal
        // half-extents, a leaning run — since a ring at the origin would agree
        // by symmetry whatever the arithmetic did.
        let sections = [
            Section::at(-0.6, 0.2, 0.15),
            Section::offset(-0.1, 0.13, -0.22, 0.31, 0.08),
            Section::offset(0.35, -0.07, 0.19, 0.09, 0.27),
            Section::at(0.8, 0.05, 0.05),
        ];
        let mut by_shell = Builder::default();
        by_shell.shell(&sections);
        let mut by_loft = Builder::default();
        by_loft.loft(&sections, 4);

        assert_eq!(
            by_shell.verts, by_loft.verts,
            "a four-sided loft moved a vertex the shell it replaced did not"
        );
        assert_eq!(
            by_shell.faces, by_loft.faces,
            "a four-sided loft wound a face differently from the shell"
        );
    }

    #[test]
    fn the_saucer_is_as_round_as_it_is_long() {
        // The one thing the enterprise's disc is for, and the thing it was not:
        // it used to be a rectangle lofted along the track, which is a
        // hexagonal slab seen from above. Nobody could see that while the
        // camera was pinned to the beam. The camera goes over the top now, and
        // `orbit.txt` is shot at thirty-five degrees of elevation.
        //
        // Asked of the outline in plan rather than of the section list, so it
        // is a fact about the hull a later tidy-up would have to keep rather
        // than a restatement of how this one happens to be spelled. The bound
        // is loose because the disc is lofted through a circle's chords and a
        // chord is inside its arc — seven stations leave the outline about five
        // percent shy of round at its worst, which is a third of a subpixel at
        // the framing the shot opens on.
        // Seen from above, a station's outline is its *widest* vertex — the
        // ones on the crown and the underside are on their way to the axis and
        // are not on the silhouette at all. So the outline is gathered per
        // station rather than per vertex.
        //
        // Two filters find the disc, and it takes both. Its own stretch of the
        // track and its own plane: the engineering hull runs *under* the front
        // of the saucer and its widest ring is a hair over a third of the disc
        // across, so track alone lets it in. And then a third of the disc's
        // width, which is what leaves the bridge, the sensor dome and the
        // impulse deck out — all three stand in the disc's plane and none of
        // them is anything like that wide.
        let ship = &models()[0];
        let mut widest: HashMap<u32, f32> = HashMap::new();
        for v in &ship.verts {
            if v[2] <= SAUCER_Z - SAUCER_R || (v[1] - SAUCER_Y).abs() > SAUCER_R * 0.25 {
                continue;
            }
            let at = widest.entry(v[2].to_bits()).or_insert(0.0);
            *at = at.max(v[0].abs());
        }
        let mut stations = 0;
        for (z, half) in &widest {
            if *half < SAUCER_R * 0.30 {
                continue;
            }
            let along = f32::from_bits(*z) - SAUCER_Z;
            let r = (half * half + along * along).sqrt();
            assert!(
                r <= SAUCER_R * 1.001 && r > SAUCER_R * 0.94,
                "the saucer's outline is {r} from its centre at {along} along, \
                 against a radius of {SAUCER_R}: that is a slab, not a disc"
            );
            stations += 1;
        }
        assert!(
            stations >= 5,
            "only {stations} stations of the disc were wide enough to ask about"
        );
    }

    #[test]
    fn every_hull_stays_inside_its_own_bubble() {
        // The bubble is drawn out along the track and seated astern of the
        // hull, which trades clearance across the ship for clearance along it.
        // Both ends of that trade are guarded in `lens.rs` by a compile-time
        // assertion, but the assertion is about the nose of an abstract ship
        // one unit long; these are real hulls, with nacelles set out to the
        // side that a barrel roll swings straight up into the narrow waist.
        //
        // Two statements, because the camera can now be swung all the way
        // round and they stop being the same statement out there.
        //
        // Square to the track — the shot as it opens, every elevation of it and
        // every roll of its own — the hull is inside the **shadow**. Not merely
        // inside the ring: the ring only promises that no primary image lands
        // there, and the band between the two is where the counter-images pile
        // up into the rim, so a hull sitting in it would have the rim drawn
        // across it.
        //
        // Swung round toward the nose or the tail, that stops being achievable
        // and the honest bound is the **ring**. The reason is perspective and
        // not the bubble's shape: `Lens` sizes the bubble from the ship's own
        // range, and end-on the hull's *length* lies along the line of sight,
        // so its near end is magnified by up to half again while the bubble it
        // sits in is not. At `ZOOM_MAX` the bubble's semi-major axis is nine
        // tenths of the whole standoff, and no single screen-space scale can
        // hold an object that big — the drawn outline is the silhouette of the
        // spheroid, which is right, and not the *perspective* silhouette, which
        // would be a general conic with its own centre and a great deal more
        // machinery. Inside the ring is what keeps the bright rim off the hull,
        // and that is what the rim being legible depends on.
        //
        // Swept over the whole zoom range, a full turn of roll, every camera
        // angle in `orbits()`, and over `models()`, so the next ship is
        // covered the day it is added.
        for model in models() {
            for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
                for turn in 0..8 {
                    let mut ship = Ship::new();
                    ship.roll = turn as f32 * std::f32::consts::TAU / 8.0;
                    let (renderer, cam) = cam(120, 36, &ship);
                    let (_, h) = renderer.canvas_dims();
                    let pose = attitude(&ship);
                    for orbit in orbits() {
                        let eye = eye_at(orbit, zoom);
                        let lens = crate::lens::Lens::for_warp(
                            (cam.cx, cam.cy),
                            1.0,
                            crate::view::ship_half_on_screen(h as f32, zoom),
                            orbit.nose_in_camera(),
                        );
                        // The abeam family, where the shadow still holds: the
                        // camera square to the track, however far over the hull
                        // it has been lifted and however it has been rolled.
                        let square = orbit.azimuth == 0.0;
                        for v in model.verts.iter() {
                            let Some(p) = cam.project(place(*v, pose, &eye)) else {
                                continue;
                            };
                            let out = lens.offset(p);
                            assert!(
                                out < 1.0,
                                "{} reaches the rim of its own bubble at zoom {zoom}, \
                                 roll {}, orbit {orbit:?}: {p:?} is {out} rings out",
                                model.name,
                                ship.roll,
                            );
                            assert!(
                                !square || lens.shadowed(p),
                                "{} pokes out of its bubble abeam at zoom {zoom}, \
                                 roll {}, orbit {orbit:?}: {p:?} is {out} rings out",
                                model.name,
                                ship.roll,
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn plates_are_painted_far_to_near() {
        // The other half of the hidden-surface removal, and the half culling
        // cannot do: these hulls are assemblies of separate solids, so a
        // nacelle passing in front of an engineering hull is two plates that
        // both face the camera and both want the same subpixels.
        let mut ship = Ship::new();
        ship.roll = 0.5;
        let (_, cam) = cam(120, 36, &ship);
        for model in models() {
            let painted = plates(model, &cam, attitude(&ship), &abeam());
            for pair in painted.windows(2) {
                assert!(
                    pair[0].depth >= pair[1].depth,
                    "{}: {} was painted before {}, and it is nearer",
                    model.name,
                    pair[0].depth,
                    pair[1].depth
                );
            }
        }
    }

    #[test]
    fn a_hull_hides_the_sky_behind_it() {
        // What "opaque" means, stated in the one way that cannot be faked: the
        // canvas adds light everywhere else, so a subpixel that came out
        // *darker* than the sky it started as can only have been covered.
        //
        // Twice the rows the count below was chosen against, for the reason
        // given at `every_ship_shows_itself_at_every_attitude`.
        let sky = 0.9f32;
        let mut ship = Ship::new();
        ship.throttle = 0.5;
        let (renderer, cam) = cam(200, 112, &ship);
        let (w, h) = renderer.canvas_dims();

        for model in models() {
            let mut canvas = Canvas::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    canvas.splat(x as f32, y as f32, [1.0; 3], sky);
                }
            }
            draw(&mut canvas, &cam, &ship, model, &abeam(), 0.0);

            let covered = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|(x, y)| canvas.light_at(*x, *y) < sky * 3.0 - 1e-3)
                .count();
            assert!(
                covered > 200,
                "{} let the sky through: only {covered} subpixels were covered",
                model.name
            );
        }
    }

    /// The same ship with its bells taken off: the hull alone, lit exactly as
    /// it is in the frame beside it.
    ///
    /// A *cold* ship is not the same picture with one thing missing — the warp
    /// bubble lights the plates as well, and the drive and the bubble come up
    /// together — so the only honest way to ask what the drive put on the canvas
    /// is to fly the same ship without anything to put it there.
    fn hull_only(model: &ShipModel) -> ShipModel {
        ShipModel {
            name: model.name,
            blurb: model.blurb,
            verts: model.verts.clone(),
            faces: model.faces.clone(),
            engines: Vec::new(),
            hull: model.hull,
        }
    }

    #[test]
    fn the_drive_does_not_shine_through_the_hull() {
        // Regression: the bells and the exhaust they throw are light, the
        // plates are opaque, and `draw` laid the first over the second whatever
        // the camera was doing. From anywhere forward of the beam that put the
        // drive on the wrong side of its own ship — head-on, the enterprise's
        // nacelle bells burned as two blue lamps in the middle of a saucer
        // standing squarely in front of them, and the impulse bell as a third
        // between them.
        //
        // Asked as an exact equality, which the arithmetic allows rather than
        // merely tolerates. `fill_hull` writes a fully covered subpixel as
        // `buf * (1 - 1) + colour`, so nothing under it survives at any part in
        // anything: with the drive laid down first, the inside of the ship has
        // to come out the same bytes it comes out with no drive on board at
        // all.
        //
        // Which subpixels those are is asked of the canvas rather than worked
        // out here. A hull drawn over black and the same hull drawn over a lit
        // canvas agree exactly where — and only where — its coverage came to
        // one. `the_sky_never_shows_through_the_seams_of_a_hull` measures with
        // the same pair and keeps the other half of it: what a subpixel gained
        // from the sky is what it let through, and this wants the ones that let
        // through nothing.
        let sky = 0.9f32;
        let (renderer, cam) = cam(200, 112, &at_warp());
        let (w, h) = renderer.canvas_dims();

        for ship in [at_impulse(), at_warp()] {
            for model in models() {
                let bare = hull_only(model);
                // Every angle in this spread has the ship's axis turned well
                // past `OCCLUSION_BAND`, so the drive goes entirely under the
                // plates and the equality below is the whole claim rather than
                // most of it.
                for orbit in forward_quarter() {
                    let eye = eye_at(orbit, ZOOM_DEFAULT);
                    let lay = |model: &ShipModel, behind: f32| {
                        let mut canvas = Canvas::new(w, h);
                        for y in 0..h {
                            for x in 0..w {
                                canvas.splat(x as f32, y as f32, [1.0; 3], behind);
                            }
                        }
                        draw(&mut canvas, &cam, &ship, model, &eye, 0.0);
                        canvas
                    };
                    let (over_black, over_sky) = (lay(&bare, 0.0), lay(&bare, sky));
                    let flown = lay(model, 0.0);

                    let (mut solid, mut leaked) = (0usize, 0usize);
                    for y in 0..h {
                        for x in 0..w {
                            if over_sky.light_at(x, y) != over_black.light_at(x, y) {
                                continue;
                            }
                            solid += 1;
                            leaked +=
                                usize::from(flown.light_at(x, y) != over_black.light_at(x, y));
                        }
                    }
                    // Which is worth nothing at all unless the ship is
                    // genuinely standing in the way of something, so say so:
                    // a hull that missed the canvas would pass the line above
                    // without ever covering a subpixel.
                    assert!(
                        solid > 100,
                        "{} at {orbit:?} covered only {solid} subpixels outright, \
                         so this measured nothing",
                        model.name
                    );
                    assert_eq!(
                        leaked, 0,
                        "{} at {orbit:?}: the drive lit {leaked} subpixels the hull \
                         covers whole",
                        model.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_drive_still_washes_the_hull_it_plays_over() {
        // The other half, and the reason the order is a question rather than a
        // rule. Square to the track and behind it the plume is genuinely the
        // nearer of the two — the enterprise's impulse bell is mid-ship and its
        // plume runs between the nacelles and just under them, so from the beam
        // it lies straight across them — and it is meant to shine through as
        // the wash a hot plume puts on structure it plays over. An occlusion
        // rule that fired at every angle would take that away and read as a
        // drive going out whenever the camera moved.
        //
        // Measured on the subpixels the hull covers *whole*, which is the same
        // set the test above demands nothing on and the only set that says
        // anything: a silhouette subpixel is part sky whichever side the drive
        // was laid on, so it lights up either way and would pass this line with
        // the plume buried under the ship.
        //
        // Flown rolled, which is what walks the plume over the nacelles rather
        // than past them, and from the beam and from astern — the two sides the
        // exhaust is genuinely the nearer of the two on.
        let sky = 0.9f32;
        let mut ship = at_impulse();
        ship.roll = 0.6;
        let (renderer, cam) = cam(200, 112, &ship);
        let (w, h) = renderer.canvas_dims();
        let model = &models()[index_of("enterprise")];
        let bare = hull_only(model);

        for orbit in [Orbit::LEVEL, orbit(-60.0, 0.0, 0.0)] {
            let eye = eye_at(orbit, ZOOM_DEFAULT);
            let lay = |model: &ShipModel, behind: f32| {
                let mut canvas = Canvas::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        canvas.splat(x as f32, y as f32, [1.0; 3], behind);
                    }
                }
                draw(&mut canvas, &cam, &ship, model, &eye, 0.0);
                canvas
            };
            let (over_black, over_sky) = (lay(&bare, 0.0), lay(&bare, sky));
            let flown = lay(model, 0.0);

            let washed = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    over_sky.light_at(*x, *y) == over_black.light_at(*x, *y)
                        && flown.light_at(*x, *y) > over_black.light_at(*x, *y)
                })
                .count();
            assert!(
                washed > 0,
                "at {orbit:?} the drive stopped lighting the hull it plays over"
            );
        }
    }

    #[test]
    fn the_swap_takes_a_whole_press_of_the_key_that_crosses_it() {
        // `OCCLUSION_BAND` is sized against two scales and its doc names both,
        // which is exactly the sort of relationship that rots quietly. This is
        // the first of them: the camera steps by `ORBIT_STEP`, so a band wider
        // than one notch would strand the drive half-buried at an angle a
        // single press cannot get off, and one narrower than that lets a press
        // do the whole swap in the frames of one ease.
        //
        // Compared as a sine because that is the unit `drive_behind_hull`
        // measures in: the depth the ship's axis gains over a unit of its
        // length, not the angle itself.
        let notch = crate::view::ORBIT_STEP.sin();
        assert!(
            OCCLUSION_BAND > 0.0 && OCCLUSION_BAND <= notch,
            "the drive swaps sides over {OCCLUSION_BAND}, and one press of a \
             camera key is {notch}"
        );
    }

    #[test]
    fn a_ship_nobody_is_flying_never_crosses_the_swap() {
        // The second scale, and the load-bearing one. The autopilot weaves, so
        // the hull's own lean carries the track across square every few seconds
        // whether or not anything has touched the camera — and a swap that fired
        // on that would blink the drive on and off. Before the ramp went in it
        // did: 137 of 255 on a subpixel, thirty of them at once.
        //
        // Asked through the real function at the shot the flight opens on,
        // rather than of the yaw rate, because what has to stay small is what
        // the renderer reads. Abeam and nowhere else, and that is still the
        // question even though the autopilot now walks the camera round: off
        // the beam the swap fires because the camera has moved, which is what
        // it is for. Here it can only fire because the ship leaned.
        //
        // Asked at three frame rates, which is new and is the point. This used
        // to be a fact about 1/60: the weave was an impulse a frame against a
        // decay a second, so its amplitude went with the frame rate and the
        // lean at 500 fps — a stuck key, or a pasted burst, both of which the
        // interactive loop runs the frame cap off for — was eight times this
        // one and would have failed outright.
        use crate::autopilot::Autopilot;
        for fps in [10.0f32, 60.0, 500.0] {
            let dt = 1.0 / fps;
            let mut ship = Ship::new();
            let mut autopilot = Autopilot::default();
            let mut worst = 0.0f32;
            for frame in 0..(2.0 * Autopilot::CYCLE * fps as f64) as usize {
                autopilot.update(&mut ship, frame as f64 * dt as f64, dt);
                ship.update(dt);
                worst = worst.max(drive_behind_hull(attitude(&ship), &abeam()));
            }
            assert!(
                worst < 0.25,
                "at {fps} frames a second the autopilot's weave carries the \
                 drive {worst} of the way across the swap, which is close \
                 enough to the far side to read as a blink"
            );
        }
    }

    #[test]
    fn the_sky_never_shows_through_the_seams_of_a_hull() {
        // Why the whole hull goes to the canvas in one call, asked of the real
        // assemblies of solids in the hangar rather than of two synthetic
        // quads.
        //
        // Composed a plate at a time — which is what a painter's algorithm
        // suggests, and what this used to do — every edge two plates share
        // inside the hull is blended twice: each covers a share of the subpixel
        // their common edge runs down, and `(1 - a)(1 - b)` of the sky lives
        // through both. So the two paths are drawn side by side here, with one
        // colour for every face so that a subpixel the hull fully covers is the
        // hull colour and nothing else.
        //
        // Enclosed sky is deliberately *not* what is measured: a hull with a
        // hole through it is still a hull, and you are meant to see stars
        // through the middle of one.
        let sky = 0.9f32;
        let paint = [0.2, 0.24, 0.31];
        let ship = Ship::new(); // cold, so the drive lays nothing over this
        let (renderer, cam) = cam(200, 112, &ship);
        let (w, h) = renderer.canvas_dims();

        for model in models() {
            // Summed over a few rolls rather than taken from one pose: where a
            // shared edge falls between two samples is what decides whether it
            // leaks at all, and one attitude can happen to put every edge of a
            // simple hull on a sample boundary.
            let seams: usize = (0..6)
                .map(|step| {
                    let mut turned = Ship::new();
                    turned.roll = step as f32 * 0.21;
                    let plates = plates(model, &cam, attitude(&turned), &abeam());
                    let faces: Vec<Facet<'_>> = plates
                        .iter()
                        .map(|plate| Facet {
                            points: &plate.points,
                            color: paint,
                        })
                        .collect();
                    let lay = |behind: f32, per_face: bool| {
                        let mut canvas = Canvas::new(w, h);
                        for y in 0..h {
                            for x in 0..w {
                                canvas.splat(x as f32, y as f32, [1.0; 3], behind);
                            }
                        }
                        if per_face {
                            for face in &faces {
                                canvas.fill_hull(std::slice::from_ref(face));
                            }
                        } else {
                            canvas.fill_hull(&faces);
                        }
                        canvas
                    };
                    // The same render over black says exactly how much of the
                    // sky each path let through, with no threshold to argue
                    // about: what a subpixel gained from the sky, and nothing
                    // it gained from the hull.
                    let survived = |per_face: bool| {
                        let (over_sky, over_black) = (lay(sky, per_face), lay(0.0, per_face));
                        (0..h)
                            .flat_map(|y| (0..w).map(move |x| (x, y)))
                            .map(|(x, y)| over_sky.light_at(x, y) - over_black.light_at(x, y))
                            .collect::<Vec<_>>()
                    };
                    // Solid hull composed together, and leaking composed one at
                    // a time. A silhouette subpixel is partly covered whichever
                    // way round it is drawn and does not appear here.
                    survived(false)
                        .iter()
                        .zip(&survived(true))
                        .filter(|(together, apart)| **together <= 1e-6 && **apart > 1e-3)
                        .count()
                })
                .sum();
            assert!(
                seams > 20,
                "{}: only {seams} subpixels over six attitudes came out solid \
                 hull composed together and let the sky through composed one \
                 plate at a time, so this measured almost nothing",
                model.name
            );
        }
    }

    #[test]
    fn a_hull_turned_by_less_than_a_subpixel_moves_by_less_than_a_subpixel() {
        // What the finer measurement is for, at the fleet level. A plate is one
        // or two subpixels thick at the framing the shot opens on, and under a
        // rasteriser that puts an edge on one side of a subpixel or the other
        // an outline that small does not move when the ship rolls — it sits
        // still and then jumps a whole subpixel, which reads as crawling.
        //
        // So: rolls far too small to move any edge a whole subpixel, and both
        // halves of the property. Most steps have to change *something*, or the
        // outline is still snapping; and no subpixel may change by anything
        // like the hull's own brightness in one step, which is the popping the
        // first half would otherwise be satisfied by.
        let ship = Ship::new(); // cold, so only the plates are drawn
        let (renderer, cam) = cam(120, 72, &ship);
        let (w, h) = renderer.canvas_dims();

        for model in models() {
            let frame = |roll: f32| {
                let mut turned = Ship::new();
                turned.roll = roll;
                let mut canvas = Canvas::new(w, h);
                draw(&mut canvas, &cam, &turned, model, &abeam(), 0.0);
                (0..h)
                    .flat_map(|y| (0..w).map(move |x| (x, y)))
                    .map(|(x, y)| canvas.light_at(x, y))
                    .collect::<Vec<_>>()
            };

            // A hull is at most one unit from its own axis and about nine
            // subpixels to the unit here, so a thousandth of a radian moves the
            // furthest point of it by roughly a hundredth of a subpixel.
            let step = 0.001;
            let mut moved = 0;
            let mut previous = frame(0.0);
            for i in 1..=12 {
                let current = frame(i as f32 * step);
                let mut biggest = 0.0f32;
                for (a, b) in previous.iter().zip(&current) {
                    biggest = biggest.max((a - b).abs());
                }
                if biggest > 0.0 {
                    moved += 1;
                }
                assert!(
                    biggest < 0.3,
                    "{}: a roll of a thousandth of a radian moved a subpixel \
                     by {biggest}, which is a plate jumping rather than \
                     sliding",
                    model.name
                );
                previous = current;
            }
            assert!(
                moved >= 8,
                "{}: only {moved} of twelve hundredth-of-a-subpixel turns \
                 changed the frame at all, so the outline is still snapping \
                 from one subpixel to the next",
                model.name
            );
        }
    }

    #[test]
    fn the_ships_do_not_look_like_one_another() {
        // A hangar is only a feature if the ships in it read as different
        // ships. Compared as silhouettes on the canvas the camera actually
        // uses, at twice the rows for the reason given at
        // `every_ship_shows_itself_at_every_attitude` — two ships compared at
        // a size neither of them has any detail left at would agree, and the
        // test would be measuring the resolution rather than the hulls.
        let ship = Ship::new();
        let (renderer, cam) = cam(120, 72, &ship);
        let (w, h) = renderer.canvas_dims();
        let masks: Vec<Vec<bool>> = models()
            .iter()
            .map(|model| {
                let mut canvas = Canvas::new(w, h);
                draw(&mut canvas, &cam, &ship, model, &abeam(), 0.0);
                (0..w * h)
                    .map(|i| canvas.light_at(i % w, i / w) > 0.02)
                    .collect()
            })
            .collect();

        for (i, a) in masks.iter().enumerate() {
            for (j, b) in masks.iter().enumerate().skip(i + 1) {
                let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
                let either = a.iter().zip(b).filter(|(x, y)| **x || **y).count();
                let overlap = both as f32 / either.max(1) as f32;
                assert!(
                    overlap < 0.7,
                    "{} and {} are {:.0}% the same shape",
                    models()[i].name,
                    models()[j].name,
                    overlap * 100.0
                );
            }
        }
    }

    #[test]
    fn a_hull_draws_on_any_terminal_and_never_out_of_bounds() {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        for model in models() {
            for (cols, rows) in [(1usize, 1usize), (20, 6), (80, 24), (400, 120), (2, 60)] {
                // `total.is_finite()` was the whole of this, and it could not
                // fail: `footprint_at` asserts exactly that of every subpixel
                // it visits, and this is a sum over a subset of them. What the
                // name promises is the bound, so count against it — a hull
                // that wrote outside the canvas would panic, and one that
                // wrote outside its own *count* of the canvas would not.
                let (lit, total) = footprint(model, &ship, cols, rows);
                assert!(
                    lit <= cols * rows * 2,
                    "{} lit {lit} of {cols}x{rows}'s {} subpixels",
                    model.name,
                    cols * rows * 2
                );
                assert!(total.is_finite(), "{} at {cols}x{rows}", model.name);
                // And on a terminal with room for it, the ship is actually
                // there. Degenerate framings are allowed to come out empty;
                // this one is not.
                if (cols, rows) == (400, 120) {
                    assert!(lit > 0, "{} drew nothing at {cols}x{rows}", model.name);
                }
            }
        }
    }

    #[test]
    fn the_engines_light_with_the_throttle_and_change_colour_at_warp() {
        let model = &models()[0];
        let brightness = |ship: &Ship| footprint(model, ship, 120, 36).1;

        let mut parked = Ship::new();
        parked.throttle = 0.0;
        parked.speed = 0.0;
        let mut impulse = Ship::new();
        impulse.speed = crate::ship::CRUISE_MAX;
        let mut warping = Ship::new();
        warping.throttle = 1.0;
        warping.toggle_warp();
        for _ in 0..900 {
            warping.update(1.0 / 60.0);
        }

        assert!(
            brightness(&impulse) > brightness(&parked),
            "the bells never lit"
        );
        assert!(
            brightness(&warping) > brightness(&impulse),
            "warp did not lift them"
        );
    }

    /// How far forward and how far aft the hull itself reaches on screen, in
    /// subpixels. The nose is to screen right, so the first of the two is the
    /// left-hand edge of the ship and everything past it is exhaust.
    fn hull_span(model: &ShipModel, cam: &Camera, ship: &Ship, eye: &Eye) -> (f32, f32) {
        let pose = attitude(ship);
        model
            .verts
            .iter()
            .filter_map(|v| cam.project(place(*v, pose, eye)))
            .fold((f32::MAX, f32::MIN), |(aft, fore), p| {
                (aft.min(p.0), fore.max(p.0))
            })
    }

    /// The drive's light that falls clear of the hull: how many subpixels of it
    /// there are, how far aft of the hull the furthest reaches, and how much
    /// light there is in it altogether.
    ///
    /// Measured past the hull's own silhouette rather than from the middle of
    /// the frame, because the hulls are different lengths and the thing under
    /// test is what comes out of the back of them.
    ///
    /// A word on which of the three to reach for. The reach is the honest
    /// measure of *length*, but only while the plume fits: it is taken off the
    /// canvas, so a flame that runs out of the frame reports the frame's width
    /// instead of its own and two very different plumes come back equal. Any
    /// test that pushes the length — the zoom, the surge — either wants a
    /// canvas wide enough or wants the light rather than the reach.
    fn wake(
        model: &ShipModel,
        ship: &Ship,
        cols: usize,
        rows: usize,
        time: f64,
    ) -> (usize, f32, f32) {
        let (renderer, cam) = cam(cols, rows, ship);
        wake_at(model, ship, &renderer, &cam, &abeam(), time)
    }

    /// The same, against a camera and a standoff the caller has chosen.
    fn wake_at(
        model: &ShipModel,
        ship: &Ship,
        renderer: &Renderer,
        cam: &Camera,
        eye: &Eye,
        time: f64,
    ) -> (usize, f32, f32) {
        let (w, h) = renderer.canvas_dims();
        let mut canvas = Canvas::new(w, h);
        draw(&mut canvas, cam, ship, model, eye, time);
        let (aft, _) = hull_span(model, cam, ship, eye);
        let (mut count, mut furthest, mut total) = (0usize, 0.0f32, 0.0f32);
        for y in 0..h {
            for x in 0..w {
                let light = canvas.light_at(x, y);
                assert!(light.is_finite(), "{} drew a NaN", model.name);
                if (x as f32) < aft - 1.0 && light > 0.02 {
                    count += 1;
                    furthest = furthest.max(aft - x as f32);
                    total += light;
                }
            }
        }
        (count, furthest, total)
    }

    /// A ship at the top of the sublight range, and one at full warp.
    fn at_impulse() -> Ship {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.speed = crate::ship::CRUISE_MAX;
        ship
    }

    fn at_warp() -> Ship {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..900 {
            ship.update(1.0 / 60.0);
        }
        // Long enough that the engage transients have gone: what is being
        // measured is the drive running, not the moment it caught.
        ship.flash = 0.0;
        ship
    }

    #[test]
    fn a_cold_drive_lays_no_trail() {
        // Exact, not approximate: `draw_engines` gives up before it draws
        // anything at all when the ship is not moving, and three of the hull
        // tests above lean on that — they fly a `Ship::new()`, whose speed is
        // zero, precisely so the drive is not in their measurements.
        let parked = Ship::new();
        for model in models() {
            let (count, _, _) = wake(model, &parked, 200, 60, 0.0);
            assert_eq!(count, 0, "{} lit its drive standing still", model.name);
        }
    }

    #[test]
    fn the_drive_lays_a_trail_that_lengthens_with_the_throttle_and_again_at_warp() {
        // The whole point of the thing: from out here the sky says how fast the
        // ship is going and the ship itself used to say nothing. Both steps
        // matter — a trail that only appears at warp leaves the entire sublight
        // range looking identical, which is where it started.
        for model in models() {
            let idle = {
                let mut ship = Ship::new();
                ship.speed = crate::ship::CRUISE_MAX * 0.2;
                ship
            };
            let (_, slow, _) = wake(model, &idle, 400, 60, 0.0);
            let (_, cruise, _) = wake(model, &at_impulse(), 400, 60, 0.0);
            let (_, warp, _) = wake(model, &at_warp(), 400, 60, 0.0);
            assert!(
                cruise > slow + 1.0,
                "{}: opening up did not lengthen the trail — {slow} then {cruise}",
                model.name
            );
            assert!(
                warp > cruise * 1.5,
                "{}: warp did not stretch it — {cruise} then {warp}",
                model.name
            );
        }
    }

    #[test]
    fn the_trail_streams_astern_and_not_ahead() {
        // `to_camera` puts the nose to screen right, so exhaust is everything
        // off to the left. Getting the sign wrong here would draw a ship
        // flying backwards down its own headlight, which is the sort of thing
        // that looks fine in a still and absurd the moment it moves.
        let ship = at_warp();
        for model in models() {
            let (renderer, cam) = cam(200, 60, &ship);
            let (w, h) = renderer.canvas_dims();
            let mut canvas = Canvas::new(w, h);
            draw(&mut canvas, &cam, &ship, model, &abeam(), 0.0);
            let (_, fore) = hull_span(model, &cam, &ship, &abeam());

            let ahead = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|(x, y)| (*x as f32) > fore + 1.0 && canvas.light_at(*x, *y) > 0.02)
                .count();
            let (behind, reach, _) = wake(model, &ship, 200, 60, 0.0);
            assert!(
                reach > 4.0 && behind > ahead * 4,
                "{}: {behind} subpixels astern against {ahead} ahead, reaching {reach}",
                model.name
            );
        }
    }

    #[test]
    fn the_trail_is_the_same_number_of_ships_long_at_every_zoom() {
        // The plume is built in hull units and posed by the same stack as the
        // plates, so the dolly has to carry it exactly as it carries the ship —
        // the same property `the_bubble_is_the_same_number_of_ships_across_at_\
        // every_zoom` pins for the warp bubble, and for the same reason: a
        // length written against the canvas would swell and shrink against a
        // hull that was not.
        //
        // Flown at *impulse*, and that is a real narrowing rather than a
        // convenience. A lit warp drive throws its lance at the frame edge on
        // purpose, so above light speed the length is measured against the
        // canvas and there is nothing left for the zoom to preserve. Below it,
        // which is the whole of the range this property was ever about, the
        // reach is still hull units through the same projection.
        //
        // Measured off the canvas rather than off the formula, which is what
        // makes it worth having, and the terminal is wide because of that: at
        // `ZOOM_MAX` the plume runs several ships past a hull already a quarter
        // of the height, and on an ordinary canvas it leaves the frame and
        // reports the frame's width instead of its own. The tolerance is loose
        // because at `ZOOM_MIN` the whole thing is a dozen subpixels, where a
        // subpixel of rounding is most of the spread. A plume pinned to the
        // frame rather than to the ship would be out by the zoom range itself,
        // which is more than fourfold.
        let ship = at_impulse();
        let model = &models()[0];
        let renderer = Renderer::new(700, 60, ColorMode::Truecolor, 1.9);
        let cam = renderer.exterior_camera(&ship, 0.0);
        let (_, h) = renderer.canvas_dims();

        let mut first = None;
        for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
            let (_, furthest, _) = wake_at(
                model,
                &ship,
                &renderer,
                &cam,
                &eye_at(Orbit::LEVEL, zoom),
                0.0,
            );
            let ships = furthest / crate::view::ship_half_on_screen(h as f32, zoom);
            let want: f32 = *first.get_or_insert(ships);
            assert!(
                (ships - want).abs() < want * 0.25,
                "the trail is {ships} ships long at zoom {zoom}, against {want} at the others"
            );
        }
    }

    /// The drive's light in one column, a chosen number of the ship's own
    /// half-lengths aft of the vanishing point.
    ///
    /// Measured in ships rather than in subpixels so the sample lands at the
    /// same place in the plume whatever the terminal is: both the hull and the
    /// lance scale with the canvas, so a column two ship-halves back is the
    /// same fraction of the way down the flame at every size. A column fixed in
    /// subpixels would be inside the hull on one terminal and past the tip on
    /// another, and would compare nothing.
    fn drive_column(
        model: &ShipModel,
        ship: &Ship,
        cols: usize,
        rows: usize,
        ships_aft: f32,
    ) -> (f32, [f32; 3]) {
        let (renderer, cam) = cam(cols, rows, ship);
        let (w, h) = renderer.canvas_dims();
        let mut canvas = Canvas::new(w, h);
        draw(&mut canvas, &cam, ship, model, &abeam(), 0.0);
        let half = crate::view::ship_half_on_screen(h as f32, ZOOM_DEFAULT);
        let x = (cam.cx - half * ships_aft)
            .round()
            .clamp(0.0, (w - 1) as f32) as usize;
        let mut best = 0.0f32;
        let mut color = [0.0; 3];
        for y in 0..h {
            let light = canvas.light_at(x, y);
            if light > best {
                best = light;
                color = canvas.color_at(x, y);
            }
        }
        (best, color)
    }

    #[test]
    fn a_lit_warp_drive_trails_off_the_edge_of_the_frame() {
        // What "very long" was asked to mean, and the reason the reach stops
        // being hull units the moment the drive is lit. Checked at both ends of
        // the dolly as well as at three terminals, because the lance is
        // stretched in screen space after the projection — if it were solved
        // for in hull units instead it would come up short at one zoom and
        // overshoot at the other.
        //
        // `Orbit::LEVEL` is a narrowing here and not a convenience, which it
        // was when it was the only camera angle there was. The beam is the one
        // framing where a plume lies flat in the image plane and so has no
        // point to vanish at, and reaching the frame edge is the right answer
        // only there. Swung toward the nose the lance stops at that point
        // instead — see `a_plume_stops_at_the_point_it_vanishes_at` — so
        // generalising this sweep over `orbits()` would pin the bug that one
        // was written for.
        let ship = at_warp();
        assert!(
            ship.warp_engaged,
            "the drive has to be lit for this to mean anything"
        );
        for model in models() {
            for (cols, rows) in [(120usize, 36usize), (200, 60), (400, 120)] {
                let (renderer, cam) = cam(cols, rows, &ship);
                let (w, h) = renderer.canvas_dims();
                for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
                    let mut canvas = Canvas::new(w, h);
                    draw(
                        &mut canvas,
                        &cam,
                        &ship,
                        model,
                        &eye_at(Orbit::LEVEL, zoom),
                        0.0,
                    );
                    let reaches = (0..h).any(|y| canvas.light_at(0, y) > 0.02);
                    assert!(
                        reaches,
                        "{} stopped short of the frame edge at {cols}x{rows}, zoom {zoom}",
                        model.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_lance_burns_as_brightly_on_any_terminal() {
        // The whole point of dividing `Canvas::streak_spread` back out. A lit
        // drive's reach is the frame's, not the ship's, and `draw_streak`
        // spreads a streak's light along its length — so left alone the drive
        // would burn dimmer the wider the terminal, and the same flight would
        // not look the same on two machines. That is the one thing this
        // renderer's tests exist to stop.
        //
        // Sampled two ship-halves aft, which is out in the lance and clear of
        // the bell's own glow. The tolerance covers the lane count, which is
        // chosen from the fan's width in subpixels and so lands on a different
        // integer at each size.
        let ship = at_warp();
        let model = &models()[0];
        let mut first = None;
        for (cols, rows) in [(120usize, 36usize), (200, 60), (400, 120)] {
            let (light, _) = drive_column(model, &ship, cols, rows, 2.0);
            let want: f32 = *first.get_or_insert(light);
            assert!(
                light > 0.0 && (light - want).abs() < want * 0.4,
                "the lance is {light} at {cols}x{rows}, against {want} at the other sizes"
            );
        }
    }

    #[test]
    fn the_drive_burns_blue_at_impulse_and_whitens_at_warp() {
        // Measured off the canvas rather than off the two constants, so an edit
        // to either end of the ramp is covered rather than restated. Blue is
        // the whole read at impulse — a drive that runs amber down there puts
        // the hotter-looking colour on the colder setting — and whitening is
        // what stops engaging the drive from being a change of length alone.
        let model = &models()[0];
        let (_, cruise) = drive_column(model, &at_impulse(), 200, 60, 1.6);
        let (_, warp) = drive_column(model, &at_warp(), 200, 60, 1.6);

        assert!(
            cruise[2] > cruise[0] * 1.5,
            "the drive is not blue at impulse: {cruise:?}"
        );
        let whiteness = |c: [f32; 3]| c[0] / c[2].max(f32::MIN_POSITIVE);
        assert!(
            whiteness(warp) > whiteness(cruise) * 1.5,
            "warp did not whiten the drive: {cruise:?} then {warp:?}"
        );
        assert!(
            warp[2] > warp[0],
            "warp went past white into the red: {warp:?}"
        );
    }

    #[test]
    fn a_trail_swung_toward_the_camera_is_cut_rather_than_dropped() {
        // `Camera::project` does not clip a point behind its near plane, it
        // answers `None` and the caller drops it — so an unclipped plume under
        // a hard yaw at the close end of the dolly does not shorten, it
        // disappears, and a drive that blinks out mid-manoeuvre reads as a
        // fault. The closed-form cut in `draw_trail` is what this is here for,
        // over the same attitude sweep the hull's own near-plane test uses.
        for model in models() {
            for step in 0..8 {
                let mut ship = at_warp();
                ship.roll = step as f32 / 8.0 * std::f32::consts::TAU;
                ship.pitch_rate = MAX_PITCH_RATE;
                ship.yaw_rate = if step % 2 == 0 {
                    MAX_YAW_RATE
                } else {
                    -MAX_YAW_RATE
                };
                let pose = attitude(&ship);

                for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
                    let eye = eye_at(Orbit::LEVEL, zoom);
                    let renderer = Renderer::new(200, 60, ColorMode::Truecolor, 1.9);
                    let cam = renderer.exterior_camera(&ship, 0.0);
                    let (w, h) = renderer.canvas_dims();
                    let mut canvas = Canvas::new(w, h);
                    draw(&mut canvas, &cam, &ship, model, &eye, 0.0);

                    let (aft, _) = hull_span(model, &cam, &ship, &eye);
                    let lit = (0..h)
                        .flat_map(|y| (0..w).map(move |x| (x, y)))
                        .filter(|(x, y)| (*x as f32) < aft - 1.0 && canvas.light_at(*x, *y) > 0.02)
                        .count();
                    assert!(
                        lit > 0,
                        "{} lost its trail whole at zoom {zoom}, step {step}, pose {pose:?}",
                        model.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_plume_abeam_has_no_point_to_vanish_at() {
        // Why swinging the camera is what broke this and the shot it opens on
        // never showed it, and — said here rather than left to a hash — why the
        // reference `side.txt` could not move for the fix.
        //
        // `Eye::to_camera` at `Orbit::LEVEL` is exactly `(x, y, z) -> (z, y,
        // distance - x)`, so the hull's own axis lands flat in the image plane
        // and its depth term is exactly zero. A plume laid along it recedes
        // from the eye not at all, has no vanishing point to converge on, and
        // takes the untouched arithmetic through `draw_trail`.
        let ship = at_warp();
        let (_, cam) = cam(200, 60, &ship);
        let pose = attitude(&ship);
        assert_eq!(pose, (0.0, 0.0, 0.0), "this flight is not flying level");

        let eye = abeam();
        let origin = place([0.0, 0.0, 0.0], pose, &eye);
        let astern = place([0.0, 0.0, -1.0], pose, &eye);
        let along = [
            astern[0] - origin[0],
            astern[1] - origin[1],
            astern[2] - origin[2],
        ];
        assert_eq!(
            along[2], 0.0,
            "the beam shot has depth along the hull: {along:?}"
        );
        assert!(
            cam.vanishing_point(along).is_none(),
            "a plume laid flat in the image plane was given a point to vanish at"
        );
    }

    /// How far clear of the vanishing point the whole ship has to be before the
    /// test below will ask its question. A bell's glow reaches a dozen subpixels
    /// at the close end of the dolly, and a hull nearly on top of the point it
    /// vanishes at has no plume to speak of anyway — `draw_trail` gives up on a
    /// projected length under one subpixel.
    const CLEAR_OF_THE_HORIZON: f32 = 25.0;

    #[test]
    fn a_plume_stops_at_the_point_it_vanishes_at() {
        // The bug this is here for. `draw_trail` stretches a lit drive's lance
        // in screen space to the frame diagonal — an absolute length with
        // nothing to do with the plume — and a ray running away from the eye
        // projects onto a point that *approaches* its direction's vanishing
        // point and never arrives. Past that point there is no exhaust to draw,
        // so the stretch drew some anyway: from anywhere forward of the beam
        // every lance went clean through and out the far side, where a
        // symmetric pair of bells swap over and cross. It measured 612,286
        // units of drive light on the wrong side of the point, reaching 231
        // pixels past it, at `--orbit 75,12,0`.
        //
        // Stated as a half-plane rather than as a length, because that is what
        // the property is: the vanishing point is where a plume ends, whatever
        // the terminal, the zoom or the hull. Asked only where the ship is
        // comfortably clear of that point, since a drive halo straddling it is
        // not the lance — and the cases are counted against a floor, so a sweep
        // that stopped reaching them could not go quiet about it. The floor is
        // there for the thinning as much as for the collapse: most of what
        // qualifies does so at one end of the dolly, and a later nudge to
        // `SHIP_SCREEN_FRAC` or to the gate below could quietly take the
        // question down to a single frame while the test went on passing.
        //
        // Flown at impulse as well as at warp, which the lance itself does not
        // need: down there the plume is the segment the projection gave it and
        // could not overshoot if it tried. What that pass is for is the other
        // half of the same arithmetic — a flame narrows toward the point it is
        // receding to exactly as a lance does, off the one division, and that
        // branch has nothing else flying through it.
        let mut asked = vec![0usize; models().len()];
        for ship in [at_warp(), at_impulse()] {
            // Neither of these turns on the model, the orbit or the zoom, and a
            // fresh `Renderer` inside all three is a canvas the size of the
            // frame allocated a few hundred times over.
            let (renderer, cam) = cam(200, 60, &ship);
            let (w, h) = renderer.canvas_dims();
            let pose = attitude(&ship);
            for (index, model) in models().iter().enumerate() {
                for orbit in orbits().into_iter().chain(forward_quarter()) {
                    for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
                        let eye = eye_at(orbit, zoom);
                        let origin = place([0.0, 0.0, 0.0], pose, &eye);
                        let astern = place([0.0, 0.0, -1.0], pose, &eye);
                        let Some(vanish) = cam.vanishing_point([
                            astern[0] - origin[0],
                            astern[1] - origin[1],
                            astern[2] - origin[2],
                        ]) else {
                            continue;
                        };

                        // Which way the exhaust runs on screen: from the drive
                        // toward the point it is heading for. Every bell shares that
                        // point, so one line settles all of them.
                        let heads: Vec<(f32, f32)> = model
                            .engines
                            .iter()
                            .filter_map(|bell| cam.project(place(bell.at, pose, &eye)))
                            .collect();
                        let Some(mid) = heads
                            .iter()
                            .copied()
                            .reduce(|a, b| (a.0 + b.0, a.1 + b.1))
                            .map(|s| (s.0 / heads.len() as f32, s.1 / heads.len() as f32))
                        else {
                            continue;
                        };
                        let (ux, uy) = (vanish.0 - mid.0, vanish.1 - mid.1);
                        let range = ux.hypot(uy);
                        if !range.is_finite() || range <= 0.0 {
                            continue;
                        }
                        let (ux, uy) = (ux / range, uy / range);
                        let past = |p: (f32, f32)| (p.0 - vanish.0) * ux + (p.1 - vanish.1) * uy;

                        let clear = heads.iter().all(|p| past(*p) <= -CLEAR_OF_THE_HORIZON)
                            && model
                                .verts
                                .iter()
                                .filter_map(|v| cam.project(place(*v, pose, &eye)))
                                .all(|p| past(p) <= -CLEAR_OF_THE_HORIZON);
                        if !clear {
                            continue;
                        }
                        // Counted only when the point is on the canvas, and that is
                        // the whole difference between a test and a shape of one.
                        // A vanishing point off the side of the frame leaves the
                        // overshoot off it too, where `draw_streak` clips it away
                        // and the sweep sails past a lance running three frame
                        // widths through the sky. Which is exactly what the angles
                        // already in `orbits()` do.
                        let onscreen = (0.0..w as f32).contains(&vanish.0)
                            && (0.0..h as f32).contains(&vanish.1);
                        if onscreen {
                            asked[index] += 1;
                        }

                        let mut canvas = Canvas::new(w, h);
                        draw(&mut canvas, &cam, &ship, model, &eye, 0.0);
                        for y in 0..h {
                            for x in 0..w {
                                let light = canvas.light_at(x, y);
                                assert!(light.is_finite(), "{} drew a NaN", model.name);
                                let over = past((x as f32, y as f32));
                                assert!(
                                    light <= 0.02 || over <= 0.0,
                                    "{} lit a subpixel {over:.1} past the point its plume \
                                 vanishes at, from {orbit:?} at zoom {zoom}",
                                    model.name
                                );
                            }
                        }
                    }
                }
            }
        }
        // Counted per ship rather than in total, and that is the fix for the
        // way this number last moved. A total is a floor on the *hangar's size*
        // as much as on the coverage, and the hangar is what changed under it:
        // six hulls became two, which took the count from 154 to 56 without a
        // word about the sweep having got any narrower. Per ship it is the same
        // question at any hangar size.
        //
        // 28 and 30 today. The floor is well under both so an ordinary change
        // to a hull or to the framing does not have to move it, and well over
        // the handful that would mean the sweep had stopped asking the question
        // in any but a corner of the range.
        for (index, count) in asked.iter().enumerate() {
            assert!(
                *count >= 16,
                "only {count} frames put a vanishing point on the canvas with {} clear \
                 of it, which is too few to be sure of noticing a lance running through one",
                models()[index].name
            );
        }
    }

    #[test]
    fn the_plume_keeps_its_width_when_the_ship_rolls() {
        // Why the fan is spread perpendicular in *screen* space and not in the
        // hull's. Hull `x` is almost pure camera depth from this beam, so a fan
        // opened along hull `y` alone would collapse to a hairline every time
        // the ship rolled ninety degrees — an axisymmetric plume must not
        // narrow with a roll.
        //
        // Flown on the Normandy, whose one bell sits on the ship's own axis:
        // its plume lands in the same place at every roll, so the only thing
        // left that can move is the width of the fan around it. It inherits the
        // question from the Needle, which was the ship with that property until
        // the hangar was emptied — and it is not a coincidence that there was
        // another. A hull is built with its drive on the axis or it is not, and
        // this test needs one that is.
        let model = &models()[index_of("normandy")];
        let (mut narrowest, mut widest) = (usize::MAX, 0usize);
        for step in 0..8 {
            let mut ship = at_warp();
            ship.roll = step as f32 / 8.0 * std::f32::consts::TAU;
            let (count, _, _) = wake(model, &ship, 200, 60, 0.0);
            narrowest = narrowest.min(count);
            widest = widest.max(count);
        }
        assert!(
            narrowest > 0 && narrowest * 4 > widest * 3,
            "the plume ran from {narrowest} subpixels to {widest} across a roll"
        );
    }

    #[test]
    fn the_trail_flickers_without_ever_going_out_or_running_away() {
        // The gutter has to move, stay bounded, and still be moving after the
        // sort of run a screensaver gets. That last one is why the phase is
        // taken in `f64`: an `f32` accumulator goes coarse enough to stop
        // advancing after about six days, and the flame would simply freeze.
        //
        // Measured over the wake alone rather than over the whole frame. The
        // hull does not flicker and there is a great deal more of it, so a
        // gutter perfectly visible in the flame comes to about a percent of the
        // total light and any threshold that catches it is really measuring
        // the plates.
        let ship = at_warp();
        let model = &models()[0];
        let sample = |time: f64| wake(model, &ship, 200, 60, time).2;

        let steady = sample(0.0);
        for start in [0.0, 86_400.0, 6.0 * 86_400.0, 40.0 * 86_400.0] {
            let (mut lowest, mut highest) = (f32::MAX, 0.0f32);
            for step in 0..24 {
                let light = sample(start + step as f64 * 0.037);
                assert!(
                    light.is_finite() && light > 0.0,
                    "the flame went out at {start}"
                );
                lowest = lowest.min(light);
                highest = highest.max(light);
            }
            assert!(
                highest > lowest * 1.02,
                "the flame stopped guttering {} days in: {lowest} to {highest}",
                start / 86_400.0
            );
            assert!(
                highest < steady * 3.0,
                "the flame ran away {} days in: {highest} against {steady}",
                start / 86_400.0
            );
        }
    }

    #[test]
    fn the_drive_catching_surges_the_trail_and_dropping_out_guts_it() {
        // The two moments the drive has, said by the ship rather than by the
        // frame. The screen-wide white-out already covers the instant of an
        // engage, so the surge is what is left of it as that fades; the dropout
        // has no flash at all, and without this it would look exactly like a
        // hand easing the throttle back.
        // Both are measured as light rather than as reach, because both move
        // the length as well and a plume that has run off the side of the frame
        // reports the frame's width whatever it is really doing.
        let model = &models()[0];
        let steady = at_warp();
        let (_, _, held) = wake(model, &steady, 400, 60, 0.0);

        let mut caught = steady.clone();
        caught.flash = 1.0;
        let (_, _, surged) = wake(model, &caught, 400, 60, 0.0);
        assert!(
            surged > held * 1.3,
            "the drive catching did not throw the flame: {held} then {surged}"
        );

        let mut quitting = steady.clone();
        assert!(!quitting.toggle_warp());
        assert!(quitting.dropping_out());
        let (_, _, guttered) = wake(model, &quitting, 400, 60, 0.0);
        assert!(
            guttered * 2.0 < held,
            "the drive quitting did not put the flame out: {held} then {guttered}"
        );
    }

    #[test]
    fn the_hull_leans_with_the_stick_but_never_loses_its_profile() {
        // Roll is taken as flown; pitch and yaw are transients off the rates.
        // Between them, no input can turn the ship to face the camera.
        let mut ship = Ship::new();
        ship.roll = 2.0;
        ship.bank = 0.3;
        let (roll, _, _) = attitude(&ship);
        assert!(
            (roll - 2.3).abs() < 1e-5,
            "roll is not flown as flown: {roll}"
        );

        // Past full deflection on both axes, which the impulses cannot reach
        // but a clamp that was quietly dropped would let through.
        for pitch in [-2.0f32, -MAX_PITCH_RATE, 0.0, MAX_PITCH_RATE, 2.0] {
            for yaw in [-2.0f32, -MAX_YAW_RATE, 0.0, MAX_YAW_RATE, 2.0] {
                let mut ship = Ship::new();
                ship.pitch_rate = pitch;
                ship.yaw_rate = yaw;
                let (_, p, y) = attitude(&ship);
                assert!(p.abs() <= PITCH_LEAN + 1e-6, "pitch ran away to {p}");
                assert!(y.abs() <= YAW_LEAN + 1e-6, "yaw ran away to {y}");
            }
        }

        // And the stick has to actually do something, or the clamps above are
        // satisfied by a hull that never leans at all.
        let mut ship = Ship::new();
        ship.pitch_rate = MAX_PITCH_RATE;
        let (_, p, _) = attitude(&ship);
        assert!(
            (p - PITCH_LEAN).abs() < 1e-6,
            "full deflection should lean the hull the whole way: {p}"
        );
    }

    #[test]
    fn the_nose_is_to_the_right_and_pitching_down_puts_it_below_the_tail() {
        let ship = Ship::new();
        let (_, cam) = cam(120, 36, &ship);
        let level = attitude(&ship);
        let nose = cam
            .project(place([0.0, 0.0, 1.0], level, &abeam()))
            .unwrap();
        let tail = cam
            .project(place([0.0, 0.0, -1.0], level, &abeam()))
            .unwrap();
        assert!(nose.0 > tail.0, "the ship is flying backwards");
        assert!((nose.1 - tail.1).abs() < 0.5, "level flight is not level");

        let mut down = Ship::new();
        down.pitch_rate = MAX_PITCH_RATE; // positive is nose-down, per the flight model
        let pose = attitude(&down);
        let nose = cam.project(place([0.0, 0.0, 1.0], pose, &abeam())).unwrap();
        let tail = cam
            .project(place([0.0, 0.0, -1.0], pose, &abeam()))
            .unwrap();
        assert!(nose.1 > tail.1, "nose-down did not put the nose down");

        // And a roll to starboard swings the top of the fin toward the camera
        // — the camera is off the starboard beam — rather than tipping the
        // whole picture, which is what a camera that rolled with the ship would
        // do and is exactly the thing that makes a barrel roll worth watching.
        let mut rolled = Ship::new();
        rolled.roll = std::f32::consts::FRAC_PI_2;
        let pose = attitude(&rolled);
        let top = place([0.0, -1.0, 0.0], pose, &abeam());
        assert!(
            top[2] < STANDOFF - 0.4,
            "a quarter roll did not swing the top toward the camera: {top:?}"
        );
        assert!(
            top[1].abs() < 0.2,
            "and it should no longer be above anything: {top:?}"
        );
    }

    #[test]
    fn a_ship_flown_off_the_stick_still_points_where_it_is_going() {
        // Regression: the profile was posed from `ship.pitch`, the accumulated
        // attitude the panel reads out. Nothing out here is measured against
        // that — the ship flies where its nose points and the band of sky
        // streams along that track — so a few seconds of `W` in the cockpit
        // left the hull nose-high for the rest of the flight, against stars
        // still streaming dead level past it. The lean comes off the rate now,
        // so the nose goes back on the track the moment the stick is let go.
        let mut ship = Ship::new();
        for _ in 0..120 {
            ship.nudge_pitch(-1.0);
            ship.update(1.0 / 60.0);
        }
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(
            ship.pitch.abs() > 0.5,
            "the stick never moved the ship, so there is nothing to be wrong: {}",
            ship.pitch
        );

        let (_, cam) = cam(120, 36, &ship);
        let pose = attitude(&ship);
        let nose = cam.project(place([0.0, 0.0, 1.0], pose, &abeam())).unwrap();
        let tail = cam
            .project(place([0.0, 0.0, -1.0], pose, &abeam()))
            .unwrap();
        assert!(nose.0 > tail.0, "the ship is flying backwards");
        assert!(
            (nose.1 - tail.1).abs() < 0.5,
            "a ship that pitched about is still flying {:.1} subpixels crooked",
            nose.1 - tail.1
        );

        // While the stick is *being* worked it does lean, or the fix above is
        // just a hull that ignores the pilot.
        ship.nudge_pitch(-1.0);
        let pose = attitude(&ship);
        let nose = cam.project(place([0.0, 0.0, 1.0], pose, &abeam())).unwrap();
        let tail = cam
            .project(place([0.0, 0.0, -1.0], pose, &abeam()))
            .unwrap();
        assert!(nose.1 < tail.1 - 1.0, "pulling up did not raise the nose");
    }
}
