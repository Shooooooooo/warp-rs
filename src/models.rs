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
//! accumulated. There is still no depth buffer. Three things stand in for one:
//! the star band starts well beyond the ship, so nothing can be in front of it;
//! plates facing away are culled; and the rest are painted far to near, which
//! is what sorts a nacelle against the engineering hull behind it.

use crate::canvas::Canvas;
use crate::ship::{Ship, MAX_PITCH_RATE, MAX_YAW_RATE};
use crate::starfield::{self, Camera, Streak};
use crate::view::{Eye, HULL_REACH, MIN_SHIP_DISTANCE};
use std::sync::OnceLock;

/// The zoom may push the camera in, and pushing it in far enough would put the
/// near corner of a rolled hull through the plane the projection gives up at —
/// where `Camera::project` answers `None` and [`plates`] drops the face. The
/// hull would come apart a plate at a time rather than fail outright, which is
/// the sort of thing that goes unnoticed for a while, so the closest the zoom
/// can get is a compile-time fact.
///
/// Its opposite number — the ship not reaching *out* into the star band — is in
/// [`crate::exterior`], beside the near wall it is about.
const _: () = assert!(
    MIN_SHIP_DISTANCE - HULL_REACH > starfield::Z_NEAR,
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
/// longer flame, which hands the fleet its variety for nothing: the Beetle's
/// 0.17 bells trail the furthest, the Enterprise's 0.07 impulse engine leaves a
/// stub above two nacelle lances. At 12.0 a nacelle plume is 1.3 hull units at
/// full impulse — a little under the hull's own length, which is enough to read
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
/// The same *shape* as the sky's `1.0 + warp * warp * 5.0` in
/// [`crate::exterior`], and deliberately not the same number. Taken literally —
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
/// equal length draw a *rectangle* — `draw_streak` ramps down to
/// `TAIL_BRIGHTNESS` rather than to nothing, so the far end stops dead at a
/// third of full brightness and the sides stop dead at the outermost lane, and
/// the whole thing reads as a brown block bolted to the tail. Shortening the
/// outer lanes carries the tails round in a curve from the widest point to a
/// tip, which is the shape a flame has.
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
const PLUME_NEAR: f32 = starfield::Z_NEAR * 1.05;
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
/// Phase offsets, per bell and per lane of the fan. The first stops a four-bell
/// freighter pulsing in lockstep; the second is small on purpose, because a fan
/// whose lanes disagree strongly reads as static rather than as fire.
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
        assert!(sections.len() >= 2, "a shell needs two sections to loft");
        let rings: Vec<u16> = sections
            .iter()
            .map(|s| self.push_ring(s.corners()))
            .collect();

        for pair in rings.windows(2) {
            let (aft, fore) = (pair[0], pair[1]);
            for i in 0..4u16 {
                let j = (i + 1) % 4;
                self.faces.push(vec![aft + i, aft + j, fore + j, fore + i]);
            }
        }
        // The nose cap faces forward; the tail cap is the same ring the other
        // way about, so its normal points aft.
        let fore = rings[rings.len() - 1];
        self.faces.push(vec![fore, fore + 1, fore + 2, fore + 3]);
        let aft = rings[0];
        self.faces.push(vec![aft + 3, aft + 2, aft + 1, aft]);
    }

    /// A hoop standing across the ship, drawn as a square-section tube. The
    /// only shape here a shell cannot make, and the whole silhouette of one of
    /// the ships.
    fn hoop(&mut self, centre: [f32; 3], major: f32, minor: f32, arcs: usize) {
        const SIDES: usize = 4;
        let base = self.verts.len() as u16;
        for a in 0..arcs {
            let theta = a as f32 / arcs as f32 * std::f32::consts::TAU;
            // The hoop stands in the plane across the ship's track, so its
            // radius sweeps through `y` and `z` and its tube reaches out
            // in `x`.
            let radial = [0.0, theta.sin(), theta.cos()];
            for s in 0..SIDES {
                let phi = s as f32 / SIDES as f32 * std::f32::consts::TAU;
                let out = major + minor * phi.sin();
                self.verts.push([
                    centre[0] + minor * phi.cos(),
                    centre[1] + out * radial[1],
                    centre[2] + out * radial[2],
                ]);
            }
        }
        for a in 0..arcs {
            let next_a = (a + 1) % arcs;
            for s in 0..SIDES {
                let next_s = (s + 1) % SIDES;
                let at = |ai: usize, si: usize| base + (ai * SIDES + si) as u16;
                self.faces.push(vec![
                    at(a, s),
                    at(a, next_s),
                    at(next_a, next_s),
                    at(next_a, s),
                ]);
            }
        }
    }

    /// A thin box, for a wing or a fin: the same solid a plate would be if a
    /// plate had an outside.
    fn plate(&mut self, aft: Section, fore: Section) {
        self.shell(&[aft, fore]);
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
    MODELS.get_or_init(|| {
        vec![
            enterprise(),
            dart(),
            hauler(),
            needle(),
            beetle(),
            trident(),
        ]
    })
}

