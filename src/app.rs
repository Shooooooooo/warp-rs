//! Running a flight: the loops that drive one, and the state they drive.
//!
//! Three of them, sharing a `Flight` and differing only in where the frames
//! end up — an interactive terminal, stdout, or a PNG.

use crate::autopilot::Autopilot;
use crate::cli::{self, resolved_size, Args};
use crate::exterior::ExteriorField;
use crate::hud::Readout;
use crate::menu::{self, Menu};
use crate::models::{self, ShipModel};
use crate::render::{Exterior, Renderer};
use crate::ship::{wrap_signed, Ship};
#[cfg(feature = "snapshot")]
use crate::snapshot;
use crate::starfield::StarField;
use crate::term::RawGuard;
use crate::view::{self, Orbit, ViewMode};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::{terminal, QueueableCommand};
use std::io::{self, BufWriter, Write};
use std::time::{Duration, Instant};

/// Physics runs on a fixed step so the flight model behaves the same whether
/// the terminal can keep up or not.
const SIM_STEP: f32 = 1.0 / 120.0;
/// A stalled process must not fast-forward the universe on the next frame.
const MAX_FRAME_DT: f32 = 0.25;
/// The widest step [`Flight::advance`] will take, whatever it is handed.
///
/// Deliberately looser than `MAX_FRAME_DT`, which is the interactive loop's own
/// limit and is tight because a frame on a real terminal is never a quarter of
/// a second. This one sits underneath *every* caller, so it has to leave the
/// legitimate ones alone: headless and snapshot step at `1.0 / --fps` and
/// `--fps` is floored at 1, so a second is the longest step anything in the
/// tree asks for. Past that it is not a frame, and the fixed-step loop below
/// would grind through a hundred and twenty simulation steps for every second
/// of it.
const MAX_STEP_DT: f32 = 1.0;
/// The smallest pool the `-` key will leave behind.
///
/// Low enough to sit under any pool worth pressing `-` on, and deliberately not
/// lower: `+` multiplies by 1.25 and truncates, so at one, two and three stars
/// the answer is the same number back and the key swallows the press without
/// moving anything. Four is where growth starts; this stands clear of it.
/// Making `+` grow by at least one star instead would let this go to one, and
/// it rewrites the growth arithmetic to fix a floor that is easier to move.
///
/// It was 64 once, chosen against an automatic minimum of 300 that is as gone
/// as the density that replaced it. A floor up there sat *over* the count on
/// any window the old density opened thin — 40x12 started with nineteen stars —
/// so `-` added forty-five of them and landed both keys on the same number,
/// which is a control doing the opposite of what it is labelled. Nothing opens
/// under this one now that [`cli::DEFAULT_STARS`] is a fixed 256, but it is not
/// thereby unreachable: `-` walks that pool down to nine and then onto this,
/// where it stops, and `+` climbs back off it to ten.
const POOL_FLOOR: usize = 8;
/// So the clamp in [`Flight::resize_pool`] cannot be made to panic by a later
/// edit to either end of it: `Ord::clamp` requires its minimum below its
/// maximum, and these are the two ends.
const _: () = assert!(
    POOL_FLOOR <= cli::MAX_STARS,
    "the star keys have a floor past their own ceiling"
);
/// Mixed into `--seed` for the outside view's sky, so the two fields are
/// independent of each other and of when either was built.
const EXTERIOR_SEED: u64 = 0x51DE_0000_0000_0000;

/// Fly, in whichever of the three modes the arguments asked for.
pub fn run(args: &Args) -> io::Result<()> {
    #[cfg(feature = "snapshot")]
    if let Some(path) = &args.snapshot {
        return run_snapshot(args, path);
    }
    if args.headless {
        run_headless(args)
    } else {
        run_interactive(args)
    }
}

/// The shape of a flight: everything except where the frames end up.
pub struct Flight {
    ship: Ship,
    field: StarField,
    /// The band of sky the outside view flies alongside, built the first time
    /// that view is asked for. A cockpit-only run pays nothing for it — not the
    /// pool, and not a single draw from its generator.
    exterior: Option<ExteriorField>,
    renderer: Renderer,
    autopilot: Autopilot,
    /// Which camera is flying, which ship is being flown, and whether the
    /// picker is up over the top of it.
    view: ViewMode,
    model: usize,
    menu: Option<Menu>,
    /// How far the outside camera has been pushed in or out, and where it is
    /// being asked to go. Kept apart so a wheel — which arrives as a burst of
    /// notches rather than as one — is eased into rather than jumped to.
    ///
    /// Both stay `f32` where `time` below is `f64`: these are bounded at both
    /// ends and are not accumulators, so the drift that argument is about
    /// cannot reach them.
    zoom: f32,
    zoom_target: f32,
    /// Which way round the ship the outside camera has been swung, and where it
    /// is being asked to swing to. The same pair for the same reason, and eased
    /// the same way — a key held down arrives as auto-repeat, which is a burst
    /// of steps rather than one.
    ///
    /// `f32` too, and this one needs its own excuse: two of the three angles go
    /// all the way round without stopping, which is exactly the accumulator the
    /// argument above is about. [`Orbit::held`] is what settles it — both the
    /// eased angle and the target are folded back into a single turn, so
    /// neither can grow however long a key is leaned on.
    orbit: Orbit,
    orbit_target: Orbit,
    /// Kept so a field built later gets the sky the seed asked for.
    seed: u64,
    /// Wall time since launch, in seconds. `f64` because it only ever grows and
    /// a screensaver is expected to be left running: as an `f32` accumulator it
    /// stopped advancing altogether after about six days, freezing the twinkle
    /// and the shake it drives. The accumulator below stays `f32` — it is
    /// bounded by one sim step and never drifts.
    time: f64,
    accumulator: f32,
}

impl Flight {
    pub fn new(args: &Args, cols: usize, rows: usize) -> Self {
        let mut ship = Ship::new();
        ship.throttle = args.throttle;
        if args.engage {
            ship.toggle_warp();
        }

        let renderer = Renderer::new(cols, rows, args.color.resolve(), args.exposure)
            .with_hull_samples(args.aa);
        let cam = renderer.camera(&ship, 0.0);
        let seed = seed(args);
        let field = StarField::new(args.stars, seed, &cam);

        let mut flight = Self {
            ship,
            field,
            exterior: None,
            renderer,
            autopilot: Autopilot::default(),
            view: ViewMode::Cockpit,
            model: args.ship,
            menu: None,
            zoom: view::ZOOM_DEFAULT,
            zoom_target: view::ZOOM_DEFAULT,
            orbit: args.orbit,
            orbit_target: args.orbit,
            seed,
            time: 0.0,
            accumulator: 0.0,
        };
        if args.view.resolve() == ViewMode::Side {
            flight.set_view(ViewMode::Side, args);
        }
        flight
    }

    /// Which camera is flying.
    pub fn view(&self) -> ViewMode {
        self.view
    }

    /// The next camera round, building the sky it needs if this is the first
    /// time it has been asked for.
    ///
    /// The zoom is left alone. It belongs to the outside camera, and going in
    /// to look at something and coming back out to find the shot re-framed
    /// would be its own small annoyance; `R` is the way to put it back.
    pub fn cycle_view(&mut self, args: &Args) {
        self.set_view(self.view.next(), args);
    }

    /// How far the outside camera has been pushed in or out.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Push it a notch: positive is closer and bigger, negative is further off.
    ///
    /// Geometric, so a notch is the same size of change wherever it is taken
    /// from — an additive step big enough to be worth pressing out at the far
    /// end would shove the near view straight through its stop. Multiplied in
    /// and divided out by the one constant rather than raised to `dir`, so a
    /// press and its opposite land back where they started instead of a `powf`
    /// away from it.
    ///
    /// It is the *target* that is held to the range, not the eased value. Clamp
    /// the eased one and the target winds up somewhere past the end, and the
    /// first notch back does nothing at all.
    pub fn nudge_zoom(&mut self, dir: f32) {
        let step = if dir >= 0.0 {
            view::ZOOM_STEP
        } else {
            1.0 / view::ZOOM_STEP
        };
        self.zoom_target = (self.zoom_target * step).clamp(view::ZOOM_MIN, view::ZOOM_MAX);
    }

    /// Back to the framing the flight opened on. Snapped rather than eased,
    /// because `R` is the key for when the view has got away from you and
    /// watching it saunter back is not what is wanted.
    pub fn reset_zoom(&mut self) {
        self.zoom = view::ZOOM_DEFAULT;
        self.zoom_target = view::ZOOM_DEFAULT;
    }

    pub fn orbit(&self) -> Orbit {
        self.orbit
    }

    /// Where it is being asked to go, which is what a keypress moves. The eased
    /// value above is what a frame is drawn from.
    pub fn orbit_target(&self) -> Orbit {
        self.orbit_target
    }

    /// Swing the camera round the ship, over it, or about its own view axis, a
    /// step at a time.
    ///
    /// One function for the three axes rather than three, because they arrive
    /// together and are held together: [`Orbit::held`] folds all three onto a
    /// single turn, and none of them stops. As with the zoom it is the *target*
    /// that is held, not the eased value — and unlike the zoom the step is
    /// additive, since an angle has no far end to be shoved about and every
    /// part of a turn is worth the same.
    pub fn nudge_orbit(&mut self, azimuth: f32, elevation: f32, roll: f32) {
        self.orbit_target = Orbit {
            azimuth: self.orbit_target.azimuth + azimuth * view::ORBIT_STEP,
            elevation: self.orbit_target.elevation + elevation * view::ORBIT_STEP,
            roll: self.orbit_target.roll + roll * view::ORBIT_STEP,
        }
        .held();
    }

    /// Back to the shot the flight opened on, snapped rather than eased, for
    /// the same reason the zoom is: `R` is the key for when the view has got
    /// away from you.
    ///
    /// Where it opened, not [`Orbit::LEVEL`] — `--orbit` is a starting angle
    /// the same way `--throttle` is a starting throttle, and `R` has always put
    /// that one back rather than the flight model's own default.
    pub fn reset_orbit(&mut self, orbit: Orbit) {
        self.orbit = orbit.held();
        self.orbit_target = self.orbit;
    }

