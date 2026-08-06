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
use crate::ship::{Ship, MAX_YAW_RATE};
use crate::starfield::Camera;
use crate::view::SHIP_DISTANCE;
use std::sync::OnceLock;

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
const IMPULSE_FLAME: [f32; 3] = [1.00, 0.62, 0.28];
const WARP_FLAME: [f32; 3] = [0.62, 0.80, 1.00];

/// Lean the hull takes from a turn, in radians at full deflection. The camera
/// rides with the ship, so this is the only thing that says a turn is happening.
const YAW_LEAN: f32 = 0.35;
/// How much of the ship's own pitch the profile shows, and the most it shows.
/// Full deflection is 76 degrees, which side-on reads as flying sideways.
const PITCH_LEAN: f32 = 0.40;
const PITCH_LIMIT: f32 = 0.55;

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
            // radius sweeps through `y` and `z` and its tube reaches out in `x`.
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
/// Everything vertical is drawn thicker than scale. The whole ship is a little
/// over half the canvas height long, so on a thirty-row terminal one unit here
/// is about twenty subpixels, and an honest saucer would come out a single
/// subpixel thick — a line, not a shape.
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

/// Turn a hull-space point into the side camera's space.
///
/// `(x, y, z) → (z, y, −x)` is a proper quarter turn about the ship's own down
/// axis — determinant one, so face winding survives it — putting the nose to
/// screen right and the starboard side toward the camera. The mirror image,
/// `(z, y, x)`, would look almost the same and invert every facing test.
fn to_camera(v: [f32; 3]) -> [f32; 3] {
    [v[2], v[1], SHIP_DISTANCE - v[0]]
}

/// The attitude the hull is holding, as roll, pitch and yaw in radians.
///
/// Roll is taken as flown: the camera does not roll with the ship, so `Q` and
/// `E` read as a barrel roll against a level sky, which is the best thing in
/// this view. Pitch is scaled down and capped — the flight model allows 76
/// degrees, and a profile at 76 degrees reads as a ship flying sideways — and
/// yaw is a transient lean out of the turn rate rather than an accumulated
/// heading, because a camera bolted to the ship's beam would otherwise end up
/// looking at its transom.
fn attitude(ship: &Ship) -> (f32, f32, f32) {
    let roll = ship.roll + ship.bank;
    let pitch = (ship.pitch * PITCH_LEAN).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    let yaw = (ship.yaw_rate / MAX_YAW_RATE).clamp(-1.0, 1.0) * YAW_LEAN;
    (roll, pitch, yaw)
}