/// The one flown when nothing has said otherwise: the first in the list.
///
/// It does not *supply* that default — `--ship` carries the name as a string,
/// because clap wants one and an index would be a poor thing to type. This is
/// what the picker opens on, and
/// `the_view_and_the_ship_can_be_chosen_at_the_command_line` in `cli.rs` is
/// what holds the two of them together.
pub const DEFAULT_MODEL: usize = 0;

/// Look a ship up by the name `--ship` and the picker use.
pub fn by_name(name: &str) -> Option<usize> {
    let name = name.trim().to_ascii_lowercase();
    models().iter().position(|m| m.name == name)
}

/// Saucer, neck, engineering hull, two nacelles on swept pylons — a bow to the
/// ship every warp drive since has been drawn against.
///
/// It is designed as a *profile*, because that is the only view this camera
/// gives, and the profile is three masses stacked in a particular order: the
/// saucer highest and furthest forward, the nacelles just below and well aft of
/// it, and the engineering hull slung underneath on a neck that leans forward.
/// Get that stacking wrong — nacelles above the saucer, say — and every line is
/// still in the right place while the ship stops being this ship. Seen from the
/// beam the two nacelles line up into one, which is the silhouette everybody
/// already has in mind, and the reason this hull is the one flown by default.
///
/// Everything vertical is drawn thicker than scale. The shot opens with the
/// whole ship a quarter of the canvas height long — [`SHIP_SCREEN_FRAC`] of it
/// either side of centre — so on a thirty-row terminal, sixty subpixels tall,
/// one unit here is about seven subpixels, and an honest saucer would not come
/// out even a subpixel thick: nothing at all, rather than a shape. Zooming in
/// buys that back, but the hull has to read at the framing it opens on.
///
/// [`SHIP_SCREEN_FRAC`]: crate::view::SHIP_SCREEN_FRAC
fn enterprise() -> ShipModel {
    let mut b = Builder::default();
    // The saucer: widest across the middle, and about as long as it is wide, so
    // from above it would read as the disc it is. Deliberately thin — a saucer
    // as deep as a nacelle reads as a third nacelle — with the bridge dome
    // above it doing the work of saying which way up the ship goes.
    b.shell(&[
        Section::offset(0.22, 0.0, -0.31, 0.12, 0.032),
        Section::offset(0.36, 0.0, -0.32, 0.30, 0.065),
        Section::offset(0.58, 0.0, -0.32, 0.42, 0.085),
        Section::offset(0.80, 0.0, -0.32, 0.36, 0.070),
        Section::offset(0.94, 0.0, -0.31, 0.19, 0.044),
        Section::offset(1.00, 0.0, -0.30, 0.05, 0.024),
    ]);
    b.shell(&[
        Section::offset(0.50, 0.0, -0.40, 0.10, 0.040),
        Section::offset(0.68, 0.0, -0.41, 0.13, 0.048),
        Section::offset(0.82, 0.0, -0.39, 0.08, 0.030),
    ]);
    // The neck, leaning forward as it climbs from the engineering hull.
    b.shell(&[
        Section::offset(-0.02, 0.0, 0.16, 0.055, 0.15),
        Section::offset(0.34, 0.0, -0.18, 0.05, 0.11),
    ]);
    // The engineering hull, slung below and aft, with the deflector at its
    // forward end.
    b.shell(&[
        Section::offset(-0.82, 0.0, 0.34, 0.055, 0.055),
        Section::offset(-0.64, 0.0, 0.34, 0.12, 0.13),
        Section::offset(-0.22, 0.0, 0.34, 0.145, 0.155),
        Section::offset(0.20, 0.0, 0.32, 0.12, 0.13),
        Section::offset(0.34, 0.0, 0.31, 0.08, 0.085),
        Section::offset(0.40, 0.0, 0.31, 0.04, 0.045),
    ]);
    for side in [-1.0f32, 1.0] {
        // Pylons, reaching out, up and aft all at once: the strut is what makes
        // the gap between hull and nacelle read as a gap rather than as a join.
        b.plate(
            Section::offset(-0.56, side * 0.40, 0.06, 0.035, 0.10),
            Section::offset(-0.20, side * 0.10, 0.28, 0.045, 0.11),
        );
        // Nacelles: below the saucer's plane and well behind it, which is the
        // half of the stacking that says this is not a flying saucer with an
        // engine bolted on.
        b.shell(&[
            Section::offset(-0.88, side * 0.42, -0.06, 0.050, 0.050),
            Section::offset(-0.76, side * 0.42, -0.06, 0.080, 0.085),
            Section::offset(-0.10, side * 0.42, -0.06, 0.080, 0.085),
            Section::offset(0.02, side * 0.42, -0.06, 0.050, 0.055),
        ]);
    }
    b.finish(
        "enterprise",
        "Heavy cruiser. Saucer, neck, and two nacelles.",
        [0.21, 0.24, 0.31],
        vec![
            engine([-0.42, -0.06, -0.92], 0.11),
            engine([0.42, -0.06, -0.92], 0.11),
            // Impulse, out of the back of the saucer, and much the smaller.
            engine([0.0, -0.31, 0.21], 0.07),
        ],
    )
}