    fn set_view(&mut self, view: ViewMode, args: &Args) {
        self.view = view;
        if view == ViewMode::Side && self.exterior.is_none() {
            let cam = self.renderer.exterior_camera(&self.ship, self.time);
            // Its own generator, from a seed of its own, so the cockpit's
            // stream is untouched whenever this happens — and so the two skies
            // are not the same pattern seen twice.
            self.exterior = Some(ExteriorField::new(
                args.stars,
                self.seed ^ EXTERIOR_SEED,
                &cam,
                self.orbit,
            ));
        }
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Put the picker up, and go outside to look at what it is offering: a list
    /// of ship names is a poor way to choose a ship.
    pub fn open_menu(&mut self, args: &Args) {
        self.menu = Some(Menu::new(self.model));
        self.set_view(ViewMode::Side, args);
    }

    /// Take it down, keeping the ship that was being flown when it went up.
    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    pub fn menu_move(&mut self, delta: isize) {
        if let Some(menu) = &mut self.menu {
            menu.move_cursor(delta);
        }
    }

    /// Fly whatever the cursor is on, and take the picker down.
    pub fn menu_confirm(&mut self) {
        if let Some(menu) = self.menu.take() {
            self.model = menu.cursor();
        }
    }

    /// The ship to draw: the cursor's while the picker is up, so moving through
    /// the list flies each one in turn instead of describing it.
    fn drawn_model(&self) -> &'static ShipModel {
        let index = self.menu.as_ref().map_or(self.model, Menu::cursor);
        &models::models()[index.min(models::models().len() - 1)]
    }

    /// Advance by `dt` of wall time, in fixed physics steps.
    ///
    /// Only the sky being looked at is stepped. That is safe because both
    /// fields recompute where a star was from where it is at the top of their
    /// update, so a field coming back after a spell out of view draws a proper
    /// short streak on its first frame rather than one long scratch.
    pub fn advance(&mut self, dt: f32) {
        // Held to something a frame could plausibly be *here*, rather than at
        // the one call site that used to do it. This is public, and `lib.rs`
        // offers a flight to any program that cares to fly one, so the guard
        // belongs with the loop it protects. Both ways an unchecked step goes
        // wrong are quiet ones: an enormous `dt` is unbounded work, and a NaN
        // one is worse than that — it poisons the accumulator, `NaN >=
        // SIM_STEP` is never true again, and the flight goes on drawing frames
        // for the rest of its life without simulating another one.
        let dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_STEP_DT)
        } else {
            0.0
        };
        self.time += dt as f64;
        self.accumulator += dt;
        while self.accumulator >= SIM_STEP {
            self.ship.update(SIM_STEP);
            // The camera catching up with the zoom it was asked for, in the
            // same frame-rate-independent form everything else here eases in.
            // Stepped whichever view is flying: it is one multiply, and a zoom
            // that only settled while it was being looked at would arrive
            // mid-move on the frame the camera came back.
            //
            // When there is nothing to catch up with — every headless and
            // snapshot flight, which take no input at all — the difference is
            // exactly zero and so is the whole term, so this cannot move a
            // reference frame by an ulp.
            self.zoom +=
                (self.zoom_target - self.zoom) * (1.0 - (-view::ZOOM_EASE * SIM_STEP).exp());
            // And the same for the orbit, with one difference: all three of its
            // angles go round, so each chases its target the short way rather
            // than unwinding three hundred and fifty degrees to reach a target
            // ten degrees away. Taking the raw difference on an axis that wraps
            // is the bug this is written to avoid, and the elevation had it
            // until the elevation stopped being clamped.
            //
            // `wrap_signed` of an exact zero is an exact zero, so a flight
            // nobody is swinging the camera on still adds nothing at all here.
            let ease = 1.0 - (-view::ORBIT_EASE * SIM_STEP).exp();
            let chase = |from: f32, to: f32| from + wrap_signed(to - from) * ease;
            self.orbit = Orbit {
                azimuth: chase(self.orbit.azimuth, self.orbit_target.azimuth),
                elevation: chase(self.orbit.elevation, self.orbit_target.elevation),
                roll: chase(self.orbit.roll, self.orbit_target.roll),
            }
            .held();
            match self.view {
                ViewMode::Cockpit => {
                    let cam = self.renderer.camera(&self.ship, self.time);
                    self.field.update(
                        SIM_STEP,
                        self.ship.speed,
                        self.ship.yaw_rate,
                        self.ship.pitch_rate,
                        self.ship.roll_rate,
                        &cam,
                    );
                }
                ViewMode::Side => {
                    let cam = self.renderer.exterior_camera(&self.ship, self.time);
                    if let Some(field) = &mut self.exterior {
                        field.update(SIM_STEP, self.ship.speed, &cam, self.orbit);
                    }
                }
            }
            self.accumulator -= SIM_STEP;
        }
    }

    pub fn draw(&mut self, fps: f32, paused: bool, hints: bool) {
        let model = self.drawn_model();
        let readout = Readout {
            ship: &self.ship,
            fps,
            stars: self.stars(),
            paused,
            hints,
            view: self.view,
            model: model.name,
        };
        match (self.view, &mut self.exterior) {
            (ViewMode::Side, Some(field)) => {
                let cam = self.renderer.exterior_camera(&self.ship, self.time);
                let scene = Exterior {
                    field,
                    ship: &self.ship,
                    model,
                    time: self.time,
                    zoom: self.zoom,
                    orbit: self.orbit,
                };
                self.renderer.render_exterior(scene, &cam, &readout);
            }
            _ => {
                let cam = self.renderer.camera(&self.ship, self.time);
                self.renderer
                    .render(&self.field, &self.ship, &cam, self.time, &readout);
            }
        }
        // Over the top of everything, panel included: it is a dialogue, and it
        // is drawn here rather than inside the renderer so the pinned cockpit
        // path stays exactly as it was.
        if let Some(menu) = &self.menu {
            menu::draw(self.renderer.screen(), menu);
        }
    }

    /// How many stars are currently in flight, in the view being flown.
    pub fn stars(&self) -> usize {
        match (self.view, &self.exterior) {
            (ViewMode::Side, Some(field)) => field.len(),
            _ => self.field.len(),
        }
    }

    /// Grow or shrink the sky being looked at.
    ///
    /// Held at the top to the ceiling `--stars` is held to, so a pool these
    /// keys walk to can never be one the command line would have refused. It
    /// used to be a separate 20 000 that sat *under* what `--stars` allowed,
    /// which made `+` shrink a pool the flag had asked for.
    ///
    /// An empty sky is the one pool the floor may not lift, and answering that
    /// takes an early return rather than a smaller number. `--stars 0` is a
    /// thing to ask for rather than a mistake to correct, and taken through the
    /// clamp below it comes out at [`POOL_FLOOR`] for *both* keys — so `-` would
    /// put eight stars into a sky somebody emptied on purpose, which is the
    /// exact fault this floor came down from 64 to stop, arriving by the other
    /// door. There is also no count that could be passed down instead: both
    /// fields hold whatever they are handed at one star or more, and that guard
    /// is theirs to keep. `+` is deliberately *not* caught by it and climbs to
    /// the floor, because zero is a fixed point of a multiply and a key that
    /// swallows the press for the rest of the flight is worse than one plainly
    /// not connected.
    fn resize_pool(&mut self, scale: f32) {
        if scale < 1.0 && self.stars() == 0 {
            return;
        }
        let wanted = |len: usize| ((len as f32 * scale) as usize).clamp(POOL_FLOOR, cli::MAX_STARS);
        match (self.view, &mut self.exterior) {
            (ViewMode::Side, Some(field)) => {
                let n = wanted(field.len());
                field.resize_pool(n);
            }
            _ => {
                let n = wanted(self.field.len());
                self.field.resize_pool(n);
            }
        }
    }

    /// Write the last drawn frame out as a self-contained block of text.
    pub fn present_plain(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.renderer.present_plain(out)
    }

    /// Adapt to a new terminal size. Reports whether anything actually moved,
    /// so the caller can skip the repaint a resize otherwise forces.
    pub fn resize(&mut self, args: &Args, cols: usize, rows: usize) -> bool {
        // `--size` is a fixed size, not a starting point. Without this the flag
        // held only until the first resize event, after which the terminal
        // silently won.
        if args.size.is_some() {
            return false;
        }
        let (cols, rows) = (cols.max(1), rows.max(1));
        if self.renderer.screen().dims() == (cols, rows) {
            return false;
        }

        self.renderer.resize(cols, rows);
        let cam = self.renderer.camera(&self.ship, self.time);
        self.field.retarget(&cam);
        let side = self.renderer.exterior_camera(&self.ship, self.time);
        if let Some(field) = &mut self.exterior {
            field.retarget(&side);
        }
        // And the pool is not touched. It used to be re-derived from the new
        // canvas whenever `--stars` was 0, which was the density's whole
        // mechanism seen from here: a window dragged wider grew the sky under a
        // flight already under way. `--stars` is a plain count now, so a resize
        // is a question about geometry only — which is what the two `retarget`
        // calls above answer, and they answer all of it.
        true
    }
}

fn seed(args: &Args) -> u64 {
    args.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5EED)
    })
}

/// What a keypress asked for.
enum Action {
    Continue,
    Quit,
}

