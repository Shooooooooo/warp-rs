//! The ships, and how to draw one.

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
const _: () = assert!(
    MIN_SHIP_DISTANCE - HULL_REACH > camera::Z_NEAR,
    "the zoom can push a hull through the near plane, and a plate that cannot \
     be projected is simply not drawn"
);

/// How much nearer plates outshine further ones.
const DEPTH_SHADE: f32 = 1.2;

/// The direction of the light *from* the hull, in the camera's space: over the
/// viewer's shoulder and a good way above. Unit length; `x` is to screen right,
/// `y` is down, `z` is away from the eye.
const LIGHT: [f32; 3] = [0.26, -0.90, -0.35];
/// What a plate facing away from the lamp still shows. Not zero: unlit is not
/// the same as absent, and a hull against a black sky needs its dark side to
/// still read as hull.
const AMBIENT: f32 = 0.13;
const DIFFUSE: f32 = 0.95;
/// How much the warp bubble lights the ship it is wrapped around, at full warp.
/// The glow inside the bubble is drawn behind the hull, so without this the
/// ship gets steadily darker than its own backdrop as the drive spools and ends
/// up a murky silhouette against it — which is the wrong way round for
/// something sitting *inside* the light.
const BUBBLE_LIGHT: f32 = 0.45;
/// Colour of a lit engine at impulse, and at warp.
const IMPULSE_FLAME: [f32; 3] = [0.24, 0.55, 1.00];
const WARP_FLAME: [f32; 3] = [0.82, 0.92, 1.00];

/// How far aft a bell throws its exhaust, in hull units per unit of bell
/// radius, at the top of the sublight range.
const TRAIL_PER_RADIUS: f32 = 12.0;
/// What full warp multiplies that reach by, quadratic in the warp ramp.
const TRAIL_STRETCH: f32 = 2.8;
/// Per-lane brightness of a plume, before the length falloff spreads it.
const TRAIL_INTENSITY: f32 = 0.045;
/// How much brighter a lit warp drive burns than a lit impulse one.
const TRAIL_WARP_LIFT: f32 = 1.5;
/// The middle of the fleet's range of bell radii. Brightness is scaled by the
/// ratio to it, so the Enterprise's impulse engine — "much the smaller", by its
/// own comment — does not throw the same plume as its nacelles.
const NOMINAL_BELL: f32 = 0.12;
/// The most streaks one plume is drawn from.
const MAX_PLUME_LANES: usize = 9;
/// Half-width of the fan where it leaves the bell, and where it is widest, both
/// as multiples of the bell's own radius on screen. Exhaust leaves a nozzle
/// narrower than the nozzle and expands into vacuum, and lanes that all
/// converged on one point would pile their whole light onto the single subpixel
/// the bell's glow already owns.
const PLUME_THROAT: f32 = 0.7;
const PLUME_FLARE: f32 = 1.6;
/// How much shorter the outermost lane of the fan is than the centre one.
const PLUME_TAPER: f32 = 0.55;
const PLUME_TAPER_AT_WARP: f32 = 0.15;
/// How much dimmer the outermost lane is than the centre one, quadratically.
/// The skirt of a plume is not its core, and an edge that ends at full value
/// draws a line down each side of it.
const PLUME_EDGE_FADE: f32 = 0.8;
/// Where a plume is cut in the camera's space.
const PLUME_NEAR: f32 = camera::Z_NEAR * 1.05;
/// How hard the flame gutters at impulse, and at warp.
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
const TRAIL_SURGE: f32 = 1.6;
const TRAIL_SURGE_REACH: f32 = 0.6;
/// What is left of a plume while the drive is spinning down. Speed alone cannot
/// say this: from out here a dropout and a throttle eased back are the same
/// falling number, and only one of them should put the flame out.
const TRAIL_DROPOUT: f32 = 0.35;

/// How far the track has to be turned out of the image plane before the hull
/// counts as wholly in front of the drive, as the sine of that angle. About six
/// degrees.
const OCCLUSION_BAND: f32 = 0.1045;

/// Lean the hull takes from a turn, in radians at full deflection. The camera
/// rides with the ship, so this is the only thing that says a turn is
/// happening.
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

/// A hull the picker can offer and `--ship` can name.
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
    fn shell(&mut self, sections: &[Section]) {
        self.loft(sections, 4);
    }

    /// The same, with each section's rectangle cut into `sides` points.
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
#[cfg(test)]
pub const DEFAULT_MODEL: usize = 0;

/// Look a ship up by the name `--ship` and the picker use.
pub fn by_name(name: &str) -> Option<usize> {
    let name = name.trim().to_ascii_lowercase();
    models().iter().position(|m| m.name == name)
}