/// A knife with a fin: all nose and engine, nothing spare.
fn dart() -> ShipModel {
    let mut b = Builder::default();
    b.shell(&[
        Section::at(-0.74, 0.16, 0.14),
        Section::at(-0.30, 0.20, 0.17),
        Section::at(0.22, 0.17, 0.13),
        Section::at(0.74, 0.08, 0.06),
        Section::at(1.00, 0.02, 0.02),
    ]);
    // Wings, swept back and down from the flanks.
    for side in [-1.0f32, 1.0] {
        b.plate(
            Section::offset(-0.66, side * 0.55, 0.03, 0.36, 0.02),
            Section::offset(-0.14, side * 0.28, 0.02, 0.10, 0.03),
        );
    }
    // A fin standing up over the tail.
    b.plate(
        Section::offset(-0.72, 0.0, -0.44, 0.03, 0.26),
        Section::offset(-0.16, 0.0, -0.20, 0.03, 0.06),
    );
    b.finish(
        "dart",
        "Interceptor. All nose and engine.",
        [0.19, 0.22, 0.28],
        vec![
            engine([-0.24, 0.0, -0.78], 0.13),
            engine([0.24, 0.0, -0.78], 0.13),
        ],
    )
}

/// A brick with a bridge on it, and containers slung underneath.
fn hauler() -> ShipModel {
    let mut b = Builder::default();
    b.shell(&[
        Section::at(-0.84, 0.30, 0.24),
        Section::at(0.58, 0.34, 0.27),
        Section::offset(0.94, 0.0, -0.04, 0.16, 0.14),
    ]);
    // The bridge, stepped up out of the spine.
    b.shell(&[
        Section::offset(0.06, 0.0, -0.38, 0.16, 0.14),
        Section::offset(0.42, 0.0, -0.36, 0.14, 0.12),
    ]);
    // Two slung containers, which is what makes it read as cargo.
    for side in [-1.0f32, 1.0] {
        b.shell(&[
            Section::offset(-0.60, side * 0.40, 0.42, 0.14, 0.18),
            Section::offset(0.34, side * 0.40, 0.42, 0.14, 0.18),
        ]);
    }
    // The engine housing, wider than the tail it hangs off.
    b.shell(&[
        Section::offset(-1.00, 0.0, 0.02, 0.30, 0.20),
        Section::offset(-0.84, 0.0, 0.0, 0.26, 0.18),
    ]);
    b.finish(
        "hauler",
        "Bulk freighter. Slow, and does not care.",
        [0.24, 0.21, 0.17],
        vec![
            engine([-0.17, -0.06, -1.02], 0.11),
            engine([0.17, -0.06, -1.02], 0.11),
            engine([-0.17, 0.10, -1.02], 0.11),
            engine([0.17, 0.10, -1.02], 0.11),
        ],
    )
}

