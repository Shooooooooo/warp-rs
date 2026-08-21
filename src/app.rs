//! Running a flight: the loops that drive one, and the state they drive.
//!
//! Three of them, sharing a `Flight` and differing only in where the frames
//! end up — an interactive terminal, stdout, or a PNG.

use crate::autopilot::Autopilot;
use crate::cli::{self, resolved_size, Args};
use crate::hud::Readout;
use crate::menu::{self, Menu};
use crate::models::{self, ShipModel};
use crate::render::{Exterior, Renderer};
use crate::ship::{wrap_signed, Ship};
#[cfg(feature = "snapshot")]
use crate::snapshot;
use crate::term::RawGuard;
use crate::universe::Universe;
use crate::view::{self, Orbit, ViewMode};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::{terminal, QueueableCommand};
use std::io::{self, BufWriter, IsTerminal, Write};
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
/// How much fainter or brighter one press of `+` or `-` asks the sky to go.
///
/// Half a magnitude, which is about a factor of two in how many stars there
/// are: a visible change on a single tap and a couple of seconds of auto-repeat
/// across the whole range, the bargain the zoom's 1.12 and the orbit's turn in
/// sixty strike. It replaced a multiply of 1.25 on a literal pool, which had to
/// carry a floor of its own because zero and one are fixed points of a multiply
/// and the key would swallow the press. An addition has no fixed points, so the
/// floor is gone and the two ends of the flag are the two ends of the key —
/// which is the invariant `cli::MAX_STARS` used to hold, one door further back.
const MAGNITUDE_STEP: f32 = 0.5;

/// How much of a cut is spent going dark, as a fraction of `--fade`.
///
/// Held at both ends. Smaller and the outgoing picture is gone before the eye
/// has registered that anything happened, which is a hard switch with a flicker
/// on it rather than a cut. Larger and the shot sags — a picture that sits
/// there dying reads as a fault rather than as a change of camera. It also sets
/// what a *run* opens on, since the program starts at the bottom of a cut it
/// was never on the other side of: a fresh flight rises over the other
/// `1 - FADE_OUT_SHARE` of the fade, which at the default is the four tenths of
/// a second anybody actually looks at once a run.
const FADE_OUT_SHARE: f32 = 0.3;
/// Both ends of that fraction, as a build failure rather than a runtime one:
/// [`fade_t`] divides by `fade - fade * FADE_OUT_SHARE` on the rise, which is
/// nonzero for a positive fade exactly while this is strictly inside the unit
/// interval.
const _: () = assert!(
    FADE_OUT_SHARE > 0.0 && FADE_OUT_SHARE < 1.0,
    "a cut has to spend some of itself going down and some coming back"
);

/// Where a cut has got to, as the parameter the light is ramped from: 1.0 for a
/// settled shot, 0.0 at the bottom of the dip.
///
/// A closed form of the clock rather than a decaying transient like
/// [`Ship::flash`], and the reason is not the one it looks like. `exp(-k·dt)`
/// is frame-rate independent too — that is why everything in `ship.rs` eases
/// that way. What a closed form buys is what the autopilot's throttle bought
/// when it stopped stepping: independence that is *structural*, because nothing
/// is being stepped at all, and an end that is reached exactly rather than
/// approached. That second half is load-bearing. A gain a single ulp under one
/// would repaint the whole sky for the rest of the flight, where this returns a
/// literal 1.0 and `v * 1.0` is the identity.
///
/// `from` is where the fall starts, and it is a field rather than a constant
/// because a second cut can land while the first is still going down. Stamping
/// a fresh cut with `from` at one would slam the picture back to full
/// brightness and drop it again — a flash in the one feature whose whole job is
/// to look right, and auto-repeat on `C` reaches it constantly.
fn fade_t(elapsed: f32, fade: f32, from: f32) -> f32 {
    if fade <= 0.0 {
        return 1.0;
    }
    let out = fade * FADE_OUT_SHARE;
    if elapsed < out {
        from * (1.0 - elapsed / out)
    } else {
        ((elapsed - out) / (fade - out)).min(1.0)
    }
}