/// The saucer's radius, in hull units.
const SAUCER_R: f32 = 0.415;
/// Where the disc's centre sits along the track, which puts its front edge on
/// the nose.
const SAUCER_Z: f32 = 1.0 - SAUCER_R;
/// And where its plane sits across the track: far enough above centre that the
/// engineering hull hanging under it and the nacelles standing over it come out
/// either side of the axis the ship rolls about.
const SAUCER_Y: f32 = -0.172;

/// How much taller than scale this hull is drawn.
const STOUT: f32 = 1.4;

/// Saucer, neck, engineering hull, two nacelles on pylons — a bow to the ship
/// every warp drive since has been drawn against.
fn enterprise() -> ShipModel {
    let mut b = Builder::default();

    // Reference units into hull units.
    let z_of = |z: f32| SAUCER_Z + z * SAUCER_R;
    let x_of = |x: f32| x * SAUCER_R;
    let y_of = |up: f32| SAUCER_Y - up * SAUCER_R * STOUT;
    let girth = |g: f32| g * SAUCER_R * STOUT;

    // The saucer, lofted along the track through the disc's own chords: the
    // half-width at each station is the circle's, so the plan outline is round
    // by construction rather than by choosing flattering numbers.
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
    // The neck, leaning forward as it climbs from the engineering hull.
    b.shell(&[
        Section::offset(z_of(-1.313), 0.0, y_of(-0.409), girth(0.048), girth(0.100)),
        Section::offset(z_of(-0.373), 0.0, y_of(-0.055), girth(0.048), girth(0.100)),
    ]);
    // The engineering hull: a tube slung below and aft, fattest a third of the
    // way along and tapering both ways.
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
        // underside.
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
        // front.
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
        // The intercooler down its inboard flank.
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
            // taken off the reference.
            engine([-x_of(0.649), y_of(0.121), z_of(-3.58)], 0.11),
            engine([x_of(0.649), y_of(0.121), z_of(-3.58)], 0.11),
            // Impulse, out of the back of the saucer, and much the smaller.
            engine([0.0, y_of(-0.023), z_of(-1.05)], 0.07),
        ],
    )
}

/// The Normandy's reference units into hull units.
const NORMANDY_SCALE: f32 = 1.96 / 461.61;
/// The station along the reference that lands on `z = 0`: halfway between the
/// tail cap at `-121.58` and the nose at `+340.03`.
const NORMANDY_MID: f32 = 109.225;
/// The height that lands on `y = 0`, and it is the *tail cap's own centre*
/// rather than the mesh's mid-height or the hull's.
const NORMANDY_DATUM: f32 = 55.91;

/// How much taller than scale this hull is drawn. [`STOUT`], told about the
/// same axis and for very nearly the same reason.
const NORMANDY_STOUT: f32 = 1.3;

/// How far the wings hang, in radians, measured down from level at the tip.
const WING_LEAN: f32 = 0.42;

/// A frigate: a long flat back, a nose drawn out to a blade, and two wings hung
/// low and leaning down to their tips.
fn normandy() -> ShipModel {
    let mut b = Builder::default();

    // Reference units into hull units.
    let z_of = |along: f32| (along - NORMANDY_MID) * NORMANDY_SCALE;
    let x_of = |across: f32| across * NORMANDY_SCALE;
    let y_of = |up: f32| (NORMANDY_DATUM - up) * NORMANDY_SCALE * NORMANDY_STOUT;
    let girth = |g: f32| g * NORMANDY_SCALE * NORMANDY_STOUT;
    // One frame of the fuselage, from the four numbers a frame is actually
    // measured in: how far along, how far out, and where its spine and its
    // belly sit.
    let station = |along: f32, half: f32, top: f32, bottom: f32| {
        Section::offset(
            z_of(along),
            0.0,
            y_of((top + bottom) * 0.5),
            x_of(half),
            girth((top - bottom) * 0.5),
        )
    };

    // The fuselage.
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
        // skin over the middle third of the ship.
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

        // The wing.
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
        // enterprise's blue-grey.
        [0.30, 0.30, 0.33],
        vec![
            // One bell, on the ship's own axis, out of the middle of the tail
            // cap.
            engine([0.0, 0.0, -1.0], 0.15),
        ],
    )
}