/// A pin through a hoop: the survey ship, and the one the sky bends through.
fn needle() -> ShipModel {
    let mut b = Builder::default();
    b.shell(&[
        Section::at(-0.92, 0.05, 0.05),
        Section::at(-0.60, 0.07, 0.07),
        Section::at(0.62, 0.06, 0.06),
        Section::at(0.78, 0.14, 0.14),
        Section::at(1.00, 0.04, 0.04),
    ]);
    b.hoop([0.0, 0.0, 0.04], 0.52, 0.05, 12);
    // Struts holding the hoop off the spine.
    for side in [-1.0f32, 1.0] {
        b.plate(
            Section::offset(0.0, 0.0, side * 0.30, 0.03, 0.24),
            Section::offset(0.08, 0.0, side * 0.30, 0.03, 0.24),
        );
    }
    b.finish(
        "needle",
        "Survey probe. Mostly sensor.",
        [0.18, 0.21, 0.25],
        vec![engine([0.0, 0.0, -0.96], 0.15)],
    )
}

/// Short, tall and round-shouldered — the opposite read from the Dart.
fn beetle() -> ShipModel {
    let mut b = Builder::default();
    b.shell(&[
        Section::offset(-0.78, 0.0, 0.06, 0.20, 0.14),
        Section::offset(-0.42, 0.0, 0.0, 0.42, 0.34),
        Section::offset(0.06, 0.0, -0.04, 0.52, 0.44),
        Section::offset(0.52, 0.0, 0.02, 0.36, 0.30),
        Section::offset(0.86, 0.0, 0.06, 0.12, 0.10),
    ]);
    // A blister under the chin, where the guns would be.
    b.shell(&[
        Section::offset(0.08, 0.0, 0.46, 0.18, 0.12),
        Section::offset(0.54, 0.0, 0.42, 0.12, 0.09),
    ]);
    // Canards, low and forward.
    for side in [-1.0f32, 1.0] {
        b.plate(
            Section::offset(0.10, side * 0.62, -0.02, 0.22, 0.02),
            Section::offset(0.34, side * 0.40, -0.06, 0.12, 0.03),
        );
    }
    b.finish(
        "beetle",
        "Gunship. Built round its own armour.",
        [0.25, 0.20, 0.20],
        vec![
            engine([-0.16, 0.06, -0.82], 0.17),
            engine([0.16, 0.06, -0.82], 0.17),
        ],
    )
}