fn handle_key(key: KeyEvent, flight: &mut Flight, args: &Args, paused: &mut bool) -> Action {
    // Releases only arrive from terminals speaking the kitty protocol; ignoring
    // them keeps a single press from counting twice.
    if key.kind == KeyEventKind::Release {
        return Action::Continue;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // A dialogue is up: it takes the keyboard, and in particular it takes
    // `Esc`. Nothing is worse than reaching for the key that dismisses a menu
    // and ending the flight with it.
    if flight.menu_open() && !ctrl {
        return menu_key(key, flight);
    }
    // Pointing the nose is a thing you do from behind it. The camera outside
    // rides with the ship rather than with the sky, so out there a turn moves
    // nothing an eye can see — the stars stream on exactly as they were and the
    // hull leans a few degrees — and a control that swallows the input and
    // gives nothing back is worse than one that is plainly not there. So the
    // stick flies the ship in the one view that is looking down its nose.
    let steers = flight.view() == ViewMode::Cockpit;
    // And out there the same six keys fly the *camera*, which is the thing that
    // can usefully move in a view whose whole subject is the ship. It is not
    // that pitch and yaw were switched off and something was found for them: a
    // camera that cannot be walked round the thing it is pointed at is the
    // control that is missing out here, and these are the keys everything else
    // puts it on.
    //
    // Roll goes with them rather than staying on the hull, which costs the one
    // thing this view could do that the cockpit cannot — a barrel roll flown
    // and watched from the beam. The trade is a stick that means one thing in
    // each view instead of two things in one of them, and the cockpit still
    // rolls the ship.
    let flies_the_camera = flight.view() == ViewMode::Side;
    // The zoom runs the same way and always has. There is no ship to be bigger
    // or smaller from inside one, so it is connected exactly where it has
    // something to show.
    let zooms = flight.view() == ViewMode::Side;

    match key.code {
        // `q` is on the stick, so it cannot also be the way out: nothing a
        // pilot reaches for mid-turn should end the flight.
        KeyCode::Char('c' | 'd') if ctrl => return Action::Quit,
        KeyCode::Esc => return Action::Quit,

        // The stick. WASD for the two axes that point the nose, QE for the one
        // that turns the sky about it; `i`/`k` pitch too, as they always have.
        KeyCode::Char('w' | 'W' | 'i' | 'I') if steers => flight.ship.nudge_pitch(-1.0),
        KeyCode::Char('s' | 'S' | 'k' | 'K') if steers => flight.ship.nudge_pitch(1.0),
        KeyCode::Left | KeyCode::Char('a' | 'A') if steers => flight.ship.nudge_yaw(-1.0),
        KeyCode::Right | KeyCode::Char('d' | 'D') if steers => flight.ship.nudge_yaw(1.0),
        KeyCode::Char('q' | 'Q') if steers => flight.ship.nudge_roll(-1.0),
        KeyCode::Char('e' | 'E') if steers => flight.ship.nudge_roll(1.0),

        // The same six, outside, on the camera. These sit below the ones above
        // rather than inside them: a guard that fails falls through to the next
        // arm that matches, so each key is written once per view and neither
        // spelling has to know about the other.
        //
        // The keys move the *camera*, not the ship in frame — `W` lifts it over
        // the hull so the hull is being looked down on, `D` walks it forward
        // toward the nose. An orbit camera that went the other way would read
        // as dragging the ship about, and the ship is the one thing out here
        // that is not being flown from this seat.
        KeyCode::Char('w' | 'W' | 'i' | 'I') if flies_the_camera => {
            flight.nudge_orbit(0.0, 1.0, 0.0)
        }
        KeyCode::Char('s' | 'S' | 'k' | 'K') if flies_the_camera => {
            flight.nudge_orbit(0.0, -1.0, 0.0)
        }
        KeyCode::Left | KeyCode::Char('a' | 'A') if flies_the_camera => {
            flight.nudge_orbit(-1.0, 0.0, 0.0)
        }
        KeyCode::Right | KeyCode::Char('d' | 'D') if flies_the_camera => {
            flight.nudge_orbit(1.0, 0.0, 0.0)
        }
        // Rolled the way the sky used to turn when these two rolled the ship,
        // so the key still tips the picture the direction it always tipped it.
        KeyCode::Char('q' | 'Q') if flies_the_camera => flight.nudge_orbit(0.0, 0.0, 1.0),
        KeyCode::Char('e' | 'E') if flies_the_camera => flight.nudge_orbit(0.0, 0.0, -1.0),

        // The throttle is the up and down arrows, which is where it has
        // always been: only its letters went to the stick. The other two
        // arrows are yaw, four lines above.
        KeyCode::Up => flight.ship.nudge_throttle(1.0),
        KeyCode::Down => flight.ship.nudge_throttle(-1.0),

        KeyCode::Char(' ') => {
            flight.ship.toggle_warp();
        }
        KeyCode::Char('p' | 'P') => *paused = !*paused,
        KeyCode::Char('r' | 'R') => {
            flight.ship.reset();
            flight.ship.throttle = args.throttle;
            flight.reset_zoom();
            flight.reset_orbit(args.orbit);
            *paused = false;
        }
        KeyCode::Char('+' | '=') => flight.resize_pool(1.25),
        KeyCode::Char('-' | '_') => flight.resize_pool(0.8),

        // The outside camera, in and out. Shifted as well as plain for the
        // same reason `+` is paired with `=` and `-` with `_`: which of the
        // two arrives depends on the keyboard, not on what was meant.
        KeyCode::Char('[' | '{') if zooms => flight.nudge_zoom(-1.0),
        KeyCode::Char(']' | '}') if zooms => flight.nudge_zoom(1.0),

        // The camera, and the hangar.
        KeyCode::Char('c' | 'C') => flight.cycle_view(args),
        KeyCode::Char('m' | 'M') => flight.open_menu(args),
        _ => {}
    }
    Action::Continue
}

/// Keys while the ship picker is up. It is modal on purpose: the arrows move
/// the highlight rather than the throttle, and every way out of it leaves the
/// flight running.
fn menu_key(key: KeyEvent, flight: &mut Flight) -> Action {
    match key.code {
        KeyCode::Up | KeyCode::Char('w' | 'W' | 'k' | 'K') => flight.menu_move(-1),
        KeyCode::Down | KeyCode::Char('s' | 'S' | 'j' | 'J') => flight.menu_move(1),
        KeyCode::Enter | KeyCode::Char(' ') => flight.menu_confirm(),
        KeyCode::Esc | KeyCode::Char('m' | 'M') => flight.close_menu(),
        _ => {}
    }
    Action::Continue
}

/// The wheel, which is the zoom and nothing else.
///
/// No [`Action`], because there is nothing here that could end a flight — a
/// pointer wandering across the window must never be the thing that quits.
/// Buttons and motion are ignored outright: nothing in this program is aimed
/// at, so the only mouse event it has a use for is the one that says nearer or
/// further.
///
/// While the picker is up the wheel moves the highlight, matching the arrow
/// keys the dialogue already owns. The picker forces the outside view, so the
/// alternative would be scrolling the list and zooming the ship behind it at
/// the same time.
fn handle_mouse(mouse: MouseEvent, flight: &mut Flight) {
    let scroll = match mouse.kind {
        MouseEventKind::ScrollUp => 1.0,
        MouseEventKind::ScrollDown => -1.0,
        _ => return,
    };
    if flight.menu_open() {
        flight.menu_move(-scroll as isize);
    } else if flight.view() == ViewMode::Side {
        flight.nudge_zoom(scroll);
    }
}

fn run_interactive(args: &Args) -> io::Result<()> {
    // Not `terminal::size()` directly: tmux runs a `lock-command` against a
    // tty whose window size is not set yet, so it can report zero.
    let (cols, rows) = resolved_size(args);

    // Built before the terminal is taken over. This is the only thing here that
    // allocates in bulk, and a failed allocation aborts the process outright —
    // no unwind, no `Drop`, no panic hook — so anything installed first would
    // never be undone, and the user would be left in raw mode on the alternate
    // screen with an invisible cursor and no shell prompt.
    let mut flight = Flight::new(args, cols as usize, rows as usize);

    // The wheel is a control, and a screensaver has none — it dies on contact
    // with anything, so there is nothing for the mouse to do there and no
    // reason to take it off the terminal for the duration.
    let _guard = RawGuard::new(!args.screensaver)?;
    let mut out = BufWriter::with_capacity(1 << 20, io::stdout());
    out.queue(terminal::Clear(terminal::ClearType::All))?;

    let mut paused = false;
    let mut fps = args.fps as f32;
    let frame_budget = Duration::from_secs_f32(1.0 / args.fps as f32);
    let start = Instant::now();
    let mut last = start;

    'flying: loop {
        let frame_start = Instant::now();

        // A signal asked for the process. Leave the way a key would, so the
        // guard above puts the terminal back rather than the flight being cut
        // down where it stands with the alternate screen still up. Checked
        // here rather than acted on in the handler, which can do nothing but
        // set the flag safely — so the delay is one frame's budget, and at the
        // frame rates a screensaver runs at that is the wait.
        if crate::term::interrupted() {
            break 'flying;
        }

        let elapsed = start.elapsed().as_secs_f64();
        // `--demo` flies itself and then stops; a screensaver flies itself
        // until something interrupts it.
        if let Some(limit) = args.demo {
            if elapsed >= limit as f64 {
                break 'flying;
            }
        }
        if args.demo.is_some() || args.screensaver {
            flight.autopilot.update(&mut flight.ship, elapsed);
        }

        let dt = (frame_start - last).as_secs_f32().clamp(0.0, MAX_FRAME_DT);
        last = frame_start;
        // Smoothed so the readout is legible rather than flickering.
        fps += (1.0 / dt.max(1e-4) - fps) * 0.08;

        if !paused {
            flight.advance(dt);
        }
        flight.draw(fps, paused, !args.screensaver);
        flight.renderer.present(&mut out)?;

        // Spend what is left of the frame waiting on the event queue rather
        // than sleeping through it. The wait ends the moment something is
        // typed, so a key is acted on when it arrives instead of whenever the
        // sleep happens to finish — which at 60 fps is a few milliseconds, and
        // at `--fps 5` is the difference between a screensaver that dismisses
        // when touched and one that finishes its nap first.
        //
        // Acting on the key was only ever half of answering it, though. Once
        // something has moved the flight there is a frame owing to it, and
        // going back to `poll` for the rest of the budget is that frame waiting
        // on a clock: the stick was answered at once and the picture was not.
        // So the wait is what is left of the budget *until* something has been
        // acted on, and nothing after it.
        let mut acted = false;
        loop {
            // Saturating, not checked: a frame that has already run over its
            // budget has nothing left to wait, but a terminal too slow to keep
            // up still has to be quittable, so the queue is drained either way.
            let remaining = frame_budget.saturating_sub(frame_start.elapsed());
            match event::poll(remaining) {
                Ok(true) => {}
                Ok(false) => break,
                // A signal landing in the middle of the wait cuts it short.
                // That is not a failure to report — it is the thing the flag
                // at the top of the loop exists for — so go back and read it
                // rather than handing the user an "Interrupted" and an exit
                // code for having pressed nothing.
                Err(err) if err.kind() == io::ErrorKind::Interrupted => break,
                Err(err) => return Err(err),
            }
            match event::read()? {
                // A screensaver dies on contact: any key at all gets you back
                // to your terminal, not just the ones a pilot would know.
                Event::Key(key) if args.screensaver => {
                    if key.kind != KeyEventKind::Release {
                        break 'flying;
                    }
                }
                Event::Key(key) => {
                    if let Action::Quit = handle_key(key, &mut flight, args, &mut paused) {
                        break 'flying;
                    }
                    acted = true;
                }
                // No screensaver arm above this one: capture is not asked for
                // in that mode, so nothing here can arrive.
                Event::Mouse(mouse) => {
                    handle_mouse(mouse, &mut flight);
                    acted = true;
                }
                // Only repaint if the size really changed: terminals emit
                // resize events that settle on the size already in use, and
                // clearing on those makes the field blink for no reason.
                Event::Resize(cols, rows) => {
                    let changed = flight.resize(args, cols as usize, rows as usize);
                    if changed {
                        out.queue(terminal::Clear(terminal::ClearType::All))?;
                        flight.renderer.screen().force_redraw();
                        acted = true;
                    }
                }
                _ => {}
            }
            // The queue is emptied before the wait is cut short, so a burst of
            // wheel notches is one frame rather than one frame each — and a
            // resize the terminal settles out of, which changes nothing, does
            // not buy itself a repaint.
            if acted && !event::poll(Duration::ZERO)? {
                break;
            }
        }
    }

    Ok(())
}