/// Place a hull-space point: roll about the nose, then pitch, then yaw, then
/// out into the camera's space.
fn place(v: [f32; 3], (roll, pitch, yaw): (f32, f32, f32)) -> [f32; 3] {
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
    to_camera([x, y, z])
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
fn plates(model: &ShipModel, cam: &Camera, pose: (f32, f32, f32)) -> Vec<Plate> {
    let placed: Vec<[f32; 3]> = model.verts.iter().map(|v| place(*v, pose)).collect();
    let screen: Vec<Option<(f32, f32)>> = placed.iter().map(|v| cam.project(*v)).collect();

    let mut plates: Vec<Plate> = Vec::with_capacity(model.faces.len());
    for face in &model.faces {
        // A plate with a vertex behind the near plane cannot be measured, let
        // alone drawn. Nothing should reach that — the hull sits four units
        // clear of it — but `project` answers with an `Option`, and a rolled
        // fin is exactly the thing that would find out.
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
/// well beyond the hull, so nothing can come between it and the camera, and the
/// hull sorts against itself.
pub fn draw(canvas: &mut Canvas, cam: &Camera, ship: &Ship, model: &ShipModel) {
    let pose = attitude(ship);
    let bubble = ship.warp_intensity() * BUBBLE_LIGHT;
    for plate in plates(model, cam, pose) {
        // Nearer plates read a shade brighter. It is a small effect on purpose:
        // the lighting says which way a plate faces, and this only says which
        // of two plates facing the same way is the closer.
        let near = (SHIP_DISTANCE / plate.depth).powf(DEPTH_SHADE);
        let paint = (plate.shade + bubble) * near;
        let mut lit = [0.0; 3];
        for (channel, base) in lit.iter_mut().zip(model.hull) {
            *channel = base * paint;
        }
        canvas.fill_convex(&plate.points, lit);
    }

    draw_engines(canvas, cam, ship, model, pose);
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

/// The glow out of the bells: amber on impulse, blue-white at warp, and out
/// with the throttle.
fn draw_engines(
    canvas: &mut Canvas,
    cam: &Camera,
    ship: &Ship,
    model: &ShipModel,
    pose: (f32, f32, f32),
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

    for bell in &model.engines {
        let at = place(bell.at, pose);
        let Some(screen) = cam.project(at) else {
            continue;
        };
        let radius = bell.radius * cam.focal / at[2].max(f32::MIN_POSITIVE);
        canvas.add_glow(
            screen.0,
            screen.1,
            radius * (1.0 + 0.9 * lit),
            color,
            0.10 + 0.75 * lit,
        );
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
    use std::collections::HashMap;

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
        let (renderer, cam) = cam(cols, rows, ship);
        let (w, h) = renderer.canvas_dims();
        let mut canvas = Canvas::new(w, h);
        draw(&mut canvas, &cam, ship, model);
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
        for model in models() {
            for step in 0..12 {
                let mut ship = Ship::new();
                ship.roll = step as f32 / 12.0 * std::f32::consts::TAU - std::f32::consts::PI;
                ship.pitch = if step % 2 == 0 { 1.2 } else { -1.2 };
                ship.yaw_rate = if step % 3 == 0 { MAX_YAW_RATE } else { 0.0 };
                let (lit, _) = footprint(model, &ship, 120, 36);
                assert!(
                    lit > 60,
                    "{} nearly vanished at step {step}: {lit} subpixels",
                    model.name
                );
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
            let facing = plates(model, &cam, attitude(&ship));
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
    fn plates_are_painted_far_to_near() {
        // The other half of the hidden-surface removal, and the half culling
        // cannot do: these hulls are assemblies of separate solids, so a
        // nacelle passing in front of an engineering hull is two plates that
        // both face the camera and both want the same subpixels.
        let mut ship = Ship::new();
        ship.roll = 0.5;
        let (_, cam) = cam(120, 36, &ship);
        for model in models() {
            let painted = plates(model, &cam, attitude(&ship));
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
        let sky = 0.9f32;
        let mut ship = Ship::new();
        ship.throttle = 0.5;
        let (renderer, cam) = cam(200, 56, &ship);
        let (w, h) = renderer.canvas_dims();

        for model in models() {
            let mut canvas = Canvas::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    canvas.splat(x as f32, y as f32, [1.0; 3], sky);
                }
            }
            draw(&mut canvas, &cam, &ship, model);

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
        // Five ships is only a feature if they read as five ships. Compared as
        // silhouettes on the canvas the camera actually uses.
        let ship = Ship::new();
        let (renderer, cam) = cam(120, 36, &ship);
        let (w, h) = renderer.canvas_dims();
        let masks: Vec<Vec<bool>> = models()
            .iter()
            .map(|model| {
                let mut canvas = Canvas::new(w, h);
                draw(&mut canvas, &cam, &ship, model);
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

    #[test]
    fn the_hull_leans_with_the_stick_but_never_loses_its_profile() {
        // Roll is taken as flown, pitch is scaled down, and yaw is a transient.
        // Between them, no input can turn the ship to face the camera.
        let mut ship = Ship::new();
        ship.roll = 2.0;
        ship.bank = 0.3;
        let (roll, _, _) = attitude(&ship);
        assert!(
            (roll - 2.3).abs() < 1e-5,
            "roll is not flown as flown: {roll}"
        );

        for pitch in [-1.4f32, -0.3, 0.0, 0.9, 1.4] {
            for rate in [-MAX_YAW_RATE, 0.0, MAX_YAW_RATE] {
                let mut ship = Ship::new();
                ship.pitch = pitch;
                ship.yaw_rate = rate;
                let (_, p, y) = attitude(&ship);
                assert!(p.abs() <= PITCH_LIMIT + 1e-6, "pitch ran away to {p}");
                assert!(y.abs() <= YAW_LEAN + 1e-6, "yaw ran away to {y}");
            }
        }
    }

    #[test]
    fn the_nose_is_to_the_right_and_pitching_down_puts_it_below_the_tail() {
        let ship = Ship::new();
        let (_, cam) = cam(120, 36, &ship);
        let level = attitude(&ship);
        let nose = cam.project(place([0.0, 0.0, 1.0], level)).unwrap();
        let tail = cam.project(place([0.0, 0.0, -1.0], level)).unwrap();
        assert!(nose.0 > tail.0, "the ship is flying backwards");
        assert!((nose.1 - tail.1).abs() < 0.5, "level flight is not level");

        let mut down = Ship::new();
        down.pitch = 1.0; // positive is nose-down, per the flight model
        let pose = attitude(&down);
        let nose = cam.project(place([0.0, 0.0, 1.0], pose)).unwrap();
        let tail = cam.project(place([0.0, 0.0, -1.0], pose)).unwrap();
        assert!(nose.1 > tail.1, "nose-down did not put the nose down");

        // And a roll to starboard swings the top of the fin toward the camera
        // — the camera is off the starboard beam — rather than tipping the
        // whole picture, which is what a camera that rolled with the ship would
        // do and is exactly the thing that makes a barrel roll worth watching.
        let mut rolled = Ship::new();
        rolled.roll = std::f32::consts::FRAC_PI_2;
        let pose = attitude(&rolled);
        let top = place([0.0, -1.0, 0.0], pose);
        assert!(
            top[2] < SHIP_DISTANCE - 0.4,
            "a quarter roll did not swing the top toward the camera: {top:?}"
        );
        assert!(
            top[1].abs() < 0.2,
            "and it should no longer be above anything: {top:?}"
        );
    }
}