/// The shutter a frame is resolved through, from where the cut has got to.
///
/// Smoothstep for soft ends at both stops, then squared — and the square is
/// chosen against the tonemap's gamma of 2.2 rather than by eye. Perceived
/// level goes as the gain to the power of one over that, so a linear ramp on
/// the gain is already half-bright a fifth of the way through and the fade
/// reads as a step followed by a wait. Squaring the eased parameter puts the
/// level at very nearly the parameter itself, so half way through a cut in time
/// is half way through it in brightness. `e * e` rather than a `powf` of 2.2
/// because it is exact at both ends and costs no call; the tenth of an exponent
/// left over leaves the picture very slightly front-loaded, which is the right
/// way round for something arriving.
fn fade_gain(elapsed: f32, fade: f32, from: f32) -> f32 {
    let t = fade_t(elapsed, fade, from).clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    eased * eased
}

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
    /// The sky, and there is one of it. Both cameras look at the same volume
    /// from different places, which is what a universe is and what the two
    /// screen-space fields this replaced could never be: they were laid out in
    /// front of whichever eye was flying, so swinging the camera round the ship
    /// showed the sky it had already been showing.
    sky: Universe,
    renderer: Renderer,
    autopilot: Autopilot,
    /// Which camera is flying, which ship is being flown, and whether the
    /// picker is up over the top of it.
    view: ViewMode,
    /// Which camera the *picture* was on when the current cut was made, and
    /// where that cut is in its dip.
    ///
    /// `view` above is which camera the *flight* is on: it changes the instant
    /// `C` is pressed, so the stick, the wheel and the hint tiers all change
    /// hands at once and nothing has to know about a switch that has not landed
    /// yet. What this frame is drawn from is [`Flight::shown`], which holds
    /// `previous` until the bottom of the dip. The cut is made in the dark,
    /// where there is nothing on screen to change over.
    ///
    /// `cut` joins `time` below on the `f64` list and for the same reason: it
    /// tracks a clock that only grows, and a screensaver left up for a week
    /// would have an `f32` copy of it unable to resolve half a second at all.
    /// `from` and `fade` stay `f32` for the reason the zoom does — both are
    /// bounded and neither accumulates.
    previous: ViewMode,
    cut: f64,
    from: f32,
    /// How long a cut takes, straight off the command line. Carried here rather
    /// than read off `args` in [`Flight::draw`] so that function's signature
    /// never moves: `tests/flight.rs` and `examples/bench.rs` both call it.
    fade: f32,
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
    /// Whether a hand has been put on the camera since the flight opened.
    ///
    /// Only [`Flight::fly_itself`] reads it, and it reads it to stand off: an
    /// autopilot writing the orbit target every frame would leave the camera
    /// keys pressing against something that overwrote them before the next
    /// draw, and a control that swallows a press and gives nothing back is
    /// exactly what the split in `handle_key` exists to avoid. `R` clears it,
    /// so the autopilot can be handed the camera back. The ship stays on
    /// autopilot either way — `--demo` still flies itself and still stops.
    ///
    /// It cannot reach a reference frame: headless and snapshot flights take no
    /// input at all, so nothing there ever sets it.
    hands_on: bool,
    /// The number this flight's sky was built from.
    ///
    /// Kept because it is the only thing about a run that cannot be recovered
    /// by looking at it: `--seed` is optional and defaults to the clock, so a
    /// flight worth keeping a picture of is one nobody can shoot again. Held
    /// here rather than asked of [`Universe`] because it is a property of the
    /// flight rather than of the sky — the same seed would build the same sky
    /// at a different magnitude.
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
        let seed = seed(args);
        let sky = Universe::new(args.magnitude, seed);

        // The camera the shot opens on, set here rather than by a `set_view`
        // call after the fact — which is what this used to do, and which would
        // now arm a whole cut at `t = 0`, dipping `--view side` down from a
        // cockpit nobody asked to see.
        let view = args.view.resolve();
        Self {
            ship,
            sky,
            renderer,
            autopilot: Autopilot::default(),
            view,
            previous: view,
            // A shot opens at the bottom of a cut it was never on the other
            // side of: `time` starts at zero, so seating the cut exactly one
            // fall behind it puts the first frame on the trough and the sky
            // arrives over the rise. One field, one curve, and no special case
            // for the opening.
            cut: -((args.fade * FADE_OUT_SHARE) as f64),
            from: 1.0,
            fade: args.fade,
            model: args.ship,
            menu: None,
            zoom: view::ZOOM_DEFAULT,
            zoom_target: view::ZOOM_DEFAULT,
            orbit: args.orbit,
            orbit_target: args.orbit,
            hands_on: false,
            seed,
            time: 0.0,
            accumulator: 0.0,
        }
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
    pub fn cycle_view(&mut self) {
        self.set_view(self.view.next());
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
        self.hands_on = true;
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

    /// Put the stick over: the three impulses `WASD` and `QE` hand the ship
    /// from the pilot's seat, for a caller that has no keyboard.
    ///
    /// Public for the same reason [`Self::nudge_orbit`] and [`Self::nudge_zoom`]
    /// are, and it completes the set: `handle_key` reaches the ship only because
    /// it lives in this module, so until this existed nothing outside could fly
    /// a turn. Two things need to, and they need it more than they did: the
    /// autopilot used to weave, so a turn could be had by leaving a flight
    /// alone, and it flies straight now — this is the only thing in the tree
    /// that puts a stick over. `examples/bench.rs` measures what a hard turn
    /// costs to draw. And `tests/flight.rs` drives everything through the surface
    /// another program would have to use, so a turn it could not fly is a turn
    /// that surface cannot be said to support.
    ///
    /// Impulses rather than rates, because that is what the ship takes: see the
    /// note on [`crate::ship`] about terminals reporting presses and not
    /// releases, and the identity in `crate::autopilot` about scaling one by
    /// `dt`.
    pub fn nudge_stick(&mut self, yaw: f32, pitch: f32, roll: f32) {
        self.ship.nudge_yaw(yaw);
        self.ship.nudge_pitch(pitch);
        self.ship.nudge_roll(roll);
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
        self.hands_on = true;
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
        // And the autopilot has the camera again. `R` is the key for putting
        // the view back, and on an unattended flight the view it goes back to
        // is the one that was flying itself.
        self.hands_on = false;
    }

    /// Fly a frame with nobody at the controls: the ship, and the camera that
    /// is watching it.
    ///
    /// Both `--demo` and `--screensaver` come through here, and the camera half
    /// is the reason it exists. `Autopilot` knows nothing about a `Flight` —
    /// it flies a `Ship` and answers where a camera should be, and the joining
    /// up happens here, where the targets it writes are the same ones a key
    /// writes and the same ease in [`Flight::advance`] smooths both.
    ///
    /// The swing is *added* to what `--orbit` asked for rather than replacing
    /// it, so the flag still parks the shot and the autopilot walks out from
    /// there. Both views are driven, not just the one that can see it: the
    /// cockpit reads neither the orbit nor the zoom, so it costs nothing there
    /// and means cycling out to the camera mid-flight lands on a live shot
    /// rather than one that has been parked since launch.
    pub fn fly_itself(&mut self, args: &Args, elapsed: f64) {
        self.autopilot.update(&mut self.ship, elapsed);
        if self.hands_on {
            return;
        }
        let (swing, zoom) = self.autopilot.camera(elapsed);
        self.orbit_target = Orbit {
            azimuth: args.orbit.azimuth + swing.azimuth,
            elevation: args.orbit.elevation + swing.elevation,
            roll: args.orbit.roll + swing.roll,
        }
        .held();
        self.zoom_target = (view::ZOOM_DEFAULT * zoom).clamp(view::ZOOM_MIN, view::ZOOM_MAX);
    }

    /// Change camera, and cut to it through black.
    ///
    /// It used to build the outside view's sky here, the first time that view
    /// was asked for, from a seed of its own so the cockpit's stream stayed
    /// untouched. There is one sky now and it is already built, so a view is a
    /// question about where to stand and nothing else — which is also why the
    /// `C` key can no longer show two different universes.
    ///
    /// The whole of the cut is here because this is the only funnel: `C` comes
    /// through [`Self::cycle_view`] and `M` through [`Self::open_menu`], which
    /// forces the outside view. The early return is what keeps `M` pressed
    /// while already outside from dipping the picture for a dialogue that is
    /// only opening.
    ///
    /// `previous` is taken from [`Self::shown`] rather than from `self.view`,
    /// and the difference only shows when a second cut lands inside the first
    /// one's fall: what has to fade out is whatever is actually on screen, not
    /// a camera the flight had committed to and never drawn. Order is
    /// load-bearing too — both `shown` and `fade_t` read the *old* `cut`, so
    /// they have to be asked before it is overwritten.
    fn set_view(&mut self, view: ViewMode) {
        if view == self.view {
            return;
        }
        self.previous = self.shown();
        self.from = fade_t(self.since_cut(), self.fade, self.from);
        self.cut = self.time;
        self.view = view;
    }

    /// How long the current cut has been running, in flight seconds.
    ///
    /// Subtracted in `f64` and only the small difference narrowed, which is the
    /// same care [`Renderer::camera`] takes with the shake: the two terms are a
    /// screensaver's whole uptime apart from each other and their difference is
    /// under a second.
    fn since_cut(&self) -> f32 {
        (self.time - self.cut) as f32
    }

    /// Which camera *this frame* is built from, as against [`Self::view`],
    /// which is the one the flight is being flown on. They differ only over the
    /// fall of a cut.
    fn shown(&self) -> ViewMode {
        if self.since_cut() < self.fade * FADE_OUT_SHARE {
            self.previous
        } else {
            self.view
        }
    }

    /// The shutter this frame is resolved through.
    fn gain(&self) -> f32 {
        fade_gain(self.since_cut(), self.fade, self.from)
    }

    /// Land whatever cut is in flight, at once.
    ///
    /// `P` stops [`Self::advance`], which is the only thing that moves `time`,
    /// so a cut made while the flight is stopped never gets anywhere: it sits
    /// at zero elapsed, where the gain is exactly one and `shown` is still the
    /// old camera. Pressing `C` while paused would therefore hold the outgoing
    /// view at full brightness and look like a key that does nothing at all,
    /// which is the thing `handle_key`'s whole split over which control belongs
    /// to which view exists to avoid.
    ///
    /// So: there is nothing to dissolve between while nothing is moving, and a
    /// cut made with the flight stopped lands where it was going. The nearest
    /// precedent is `R`, which snaps the orbit and the zoom rather than easing
    /// them, because a key pressed when the view has got away from you wants an
    /// answer now. This owes nothing on the way out either, unlike the camera
    /// bug `P` used to have: `from` goes back to one and the picture is
    /// settled, so unpausing resumes an ordinary flight rather than replaying a
    /// stored dip.
    ///
    /// A whole second past the end rather than exactly the end, because
    /// `time - fade` and then the difference back again do not round-trip to
    /// `fade` at large `time`, and a gain one ulp under one is a gain that
    /// repaints the frame.
    fn land_cut(&mut self) {
        self.cut = self.time - self.fade as f64 - 1.0;
        self.from = 1.0;
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Put the picker up, and go outside to look at what it is offering: a list
    /// of ship names is a poor way to choose a ship.
    pub fn open_menu(&mut self) {
        self.menu = Some(Menu::new(self.model));
        self.set_view(ViewMode::Side);
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
            // When there is nothing to catch up with the difference is exactly
            // zero and so is the whole term, so this cannot move a reference
            // frame by an ulp. That covers the four flights in the reference
            // that carry no autopilot — it stopped covering the rest of them
            // when the autopilot started flying the camera, which is what
            // `drift.txt` is recorded to watch.
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
            // whose camera is neither being flown by a hand nor by the
            // autopilot still adds nothing at all here.
            let ease = 1.0 - (-view::ORBIT_EASE * SIM_STEP).exp();
            let chase = |from: f32, to: f32| from + wrap_signed(to - from) * ease;
            self.orbit = Orbit {
                azimuth: chase(self.orbit.azimuth, self.orbit_target.azimuth),
                elevation: chase(self.orbit.elevation, self.orbit_target.elevation),
                roll: chase(self.orbit.roll, self.orbit_target.roll),
            }
            .held();
            // One sky, and it is stepped whichever camera is flying — there
            // is no longer a second one to be kept warm or skipped. What the
            // step does is take away whatever has fallen past the limit and
            // put it back where the flow is bringing stars in from; the ship
            // does all the moving, in `Ship::coast`.
            self.sky.advance(
                self.ship.position,
                self.ship.axes,
                SIM_STEP,
                self.ship.warp_intensity(),
                self.ship.velocity_ly_per_s(),
            );
            self.accumulator -= SIM_STEP;
        }
    }

    /// Build a frame, and put an instrument panel over it if `panel` says so.
    ///
    /// `panel` is off exactly when the flight is flying itself, which the three
    /// loops each ask `Args::unattended` and hand down. It is a parameter
    /// rather than something read off `args` in here so a caller driving a
    /// flight from outside the binary can have either — `tests/flight.rs` flies
    /// the autopilot and reads the warp banner off the result.
    ///
    /// The ship picker is deliberately *not* covered by it. A panel is chrome
    /// that arrives unbidden; the picker is a dialogue somebody pressed `M`
    /// for, and hiding one of those would leave the key swallowing a press and
    /// giving nothing back, which is the thing `handle_key`'s whole split over
    /// which view a control belongs to exists to avoid.
    ///
    /// `paused` does one thing beyond the readout, and a caller driving a
    /// flight from outside the binary should know it: it lands any camera cut
    /// that is in flight, at once. A cut dips the picture through black as a
    /// function of the flight's own clock, and a pause is what stops that
    /// clock — so a cut begun with the flight stopped would otherwise sit
    /// where it was armed, showing the outgoing camera, until the flight was
    /// let go again.
    pub fn draw(&mut self, fps: f32, paused: bool, panel: bool) {
        // A cut cannot get anywhere with the clock stopped, so it lands here
        // instead — see [`Self::land_cut`]. This is the only function in the
        // tree handed both the flight and whether it is stopped, which is why
        // it covers `C` while paused, `M` while paused, and a `P` taken in the
        // middle of a cut, all in one line.
        if paused {
            self.land_cut();
        }
        let gain = self.gain();
        let model = self.drawn_model();
        let readout = Readout {
            ship: &self.ship,
            fps,
            magnitude: self.magnitude(),
            paused,
            panel,
            // The camera the flight is on rather than the one on screen: the
            // hint tiers name keys, and over the fall of a cut the keys have
            // already changed hands. The `SHIP` row goes the same way, since it
            // belongs to whoever is flying rather than to what is drawn.
            view: self.view,
            model: model.name,
        };
        match self.shown() {
            ViewMode::Side => {
                let cam = self.renderer.exterior_camera(&self.ship, self.time);
                let scene = Exterior {
                    sky: &self.sky,
                    ship: &self.ship,
                    model,
                    time: self.time,
                    zoom: self.zoom,
                    orbit: self.orbit,
                };
                self.renderer.render_exterior(scene, &cam, gain, &readout);
            }
            ViewMode::Cockpit => {
                let cam = self.renderer.camera(&self.ship, self.time);
                self.renderer
                    .render(&self.sky, &self.ship, &cam, self.time, gain, &readout);
            }
        }
        // Over the top of everything, panel included: it is a dialogue, and it
        // is drawn here rather than inside the renderer so the pinned cockpit
        // path stays exactly as it was.
        if let Some(menu) = &self.menu {
            menu::draw(self.renderer.screen(), menu);
        }
    }

    /// How many stars are in flight — over the whole sphere, not on screen.
    ///
    /// It no longer depends on which camera is flying, because there is no
    /// longer a sky per camera.
    pub fn stars(&self) -> usize {
        self.sky.len()
    }

    /// The faintest star this sky holds.
    pub fn magnitude(&self) -> f32 {
        self.sky.limit()
    }

    /// The number this flight's sky was built from, whether it came from
    /// `--seed` or from the clock.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Ask the sky for a fainter or a brighter limit.
    ///
    /// Held to the two ends `--magnitude` is held to, so a sky these keys walk
    /// to can never be one the command line would have refused. That invariant
    /// is older than the flag it now guards: the `+` key used to carry a
    /// ceiling of its own, 20 000, which sat *under* what `--stars` allowed, so
    /// `--stars 100000` and a single press shrank the pool by four fifths.
    ///
    /// An empty sky used to need an early return here and no longer does. It
    /// was a fixed point of the old multiply — `-` would clamp a deliberately
    /// empty pool up to a floor and `+` could never climb off zero — and an
    /// addition has neither problem, so a sky can be emptied and refilled by
    /// the same two keys that set it.
    fn step_magnitude(&mut self, delta: f32) {
        let limit = (self.sky.limit() + delta).clamp(cli::MIN_MAGNITUDE, cli::MAX_MAGNITUDE);
        self.sky.set_limit(limit);
    }

    /// Write the last drawn frame out as a self-contained block of text.
    pub fn present_plain(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.renderer.present_plain(out)
    }

    /// Adapt to a new terminal size. Reports whether the canvas actually moved,
    /// so the caller can skip the repaint a resize otherwise forces.
    pub fn resize(&mut self, args: &Args, cols: usize, rows: usize) -> bool {
        // `--size` is a fixed size, not a starting point. Without this the flag
        // held only until the first resize event, after which the terminal
        // silently won.
        if args.size.is_some() {
            return false;
        }
        // Through the same gate `--size` goes through, and here rather than at
        // the one call site for the reason `advance` clamps its own `dt`: this
        // is `pub`, and `lib.rs` offers a flight to any program that cares to
        // fly one, so the guard belongs with the allocation it protects.
        //
        // It used to bound nothing but zero, which left the only unvetted door
        // into the canvas wide open. `cli::resolved_size` runs an ioctl answer
        // through `clamp_size` on the argument that "a terminal that claims to
        // be enormous should not be believed to the point of exhausting
        // memory over it" — and then every *later* answer, arriving as
        // `Event::Resize`, went straight through. A `stty cols 30000 rows
        // 30000` against the pane a flight is running in is 9e8 cells against
        // a `MAX_CELLS` of two million, and a failed allocation aborts the
        // process outright: no unwind, no `Drop`, no panic hook, so `RawGuard`
        // never restores and the user is dropped back into a shell in raw mode
        // on the alternate screen with no cursor and no prompt. That is the
        // exact outcome `Flight::new` is built before `RawGuard::new` to
        // prevent, reached through the other end of the same program.
        let (cols, rows) = cli::clamp_size(
            cols.clamp(1, u16::MAX as usize) as u16,
            rows.clamp(1, u16::MAX as usize) as u16,
        );
        let (cols, rows) = (cols as usize, rows as usize);
        if self.renderer.screen().dims() == (cols, rows) {
            return false;
        }

        self.renderer.resize(cols, rows);
        // And the sky is not told. It is laid out in the world rather than in
        // front of the camera, so a wider window is more of the same sky rather
        // than a differently-shaped pool — which is what the two `retarget`
        // calls that used to stand here existed to paper over, each of them
        // dropping every trail in the field to hide the seam.
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
    //
    // One exhaustive `match` rather than three `==` comparisons, and the
    // difference is a compile error rather than a style. A third camera added
    // to `ViewMode::ALL` used to answer all three of these with false, and a
    // key matching none of them falls through to nothing at all — so the new
    // view would arrive with the stick, the camera and the zoom silently
    // disconnected and the compiler perfectly happy. Now it does not build
    // until somebody has said what the stick does in it.
    let (steers, flies_the_camera, zooms) = match flight.view() {
        ViewMode::Cockpit => (true, false, false),
        ViewMode::Side => (false, true, true),
    };
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
    // rolls the ship. The zoom runs the same way and always has: there is no
    // ship to be bigger or smaller from inside one, so it is connected exactly
    // where it has something to show.
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
        KeyCode::Char('+' | '=') => flight.step_magnitude(MAGNITUDE_STEP),
        KeyCode::Char('-' | '_') => flight.step_magnitude(-MAGNITUDE_STEP),

        // The outside camera, in and out. Shifted as well as plain for the
        // same reason `+` is paired with `=` and `-` with `_`: which of the
        // two arrives depends on the keyboard, not on what was meant.
        KeyCode::Char('[' | '{') if zooms => flight.nudge_zoom(-1.0),
        KeyCode::Char(']' | '}') if zooms => flight.nudge_zoom(1.0),

        // The camera, and the hangar.
        KeyCode::Char('c' | 'C') => flight.cycle_view(),
        KeyCode::Char('m' | 'M') => flight.open_menu(),
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
    // Refused by name, the way `--stars` and `--color auto` are refused, and
    // for the same reason: the mistake is worth telling somebody about.
    //
    // Without a terminal this is where the flight ended anyway, but it ended
    // as `warp: No such device or address (os error 6)` — `RawGuard::new`'s
    // ioctl, travelling out through `main` — on a command line that answers
    // every other absurd argument with a sentence naming the limit. And with
    // a tty on stdin but not on stdout it did something worse than fail:
    // crossterm takes raw mode against the tty while every frame goes to
    // stdout, so `warp > frames.txt` turned the terminal's echo and line
    // editing off, never entered the alternate screen — that escape went into
    // the file — and left the user looking at a blank shell that appears
    // hung. `Esc` still quits it, which nothing on screen says.
    //
    // A fact read off a file descriptor rather than a capability guessed at
    // from the environment, which is the distinction `ColorMode::detect` was
    // deleted over: this decides whether to take the terminal over at all, not
    // how to spell a colour at one. `--headless` and `--snapshot` never reach
    // here, so the two modes that legitimately have no terminal are untouched.
    if !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no terminal on stdout; use --headless to write frames somewhere else",
        ));
    }

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
    // The last size the *terminal* reported, which is not the same question as
    // the last size the canvas took — see the resize arm below.
    let mut last_seen = (cols, rows);
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
        // What the simulation is stepped by, and what the readout is smoothed
        // from. Nothing else reads it: the autopilot used to, because the
        // stick it worked was impulse-driven and had to be scaled by the frame
        // it was pressed over, and it works no stick now. Note that the rate
        // this loop actually runs at is not `--fps`, which is only a cap and is
        // abandoned outright while something is being typed.
        let dt = (frame_start - last).as_secs_f32().clamp(0.0, MAX_FRAME_DT);
        last = frame_start;
        // Smoothed so the readout is legible rather than flickering.
        fps += (1.0 / dt.max(1e-4) - fps) * 0.08;

        if args.unattended() {
            // The flight's own clock rather than the wall's, and that is now
            // the whole of what a pause needs from here.
            //
            // It took two corrections to arrive at, and the first one has
            // since gone away rather than been kept: a zero step used to be
            // passed alongside, because the stick was an impulse against a
            // damper and the damper lives in `advance`, which a pause stops —
            // so a pause that did not stop the impulse ratcheted the rate with
            // nothing bleeding it off. Eleven seconds of `P` pinned the yaw at
            // `MAX_YAW_RATE` and snapped the ship into a turn on resuming. The
            // autopilot touches no impulse now, so there is no step to zero.
            //
            // The clock is the correction that still holds it up. It was left
            // on wall time on the argument that the throttle and the camera
            // are closed forms of it and so are unaffected either way. True of
            // the throttle, and not true of the camera: what the closed form
            // gives is the orbit *target*, and the camera a frame is built
            // from is `Flight::orbit`, which eases toward it in `advance` —
            // which the pause also stops. So the target walked and the camera
            // did not. `autopilot::CAMERA_TURN` is a turn every 43 seconds, so
            // ten seconds of `P` opened an eighty-four degree gap, and the
            // ease closes 7.2% of one per sim step: the camera whipped most of
            // the way round the hull inside a second of resuming. It saturates
            // at half a turn after about twenty-one seconds, which is the
            // worst it can be.
            //
            // `Flight::time` is the clock that already stops — `advance` is
            // the only thing that advances it — so handing it over costs
            // nothing and needs no second accumulator. The `--demo` deadline
            // above goes on reading the wall, which is the documented
            // behaviour: a paused demo still exits when it said it would.
            flight.fly_itself(args, flight.time);
        }

        if !paused {
            flight.advance(dt);
        }
        flight.draw(fps, paused, !args.unattended());
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
                    // Two questions, and one boolean used to answer both. What
                    // `Flight::resize` reports is whether the *canvas* moved,
                    // and under `--size` it always answers no — which is right
                    // about the canvas and wrong about the repaint, because
                    // the terminal moved whatever the canvas did. The
                    // alternate screen discards the rows past a new smaller
                    // height, `Screen::front` goes on holding the cells it drew
                    // there, and `diff` re-emits nothing: a band of the frame
                    // stays blank until each cell in it happens to take a
                    // different value, which on a starfield is mostly never.
                    //
                    // So the repaint follows the size the *terminal* last
                    // reported, and the canvas follows `resize`. The reason the
                    // gate is here at all survives intact: terminals emit
                    // resize events that settle on the size already in use, and
                    // one of those still reports the same pair and still
                    // repaints nothing.
                    let moved = (cols, rows) != last_seen;
                    last_seen = (cols, rows);
                    flight.resize(args, cols as usize, rows as usize);
                    if moved {
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
        if args.unattended() {
            flight.fly_itself(args, frame as f64 * dt as f64);
        }
        flight.advance(dt);
        flight.draw(args.fps as f32, false, !args.unattended());
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

/// The canvas a snapshot falls back to when `--size` is not given.
///
/// Named rather than written into `run_snapshot`, because the two recipes in
/// CLAUDE.md that shoot the README's front page both depend on it and neither
/// passes `--size`: a cell is two subpixels tall, so both come out 480x272 at
/// `--scale 2` and the pair stacks with its edges in line. The hero used to ask
/// for `--size 220x60`, came out 440x240, and put a forty-pixel step down the
/// side of the page that nobody had written down a reason for. A reshoot that
/// reaches for `--size` puts it back, and nothing but this constant and the
/// test beside it says so.
#[cfg(feature = "snapshot")]
pub const SNAPSHOT_SIZE: (u16, u16) = (240, 68);

#[cfg(feature = "snapshot")]
fn run_snapshot(args: &Args, path: &std::path::Path) -> io::Result<()> {
    let (cols, rows) = args.size.unwrap_or(SNAPSHOT_SIZE);
    let mut flight = Flight::new(args, cols as usize, rows as usize);
    let dt = 1.0 / args.fps as f32;

    for frame in 0..args.warmup {
        if args.unattended() {
            flight.fly_itself(args, frame as f64 * dt as f64);
        }
        flight.advance(dt);
    }
    flight.draw(args.fps as f32, false, !args.unattended());

    let (w, h) = flight.renderer.canvas_dims();
    snapshot::write_png(path, flight.renderer.pixels(), w, h, args.scale)?;
    // The seed is in here because it is the one thing about the shot that
    // cannot be read back off the picture. `--seed` is optional and falls back
    // to the clock, so without this a frame worth keeping is a frame nobody
    // can take again — including the two on the README's front page, whose
    // recipes are written down in `CLAUDE.md` precisely because they were once
    // recoverable only from the commit that shot them.
    eprintln!(
        "wrote {} ({}x{} px) at velocity {:.1} c, --seed {}",
        path.display(),
        w * args.scale,
        h * args.scale,
        flight.ship.velocity_c(),
        flight.seed()
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

    /// A flight already outside, which is where the zoom lives — and settled,
    /// with the cut that `c` armed landed and the shot opened.
    ///
    /// Landed rather than flown through, because every test built on this is
    /// about something else: a flight left inside its opening fade draws dim
    /// frames, and two dim frames agree about a great deal more than two lit
    /// ones do.
    fn outside(args: &Args) -> Flight {
        let mut flight = Flight::new(args, 80, 24);
        let mut paused = false;
        handle_key(press(KeyCode::Char('c')), &mut flight, args, &mut paused);
        assert_eq!(flight.view(), ViewMode::Side);
        flight.land_cut();
        flight
    }

    #[test]
    fn a_flight_that_has_been_up_for_days_still_advances() {
        // Regression: `time` was an `f32` accumulator. At 1/60 s steps it
        // stopped moving entirely at t = 524288 s — a little over six days —
        // taking the twinkle and the shake with it, which is precisely the
        // situation a tmux screensaver is left in.
        let args = args_for(&["--seed", "3", "--magnitude", "4"]);
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
        let args = args_for(&["--seed", "3", "--magnitude", "4", "--size", "60x20"]);
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
        let args = args_for(&["--magnitude", "3", "--size", "40x12"]);
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
        // The point of asking for a sky by its limiting magnitude, stated as
        // the property it is for. The pool used to be the canvas area times a
        // density, so these two windows opened on 19 stars and 1080 and no two
        // terminals flew the same sky; then it was a fixed count, which held
        // that line by giving up on saying anything about what a window shows.
        // A limit says both at once: the same universe on every terminal, and
        // as much of it on screen as the window can see.
        for (cols, rows) in [(40usize, 12usize), (300, 90)] {
            let size = format!("{cols}x{rows}");
            let args = args_for(&["--size", &size]);
            let flight = Flight::new(&args, cols, rows);
            assert_eq!(
                flight.stars(),
                Universe::population(cli::DEFAULT_MAGNITUDE),
                "a {cols}x{rows} window opened on its own sky"
            );
        }

        // And there is only one of it. The outside view used to build a second
        // pool of its own, from a seed of its own, the first time that camera
        // was asked for — so `C` really did show two different universes.
        let args = args_for(&["--size", "40x12"]);
        let mut outside = Flight::new(&args, 40, 12);
        let inside = outside.stars();
        outside.set_view(ViewMode::Side);
        assert_eq!(
            outside.stars(),
            inside,
            "the two views hold different skies"
        );

        let explicit = args_for(&["--magnitude", "5.0"]);
        assert_eq!(
            Flight::new(&explicit, 300, 90).stars(),
            Universe::population(5.0)
        );
    }

    #[test]
    fn an_empty_sky_is_a_limit_like_any_other() {
        // There is a magnitude below which the whole celestial sphere holds no
        // star at all, and asking for it is the way to see the tunnel, the
        // bubble and the hull with nothing streaming past them — which is what
        // `--stars 0` used to be for. Flown in both views because they used to
        // be separate pools reached by separate paths and the empty one is
        // exactly where that showed.
        let args = args_for(&["--magnitude", "-2", "--size", "60x20"]);
        let mut paused = false;
        let mut flight = Flight::new(&args, 60, 20);

        for view in [ViewMode::Cockpit, ViewMode::Side] {
            flight.set_view(view);
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
            }
            assert_eq!(flight.stars(), 0, "{view:?} spawned a sky from nowhere");
            flight.renderer.present(&mut Vec::new()).unwrap();
        }

        // And the two keys are each other's inverse here, where they used not
        // to be. A multiply on a literal pool has zero for a fixed point, so
        // `+` could never climb off an empty sky and `-` had to be stopped from
        // clamping one up to a floor; an addition on a magnitude has neither
        // problem and neither special case.
        flight.set_view(ViewMode::Cockpit);
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert_eq!(flight.stars(), 0, "`-` put stars into an empty sky");
        for _ in 0..14 {
            handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        }
        assert!(
            flight.stars() > 0,
            "`+` cannot fill a sky it was able to empty"
        );
    }

    #[test]
    fn the_magnitude_keys_move_the_limit_the_way_they_point() {
        // Regression, twice over. The keys used to scale a literal pool and
        // clamped up to a floor of 64 chosen under an automatic minimum of 300,
        // so on a thin window `-` *added* stars and both keys landed on the
        // same number; and `+` carried a ceiling of 20 000 that sat under what
        // `--stars` already allowed, so one press could shrink a pool the flag
        // had asked for by four fifths. Both ends are the flag's ends now.
        let args = args_for(&["--size", "40x12"]);
        let mut paused = false;

        let mut flight = Flight::new(&args, 40, 12);
        let opened = flight.magnitude();
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert!(
            flight.magnitude() < opened && flight.stars() < Universe::population(opened),
            "`-` took a sky of {opened} to {}",
            flight.magnitude()
        );

        let mut flight = Flight::new(&args, 40, 12);
        let opened = flight.magnitude();
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(
            flight.magnitude() > opened && flight.stars() > Universe::population(opened),
            "`+` left the sky at {opened}"
        );

        // Both ends are reached rather than approached, and they are the two
        // the command line is held to — so a sky these keys walk to can never
        // be one the flag would have refused.
        for (key, want) in [('-', cli::MIN_MAGNITUDE), ('+', cli::MAX_MAGNITUDE)] {
            let mut flight = Flight::new(&args, 40, 12);
            for _ in 0..200 {
                handle_key(press(KeyCode::Char(key)), &mut flight, &args, &mut paused);
            }
            assert_eq!(
                flight.magnitude(),
                want,
                "`{key}` leaned on walked the limit to {}",
                flight.magnitude()
            );
        }
    }

    #[test]
    fn seeds_are_honoured_and_otherwise_invented() {
        assert_eq!(seed(&args_for(&["--seed", "99"])), 99);
        let a = seed(&args_for(&[]));
        assert_ne!(a, 0, "a time-derived seed should not be degenerate");
    }

    #[test]
    fn the_same_seed_produces_the_same_flight() {
        // `--fade 0` pinned rather than left to the default, the way
        // `tests/flight.rs` pins its colour mode: thirty frames is half a
        // second and the shot opens over most of it, so what these two would
        // otherwise be comparing is mostly how dark the opening is. The same
        // note covers the sibling below.
        let render = || {
            let args = args_for(&[
                "--seed",
                "7",
                "--magnitude",
                "4.5",
                "--size",
                "40x12",
                "--fade",
                "0",
            ]);
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
            let args = args_for(&[
                "--seed",
                seed,
                "--magnitude",
                "4.5",
                "--size",
                "40x12",
                "--fade",
                "0",
            ]);
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
    fn the_help_names_every_key_that_does_something() {
        // The hint line is chosen by width, so on the eighty-column terminal
        // most people have it names four keys of a dozen: no camera, no picker,
        // no pause, no reset, no zoom, and `+`/`-` on no tier at any width.
        // Under `MIN_COLS` there is not even a printed way to quit. Widening
        // the tiers costs columns the narrow ones do not have and repaints all
        // ten reference frames, so the full list lives in `--help`, where it
        // costs nothing — and this is what keeps it in step with the keyboard.
        //
        // Each key is *driven* as well as looked up, so the list cannot quietly
        // become a description of controls that no longer exist: a key that
        // moves nothing fails here just as loudly as one the help forgot.
        use clap::CommandFactory;
        let help = cli::Args::command().render_help().to_string();
        // The key *column* of the controls block, not the help at large: a
        // single letter turns up all over a page that also spells `--magnitude
        // <MAG>`, so `contains` answers yes to keys the block never names. It
        // did, and this test passed with a whole line deleted before the
        // lookup was narrowed to the column the keys are printed in.
        let block = help.split("Controls:").nth(1).unwrap_or("");
        let names = |name: &str| {
            block.lines().any(|line| {
                let line = line.trim_start();
                let column = line.split("  ").next().unwrap_or(line);
                column
                    .split_whitespace()
                    .any(|token| token.trim_end_matches(',') == name)
            })
        };

        let args = args_for(&["--view", "side", "--magnitude", "3.5", "--size", "60x20"]);
        let keys: [(KeyCode, &str); 12] = [
            (KeyCode::Char(' '), "SPACE"),
            (KeyCode::Up, "UP"),
            (KeyCode::Down, "DOWN"),
            (KeyCode::Char('c'), "C"),
            (KeyCode::Char('+'), "+"),
            (KeyCode::Char('-'), "-"),
            (KeyCode::Char('p'), "P"),
            (KeyCode::Char('r'), "R"),
            (KeyCode::Char('m'), "M"),
            (KeyCode::Char('w'), "W"),
            (KeyCode::Char('q'), "Q"),
            (KeyCode::Char('['), "["),
        ];
        for (code, named) in keys {
            assert!(
                names(named),
                "the help does not name {named}, which is a key that flies something"
            );
            let mut flight = Flight::new(&args, 60, 20);
            let mut paused = false;
            // Flown out of the opening fade before anything is sampled, or the
            // question below stops being asked: a shot opens out of black, so
            // the frame at the press and the frame a step later differ by the
            // fade alone whatever key was pressed — and at the very first step
            // they can both be black instead. Taken off the flag rather than
            // written out, so moving the default cannot make this vacuous
            // again.
            while flight.time < args.fade as f64 {
                flight.advance(1.0 / 60.0);
            }
            let before = frame_of(&mut flight);
            handle_key(press(code), &mut flight, &args, &mut paused);
            flight.advance(1.0 / 60.0);
            assert!(
                paused || frame_of(&mut flight) != before,
                "the help names {named} and pressing it changed nothing"
            );
        }
        // And the two that end a flight, which cannot be driven the same way
        // because what they change is that there is no next frame.
        for named in ["ESC", "Ctrl-C"] {
            assert!(names(named), "the help does not name {named}");
        }
    }

    /// What one frame of a flight comes out as, for tests that ask whether a
    /// key moved anything at all.
    fn frame_of(flight: &mut Flight) -> Vec<u8> {
        flight.draw(60.0, false, true);
        let mut out = Vec::new();
        let _ = flight.present_plain(&mut out);
        out
    }

    #[test]
    fn keys_do_what_they_say() {
        let args = args_for(&["--magnitude", "3.5", "--size", "40x12"]);
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

        let stars = flight.stars();
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(flight.stars() > stars);
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert!(flight.stars() < stars * 2);

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
        let args = args_for(&["--magnitude", "3.5", "--size", "40x12"]);
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
        let args = args_for(&["--magnitude", "4", "--size", "60x20"]);
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
        assert!(flight.ship.roll.is_finite() && !flight.sky.is_empty());
    }

    #[test]
    fn c_cycles_the_camera_and_ctrl_c_still_quits() {
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
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
    fn a_cut_is_over_and_exactly_over_when_its_seconds_are_up() {
        // Every claim here is an equality rather than a tolerance, because what
        // rests on them is a bitwise one: the shutter multiplies the linear
        // light on its way to eight bits, and `v * 1.0` is the identity — so a
        // settled frame is byte for byte the frame the fade is not there for. A
        // gain a single ulp under one would repaint the whole sky for the rest
        // of a flight instead, and no reference hash would say which change did
        // it.
        let fade = cli::DEFAULT_FADE;
        let out = fade * FADE_OUT_SHARE;
        assert_eq!(fade_gain(0.0, fade, 1.0), 1.0, "a cut steps at the press");
        assert_eq!(
            fade_gain(out, fade, 1.0),
            0.0,
            "the bottom of the dip is lit"
        );
        assert_eq!(
            fade_gain(fade, fade, 1.0),
            1.0,
            "a cut is not over when it says"
        );
        assert_eq!(
            fade_gain(1e6, fade, 1.0),
            1.0,
            "a cut came back after a week"
        );

        // And no fade at all is the renderer this arrived on, at every instant
        // and from wherever a fall happened to have got to.
        for elapsed in [0.0, 0.001, 1.0, 1e9] {
            for from in [0.0, 0.5, 1.0] {
                assert_eq!(fade_gain(elapsed, 0.0, from), 1.0, "`--fade 0` faded");
            }
        }

        // Down, then up, and never past fully open.
        let steps = 600;
        let mut previous = fade_gain(0.0, fade, 1.0);
        for i in 1..=steps {
            let elapsed = out * i as f32 / steps as f32;
            let now = fade_gain(elapsed, fade, 1.0);
            assert!(now <= previous, "the fall went back up at {elapsed}");
            previous = now;
        }
        previous = fade_gain(out, fade, 1.0);
        for i in 1..=steps {
            let elapsed = out + (fade - out) * i as f32 / steps as f32;
            let now = fade_gain(elapsed, fade, 1.0);
            assert!(now >= previous, "the rise went back down at {elapsed}");
            assert!(now <= 1.0, "the shutter opened past full at {elapsed}");
            previous = now;
        }
    }

    #[test]
    fn the_camera_changes_hands_at_the_press_and_the_picture_in_the_dark() {
        // The whole of the design in one test. `C` hands the stick, the wheel
        // and the hint tiers over at once, so nothing in `handle_key` has to
        // know about a switch that has not landed; what waits is the picture,
        // and it changes over at the bottom of the dip, where there is nothing
        // on screen to change over.
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
        }

        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        assert_eq!(
            flight.view(),
            ViewMode::Side,
            "the flight did not change camera"
        );
        assert_eq!(
            flight.shown(),
            ViewMode::Cockpit,
            "the picture changed over before there was any dark to change it in"
        );
        while flight.since_cut() < args.fade * FADE_OUT_SHARE {
            assert_eq!(flight.shown(), ViewMode::Cockpit, "the picture cut early");
            flight.advance(1.0 / 60.0);
        }
        assert_eq!(
            flight.shown(),
            ViewMode::Side,
            "the picture never changed over"
        );
    }

    #[test]
    fn the_picture_dips_through_black_and_comes_back_when_the_camera_changes() {
        let args = args_for(&["--seed", "11", "--magnitude", "4.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
        }
        let light = |flight: &Flight| -> u64 {
            flight
                .renderer
                .pixels()
                .iter()
                .map(|p| p.iter().map(|v| *v as u64).sum::<u64>())
                .sum()
        };

        flight.draw(60.0, false, false);
        let before: Vec<[u8; 3]> = flight.renderer.pixels().to_vec();
        let lit = light(&flight);
        assert!(lit > 0, "nothing was lit, so there was nothing to dip");

        // A cut does not step. The frame at the press is the frame before it,
        // bit for bit — which is what makes a fall a fall rather than a jump
        // followed by one.
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        flight.draw(60.0, false, false);
        assert_eq!(
            flight.renderer.pixels(),
            &before[..],
            "the picture jumped at the press instead of starting to go"
        );

        let mut darkest = lit;
        while flight.since_cut() < args.fade {
            flight.advance(1.0 / 60.0);
            flight.draw(60.0, false, false);
            darkest = darkest.min(light(&flight));
        }
        assert!(
            darkest * 50 < lit,
            "the cut only got down to {darkest} against {lit}, which is a wobble \
             rather than a dip"
        );
        assert_eq!(flight.gain(), 1.0, "the cut never finished");
        assert!(light(&flight) > 0, "the picture never came back");
    }

    #[test]
    fn a_second_cut_fades_out_what_is_actually_on_screen() {
        // Two claims, and each masks the other if it is the only one made, so
        // both are here. Auto-repeat means a second cut landing inside the
        // first one's fall is the ordinary case rather than the awkward one.
        //
        // Measured to fail on each of the two things it guards: `previous`
        // taken from `view` rather than from `shown` hard-cuts the picture to a
        // camera nothing has drawn, and `from` dropped slams the shutter back
        // open and drops it again — a flash in the one feature whose whole job
        // is to look right.
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
        }

        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        for _ in 0..4 {
            flight.advance(1.0 / 60.0);
        }
        let falling = flight.gain();
        assert!(
            falling > 0.0 && falling < 1.0,
            "the first cut was not still on its way down: {falling}"
        );

        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        assert_eq!(
            flight.view(),
            ViewMode::Cockpit,
            "two presses should come round"
        );
        assert_eq!(
            flight.shown(),
            ViewMode::Cockpit,
            "the second cut faded out a camera that had never been drawn"
        );
        assert_eq!(
            flight.gain(),
            falling,
            "the second cut opened the shutter it should have gone on closing"
        );
        for _ in 0..4 {
            flight.advance(1.0 / 60.0);
            assert!(
                flight.gain() <= falling,
                "the picture brightened inside a fall"
            );
        }
    }

    #[test]
    fn a_cut_made_while_the_flight_is_stopped_is_a_hard_cut() {
        // `P` stops the only thing that moves `time`, and the dip is a function
        // of it — so without landing the cut it never gets anywhere: it sits at
        // zero elapsed, where the shutter is fully open and the picture is
        // still the outgoing camera. `C` would look like a key that does
        // nothing whatever.
        let args = args_for(&["--seed", "11", "--magnitude", "4.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
        }

        handle_key(press(KeyCode::Char('p')), &mut flight, &args, &mut paused);
        assert!(paused, "`P` did not stop the flight");
        handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
        flight.draw(60.0, paused, true);
        assert_eq!(
            flight.shown(),
            ViewMode::Side,
            "a cut made with the clock stopped is still waiting for it"
        );
        assert_eq!(
            flight.gain(),
            1.0,
            "a paused cut left the shutter part shut"
        );
        assert!(
            flight
                .renderer
                .pixels()
                .iter()
                .any(|p| p.iter().any(|v| *v > 0)),
            "the frame a paused cut landed on came out black"
        );

        // And it owes nothing on the way out, unlike the camera bug `P` used to
        // have: letting the flight go does not replay the dip.
        handle_key(press(KeyCode::Char('p')), &mut flight, &args, &mut paused);
        for _ in 0..30 {
            flight.advance(1.0 / 60.0);
            assert_eq!(
                flight.gain(),
                1.0,
                "unpausing replayed a cut that had landed"
            );
        }
    }

    #[test]
    fn the_shot_opens_at_the_bottom_of_a_cut_it_was_never_on_the_other_side_of() {
        // Both views, because `--view side` used to reach its camera through a
        // `set_view` call made after construction — which would now arm a whole
        // dip out of a cockpit nobody asked to see. And at two frame rates,
        // because a fade is a length of flight rather than a number of frames.
        for view in ["cockpit", "side"] {
            for step in [1.0 / 60.0, 1.0 / 10.0] {
                let args = args_for(&[
                    "--seed",
                    "3",
                    "--magnitude",
                    "4.5",
                    "--size",
                    "60x20",
                    "--engage",
                    "--view",
                    view,
                ]);
                let mut flight = Flight::new(&args, 60, 20);
                assert_eq!(flight.gain(), 0.0, "{view} did not open on black");
                assert_eq!(
                    flight.shown(),
                    flight.view(),
                    "{view} opened on the wrong camera"
                );
                flight.draw(60.0, false, false);
                assert!(
                    flight.renderer.pixels().iter().all(|p| *p == [0, 0, 0]),
                    "{view} opened on a lit frame"
                );

                // A shot opens on the trough, so what is left of the cut is its
                // rise. Nothing on the way up may overshoot.
                let rise = args.fade * (1.0 - FADE_OUT_SHARE);
                while flight.time < rise as f64 {
                    flight.advance(step);
                    assert!(flight.gain() <= 1.0, "{view} opened past full at {step}");
                }
                assert_eq!(flight.gain(), 1.0, "{view} never arrived at {step}");
                flight.draw(60.0, false, false);
                assert!(
                    flight
                        .renderer
                        .pixels()
                        .iter()
                        .any(|p| p.iter().any(|v| *v > 0)),
                    "{view} came up black"
                );
            }
        }
    }

    #[test]
    fn the_instruments_are_not_in_the_exposure() {
        // The fade is an exposure and the panel is on the glass rather than in
        // it: stamped into cells after `compose`, which is the same line the
        // snapshot PNG already draws between the scene and the chrome. So a
        // shot opens on instruments over a dark sky, and the one place the keys
        // are written down to somebody flying is never on a clock.
        let args = args_for(&["--seed", "3", "--magnitude", "4.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        flight.draw(60.0, false, true);
        assert!(
            flight.renderer.pixels().iter().all(|p| *p == [0, 0, 0]),
            "the shot did not open on black, so this measured nothing"
        );
        let mut out = Vec::new();
        let _ = flight.present_plain(&mut out);
        assert!(
            String::from_utf8_lossy(&out).contains("VELOCITY"),
            "the panel went dark with the sky"
        );
    }

    #[test]
    fn the_picker_does_not_cut_to_a_camera_it_is_already_on() {
        // `M` forces the outside view through the same funnel `C` uses, so from
        // the cockpit it cuts — and from outside it must not, or opening a
        // dialogue would dip the picture behind it for nothing.
        let args = args_for(&["--seed", "11", "--magnitude", "4.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
        }

        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);
        assert_eq!(
            flight.view(),
            ViewMode::Side,
            "`M` did not go outside to look"
        );
        // A step first, because a cut does not step at the press: at zero
        // elapsed the shutter is still fully open whether one was armed or not,
        // which is the property the dip is built on and is why asking the gain
        // straight after a keypress answers nothing.
        flight.advance(1.0 / 60.0);
        assert!(flight.gain() < 1.0, "`M` from the cockpit did not cut");

        let mut flight = outside(&args);
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
        }
        handle_key(press(KeyCode::Char('m')), &mut flight, &args, &mut paused);
        flight.advance(1.0 / 60.0);
        assert_eq!(
            flight.gain(),
            1.0,
            "`M` cut to the camera it was already on"
        );
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        // same *hull* as one that never touched it. Compared on the pixels
        // rather than the cells, because the panel is meant to differ — the
        // compass has genuinely moved, and it should say so.
        //
        // Flown under an empty sky, and that is the change this needed rather
        // than a loosened bound. It used to compare whole frames, which worked
        // while the outside sky took no steering at all: the band streamed
        // along a track a turn of the hull could not move. The sky is in the
        // world now and the camera rides the hull, so a ship that really has
        // turned really does see it swing past — that is the honest picture and
        // not a regression. What the bug was about is the hull, so the hull is
        // what is left in the frame, which makes this a sharper test than the
        // one it replaces: a difference can no longer hide among the stars.
        let args = args_for(&["--seed", "6", "--magnitude", "-2", "--size", "80x24"]);
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
            // The cut `c` armed, landed. Without this the first frames here
            // are the cockpit dipping rather than the hull, under a sky that
            // is deliberately empty — so a third of what follows would be two
            // black buffers agreeing, which is exactly what a comparison like
            // the one below must never be allowed to become.
            flight.land_cut();
            let mut pixels = Vec::new();
            for _ in 0..30 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
                pixels.extend_from_slice(flight.renderer.pixels());
            }
            pixels
        };

        let (crooked, straight) = (frames(true), frames(false));
        assert!(
            straight.iter().any(|p| p.iter().any(|v| *v > 0)),
            "the hull never lit a subpixel, so the two agreed about nothing"
        );
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
    fn both_views_are_looking_at_the_same_universe() {
        // What replaced three tests about two skies: one that the outside pool
        // was built lazily, one that building it did not disturb the cockpit's
        // generator, and one that whichever sky was not being flown stayed put
        // while nobody was in it. All three were bookkeeping for a design where
        // `C` genuinely showed two different universes.
        let args = args_for(&["--seed", "4", "--magnitude", "5.5", "--size", "60x20"]);
        let mut flight = Flight::new(&args, 60, 20);
        flight.ship.throttle = 1.0;
        for _ in 0..60 {
            flight.advance(1.0 / 60.0);
        }
        let inside: Vec<[f32; 3]> = flight.sky.positions();

        flight.cycle_view();
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
            flight.draw(60.0, false, true);
        }
        flight.cycle_view();
        let after: Vec<[f32; 3]> = flight.sky.positions();

        assert_eq!(inside.len(), after.len(), "the sky changed size out there");
        // Not equality: the flight went on, so stars that crossed their own
        // limit came back somewhere else. What must hold is that going outside
        // and coming back is the same flight as staying put — which it is
        // trivially now, because there is one pool and one `advance`.
        let moved = inside.iter().zip(&after).filter(|(a, b)| a != b).count();
        assert!(
            moved < inside.len() / 4,
            "{moved} of {} stars moved while the camera was elsewhere",
            inside.len()
        );
    }

    #[test]
    fn turning_the_ship_bends_the_sky_from_both_cameras() {
        // The ship carries the attitude and both cameras are projected through
        // it, so a turn has to reach the exposure from the seat *and* from
        // outside — where the camera rides with the hull and swings with it.
        // Asked of the frames rather than of the sky, because what is under
        // test is a picture: fly the same seed twice, once with a hand on the
        // stick and once without, and the two must differ in both views.
        //
        // Of the *starlight*, and that is the whole of what this test is worth.
        // It compared whole frames for a while and could not tell what it was
        // claiming to: in the side view the hull's own lean and the drive's
        // plume carry the difference on their own, so re-run at
        // `cli::MIN_MAGNITUDE` — where `Universe::new` holds exactly zero stars
        // — the frames still differed over three quarters of the canvas and
        // cleared the threshold with nothing in the sky to turn. A regression
        // that stopped the exposure swinging altogether would have left it
        // green, and this is the only test in the tree that reaches the curved
        // exposure through the warp bubble, so it was the ceiling on that
        // module's coverage as well as its own.
        //
        // Subtracting an empty-sky frame from a lit one is how `render::bands`
        // isolates starlight, and flying at an empty magnitude so a difference
        // cannot hide among the stars is what
        // `pitching_about_inside_does_not_leave_the_ship_crooked_outside` does.
        // This is the two together: the ship's flight does not depend on the
        // sky, so the hull, the plume and the glare are identical between the
        // two magnitudes and fall out of the subtraction.
        let empty = crate::cli::MIN_MAGNITUDE.to_string();
        for view in [0, 1] {
            let frame = |steer: bool, magnitude: &str| {
                let args = args_for(&["--seed", "3", "--magnitude", magnitude, "--size", "80x24"]);
                let mut flight = Flight::new(&args, 80, 24);
                if view == 1 {
                    flight.cycle_view();
                }
                flight.ship.throttle = 1.0;
                flight.ship.toggle_warp();
                for _ in 0..300 {
                    if steer {
                        flight.nudge_stick(1.0, -0.35, 0.0);
                    }
                    flight.advance(1.0 / 60.0);
                }
                flight.draw(60.0, false, true);
                flight.renderer.pixels().to_vec()
            };
            let starlight = |steer: bool| {
                let (lit, bare) = (frame(steer, "5.5"), frame(steer, &empty));
                lit.iter()
                    .zip(&bare)
                    .map(|(a, b)| {
                        [
                            a[0] as i16 - b[0] as i16,
                            a[1] as i16 - b[1] as i16,
                            a[2] as i16 - b[2] as i16,
                        ]
                    })
                    .collect::<Vec<_>>()
            };

            let (turned, straight) = (starlight(true), starlight(false));
            let differing = turned.iter().zip(&straight).filter(|(a, b)| a != b).count();
            assert!(
                differing > straight.len() / 20,
                "only {differing} of {} subpixels of starlight moved in view {view}",
                straight.len()
            );

            // And the empty sky is really empty, so the subtraction above is
            // removing everything that is not a star rather than most of it.
            assert_eq!(
                Universe::new(crate::cli::MIN_MAGNITUDE, 3).len(),
                0,
                "the empty-sky frame is not empty, so the starlight above is \
                 not isolated"
            );
        }
    }

    #[test]
    fn a_turn_leans_the_hull_even_with_nothing_in_the_sky() {
        // The other half of the claim above, stated on its own feet. That test
        // used to make both at once and one of them quietly stood in for the
        // other; now that it looks only at starlight, nothing was left saying
        // the hull answers the stick at all from outside.
        let empty = crate::cli::MIN_MAGNITUDE.to_string();
        let frame = |steer: bool| {
            let args = args_for(&["--seed", "3", "--magnitude", &empty, "--size", "80x24"]);
            let mut flight = Flight::new(&args, 80, 24);
            flight.cycle_view();
            flight.ship.throttle = 1.0;
            flight.ship.toggle_warp();
            for _ in 0..300 {
                if steer {
                    flight.nudge_stick(1.0, -0.35, 0.0);
                }
                flight.advance(1.0 / 60.0);
            }
            flight.draw(60.0, false, true);
            flight.renderer.pixels().to_vec()
        };
        let (turned, straight) = (frame(true), frame(false));
        let differing = turned.iter().zip(&straight).filter(|(a, b)| a != b).count();
        assert!(
            differing > straight.len() / 100,
            "the hull did not answer the stick with an empty sky: {differing} \
             of {} subpixels",
            straight.len()
        );
    }

    #[test]
    fn a_camera_swing_moves_no_star() {
        // The sharp statement of what the rebuild is for. The band this
        // replaced was laid out in the side camera's own space and re-laid
        // whenever the orbit differed from the one it was last laid against, so
        // swinging the camera rotated the whole pool — and the fold, the rim
        // and the entering-surface sampler existed to patch the seams that
        // tore. A world-space sky has nothing to re-lay, so this is bitwise.
        let args = args_for(&["--seed", "9", "--magnitude", "5.5", "--size", "80x24"]);
        let mut flight = Flight::new(&args, 80, 24);
        flight.cycle_view();
        flight.ship.throttle = 1.0;
        for _ in 0..30 {
            flight.advance(1.0 / 60.0);
        }
        let parked = flight.sky.positions();

        // All the way round, on every axis, drawing each frame so nothing can
        // be said to have been skipped.
        for _ in 0..90 {
            flight.nudge_orbit(view::ORBIT_STEP, view::ORBIT_STEP, view::ORBIT_STEP);
            flight.draw(60.0, true, true);
        }
        // And then flown on, because `advance` is what eases the orbit toward
        // where the keys put it — and what would step the sky, if a swing were
        // still something the sky had to be told about.
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
            flight.draw(60.0, false, true);
        }
        assert_ne!(
            flight.orbit(),
            args.orbit,
            "the camera did not actually move"
        );
        assert_eq!(
            parked,
            flight.sky.positions(),
            "a camera swing moved the sky it was swinging past"
        );
    }

    #[test]
    fn m_opens_the_ship_picker_and_escape_closes_it_without_quitting() {
        // The nastiest bug this feature could have: reaching for the key that
        // dismisses a dialogue and losing the terminal instead.
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--seed", "2", "--view", "side", "--ship", "normandy"]);
        let mut flight = Flight::new(&args, 80, 24);
        assert_eq!(flight.view(), ViewMode::Side, "--view side did not take");
        assert_eq!(flight.drawn_model().name, "normandy");

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
    fn the_magnitude_keys_reach_the_sky_from_either_view() {
        // It used to matter *which* sky the keys grew — there were two, and
        // the answer was whichever one was being looked at. There is one now,
        // so what is left to say is that both cameras reach it.
        let args = args_for(&["--magnitude", "5.0", "--size", "80x24"]);
        let mut paused = false;
        for outside in [false, true] {
            let mut flight = Flight::new(&args, 80, 24);
            if outside {
                handle_key(press(KeyCode::Char('c')), &mut flight, &args, &mut paused);
            }
            let opened = flight.stars();
            handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
            assert!(
                flight.stars() > opened,
                "`+` did not reach the sky from {}",
                if outside { "outside" } else { "the seat" }
            );
        }
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
        let args = args_for(&["--magnitude", "3", "--size", "20x8"]);
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
        let args = args_for(&["--magnitude", "3", "--size", "20x8"]);
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
        let args = args_for(&["--magnitude", "3", "--size", "20x8"]);
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
            // And a terminal claiming to be the largest thing a resize event
            // can express. `--size` refuses this at the command line; the
            // ioctl door had nothing on it, and 4.3e9 cells is an allocation
            // that aborts rather than fails — taking the terminal down with
            // it, since an abort runs no `Drop`.
            (u16::MAX as usize, u16::MAX as usize),
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
                !flight.sky.is_empty(),
                "the star pool emptied at {cols}x{rows}"
            );
            assert!(flight.ship.speed.is_finite());

            let (sc, sr) = flight.renderer.screen().dims();
            assert_eq!(
                (cw, ch),
                (sc, sr * 2),
                "canvas and screen disagree at {cols}x{rows}"
            );
            assert!(
                sc * sr <= cli::MAX_CELLS,
                "a resize to {cols}x{rows} was believed, at {} cells",
                sc * sr
            );
            flight.renderer.present(&mut Vec::new()).unwrap();
        }
    }

    #[test]
    fn a_preposterous_resize_is_clamped_rather_than_believed() {
        // The sibling of `a_preposterous_terminal_is_clamped_rather_than_
        // believed` in `cli.rs`, and it exists because that one only ever
        // watched the *first* answer. `resolved_size` runs the opening ioctl
        // through `clamp_size`; every later answer arrives as `Event::Resize`
        // and went straight into the canvas, so a `stty cols 30000 rows 30000`
        // against a running flight asked for nine hundred million cells. A
        // failed allocation aborts — no unwind, no `Drop`, no panic hook — so
        // the terminal is never handed back.
        //
        // Asserted as the ratio rather than as an absolute pair, because both
        // axes scale by the same factor and what has to survive is the shape.
        let args = args_for(&["--magnitude", "3.5"]);
        let mut flight = Flight::new(&args, 80, 24);
        assert!(flight.resize(&args, 30_000, 30_000));

        let (cols, rows) = flight.renderer.screen().dims();
        assert!(
            cols * rows <= cli::MAX_CELLS,
            "a preposterous resize was believed, at {cols}x{rows}"
        );
        assert!(
            cols.abs_diff(rows) * 10 < cols.max(rows),
            "a square terminal came back {cols}x{rows}"
        );

        // And a sane one is still left exactly alone.
        assert!(flight.resize(&args, 100, 30));
        assert_eq!(flight.renderer.screen().dims(), (100, 30));
    }

    #[test]
    fn a_forced_size_ignores_the_terminal_moving_underneath_it() {
        // Regression: `--size` was honoured at startup and then discarded by
        // the first resize event, so the flag meant "until the window moves".
        let args = args_for(&["--size", "120x40", "--magnitude", "3.5"]);
        let mut flight = Flight::new(&args, 120, 40);
        assert!(!flight.resize(&args, 80, 24), "a forced size must not move");
        assert_eq!(flight.renderer.canvas_dims(), (120, 80));

        // Without the flag the terminal is the authority — but only when it
        // actually says something new.
        let args = args_for(&["--magnitude", "3.5"]);
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
        // edge respawned the sky a few hundred stars at a time and the count
        // the panel showed at the time walked about while nothing had asked it
        // to. A count a window can overrule is not one. The panel reads out the
        // magnitude now and lets the count follow, so there is nothing left up
        // there to walk — but the fault was in the pool rather than in the
        // readout, which is why this still asks the pool.
        //
        // It is now true for a stronger reason than the flag being literal: the
        // sky is laid out in the world rather than in front of the camera, so a
        // resize has nothing to say to it at all and the two `retarget` calls
        // that used to stand in `resize` — each dropping every trail in a field
        // to hide the seam — are gone. The pool must not move by so much as a
        // star, from either view.
        for extra in [vec![], vec!["--magnitude", "5.5"]] {
            let args = args_for(&extra);
            let mut flight = Flight::new(&args, 40, 12);
            flight.set_view(ViewMode::Side);
            let want = flight.stars();
            let parked = flight.sky.positions();
            for (cols, rows) in [(300usize, 90usize), (1, 1), (80, 24)] {
                flight.resize(&args, cols, rows);
                assert_eq!(flight.stars(), want, "{cols}x{rows} retuned the sky");
                assert_eq!(
                    parked,
                    flight.sky.positions(),
                    "{cols}x{rows} moved a star that was already out there"
                );
            }
        }
    }

    #[test]
    fn the_brackets_and_the_wheel_ask_for_the_same_zoom() {
        // Two ways into one control. They are wired separately — one through
        // `handle_key`, one through `handle_mouse` — so nothing but a test says
        // they agree, and a wheel that zoomed the opposite way to the key it
        // shares a job with would be a very tiresome bug to live with.
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        // It was once a mechanism — the star band was laid out against the
        // camera's focal length, so a zoom that touched it re-folded the whole
        // field every notch — and it is a choice now: the sky is in the world
        // and would survive a focal change, and a lens change is a different
        // shot rather than a closer one. The property is the same either way,
        // and it fails the moment anything makes `exterior_camera` read the
        // zoom. Flown at sublight, where
        // the lens is off and an exactly identical sky is the whole claim:
        // every subpixel that differs between two zooms has to be one the ship
        // and its bubble could have reached.
        let args = args_for(&["--seed", "5", "--magnitude", "5", "--size", "80x24"]);
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
        // The sweep below is a claim about *where* the two differ, which says
        // nothing at all if they do not differ anywhere. `outside` lands the
        // opening cut so these are lit frames rather than dim ones, and this
        // is what says so.
        assert!(
            near.iter().zip(&far).any(|(a, b)| a != b),
            "the two zooms drew the same frame, so nothing below was measured"
        );

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

    #[cfg(feature = "snapshot")]
    #[test]
    fn the_two_documented_snapshots_come_out_the_same_size() {
        // The README's front page is two images stacked, and they line up only
        // because neither recipe passes `--size`: both fall back to
        // `SNAPSHOT_SIZE`, a cell is two subpixels tall, and at `--scale 2`
        // both come out 480x272. The hero used to ask for `--size 220x60`,
        // came out 440x240, and put a forty-pixel step down the side of the
        // page that no commit message explained.
        //
        // Nothing pins the bytes of those files — `frames.sha256` is the text
        // frames and knows nothing about the PNGs — so this pins the one
        // property that makes the pair a pair. Asked of the real canvas rather
        // than of `SNAPSHOT_SIZE` doubled by hand, so it goes through the
        // arithmetic a snapshot actually uses.
        let common = [
            "--engage",
            "--throttle",
            "1.0",
            "--warmup",
            "600",
            "--scale",
            "2",
        ];
        let recipe = |extra: &[&str]| {
            let mut argv = vec!["--snapshot", "docs/shot.png"];
            argv.extend_from_slice(&common);
            argv.extend_from_slice(extra);
            args_for(&argv)
        };
        let hero = recipe(&["--seed", "6"]);
        let astern = recipe(&["--view", "side", "--orbit", "245,30,0", "--seed", "8"]);

        let image = |args: &Args| {
            assert!(
                args.size.is_none(),
                "a docs recipe that passes --size is what put the step back"
            );
            let (cols, rows) = SNAPSHOT_SIZE;
            let flight = Flight::new(args, cols as usize, rows as usize);
            let (w, h) = flight.renderer.canvas_dims();
            (w * args.scale, h * args.scale)
        };

        assert_eq!(
            image(&hero),
            image(&astern),
            "the two images on the README's front page no longer stack"
        );
        assert_eq!(
            image(&hero),
            (480, 272),
            "the documented recipe stopped producing the size CLAUDE.md quotes"
        );
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
        let args = args_for(&["--seed", "9", "--magnitude", "5", "--size", "80x24"]);
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
        // Both flights press `c` at the same instant and take the same forty
        // steps, so the cut and the shutter it opens through cancel — but a
        // pair of black frames would cancel too, and this is the acceptance
        // test for the whole camera. Forty steps is two thirds of a second,
        // which clears the default fade, and this is what says it did.
        assert!(
            want.iter().any(|p| p.iter().any(|v| *v > 0)),
            "the shot came out black, so nothing below was compared"
        );
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
    fn a_flight_that_flies_itself_shows_no_instruments() {
        // The panel is chrome a pilot reads, and `--demo` and `--screensaver`
        // have no pilot: what somebody watching either of those wants is the
        // sky, not a throttle bar and a list of keys. Asked through the gate
        // the loops actually use rather than of `hud::draw` directly, because
        // what went wrong before was narrower than the panel — `hints` was
        // switched off for a screensaver and the other four rows went on being
        // drawn over it.
        //
        // Both views, because the panel carries a `SHIP` row from outside that
        // the cockpit does not, and both cameras go through the same gate.
        // Drawn and presented by hand rather than through `frame_of`, which
        // asks for a panel of its own — the gate is the subject here, so it has
        // to be this call that sets it. And the colour codes come out first,
        // because the sky shows through the panel: a star behind a word puts an
        // escape sequence in the middle of it, which would break the word for a
        // `contains` in *either* direction. That, and a seed, are why this used
        // to fail about half the time it was run.
        let glyphs = |flight: &mut Flight, panel: bool| {
            flight.draw(60.0, false, panel);
            let mut out = Vec::new();
            let _ = flight.present_plain(&mut out);
            String::from_utf8_lossy(&out)
                .split('\u{1b}')
                .map(|chunk| chunk.split_once('m').map_or(chunk, |(_, rest)| rest))
                .collect::<String>()
        };

        for view in ["cockpit", "side"] {
            for mode in [vec!["--demo"], vec!["--screensaver"], vec!["--demo", "12"]] {
                let mut argv = mode.clone();
                argv.extend_from_slice(&["--view", view, "--size", "120x36", "--seed", "1"]);
                let args = args_for(&argv);
                assert!(args.unattended(), "{argv:?} is not an unattended flight");
                let mut flight = Flight::new(&args, 120, 36);
                flight.advance(1.0 / 60.0);
                let frame = glyphs(&mut flight, !args.unattended());
                for word in ["VELOCITY", "DISTANCE", "THR", "pause", "warp"] {
                    assert!(!frame.contains(word), "{argv:?} put {word} on the frame");
                }
            }
            // And the control, which is the half that says the gate is a gate
            // rather than a panel that stopped working.
            let args = args_for(&[
                "--engage", "--view", view, "--size", "120x36", "--seed", "1",
            ]);
            assert!(!args.unattended(), "a flown flight reads as unattended");
            let mut flight = Flight::new(&args, 120, 36);
            flight.advance(1.0 / 60.0);
            let frame = glyphs(&mut flight, !args.unattended());
            for word in ["VELOCITY", "THR"] {
                assert!(
                    frame.contains(word),
                    "a flown {view} frame is missing {word}"
                );
            }
        }
    }

    #[test]
    fn an_unattended_flight_swings_the_camera_and_a_flown_one_does_not() {
        // The gap the autopilot's camera closes, and the line it must not
        // cross. A flight nobody is at flies its own camera; a flight somebody
        // is at is left exactly where `--orbit` parked it, to the bit, however
        // long it runs.
        let args = args_for(&["--view", "side", "--magnitude", "2.5", "--size", "60x20"]);
        let mut flown = Flight::new(&args, 60, 20);
        for frame in 0..600 {
            flown.advance(1.0 / 60.0);
            let _ = frame;
        }
        assert_eq!(
            flown.orbit_target(),
            args.orbit,
            "a flight nobody asked to fly itself moved its own camera"
        );

        let mut unattended = Flight::new(&args, 60, 20);
        for frame in 0..600 {
            unattended.fly_itself(&args, frame as f64 / 60.0);
            unattended.advance(1.0 / 60.0);
        }
        assert!(
            !unattended.orbit_target().is_level(),
            "ten seconds of autopilot left the camera on the beam"
        );
        assert!(
            !unattended.orbit().is_level(),
            "the camera was asked to swing and never eased there"
        );
    }

    #[test]
    fn a_flight_can_say_which_sky_it_is_flying() {
        // `run_snapshot` reports the frame it wrote and the velocity it wrote
        // it at, and used to leave out the only thing about the shot that
        // cannot be read back off the picture. `--seed` is optional and falls
        // back to the clock, so a frame worth keeping was a frame nobody could
        // take again — and the seed was held nowhere a caller could reach.
        let args = args_for(&["--seed", "4321", "--size", "40x12"]);
        assert_eq!(Flight::new(&args, 40, 12).seed(), 4321);

        // And without the flag it is whatever the clock gave, which is the
        // case worth reporting: two flights a moment apart are two skies, and
        // the number is the only way back to either.
        let args = args_for(&["--size", "40x12"]);
        let (first, second) = (
            Flight::new(&args, 40, 12).seed(),
            Flight::new(&args, 40, 12).seed(),
        );
        assert_ne!(first, second, "two unseeded flights took the same seed");
    }

    #[test]
    fn a_paused_demo_freezes_the_schedule_it_is_flying_to() {
        // `P` gates `advance` and not the autopilot, which is deliberate and
        // documented — a paused demo goes on flying itself and repainting. That
        // took two corrections, and only one of them is still load-bearing.
        //
        // The first: `advance` is where the steering damper lives, so an
        // autopilot that went on working the stick through a pause was pushing
        // against nothing, and eleven seconds of `P` walked the yaw all the way
        // to `MAX_YAW_RATE` at any frame rate. Zeroing the step fixed it and
        // the autopilot has since stopped touching the stick altogether, so
        // that fault is unreachable rather than guarded — the rate assertion
        // below is a control now, and what it watches for is something *else*
        // moving the stick behind a pause.
        //
        // The second is the one holding this up. The clock stayed on the wall
        // on the argument that the
        // throttle and the camera are closed forms of it and so are unaffected
        // — true of the throttle, and false of the camera, because the closed
        // form gives the orbit *target* and a frame is built from
        // `Flight::orbit`, which eases toward it in `advance`. The pause stops
        // the ease and not the target, so the gap opened at
        // `autopilot::CAMERA_TURN` — a turn every 43 seconds — and closed at
        // 7.2% per sim step on resume, whipping the camera round the hull.
        //
        // So the clock a paused flight hands its autopilot is `Flight::time`,
        // which `advance` is the only thing that moves. What that costs is the
        // sentence "a paused demo goes on flying itself": it does not any more,
        // and the two things it does go on doing — repainting, and exiting at
        // its deadline — are the two that were ever worth having. A pause that
        // stops the world and not the schedule it is flying to is a pause that
        // owes the flight a jump on the way out.
        let args = args_for(&[
            "--demo",
            "--view",
            "side",
            "--magnitude",
            "2.5",
            "--size",
            "60x20",
        ]);
        let mut flight = Flight::new(&args, 60, 20);
        // Ten seconds of flying, so there is a schedule underway to freeze.
        for frame in 0..(10 * 60) {
            flight.fly_itself(&args, flight.time);
            flight.advance(1.0 / 60.0);
            let _ = frame;
        }
        // Exactly what the loop does while `paused` is set: no step, and no
        // clock either, because `advance` is not called. Snapshotted after the
        // first such frame rather than before it: the clock the loop hands over
        // is the one `advance` left behind, so the first paused frame catches
        // up by the step the last flying one took and every frame after it is
        // handed the same number.
        flight.fly_itself(&args, flight.time);
        let underway = (
            flight.ship.throttle,
            flight.orbit_target,
            flight.ship.warp_engaged,
        );
        // An exact zero now rather than whatever the weave had left behind,
        // which is why it is read rather than asserted: what has to hold is
        // that thirty seconds of pause do not move it, wherever it starts.
        let held = (flight.ship.yaw_rate, flight.ship.pitch_rate);

        for _ in 0..(30 * 60) {
            flight.fly_itself(&args, flight.time);
        }
        // Held rather than zero, and the distinction is kept deliberately even
        // though the value is now an exact zero: the damper is in `advance` and
        // a pause stops it, so whatever rate the ship is carrying it goes on
        // carrying. What must not happen is the rate *growing*, which is what
        // an impulse pushing against a stopped damper does.
        assert_eq!(
            (flight.ship.yaw_rate, flight.ship.pitch_rate),
            held,
            "thirty seconds of pause moved the stick"
        );
        assert_eq!(
            (
                flight.ship.throttle,
                flight.orbit_target,
                flight.ship.warp_engaged
            ),
            underway,
            "thirty seconds of pause moved the flight the autopilot was flying"
        );
        // And the camera the *frame* is built from has not been left behind by
        // the one the autopilot is asking for, which is the whole of the snap.
        let gap = [
            wrap_signed(flight.orbit_target.azimuth - flight.orbit.azimuth),
            wrap_signed(flight.orbit_target.elevation - flight.orbit.elevation),
            wrap_signed(flight.orbit_target.roll - flight.orbit.roll),
        ];
        assert!(
            gap.iter().all(|angle| angle.abs() < 0.05),
            "the pause opened a {gap:?} gap between the camera and where it is being asked to go"
        );

        // And it picks the schedule back up rather than jumping into it.
        flight.fly_itself(&args, flight.time);
        flight.advance(1.0 / 60.0);
        assert!(
            (flight.ship.throttle - underway.0).abs() < 0.05,
            "the throttle jumped from {} to {} on the way out of a pause",
            underway.0,
            flight.ship.throttle
        );
    }

    #[test]
    fn a_hand_on_the_camera_takes_it_off_the_autopilot() {
        // A control that swallows a press and gives nothing back is worse than
        // one plainly not connected, so the autopilot stands off the moment
        // somebody swings the camera themselves — and `R`, which is the key for
        // putting the view back, hands it over again.
        let args = args_for(&[
            "--demo",
            "--view",
            "side",
            "--magnitude",
            "2.5",
            "--size",
            "60x20",
        ]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut paused = false;

        handle_key(press(KeyCode::Char('d')), &mut flight, &args, &mut paused);
        let asked = flight.orbit_target();
        for frame in 0..120 {
            flight.fly_itself(&args, frame as f64 / 60.0);
        }
        assert_eq!(
            flight.orbit_target(),
            asked,
            "the autopilot overwrote a camera somebody had hold of"
        );
        // The ship is still flying itself — only the camera was handed over.
        assert!(flight.ship.throttle > 0.15, "the demo stopped flying");

        handle_key(press(KeyCode::Char('r')), &mut flight, &args, &mut paused);
        for frame in 120..360 {
            flight.fly_itself(&args, frame as f64 / 60.0);
        }
        assert_ne!(
            flight.orbit_target(),
            asked,
            "`R` did not give the camera back to the autopilot"
        );
    }

    #[test]
    fn a_camera_notch_and_its_opposite_land_back_where_they_started() {
        // Stronger than the zoom's version of this, and worth stating because
        // it is why the step is additive: `x + s - s` is exactly `x` where a
        // geometric step has to settle for very nearly.
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
        let args = args_for(&["--magnitude", "3.5", "--size", "80x24"]);
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
            "--seed",
            "3",
            "--magnitude",
            "4.5",
            "--size",
            "60x20",
            "--engage",
        ]);
        let mut flight = Flight::new(&args, 60, 20);
        for frame in 0..3000 {
            flight.fly_itself(&args, frame as f64 / 60.0);
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