/// The attitude the hull is holding, as roll, pitch and yaw in radians.
fn attitude(ship: &Ship) -> (f32, f32, f32) {
    let roll = ship.roll + ship.bank;
    let pitch = (ship.pitch_rate / MAX_PITCH_RATE).clamp(-1.0, 1.0) * PITCH_LEAN;
    let yaw = (ship.yaw_rate / MAX_YAW_RATE).clamp(-1.0, 1.0) * YAW_LEAN;
    (roll, pitch, yaw)
}

/// Place a hull-space point: roll about the nose, then pitch, then yaw, then
/// out into the camera's space.
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
fn plates(model: &ShipModel, cam: &Camera, pose: (f32, f32, f32), eye: &Eye) -> Vec<Plate> {
    let placed: Vec<[f32; 3]> = model.verts.iter().map(|v| place(*v, pose, eye)).collect();
    let screen: Vec<Option<(f32, f32)>> = placed.iter().map(|v| cam.project(*v)).collect();

    let mut plates: Vec<Plate> = Vec::with_capacity(model.faces.len());
    for face in &model.faces {
        // A plate with a vertex behind the near plane cannot be measured, let
        // alone drawn.
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
        // Lambert, and nothing more: one light, no falloff, no specular.
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
            // Nearer plates read a shade brighter.
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
    // wherever none does.
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
    // `u × w`, with `w` running back around the face rather than on to the next
    // vertex.
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
    // The drive catching throws the flame; the drive quitting puts it out.
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

/// The exhaust behind one bell: a short fan of streaks laid down the ship's own
/// axis, brightest where it leaves the nozzle.
fn draw_trail(canvas: &mut Canvas, cam: &Camera, flame: Flame<'_>) {
    // Two incommensurate rates, beaten together, so the flame never gutters on
    // a clean sine — and staggered per bell so a hull with several of them does
    // not pulse in lockstep.
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
    // through, so the plume rolls and leans with the ship and follows the dolly
    // without a constant of its own.
    let tail = [flame.bell.at[0], flame.bell.at[1], flame.bell.at[2] - reach];
    let mut end = place(tail, flame.pose, flame.eye);
    // `place` is a rotation and a translation, so the plume is still a straight
    // segment out here and can be cut against the near plane in closed form.
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

    // The screen perpendicular.
    let (mut dx, mut dy) = (foot.0 - flame.head.0, foot.1 - flame.head.1);
    let span = dx.hypot(dy);
    if !span.is_finite() || span < 1.0 {
        return;
    }
    let (px, py) = (-dy / span, dx / span);

    // Where this plume's own direction vanishes, if it has such a point, and
    // how far off that is in multiples of the plume's own projected length.
    let horizon = cam
        .vanishing_point([
            end[0] - flame.root[0],
            end[1] - flame.root[1],
            end[2] - flame.root[2],
        ])
        .map(|v| ((v.0 - flame.head.0) * dx + (v.1 - flame.head.1) * dy) / (span * span))
        .filter(|h| h.is_finite() && *h > 1.0);

    // How far along the way to that point the tip finished up.
    let mut gone = 0.0f32;

    // A lit warp drive does not trail, it tears: from the beam the lance runs
    // clean off the side of the frame, and it gets there the moment the drive
    // catches rather than growing into it over the warp range.
    if flame.engaged {
        // The diagonal reaches the frame edge from anywhere inside it whatever
        // direction the plume is pointing, and `draw_streak` clips, so there is
        // nothing to be gained by working out which edge it leaves by.
        let mut lance = cam.width.hypot(cam.height) / span;
        // The frame is not the only end a lance can have, and where the two
        // disagree the frame is wrong.
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
    // A flame comes to a tip; a lance runs off the frame in every lane.
    let taper = PLUME_TAPER + (PLUME_TAPER_AT_WARP - PLUME_TAPER) * flame.warp;
    // Enough lanes that they land about a subpixel apart across the widest end
    // of the fan, and no more.
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
        // curve from the widest point to a tip.
        let shorten = 1.0 - taper * out;
        let nozzle = (
            flame.head.0 + px * offset * throat,
            flame.head.1 + py * offset * throat,
        );
        // The far end of the fan stands further off than the nozzle does, so it
        // is smaller, and `1 - gone` is the ratio of those two depths exactly.
        let flare = widest * (1.0 - gone * shorten);
        let tip = (
            nozzle.0 + (dx + px * offset * flare) * shorten,
            nozzle.1 + (dy + py * offset * flare) * shorten,
        );
        // Divided back out, so what the constants above name is the brightness
        // at the nozzle rather than the brightness of a lane of some particular
        // length.
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
        // once — a closed, consistently oriented surface.
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
        // particular angle.
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
        // The camera's standoff assumes it, and so does every claim in this
        // module that nothing can come between the hull and the eye: a hull
        // that reached further would need a depth buffer to sort against the
        // things this one is allowed to be in front of by construction.
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
            // Only the folded spellings are worth asking about.
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
        // face at a time and goes on looking like a ship while it does.
        for model in models() {
            for step in 0..8 {
                let mut ship = Ship::new();
                ship.roll = step as f32 / 8.0 * std::f32::consts::TAU;
                ship.pitch_rate = MAX_PITCH_RATE;
                ship.yaw_rate = MAX_YAW_RATE;
                let pose = attitude(&ship);
                let (_, cam) = cam(120, 36, &ship);

                for zoom in [ZOOM_MIN, ZOOM_DEFAULT, ZOOM_MAX] {
                    // Swung round the ship as well as pushed in and out.
                    for eye in orbits().into_iter().map(|o| eye_at(o, zoom)) {
                        for v in &model.verts {
                            let at = place(*v, pose, &eye);
                            assert!(
                                cam.project(at).is_some(),
                                "{} lost a vertex through the near plane at zoom {zoom}: {at:?}",
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
        // Back-face culling is half of the hidden-surface removal here, and the
        // cheap half: a plate pointing away from the camera is dropped on the
        // sign of its projected area, before anything is painted.
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
        // bright.
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
        // off square.
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
        // hexagonal slab seen from above.
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
        // the camera was doing.
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
                    // genuinely standing in the way of something, so say so: a
                    // hull that missed the canvas would pass the line above
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
        // rule.
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
        // which is exactly the sort of relationship that rots quietly.
        let notch = crate::view::ORBIT_STEP.sin();
        assert!(
            OCCLUSION_BAND > 0.0 && OCCLUSION_BAND <= notch,
            "the drive swaps sides over {OCCLUSION_BAND}, and one press of a \
             camera key is {notch}"
        );
    }

    #[test]
    fn a_ship_nobody_is_flying_holds_the_drive_on_one_side_of_the_swap() {
        // The second scale, and the load-bearing one.
        use crate::autopilot::Autopilot;
        for fps in [10.0f32, 60.0, 500.0] {
            let dt = 1.0 / fps;
            let mut ship = Ship::new();
            let mut autopilot = Autopilot::default();
            let square = drive_behind_hull(attitude(&ship), &abeam());
            for frame in 0..(2.0 * Autopilot::CYCLE * fps as f64) as usize {
                autopilot.update(&mut ship, frame as f64 * dt as f64);
                ship.update(dt);
                assert_eq!(
                    drive_behind_hull(attitude(&ship), &abeam()),
                    square,
                    "at {fps} frames a second the autopilot moved the drive off \
                     the side it was drawn on, at frame {frame}"
                );
            }
        }
    }

    #[test]
    fn the_sky_never_shows_through_the_seams_of_a_hull() {
        // Why the whole hull goes to the canvas in one call, asked of the real
        // assemblies of solids in the hangar rather than of two synthetic
        // quads.
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
                    // a time.
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
        // What the finer measurement is for, at the fleet level.
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
        // ships.
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
                // it visits, and this is a sum over a subset of them.
                let (lit, total) = footprint(model, &ship, cols, rows);
                assert!(
                    lit <= cols * rows * 2,
                    "{} lit {lit} of {cols}x{rows}'s {} subpixels",
                    model.name,
                    cols * rows * 2
                );
                assert!(total.is_finite(), "{} at {cols}x{rows}", model.name);
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
        // ship is going and the ship itself used to say nothing.
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
        // off to the left.
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

    /// The drive's light in one column, a chosen number of the ship's own half-
    /// lengths aft of the vanishing point.
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
        // being hull units the moment the drive is lit.
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
        // The whole point of dividing `Canvas::streak_spread` back out.
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
        // to either end of the ramp is covered rather than restated.
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
        // fault.
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
    /// test below will ask its question. A bell's glow reaches a dozen
    /// subpixels at the close end of the dolly, and a hull nearly on top of the
    /// point it vanishes at has no plume to speak of anyway — `draw_trail`
    /// gives up on a projected length under one subpixel.
    const CLEAR_OF_THE_HORIZON: f32 = 25.0;

    #[test]
    fn a_plume_stops_at_the_point_it_vanishes_at() {
        // The bug this is here for.
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
                        // toward the point it is heading for.
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
                        // Counted only when the point is on the canvas, and
                        // that is the whole difference between a test and a
                        // shape of one.
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
        // way this number last moved.
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
        // hull's.
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
        // sort of run a screensaver gets.
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
        // frame.
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
        // attitude the panel reads out.
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