/// Three prongs stacked so all of them show from the beam.
fn trident() -> ShipModel {
    let mut b = Builder::default();
    b.shell(&[
        Section::at(-0.88, 0.12, 0.16),
        Section::at(-0.20, 0.15, 0.20),
        Section::at(0.24, 0.12, 0.15),
    ]);
    for tine in [-0.46f32, 0.0, 0.46] {
        b.shell(&[
            Section::offset(0.18, 0.0, tine * 0.55, 0.06, 0.07),
            Section::offset(0.70, 0.0, tine, 0.05, 0.06),
            Section::offset(0.98, 0.0, tine, 0.02, 0.02),
        ]);
    }
    // Swept tail vanes, above and below.
    for side in [-1.0f32, 1.0] {
        b.plate(
            Section::offset(-0.86, 0.0, side * 0.52, 0.04, 0.22),
            Section::offset(-0.42, 0.0, side * 0.24, 0.04, 0.10),
        );
    }
    b.finish(
        "trident",
        "Line warship. Three drives, one spine.",
        [0.20, 0.21, 0.27],
        vec![
            engine([0.0, -0.30, -0.90], 0.12),
            engine([0.0, 0.0, -0.92], 0.13),
            engine([0.0, 0.30, -0.90], 0.12),
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
        // against a near plane of `starfield::Z_NEAR`, which is the `const`
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
    for plate in plates(model, cam, pose, eye) {
        // Nearer plates read a shade brighter. It is a small effect on purpose:
        // the lighting says which way a plate faces, and this only says which
        // of two plates facing the same way is the closer. Measured against the
        // standoff in force rather than a fixed one, so the ratio stays centred
        // on the hull's own middle — and flattens as the camera pulls back,
        // which is what a longer standoff really does to a subject.
        let near = (eye.distance / plate.depth).powf(DEPTH_SHADE);
        let paint = (plate.shade + bubble) * near;
        let mut lit = [0.0; 3];
        for (channel, base) in lit.iter_mut().zip(model.hull) {
            *channel = base * paint;
        }
        canvas.fill_convex(&plate.points, lit);
    }

    draw_engines(canvas, cam, ship, model, pose, eye, time);
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

/// The drive: a glow out of each bell — blue on impulse, whitening at warp, and
/// out with the throttle — with the exhaust it throws behind it.
///
/// Both are drawn here, after the plates, and that placement is the one thing
/// in this function that is a decision rather than a number. Everything in
/// `Canvas` accumulates except `fill_convex`, so an opaque write over a glow
/// erases it and the bells have always had to come last. The trail inherits
/// that, and it is not free: the Enterprise's impulse engine is the one bell in
/// the fleet that is not on the tail, and its plume clears the nacelle tops by
/// 0.165 hull units — a subpixel and a half at the reference framing, so a roll
/// walks it straight across them. Drawn under the plates it would be chopped by
/// a silhouette it is barely clear of; drawn over them it shines through as the
/// wash a hot plume genuinely puts on structure it plays over. The second is
/// the cheaper mistake, and it leaves one rule here instead of two.
fn draw_engines(
    canvas: &mut Canvas,
    cam: &Camera,
    ship: &Ship,
    model: &ShipModel,
    pose: (f32, f32, f32),
    eye: &Eye,
    time: f64,
) {
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
            },
        );
        canvas.add_glow(
            screen.0,
            screen.1,
            radius * (1.0 + 0.9 * lit),
            color,
            0.10 + 0.75 * lit,
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
    /// frame edge. Not the same question as `warp > 0.0`: a drive spinning
    /// down is superluminal and switched off at once.
    engaged: bool,
    surge: f32,
    surge_reach: f32,
    gutter: f32,
    flicker_amt: f32,
    time: f64,
}

/// The exhaust behind one bell: a short fan of streaks laid down the ship's
/// own axis, brightest where it leaves the nozzle.
///
/// Drawn with [`Canvas::draw_streak`] — the primitive the whole sky is made of
/// — because three of the four things a plume wants fall out of it for nothing:
/// the ramp already runs from `TAIL_BRIGHTNESS` at `from` to full at `to`, so
/// putting the head at the nozzle brightens the exhaust where it is hottest;
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
    // a clean sine — and staggered per bell so a four-bell freighter does not
    // pulse in lockstep. Evaluated in `f64`, like the camera shake: there are
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

    // A lit warp drive does not trail, it tears: the lance runs clean off the
    // side of the frame, and it gets there the moment the drive catches rather
    // than growing into it over the warp range. The white-out on `Ship::flash`
    // covers that instant, so the jump arrives under cover.
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
        let out = cam.width.hypot(cam.height) / span;
        dx *= out;
        dy *= out;
    }

    let brightness = TRAIL_INTENSITY
        * flame.lit
        * flame.gutter
        * flame.surge
        * (1.0 + flame.warp * TRAIL_WARP_LIFT)
        * (flame.bell.radius / NOMINAL_BELL);
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
        // rectangle instead: `draw_streak` ramps down to `TAIL_BRIGHTNESS`
        // rather than to nothing, so the far end would stop dead at a third of
        // full brightness and the sides at the outermost lane.
        let shorten = 1.0 - taper * out;
        let nozzle = (
            flame.head.0 + px * offset * throat,
            flame.head.1 + py * offset * throat,
        );
        let tip = (
            nozzle.0 + (dx + px * offset * widest) * shorten,
            nozzle.1 + (dy + py * offset * widest) * shorten,
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
    /// shot as it opens, both poles, head-on and dead astern, the view from
    /// port, and two corners with all three angles off zero at once.
    ///
    /// The ends are the point. Head-on is where the bubble stops being
    /// elongated and the hull presents its cross-section instead of its
    /// profile; the poles are where the hull's beam swings into the frame's
    /// vertical; and the corners are the only place the outline is turned off
    /// the horizontal at all.
    fn orbits() -> Vec<Orbit> {
        vec![
            Orbit::LEVEL,
            orbit(90.0, 0.0, 0.0),
            orbit(-90.0, 0.0, 0.0),
            orbit(180.0, 0.0, 0.0),
            orbit(0.0, 90.0, 0.0),
            orbit(0.0, -90.0, 0.0),
            orbit(0.0, 0.0, 137.0),
            orbit(55.0, 35.0, 20.0),
            orbit(-140.0, -62.0, -100.0),
            orbit(35.0, 89.0, 45.0),
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
            assert_eq!(by_name(model.name), Some(index_of(model.name)));
            assert_eq!(
                by_name(&model.name.to_uppercase()),
                Some(index_of(model.name))
            );
        }
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
                            // And the far end: the whole hull stays inside the
                            // wall the star band starts at, or a star could be
                            // drawn over it.
                            assert!(
                                at[2] < crate::exterior::Z_NEAR,
                                "{} reached into the star band at zoom {zoom}: {at:?}",
                                model.name
                            );
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
        // angle in `orbits()`, and over `models()`, so a seventh ship is
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
                let (_, total) = footprint(model, &ship, cols, rows);
                assert!(total.is_finite(), "{} at {cols}x{rows}", model.name);
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
    fn the_plume_keeps_its_width_when_the_ship_rolls() {
        // Why the fan is spread perpendicular in *screen* space and not in the
        // hull's. Hull `x` is almost pure camera depth from this beam, so a fan
        // opened along hull `y` alone would collapse to a hairline every time
        // the ship rolled ninety degrees — an axisymmetric plume must not
        // narrow with a roll.
        //
        // Flown on the Needle, whose one bell sits on the ship's own axis: its
        // plume lands in the same place at every roll, so the only thing left
        // that can move is the width of the fan around it.
        let model = &models()[index_of("needle")];
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