/// Fly `args.frames` frames on a fixed timestep, writing each one out as a
/// self-contained block of text. All of `--headless` except where it goes.
///
/// Public, and split out from the loop below, because the reference frames in
/// `tests/golden` are of exactly these bytes. Until this was reachable the only
/// thing in the tree that could produce them was the binary, so the check that
/// they had not moved lived in CI alone — and a green `cargo test` said nothing
/// at all about whether an edit had repainted the whole sky.
pub fn render_headless(args: &Args, out: &mut impl Write) -> io::Result<()> {
    let (cols, rows) = resolved_size(args);
    let mut flight = Flight::new(args, cols as usize, rows as usize);
    let dt = 1.0 / args.fps as f32;

    for frame in 0..args.frames {
        if args.demo.is_some() {
            flight
                .autopilot
                .update(&mut flight.ship, frame as f64 * dt as f64);
        }
        flight.advance(dt);
        flight.draw(args.fps as f32, false, true);
        flight.present_plain(out)?;
    }
    Ok(())
}

/// Render frames to stdout with a fixed timestep. No raw mode, no alternate
/// screen — the same seed always produces the same bytes.
fn run_headless(args: &Args) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(1 << 20, io::stdout());
    render_headless(args, &mut out)?;
    out.flush()
}

#[cfg(feature = "snapshot")]
fn run_snapshot(args: &Args, path: &std::path::Path) -> io::Result<()> {
    let (cols, rows) = args.size.unwrap_or((240, 68));
    let mut flight = Flight::new(args, cols as usize, rows as usize);
    let dt = 1.0 / args.fps as f32;

    for frame in 0..args.warmup {
        if args.demo.is_some() {
            flight
                .autopilot
                .update(&mut flight.ship, frame as f64 * dt as f64);
        }
        flight.advance(dt);
    }
    flight.draw(args.fps as f32, false, true);

    let (w, h) = flight.renderer.canvas_dims();
    snapshot::write_png(path, flight.renderer.pixels(), w, h, args.scale)?;
    eprintln!(
        "wrote {} ({}x{} px) at velocity {:.1} c",
        path.display(),
        w * args.scale,
        h * args.scale,
        flight.ship.velocity_c()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args_for;

    /// A key going down, with nothing held. Written out nine times before this
    /// existed, which is nine places to miss if the modifier ever matters.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A notch of the wheel. The column and row are not read by anything here
    /// — nothing in this program is aimed at — so they are a fixed nowhere.
    fn wheel(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A flight already outside, which is where the zoom lives.
    fn outside(args: &Args) -> Flight {
        let mut flight = Flight::new(args, 80, 24);
        let mut paused = false;
        handle_key(press(KeyCode::Char('c')), &mut flight, args, &mut paused);
        assert_eq!(flight.view(), ViewMode::Side);
        flight
    }

    #[test]
    fn a_flight_that_has_been_up_for_days_still_advances() {
        // Regression: `time` was an `f32` accumulator. At 1/60 s steps it
        // stopped moving entirely at t = 524288 s — a little over six days —
        // taking the twinkle and the shake with it, which is precisely the
        // situation a tmux screensaver is left in.
        let args = args_for(&["--seed", "3", "--stars", "400"]);
        let mut flight = Flight::new(&args, 60, 20);
        let dt = 1.0 / 60.0;

        for start in [0.0f64, 3_600.0, 86_400.0, 524_288.0, 10_000_000.0] {
            let days = start / 86_400.0;
            flight.time = start;
            flight.advance(dt);
            assert!(
                flight.time > start,
                "the clock stopped at {start} s ({days:.1} days)"
            );

            // The counter moving is not enough on its own: the phase it drives
            // has to still differ from one frame to the next.
            flight.ship.shake = 1.0;
            let a = flight.renderer.camera(&flight.ship, start);
            let b = flight.renderer.camera(&flight.ship, start + dt as f64);
            assert!(
                (a.cx - b.cx).abs() + (a.cy - b.cy).abs() > 1e-6,
                "the shake froze at {start} s ({days:.1} days)"
            );
        }
    }

    #[test]
    fn a_step_that_is_not_a_step_does_not_end_the_flight() {
        // Regression: `advance` is public and took whatever it was given. A NaN
        // `dt` poisoned the accumulator — `NaN >= SIM_STEP` is false forever —
        // so the flight went on drawing and never simulated another step. The
        // interactive loop clamps before it calls in, which is the only reason
        // the binary never met this; nothing else in the tree was covered.
        let args = args_for(&["--seed", "3", "--stars", "300", "--size", "60x20"]);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, -0.0] {
            let mut flight = Flight::new(&args, 60, 20);
            flight.ship.throttle = 1.0;
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
            }

            flight.advance(bad);
            assert!(
                flight.time.is_finite(),
                "{bad} made the clock {}",
                flight.time
            );
            assert!(
                flight.accumulator.is_finite(),
                "{bad} poisoned the accumulator"
            );

            // And the frames after it still move the ship and the clock.
            let (clock, speed) = (flight.time, flight.ship.speed);
            for _ in 0..60 {
                flight.advance(1.0 / 60.0);
            }
            assert!(flight.time > clock, "the clock stopped after {bad}");
            assert!(flight.ship.speed > speed, "the flight froze after {bad}");
        }
    }

    #[test]
    fn an_enormous_step_is_bounded_work_rather_than_a_hang() {
        // The simulation steps at 1/120 s, so an unclamped `advance` grinds
        // through a hundred and twenty of them for every second handed to it:
        // `advance(10_000.0)` was five seconds of work inside a frame that had
        // sixteen milliseconds. Asserted on the clock rather than on a
        // stopwatch, which is the same property without the flake.
        let args = args_for(&["--stars", "100", "--size", "40x12"]);
        let mut flight = Flight::new(&args, 40, 12);
        for huge in [1.0e3f32, 1.0e9, f32::MAX] {
            let before = flight.time;
            flight.advance(huge);
            let stepped = flight.time - before;
            assert!(
                stepped <= MAX_STEP_DT as f64,
                "{huge} advanced the clock by {stepped}"
            );
        }
        // A step inside the ceiling is still taken in full, or the clamp has
        // quietly become a cap on the frame rate.
        let before = flight.time;
        flight.advance(MAX_STEP_DT);
        assert!((flight.time - before - MAX_STEP_DT as f64).abs() < 1e-6);
    }

    #[test]
    fn the_sky_a_flight_opens_with_does_not_depend_on_the_window() {
        // The point of the fixed count, stated as the property it is for. The
        // pool used to be the canvas area times a density, so these two windows
        // opened on 19 stars and 1080 and no two terminals flew the same sky.
        // Asked of the real pool rather than of `args.stars`, since what the
        // flag says and what gets spawned were two different numbers for as
        // long as the sentinel existed.
        for (cols, rows) in [(40usize, 12usize), (300, 90)] {
            let size = format!("{cols}x{rows}");
            let args = args_for(&["--size", &size]);
            let flight = Flight::new(&args, cols, rows);
            assert_eq!(
                flight.field.len(),
                cli::DEFAULT_STARS,
                "a {cols}x{rows} window opened on its own count"
            );
        }

        // And the band outside is built from the same number, which matters
        // because it is a second call site that used to carry its own copy of
        // the density — and read it at whatever size the renderer happened to be
        // when the view was first switched to, so a terminal resized before `C`
        // was pressed left the two skies on different counts.
        let args = args_for(&["--size", "40x12"]);
        let mut outside = Flight::new(&args, 40, 12);
        outside.set_view(ViewMode::Side, &args);
        assert_eq!(outside.stars(), outside.field.len(), "the two skies differ");

        let explicit = args_for(&["--stars", "1234"]);
        assert_eq!(Flight::new(&explicit, 300, 90).field.len(), 1234);
    }

    #[test]
    fn an_empty_sky_is_a_count_like_any_other() {
        // `--stars 0` was the sentinel that asked for a count off the canvas
        // and now means what it says, which puts a genuinely empty pool through
        // `StarField::new` and `ExteriorField::new` for the first time — the
        // old floor of one in the automatic count meant nothing could reach
        // them empty. Flown in both views because the band and the cone are
        // separate pools reached by separate paths.
        let args = args_for(&["--stars", "0", "--size", "60x20"]);
        let mut paused = false;
        let mut flight = Flight::new(&args, 60, 20);

        for view in [ViewMode::Cockpit, ViewMode::Side] {
            flight.set_view(view, &args);
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
            }
            assert_eq!(flight.stars(), 0, "{view:?} spawned a sky from nowhere");
            flight.renderer.present(&mut Vec::new()).unwrap();
        }

        // And the two keys answer it differently, which is the one place they
        // are not each other's inverse. `+` has to lift it off zero or the
        // multiply is a fixed point and the key is dead for the rest of the
        // flight; `-` must not, because a key labelled *fewer* that hands back
        // eight stars is the fault `POOL_FLOOR` came down from 64 to fix.
        flight.set_view(ViewMode::Cockpit, &args);
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert_eq!(flight.stars(), 0, "`-` put stars into an empty sky");
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert_eq!(
            flight.stars(),
            POOL_FLOOR,
            "`+` swallowed the press on an empty sky"
        );
    }

    #[test]
    fn the_star_keys_move_the_pool_the_way_they_point() {
        // Regression. The keys clamped up to a floor of 64, chosen under an
        // automatic minimum of 300 and sitting over the count on any window the
        // density that replaced it opened thin: `-` added stars rather than
        // taking them away, and both keys landed on the same number. The other
        // half is the fixed point — `(n as f32 * 1.25) as usize` is `n` again
        // below four — so the floor could not simply go to one either.
        //
        // One window now rather than two. It was flown at 80x24 and 40x12
        // because *which* of them the density opened under the floor moved with
        // the density itself; both open on the same fixed count today, so the
        // second window was asking the first one's question twice.
        let args = args_for(&["--size", "40x12"]);
        let mut paused = false;

        let mut flight = Flight::new(&args, 40, 12);
        let opened = flight.field.len();
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert!(
            flight.field.len() < opened,
            "`-` took a pool of {opened} to {}",
            flight.field.len()
        );

        let mut flight = Flight::new(&args, 40, 12);
        let opened = flight.field.len();
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(flight.field.len() > opened, "`+` left the pool at {opened}");

        // The floor is reached rather than approached, and with the automatic
        // count's own floor of one gone this is the only thing left saying a
        // pool cannot be walked to nothing. `-` multiplies by 0.8, which has
        // nowhere to stop on its own: 0.8 of a pool is a pool, all the way down.
        let mut flight = Flight::new(&args, 40, 12);
        for _ in 0..40 {
            handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        }
        assert_eq!(
            flight.field.len(),
            POOL_FLOOR,
            "`-` walked the pool to {}",
            flight.field.len()
        );

        // And `+` climbs back off it, which is why the floor is not 1: the
        // growth truncates, so below four stars the key gives the same number
        // back and swallows the press. Asked through the key rather than of a
        // second copy of the step.
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(
            flight.field.len() > POOL_FLOOR,
            "`+` cannot climb off its own floor of {POOL_FLOOR}"
        );
    }

    #[test]
    fn seeds_are_honoured_and_otherwise_invented() {
        assert_eq!(seed(&args_for(&["--seed", "99"])), 99);
        let a = seed(&args_for(&[]));
        assert_ne!(a, 0, "a time-derived seed should not be degenerate");
    }

    #[test]
    fn the_same_seed_produces_the_same_flight() {
        let render = || {
            let args = args_for(&["--seed", "7", "--stars", "500", "--size", "40x12"]);
            let mut flight = Flight::new(&args, 40, 12);
            let mut out = Vec::new();
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
                flight.renderer.present_plain(&mut out).unwrap();
            }
            out
        };
        assert_eq!(render(), render(), "a seeded flight must be reproducible");
    }

    #[test]
    fn different_seeds_produce_different_skies() {
        let render = |seed: &str| {
            let args = args_for(&["--seed", seed, "--stars", "500", "--size", "40x12"]);
            let mut flight = Flight::new(&args, 40, 12);
            let mut out = Vec::new();
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
                flight.renderer.present_plain(&mut out).unwrap();
            }
            out
        };
        assert_ne!(render("1"), render("2"));
    }

    #[test]
    fn keys_do_what_they_say() {
        let args = args_for(&["--stars", "200", "--size", "40x12"]);
        let mut flight = Flight::new(&args, 40, 12);
        let mut paused = false;

        let before = flight.ship.throttle;
        handle_key(press(KeyCode::Up), &mut flight, &args, &mut paused);
        assert!(flight.ship.throttle > before);
        handle_key(press(KeyCode::Down), &mut flight, &args, &mut paused);
        handle_key(press(KeyCode::Down), &mut flight, &args, &mut paused);
        assert!(flight.ship.throttle < before);

        handle_key(press(KeyCode::Char(' ')), &mut flight, &args, &mut paused);
        assert!(flight.ship.warp_engaged);

        handle_key(press(KeyCode::Char('p')), &mut flight, &args, &mut paused);
        assert!(paused);

        let stars = flight.field.len();
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(flight.field.len() > stars);
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert!(flight.field.len() < stars * 2);

        // Reset has to take the attitude with it, roll included — a ship left
        // inverted after an `R` would be a reset in name only.
        handle_key(press(KeyCode::Char('e')), &mut flight, &args, &mut paused);
        flight.advance(1.0 / 60.0);
        assert!(flight.ship.roll != 0.0);
        handle_key(press(KeyCode::Char('r')), &mut flight, &args, &mut paused);
        assert!(!flight.ship.warp_engaged && !paused);
        assert_eq!(flight.ship.throttle, args.throttle);
        assert_eq!((flight.ship.roll, flight.ship.roll_rate), (0.0, 0.0));
    }

    #[test]
    fn the_stick_is_wasd_and_qe() {
        // Every axis, in both directions, on both cases of the letter — and on
        // the arrows that share the yaw axis with A and D.
        let args = args_for(&["--stars", "200", "--size", "40x12"]);
        let mut flight = Flight::new(&args, 40, 12);
        let mut paused = false;

        // Pitch, yaw, roll — the first key of each pair is the negative end.
        let rates = |ship: &Ship| [ship.pitch_rate, ship.yaw_rate, ship.roll_rate];
        let pairs = [('w', 's'), ('a', 'd'), ('q', 'e')];
        for (axis, (down, up)) in pairs.into_iter().enumerate() {
            for (key, want_negative) in [(down, true), (up, false)] {
                for key in [key, key.to_ascii_uppercase()] {
                    flight.ship.reset();
                    let before = flight.ship.throttle;
                    handle_key(press(KeyCode::Char(key)), &mut flight, &args, &mut paused);

                    let rates = rates(&flight.ship);
                    assert_eq!(
                        rates[axis] < 0.0,
                        want_negative,
                        "{key} moved the stick the wrong way: {}",
                        rates[axis]
                    );
                    // One key, one axis: the stick must not cross-couple,
                    // and W and S must no longer touch the throttle they
                    // used to be.
                    for other in (0..3).filter(|o| *o != axis) {
                        assert_eq!(rates[other], 0.0, "{key} also moved axis {other}");
                    }
                    assert_eq!(flight.ship.throttle, before, "{key} moved the throttle");
                }
            }
        }

        // `i`/`k` pitched before WASD arrived and still do.
        for (key, want_up) in [('i', true), ('k', false)] {
            flight.ship.reset();
            handle_key(press(KeyCode::Char(key)), &mut flight, &args, &mut paused);
            assert_eq!(flight.ship.pitch_rate < 0.0, want_up, "{key} pitched wrong");
        }

        // The arrows steer the same axis A and D do, and are the only throttle.
        for (code, want_left) in [(KeyCode::Left, true), (KeyCode::Right, false)] {
            flight.ship.reset();
            handle_key(press(code), &mut flight, &args, &mut paused);
            assert_eq!(flight.ship.yaw_rate < 0.0, want_left);
            assert_eq!(flight.ship.pitch_rate, 0.0, "an arrow must not pitch");
        }
        for (code, want_more) in [(KeyCode::Up, true), (KeyCode::Down, false)] {
            flight.ship.reset();
            let before = flight.ship.throttle;
            handle_key(press(code), &mut flight, &args, &mut paused);
            assert_eq!(flight.ship.throttle > before, want_more);
        }
    }

    #[test]
    fn a_roll_turns_the_sky_and_stays_where_it_is_put() {
        // The pilot's roll is attitude, not a transient: it holds after the
        // impulse has decayed, which is what makes flying inverted possible.
        let args = args_for(&["--stars", "300", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;

        for _ in 0..30 {
            handle_key(press(KeyCode::Char('e')), &mut flight, &args, &mut paused);
            flight.advance(1.0 / 60.0);
        }
        assert!(
            flight.ship.roll > 0.1,
            "E should have rolled to starboard: {}",
            flight.ship.roll
        );

        for _ in 0..300 {
            flight.advance(1.0 / 60.0);
        }
        assert!(
            flight.ship.roll_rate.abs() < 0.01,
            "the roll rate must decay"
        );
        let rolled = flight.ship.roll;
        for _ in 0..300 {
            flight.advance(1.0 / 60.0);
        }
        assert!(
            (flight.ship.roll - rolled).abs() < 1e-3,
            "the roll itself must not: {rolled} became {}",
            flight.ship.roll
        );

        // And the flight is still flyable afterwards — nothing has gone NaN and
        // the sky has not emptied.
        flight.draw(60.0, false, true);
        assert!(flight.ship.roll.is_finite() && !flight.field.is_empty());
    }

    #[test]
    fn c_cycles_the_camera_and_ctrl_c_still_quits() {
        let args = args_for(&["--stars", "200", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;
        assert_eq!(flight.view(), ViewMode::Cockpit, "a flight starts inside");

        for key in ['c', 'C'] {
            let before = flight.view();
            let action = handle_key(
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                &mut flight,
                &args,
                &mut paused,
            );
            assert!(matches!(action, Action::Continue), "{key} ended the flight");
            assert_eq!(
                flight.view(),
                before.next(),
                "{key} did not change the view"
            );
        }
        assert_eq!(
            flight.view(),
            ViewMode::Cockpit,
            "two presses should come round"
        );

        // And the key that quits still quits, which is the thing a new binding
        // on `c` could most easily break.
        assert!(matches!(
            handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &mut flight,
                &args,
                &mut paused
            ),
            Action::Quit
        ));
    }

    #[test]
    fn the_stick_flies_the_camera_outside_and_the_ship_inside() {
        // Out there the camera rides with the ship, so a turn of the *hull*
        // moves nothing an eye can see: the stars stream on as they were and
        // the hull leans a few degrees. The six keys that fly the ship in here
        // fly the camera out there instead, which is the thing that can
        // usefully move in a view whose whole subject is the ship.
        //
        // The ship is what this checks, because the ship is what would go
        // wrong: a camera control that also nudged the flight model would fly
        // the ship from a seat that is not in it.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;

        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        assert_eq!(flight.view(), ViewMode::Side);

        for code in [
            KeyCode::Char('w'),
            KeyCode::Char('W'),
            KeyCode::Char('s'),
            KeyCode::Char('i'),
            KeyCode::Char('k'),
            KeyCode::Char('a'),
            KeyCode::Char('d'),
            KeyCode::Char('A'),
            KeyCode::Char('q'),
            KeyCode::Char('e'),
            KeyCode::Char('E'),
            KeyCode::Left,
            KeyCode::Right,
        ] {
            flight.ship.reset();
            flight.reset_orbit(Orbit::LEVEL);
            let action = handle_key(press(code), &mut flight, &args, &mut paused);
            assert!(
                matches!(action, Action::Continue),
                "{code:?} ended the flight"
            );
            assert_eq!(flight.ship.pitch_rate, 0.0, "{code:?} still pitched");
            assert_eq!(flight.ship.yaw_rate, 0.0, "{code:?} still yawed");
            assert_eq!(flight.ship.roll_rate, 0.0, "{code:?} still rolled the ship");
            // And it did something: a key that swallows the input and gives
            // nothing back is the thing this view used to have.
            assert!(
                !flight.orbit_target().is_level(),
                "{code:?} moved neither the ship nor the camera"
            );
        }

        // Each of the three axes, and each of them on its own.
        for (code, axis) in [
            (KeyCode::Char('d'), 0),
            (KeyCode::Char('w'), 1),
            (KeyCode::Char('q'), 2),
        ] {
            flight.reset_orbit(Orbit::LEVEL);
            handle_key(press(code), &mut flight, &args, &mut paused);
            let o = flight.orbit_target();
            let moved = [o.azimuth, o.elevation, o.roll];
            assert!(moved[axis] > 0.0, "{code:?} did not move its own axis");
            for other in (0..3).filter(|o| *o != axis) {
                assert_eq!(moved[other], 0.0, "{code:?} moved axis {other} as well");
            }
        }
        // And the opposite of each goes the other way.
        for (code, axis) in [
            (KeyCode::Char('a'), 0),
            (KeyCode::Char('s'), 1),
            (KeyCode::Char('e'), 2),
        ] {
            flight.reset_orbit(Orbit::LEVEL);
            handle_key(press(code), &mut flight, &args, &mut paused);
            let o = flight.orbit_target();
            assert!(
                [o.azimuth, o.elevation, o.roll][axis] < 0.0,
                "{code:?} did not move its own axis back"
            );
        }

        // The throttle is what the arrows have always been.
        flight.ship.reset();
        let before = flight.ship.throttle;
        handle_key(press(KeyCode::Up), &mut flight, &args, &mut paused);
        assert!(flight.ship.throttle > before, "the throttle went quiet too");

        // And coming back inside gives the stick back, to the ship this time.
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        assert_eq!(flight.view(), ViewMode::Cockpit);
        for (code, check) in [
            (KeyCode::Char('w'), 0),
            (KeyCode::Char('a'), 1),
            (KeyCode::Char('q'), 2),
        ] {
            flight.ship.reset();
            flight.reset_orbit(Orbit::LEVEL);
            handle_key(press(code), &mut flight, &args, &mut paused);
            let rates = [
                flight.ship.pitch_rate,
                flight.ship.yaw_rate,
                flight.ship.roll_rate,
            ];
            assert!(
                rates[check] < 0.0,
                "the stick did not come back for {code:?}"
            );
            assert!(
                flight.orbit_target().is_level(),
                "{code:?} moved the camera from the pilot's seat"
            );
        }
    }

    #[test]
    fn pitching_about_inside_does_not_leave_the_ship_crooked_outside() {
        // Regression, and the path the bug was found on: fly the nose around
        // with `W`, press `C`, and the hull sat permanently nose-high while the
        // band of sky streamed past it dead level. The profile was posed from
        // `ship.pitch` — the accumulated attitude the panel reads out — and out
        // there nothing is measured against it, because the ship flies where
        // its nose points and the sky streams along that track.
        //
        // Stated as the property that was broken: once the stick has been let
        // go and the rates have decayed, a flight that pitched about draws the
        // same outside frame as one that never touched it. Compared on the
        // pixels rather than the cells, because the *panel* is meant to differ
        // — the compass has genuinely moved, and it should say so.
        let args = args_for(&["--seed", "6", "--stars", "300", "--size", "80x24"]);
        let frames = |pitched: bool| {
            let mut flight = Flight::new(&args, 80, 24);
            let mut paused = false;

            for _ in 0..120 {
                if pitched {
                    handle_key(press(KeyCode::Char('w')), &mut flight, &args, &mut paused);
                }
                flight.advance(1.0 / 60.0);
            }
            // Ten seconds for the impulse to decay to nothing, keeping whatever
            // attitude it flew the ship to.
            for _ in 0..600 {
                flight.advance(1.0 / 60.0);
            }
            if pitched {
                assert!(
                    flight.ship.pitch.abs() > 0.5,
                    "the stick never moved the ship: {}",
                    flight.ship.pitch
                );
            }

            handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
            assert_eq!(flight.view(), ViewMode::Side);
            let mut pixels = Vec::new();
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
                pixels.extend_from_slice(flight.renderer.pixels());
            }
            pixels
        };

        let (crooked, straight) = (frames(true), frames(false));
        let differing = crooked
            .iter()
            .zip(&straight)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differing, 0,
            "{differing} subpixels differ: the ship was left flying crooked"
        );
    }

    #[test]
    fn the_outside_sky_is_only_built_when_it_is_asked_for() {
        // A cockpit-only run must pay nothing for the view it never opens —
        // not the pool, and not a draw from a generator that would have to come
        // from somewhere.
        let args = args_for(&["--stars", "500", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        assert!(
            flight.exterior.is_none(),
            "the outside sky was built anyway"
        );

        flight.cycle_view(&args);
        assert!(
            flight.exterior.is_some(),
            "C did not build a sky to look at"
        );
        assert_eq!(flight.field.len(), 500, "the cockpit pool was disturbed");
    }

    #[test]
    fn the_two_skies_are_independent_of_one_another() {
        // The cockpit field's generator is the one the reference frames were
        // recorded from. Building a second field must not touch it, whenever
        // that happens, and the two skies must not be the same pattern twice.
        let args = args_for(&["--seed", "4", "--stars", "300", "--size", "60x20"]);
        let sky = |side: bool| {
            let mut flight = Flight::new(&args, 60, 20);
            if side {
                flight.cycle_view(&args);
            }
            let cam = flight.renderer.camera(&flight.ship, 0.0);
            flight
                .field
                .streaks(&cam, 0.0, 0.0)
                .map(|s| s.to)
                .collect::<Vec<_>>()
        };
        assert_eq!(sky(false), sky(true), "going outside moved the cockpit sky");
    }

    #[test]
    fn switching_views_leaves_the_other_sky_where_it_was() {
        // Only the view being flown is stepped, which is safe because both
        // fields work out where a star *was* from where it is. Nothing may
        // freeze, and coming back must not draw a scratch across the frame.
        let args = args_for(&["--seed", "8", "--stars", "400", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        flight.ship.throttle = 1.0;
        for _ in 0..60 {
            flight.advance(1.0 / 60.0);
        }
        let cam = flight.renderer.camera(&flight.ship, flight.time);
        let parked: Vec<(f32, f32)> = flight.field.streaks(&cam, 0.0, 0.0).map(|s| s.to).collect();

        flight.cycle_view(&args);
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
            flight.draw(60.0, false, true);
        }
        let cam = flight.renderer.camera(&flight.ship, flight.time);
        let after: Vec<(f32, f32)> = flight.field.streaks(&cam, 0.0, 0.0).map(|s| s.to).collect();
        assert_eq!(
            parked, after,
            "the cockpit sky moved while nobody was in it"
        );

        // Back inside, and the first frame is a normal one rather than a smear
        // of two seconds' travel.
        flight.cycle_view(&args);
        flight.advance(1.0 / 60.0);
        let cam = flight.renderer.camera(&flight.ship, flight.time);
        let longest = flight
            .field
            .streaks(&cam, 0.0, 0.0)
            .map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1))
            .fold(0.0f32, f32::max);
        assert!(longest < 40.0, "coming back drew a scratch {longest} long");
    }

    #[test]
    fn m_opens_the_ship_picker_and_escape_closes_it_without_quitting() {
        // The nastiest bug this feature could have: reaching for the key that
        // dismisses a dialogue and losing the terminal instead.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;

        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);
        assert!(flight.menu_open(), "M did not open the picker");
        assert_eq!(
            flight.view(),
            ViewMode::Side,
            "the picker should show you what it is offering"
        );

        let action = handle_key(press(KeyCode::Esc), &mut flight, &args, &mut paused);
        assert!(matches!(action, Action::Continue), "Esc ended the flight");
        assert!(!flight.menu_open(), "Esc did not close the picker");

        // With it shut, Esc is the way out again.
        assert!(matches!(
            handle_key(press(KeyCode::Esc), &mut flight, &args, &mut paused),
            Action::Quit
        ));
    }

    #[test]
    fn the_picker_takes_the_keyboard_while_it_is_up() {
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;
        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);

        let throttle = flight.ship.throttle;
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char('w'),
            KeyCode::Char('a'),
            KeyCode::Char(' '),
        ] {
            handle_key(press(code), &mut flight, &args, &mut paused);
        }
        assert_eq!(flight.ship.throttle, throttle, "the arrows flew the ship");
        assert_eq!(
            flight.ship.yaw_rate, 0.0,
            "the stick was live under the menu"
        );
        assert!(!flight.ship.warp_engaged, "space lit the drive");
    }

    #[test]
    fn choosing_a_ship_previews_it_and_keeps_it() {
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;
        let flown = |f: &Flight| f.drawn_model().name;

        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);
        let first = flown(&flight);
        handle_key(press(KeyCode::Down), &mut flight, &args, &mut paused);
        assert_ne!(
            flown(&flight),
            first,
            "the picker describes instead of showing"
        );

        let previewed = flown(&flight);
        handle_key(press(KeyCode::Enter), &mut flight, &args, &mut paused);
        assert!(!flight.menu_open());
        assert_eq!(flown(&flight), previewed, "Enter did not keep the choice");

        // And backing out leaves the ship alone.
        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);
        handle_key(press(KeyCode::Down), &mut flight, &args, &mut paused);
        handle_key(press(KeyCode::Esc), &mut flight, &args, &mut paused);
        assert_eq!(flown(&flight), previewed, "Esc changed the ship anyway");
    }

    #[test]
    fn the_outside_view_flies_and_resizes_like_the_inside_one() {
        let args = args_for(&["--seed", "2", "--view", "side", "--ship", "trident"]);
        let mut flight = Flight::new(&args, 80, 24);
        assert_eq!(flight.view(), ViewMode::Side, "--view side did not take");
        assert_eq!(flight.drawn_model().name, "trident");

        flight.ship.throttle = 1.0;
        flight.ship.toggle_warp();
        for (cols, rows) in [(80, 24), (1, 1), (200, 60), (46, 12), (120, 40)] {
            flight.resize(&args, cols, rows);
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
            }
            let (cw, ch) = flight.renderer.canvas_dims();
            assert_eq!(flight.renderer.pixels().len(), cw * ch);
            assert!(
                flight.stars() > 0,
                "the outside sky emptied at {cols}x{rows}"
            );
            flight.renderer.present(&mut Vec::new()).unwrap();
        }
        assert!(flight
            .renderer
            .pixels()
            .iter()
            .any(|p| p.iter().any(|v| *v > 40)));
    }

    #[test]
    fn the_star_keys_grow_whichever_sky_is_being_looked_at() {
        let args = args_for(&["--stars", "500", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;

        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        let outside = flight.stars();
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(flight.stars() > outside, "+ did not grow the outside sky");
        assert_eq!(flight.field.len(), 500, "it grew the wrong one");
    }

    #[test]
    fn screensaver_mode_flies_itself_forever() {
        let args = args_for(&["--screensaver"]);
        assert!(args.screensaver);
        assert!(args.demo.is_none(), "a screensaver has no deadline to hit");

        // The autopilot has to keep going indefinitely, not stall after one
        // cycle — a screensaver that freezes is worse than no screensaver.
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let dt = 1.0 / 60.0;
        let mut peak_per_cycle = vec![];
        for cycle in 0..4 {
            let mut peak: f32 = 0.0;
            let start = cycle as f64 * Autopilot::CYCLE;
            for frame in 0..(Autopilot::CYCLE / dt as f64) as usize {
                autopilot.update(&mut ship, start + frame as f64 * dt as f64);
                ship.update(dt);
                peak = peak.max(ship.velocity_c());
            }
            peak_per_cycle.push(peak);
        }
        assert!(
            peak_per_cycle.iter().all(|p| *p > 100.0),
            "every cycle should reach warp: {peak_per_cycle:?}"
        );
    }

    #[test]
    fn quit_keys_quit() {
        let args = args_for(&["--stars", "100", "--size", "20x8"]);
        let mut flight = Flight::new(&args, 20, 8);
        let mut paused = false;
        for key in [
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        ] {
            assert!(matches!(
                handle_key(key, &mut flight, &args, &mut paused),
                Action::Quit
            ));
        }
        // Neither a plain 'c' nor a plain 'd' is a quit — it would be a nasty
        // surprise if either were, and 'd' steers.
        for key in ['c', 'd'] {
            assert!(matches!(
                handle_key(
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                    &mut flight,
                    &args,
                    &mut paused
                ),
                Action::Continue
            ));
        }
    }

    #[test]
    fn q_flies_the_ship_rather_than_ending_the_flight() {
        // Regression in waiting: `q` quit before it was a control. Now it rolls
        // to port, and a pilot reaching for it must not lose the terminal.
        let args = args_for(&["--stars", "100", "--size", "20x8"]);
        let mut flight = Flight::new(&args, 20, 8);
        let mut paused = false;
        for key in ['q', 'Q'] {
            flight.ship.reset();
            let action = handle_key(
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                &mut flight,
                &args,
                &mut paused,
            );
            assert!(matches!(action, Action::Continue), "{key} quit");
            assert!(flight.ship.roll_rate < 0.0, "{key} did not roll to port");
        }
    }

    #[test]
    fn key_releases_are_ignored() {
        let args = args_for(&["--stars", "100", "--size", "20x8"]);
        let mut flight = Flight::new(&args, 20, 8);
        let mut paused = false;
        let before = flight.ship.throttle;
        let mut release = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        handle_key(release, &mut flight, &args, &mut paused);
        assert_eq!(flight.ship.throttle, before);
    }

    #[test]
    fn a_resize_storm_keeps_every_buffer_in_step() {
        // Three buffers have to agree: the canvas is two subpixel rows per
        // terminal row, the screen is one cell per terminal cell, and the
        // resolved pixel buffer has to match the canvas exactly or `compose`
        // reads off the end of it. Walk a deliberately awful sequence —
        // degenerate, lopsided, and either side of the panel's breakpoints —
        // drawing and presenting at every step.
        let args = args_for(&["--seed", "9"]);
        let mut flight = Flight::new(&args, 80, 24);

        for (cols, rows) in [
            (80, 24),
            (1, 1),
            (300, 90),
            (2, 40),
            (200, 3),
            (45, 11), // just under the panel's minimum: it goes compact
            (46, 12), // and just over: the full panel comes back
            (37, 13),
            (1, 1),
            (120, 40),
        ] {
            flight.resize(&args, cols, rows);
            for _ in 0..5 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
            }

            let (cw, ch) = flight.renderer.canvas_dims();
            assert_eq!(
                flight.renderer.pixels().len(),
                cw * ch,
                "the resolved pixel buffer is stale at {cols}x{rows}"
            );
            assert!(
                !flight.field.is_empty(),
                "the star pool emptied at {cols}x{rows}"
            );
            assert!(flight.ship.speed.is_finite());

            let (sc, sr) = flight.renderer.screen().dims();
            assert_eq!(
                (cw, ch),
                (sc, sr * 2),
                "canvas and screen disagree at {cols}x{rows}"
            );
            flight.renderer.present(&mut Vec::new()).unwrap();
        }
    }

    #[test]
    fn a_forced_size_ignores_the_terminal_moving_underneath_it() {
        // Regression: `--size` was honoured at startup and then discarded by
        // the first resize event, so the flag meant "until the window moves".
        let args = args_for(&["--size", "120x40", "--stars", "200"]);
        let mut flight = Flight::new(&args, 120, 40);
        assert!(!flight.resize(&args, 80, 24), "a forced size must not move");
        assert_eq!(flight.renderer.canvas_dims(), (120, 80));

        // Without the flag the terminal is the authority — but only when it
        // actually says something new.
        let args = args_for(&["--stars", "200"]);
        let mut flight = Flight::new(&args, 120, 40);
        assert!(
            flight.resize(&args, 80, 24),
            "an unforced size follows the terminal"
        );
        assert_eq!(flight.renderer.canvas_dims(), (80, 48));
        assert!(
            !flight.resize(&args, 80, 24),
            "settling on the same size is no change"
        );
    }

    #[test]
    fn a_resize_moves_the_canvas_and_leaves_the_pool_alone() {
        // The inversion of what this used to hold. A resize re-derived the count
        // from the new canvas whenever `--stars` was 0, so dragging a window
        // edge respawned the sky a few hundred stars at a time and the panel's
        // own count walked about while nothing had asked it to. A count a window
        // can overrule is not one. Both skies, because the retune reached both.
        for extra in [vec![], vec!["--stars", "777"]] {
            let args = args_for(&extra);
            let want = args.stars;
            let mut flight = Flight::new(&args, 40, 12);
            flight.set_view(ViewMode::Side, &args);
            for (cols, rows) in [(300usize, 90usize), (1, 1), (80, 24)] {
                flight.resize(&args, cols, rows);
                assert_eq!(
                    flight.field.len(),
                    want,
                    "{cols}x{rows} retuned the cockpit pool"
                );
                assert_eq!(flight.stars(), want, "{cols}x{rows} retuned the band");
            }
        }
    }

    #[test]
    fn the_brackets_and_the_wheel_ask_for_the_same_zoom() {
        // Two ways into one control. They are wired separately — one through
        // `handle_key`, one through `handle_mouse` — so nothing but a test says
        // they agree, and a wheel that zoomed the opposite way to the key it
        // shares a job with would be a very tiresome bug to live with.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        for (code, kind) in [
            (KeyCode::Char(']'), MouseEventKind::ScrollUp),
            (KeyCode::Char('['), MouseEventKind::ScrollDown),
        ] {
            let mut by_key = outside(&args);
            let mut by_wheel = outside(&args);
            let mut paused = false;
            for _ in 0..5 {
                handle_key(press(code), &mut by_key, &args, &mut paused);
                handle_mouse(wheel(kind), &mut by_wheel);
            }
            assert_eq!(
                by_key.zoom_target, by_wheel.zoom_target,
                "{code:?} and {kind:?} disagree"
            );
        }

        // And they run the way round they read: `]` and a wheel pushed away
        // bring the ship closer.
        let mut flight = outside(&args);
        let mut paused = false;
        handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        assert!(flight.zoom_target > view::ZOOM_DEFAULT, "] should zoom in");
        // The shifted forms are the same key on a keyboard that sends them.
        let mut shifted = outside(&args);
        handle_key(press(KeyCode::Char('}')), &mut shifted, &args, &mut paused);
        assert_eq!(shifted.zoom_target, flight.zoom_target);
    }

    #[test]
    fn the_zoom_clamps_rather_than_running_away() {
        // The stop has to be on the *target*, or the target winds up somewhere
        // out past the end while the camera sits at the limit, and the first
        // notch back does nothing at all — a control that has to be pressed
        // fifteen times before it answers reads as broken.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = outside(&args);
        let mut paused = false;

        for _ in 0..100 {
            handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        }
        assert_eq!(flight.zoom_target, view::ZOOM_MAX);
        handle_key(press(KeyCode::Char('[')), &mut flight, &args, &mut paused);
        assert!(
            flight.zoom_target < view::ZOOM_MAX,
            "one notch back off the stop did nothing"
        );

        for _ in 0..100 {
            handle_key(press(KeyCode::Char('[')), &mut flight, &args, &mut paused);
        }
        assert_eq!(flight.zoom_target, view::ZOOM_MIN);
        handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        assert!(flight.zoom_target > view::ZOOM_MIN);
    }

    #[test]
    fn a_notch_and_its_opposite_land_back_where_they_started() {
        // Geometric steps, so this is a multiply and a divide rather than a
        // pair of `powf`s that would each land an ulp or two off.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = outside(&args);
        let mut paused = false;
        for _ in 0..6 {
            handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        }
        for _ in 0..6 {
            handle_key(press(KeyCode::Char('[')), &mut flight, &args, &mut paused);
        }
        assert!(
            (flight.zoom_target - view::ZOOM_DEFAULT).abs() < 1e-5,
            "six in and six out drifted to {}",
            flight.zoom_target
        );
    }

    #[test]
    fn the_zoom_is_not_connected_in_the_cockpit() {
        // The mirror of `the_stick_loses_pitch_and_yaw_outside_the_cockpit`:
        // from the pilot's seat there is no ship to be looking at, so the keys
        // are plainly not wired rather than quietly swallowing the press.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;
        assert_eq!(flight.view(), ViewMode::Cockpit);

        for code in [KeyCode::Char('['), KeyCode::Char(']')] {
            let action = handle_key(press(code), &mut flight, &args, &mut paused);
            assert!(matches!(action, Action::Continue), "{code:?} ended it");
        }
        handle_mouse(wheel(MouseEventKind::ScrollUp), &mut flight);
        assert_eq!(flight.zoom_target, view::ZOOM_DEFAULT, "the cockpit zoomed");

        // Outside they work, and coming back in leaves the zoom where it was
        // rather than resetting it: it is state, not a mode.
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        let out_there = flight.zoom_target;
        assert!(out_there > view::ZOOM_DEFAULT);
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        assert_eq!(flight.view(), ViewMode::Cockpit);
        handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        assert_eq!(flight.zoom_target, out_there, "the cockpit moved it");
    }

    #[test]
    fn the_zoom_settles_the_same_way_however_the_frames_fall() {
        // The ease is `1 - exp(-k·dt)` rather than `k·dt` for the same reason
        // every other ease in the tree is: the second one is the classic
        // simplification and it silently makes the answer depend on the step
        // size, so a terminal keeping up would zoom at a different rate to one
        // that is not.
        //
        // A fifth of a second, delivered as twelve frames and as three. Both
        // come to the same twenty-four simulation steps, so the two answers are
        // not merely close but identical — which is what stepping the ease
        // inside the fixed-step loop buys, and what moving it outside to run on
        // the frame's own `dt` would throw away.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let settled = |dt: f32, steps: usize| {
            let mut flight = outside(&args);
            let mut paused = false;
            for _ in 0..4 {
                handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
            }
            for _ in 0..steps {
                flight.advance(dt);
            }
            flight.zoom()
        };
        let fine = settled(1.0 / 60.0, 12);
        let coarse = settled(1.0 / 15.0, 3);
        assert!(
            (fine - coarse).abs() < 1e-6,
            "a fifth of a second came out {fine} in small steps and {coarse} in large"
        );
        assert!(fine > view::ZOOM_DEFAULT, "it never left: {fine}");
        // And it does arrive, rather than easing forever toward it.
        let arrived = settled(1.0 / 60.0, 300);
        let mut asked = outside(&args);
        let mut paused = false;
        for _ in 0..4 {
            handle_key(press(KeyCode::Char(']')), &mut asked, &args, &mut paused);
        }
        assert!((arrived - asked.zoom_target).abs() < 1e-4);
    }

    #[test]
    fn the_zoom_moves_the_ship_and_leaves_the_sky_alone() {
        // Why this is a dolly and not a change of lens, stated as a property.
        // The star band is laid out against the camera's focal length, so a
        // zoom that touched the focal length would sweep the whole sky about
        // and need the field re-folded every notch. Flown at sublight, where
        // the lens is off and an exactly identical sky is the whole claim:
        // every subpixel that differs between two zooms has to be one the ship
        // and its bubble could have reached.
        let args = args_for(&["--seed", "5", "--stars", "1200", "--size", "80x24"]);
        let frame = |zoom: f32| {
            let mut flight = outside(&args);
            flight.zoom = zoom;
            flight.zoom_target = zoom;
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
            }
            flight.draw(60.0, false, false);
            flight.renderer.pixels().to_vec()
        };
        let (near, far) = (frame(view::ZOOM_MAX), frame(view::ZOOM_MIN));
        let (w, h) = (80usize, 48usize);
        assert_eq!(near.len(), w * h);

        // Generous: the hull at the widest zoom, and the bubble that would
        // hold it. Nothing outside that has any business having changed.
        let reach = view::ship_half_on_screen(h as f32, view::ZOOM_MAX) * 3.0;
        let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
        for y in 0..h {
            for x in 0..w {
                if near[y * w + x] == far[y * w + x] {
                    continue;
                }
                let r = (x as f32 - cx).hypot(y as f32 - cy);
                assert!(
                    r <= reach,
                    "the zoom moved a star {r} subpixels out, well clear of the ship"
                );
            }
        }
    }

    #[test]
    fn the_shot_still_opens_exactly_where_it_always_did() {
        // The acceptance test for the whole change, asked of pixels rather than
        // of a hash so that it fails on the machine that broke it rather than
        // in CI. A flight that never touches the camera has to draw what it
        // drew before any of this existed — the star band's fast paths, the
        // bubble's unforeshortened outline and the hull's quarter turn are all
        // written to be exact at `Orbit::LEVEL` rather than very nearly so, and
        // this is what says they are.
        //
        // Flown twice with the camera swung out and put back in between, which
        // is the harder half: the ease is asymptotic, so `R` snapping both the
        // angle and its target is the only thing that gets back to an exact
        // zero, and a camera that settled to `1e-9` instead would take every
        // slow path for the rest of the flight.
        let args = args_for(&["--seed", "9", "--stars", "900", "--size", "80x24"]);
        let mut paused = false;

        let mut plain = Flight::new(&args, 80, 24);
        handle_key(press(KeyCode::Char('c')), &mut plain, &args, &mut paused);
        for _ in 0..40 {
            plain.advance(1.0 / 60.0);
        }
        plain.draw(60.0, false, true);
        let want: Vec<[u8; 3]> = plain.renderer.pixels().to_vec();

        let mut wandered = Flight::new(&args, 80, 24);
        handle_key(press(KeyCode::Char('c')), &mut wandered, &args, &mut paused);
        for key in ['w', 'a', 'q', 'd', 's', 'e'] {
            for _ in 0..5 {
                handle_key(press(KeyCode::Char(key)), &mut wandered, &args, &mut paused);
                wandered.advance(1.0 / 60.0);
            }
        }
        handle_key(press(KeyCode::Char('r')), &mut wandered, &args, &mut paused);
        assert!(wandered.orbit().is_level(), "R did not put the camera back");
        // Re-flown from scratch so the two skies have taken the same number of
        // steps: the point is the camera, not the flight.
        let mut back = Flight::new(&args, 80, 24);
        handle_key(press(KeyCode::Char('c')), &mut back, &args, &mut paused);
        back.reset_orbit(wandered.orbit());
        for _ in 0..40 {
            back.advance(1.0 / 60.0);
        }
        back.draw(60.0, false, true);

        let got = back.renderer.pixels();
        let differing = want.iter().zip(got).filter(|(a, b)| a != b).count();
        assert_eq!(
            differing, 0,
            "{differing} subpixels moved with the camera put back where it started"
        );
    }

    #[test]
    fn the_camera_is_not_connected_in_the_cockpit() {
        // The mirror of `the_zoom_is_not_connected_in_the_cockpit`, and the
        // other half of `the_stick_flies_the_camera_outside_and_the_ship_inside`
        // — in here the six keys fly the ship, so none of them may move the
        // camera, and the camera survives a round trip through `C` because it
        // is state rather than a mode.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;

        for key in ['w', 'a', 's', 'd', 'q', 'e'] {
            handle_key(press(KeyCode::Char(key)), &mut flight, &args, &mut paused);
        }
        assert!(
            flight.orbit_target().is_level(),
            "the cockpit moved a camera it cannot see: {:?}",
            flight.orbit_target()
        );

        // And out there, put somewhere, it stays there across a trip inside.
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        for _ in 0..4 {
            handle_key(press(KeyCode::Char('d')), &mut flight, &args, &mut paused);
        }
        let swung = flight.orbit_target();
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        assert_eq!(flight.view(), ViewMode::Side);
        assert_eq!(
            flight.orbit_target(),
            swung,
            "the camera was re-parked by a trip through the cockpit"
        );
    }

    #[test]
    fn a_camera_notch_and_its_opposite_land_back_where_they_started() {
        // Stronger than the zoom's version of this, and worth stating because
        // it is why the step is additive: `x + s - s` is exactly `x` where a
        // geometric step has to settle for very nearly.
        let args = args_for(&["--stars", "200", "--size", "60x20"]);
        let mut flight = outside(&args);
        let mut paused = false;

        for (out, back) in [('d', 'a'), ('w', 's'), ('q', 'e')] {
            for _ in 0..7 {
                handle_key(press(KeyCode::Char(out)), &mut flight, &args, &mut paused);
            }
            for _ in 0..7 {
                handle_key(press(KeyCode::Char(back)), &mut flight, &args, &mut paused);
            }
            assert!(
                flight.orbit_target().is_level(),
                "{out} then {back} left the camera at {:?}",
                flight.orbit_target()
            );
        }
    }

    #[test]
    fn holding_a_camera_key_never_stops() {
        // The three angles wrap, which is the whole reason they can be `f32`
        // beside a `time` that has to be `f64`: a key leaned on for a week
        // accumulates nothing, because every step is folded back onto a single
        // turn.
        //
        // And none of them parks. `W` used to stop dead at the quarter turn,
        // which is what a clamp does and is not what a hand holding the key is
        // asking for; over the top and round is.
        let args = args_for(&["--stars", "200", "--size", "60x20"]);
        let mut flight = outside(&args);
        let mut paused = false;

        // A full loop of the elevation on its own, watched the whole way. The
        // camera has to pass the top rather than arrive at it: `> 0` for a
        // while, then `< 0` once it is over, then home.
        let mut over_the_top = false;
        let mut been_under = false;
        for _ in 0..40 {
            for _ in 0..3 {
                handle_key(press(KeyCode::Char('w')), &mut flight, &args, &mut paused);
            }
            flight.advance(1.0 / 60.0);
            let e = flight.orbit().elevation;
            assert!(e.is_finite() && e.abs() <= std::f32::consts::PI, "{e}");
            if e.abs() > std::f32::consts::FRAC_PI_2 {
                over_the_top = true;
            }
            if over_the_top && e < 0.0 {
                been_under = true;
            }
        }
        assert!(
            over_the_top,
            "the camera never got past the quarter turn: {:?}",
            flight.orbit()
        );
        assert!(
            been_under,
            "the camera went over the top and stuck there: {:?}",
            flight.orbit()
        );

        // And the other two, leaned on together for a long time.
        for _ in 0..4000 {
            handle_key(press(KeyCode::Char('d')), &mut flight, &args, &mut paused);
            handle_key(press(KeyCode::Char('q')), &mut flight, &args, &mut paused);
            flight.advance(1.0 / 60.0);
        }
        let o = flight.orbit();
        for angle in [o.azimuth, o.elevation, o.roll] {
            assert!(
                angle.is_finite() && angle.abs() <= std::f32::consts::PI,
                "the camera came apart: {o:?}"
            );
        }
        flight.draw(60.0, false, true);
    }

    #[test]
    fn the_camera_settles_the_same_way_however_the_frames_fall() {
        // The ease is `1 - exp(-k·dt)` inside a fixed sim step, exactly as the
        // zoom's and the flight model's are, so where the camera ends up does
        // not depend on how the frames happened to land. Rewriting one of these
        // as `k * dt` is the classic simplification and it makes the answer a
        // function of the frame rate.
        let args = args_for(&["--stars", "200", "--size", "60x20"]);
        let settle = |frames: usize, dt: f32| {
            let mut paused = false;
            let mut flight = outside(&args);
            for _ in 0..3 {
                handle_key(press(KeyCode::Char('d')), &mut flight, &args, &mut paused);
            }
            for _ in 0..frames {
                flight.advance(dt);
            }
            flight.orbit().azimuth
        };
        let fast = settle(12, 1.0 / 60.0);
        let slow = settle(3, 1.0 / 15.0);
        assert!(
            (fast - slow).abs() < 1e-6,
            "the camera settled to {fast} at 60 fps and {slow} at 15"
        );
    }

    #[test]
    fn resetting_puts_the_zoom_back_where_it_started() {
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = outside(&args);
        let mut paused = false;
        for _ in 0..5 {
            handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        }
        flight.advance(1.0);
        assert!(flight.zoom() > view::ZOOM_DEFAULT);

        handle_key(press(KeyCode::Char('r')), &mut flight, &args, &mut paused);
        assert_eq!(flight.zoom(), view::ZOOM_DEFAULT, "R left it zoomed");
        assert_eq!(
            flight.zoom_target,
            view::ZOOM_DEFAULT,
            "R left it still gliding back"
        );
    }

    #[test]
    fn the_picker_takes_the_wheel_as_well_as_the_keyboard() {
        // It is modal, and the wheel is no exception: while a list is up,
        // scrolling is how a list is read. The brackets go nowhere at all,
        // like every other flight key.
        let args = args_for(&["--stars", "200", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        let mut paused = false;
        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);
        assert!(flight.menu_open());

        let first = flight.drawn_model().name;
        handle_mouse(wheel(MouseEventKind::ScrollDown), &mut flight);
        assert_ne!(flight.drawn_model().name, first, "the wheel moved nothing");
        assert_eq!(
            flight.zoom_target,
            view::ZOOM_DEFAULT,
            "the wheel zoomed the ship behind the dialogue"
        );

        handle_key(press(KeyCode::Char(']')), &mut flight, &args, &mut paused);
        assert_eq!(
            flight.zoom_target,
            view::ZOOM_DEFAULT,
            "the brackets got out"
        );
    }

    #[test]
    fn a_long_flight_stays_finite() {
        let args = args_for(&[
            "--seed", "3", "--stars", "800", "--size", "60x20", "--engage",
        ]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut autopilot = Autopilot::default();
        for frame in 0..3000 {
            autopilot.update(&mut flight.ship, frame as f64 / 60.0);
            flight.advance(1.0 / 60.0);
        }
        flight.draw(60.0, false, true);
        assert!(flight.ship.speed.is_finite() && flight.ship.distance_ly.is_finite());
        assert!(flight
            .renderer
            .pixels()
            .iter()
            .any(|p| p.iter().any(|v| *v > 0)));
    }
}
