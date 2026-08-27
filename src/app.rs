//! Running a flight: the loops that drive one, and the state they drive.

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
const MAX_STEP_DT: f32 = 1.0;
/// How much fainter or brighter one press of `+` or `-` asks the sky to go.
const MAGNITUDE_STEP: f32 = 0.5;

/// How much of a cut is spent going dark, as a fraction of `--fade`.
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
    zoom: f32,
    zoom_target: f32,
    /// Which way round the ship the outside camera has been swung, and where it
    /// is being asked to swing to. The same pair for the same reason, and eased
    /// the same way — a key held down arrives as auto-repeat, which is a burst
    /// of steps rather than one.
    orbit: Orbit,
    orbit_target: Orbit,
    /// Whether a hand has been put on the camera since the flight opened.
    hands_on: bool,
    /// The number this flight's sky was built from.
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
            // arrives over the rise.
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
    pub fn cycle_view(&mut self) {
        self.set_view(self.view.next());
    }

    /// How far the outside camera has been pushed in or out.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Push it a notch: positive is closer and bigger, negative is further off.
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
    pub fn nudge_stick(&mut self, yaw: f32, pitch: f32, roll: f32) {
        self.ship.nudge_yaw(yaw);
        self.ship.nudge_pitch(pitch);
        self.ship.nudge_roll(roll);
    }

    /// Swing the camera round the ship, over it, or about its own view axis, a
    /// step at a time.
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
    pub fn reset_orbit(&mut self, orbit: Orbit) {
        self.orbit = orbit.held();
        self.orbit_target = self.orbit;
        // And the autopilot has the camera again.
        self.hands_on = false;
    }

    /// Fly a frame with nobody at the controls: the ship, and the camera that
    /// is watching it.
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
    pub fn advance(&mut self, dt: f32) {
        // Held to something a frame could plausibly be *here*, rather than at
        // the one call site that used to do it.
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
            self.zoom +=
                (self.zoom_target - self.zoom) * (1.0 - (-view::ZOOM_EASE * SIM_STEP).exp());
            // And the same for the orbit, with one difference: all three of its
            // angles go round, so each chases its target the short way rather
            // than unwinding three hundred and fifty degrees to reach a target
            // ten degrees away.
            let ease = 1.0 - (-view::ORBIT_EASE * SIM_STEP).exp();
            let chase = |from: f32, to: f32| from + wrap_signed(to - from) * ease;
            self.orbit = Orbit {
                azimuth: chase(self.orbit.azimuth, self.orbit_target.azimuth),
                elevation: chase(self.orbit.elevation, self.orbit_target.elevation),
                roll: chase(self.orbit.roll, self.orbit_target.roll),
            }
            .held();
            // One sky, and it is stepped whichever camera is flying — there is
            // no longer a second one to be kept warm or skipped.
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
    pub fn draw(&mut self, fps: f32, paused: bool, panel: bool) {
        // A cut cannot get anywhere with the clock stopped, so it lands here
        // instead — see [`Self::land_cut`].
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
            // already changed hands.
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
        // `--size` is a fixed size, not a starting point.
        if args.size.is_some() {
            return false;
        }
        // Through the same gate `--size` goes through, and here rather than at
        // the one call site for the reason `advance` clamps its own `dt`: this
        // is `pub`, and `lib.rs` offers a flight to any program that cares to
        // fly one, so the guard belongs with the allocation it protects.
        let (cols, rows) = cli::clamp_size(
            cols.clamp(1, u16::MAX as usize) as u16,
            rows.clamp(1, u16::MAX as usize) as u16,
        );
        let (cols, rows) = (cols as usize, rows as usize);
        if self.renderer.screen().dims() == (cols, rows) {
            return false;
        }

        self.renderer.resize(cols, rows);
        // And the sky is not told.
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
    // `Esc`.
    if flight.menu_open() && !ctrl {
        return menu_key(key, flight);
    }
    // Pointing the nose is a thing you do from behind it.
    let (steers, flies_the_camera, zooms) = match flight.view() {
        ViewMode::Cockpit => (true, false, false),
        ViewMode::Side => (false, true, true),
    };
    // And out there the same six keys fly the *camera*, which is the thing that
    // can usefully move in a view whose whole subject is the ship.
    match key.code {
        // `q` is on the stick, so it cannot also be the way out: nothing a
        // pilot reaches for mid-turn should end the flight.
        KeyCode::Char('c' | 'd') if ctrl => return Action::Quit,
        KeyCode::Esc => return Action::Quit,

        // The stick.
        KeyCode::Char('w' | 'W' | 'i' | 'I') if steers => flight.ship.nudge_pitch(-1.0),
        KeyCode::Char('s' | 'S' | 'k' | 'K') if steers => flight.ship.nudge_pitch(1.0),
        KeyCode::Left | KeyCode::Char('a' | 'A') if steers => flight.ship.nudge_yaw(-1.0),
        KeyCode::Right | KeyCode::Char('d' | 'D') if steers => flight.ship.nudge_yaw(1.0),
        KeyCode::Char('q' | 'Q') if steers => flight.ship.nudge_roll(-1.0),
        KeyCode::Char('e' | 'E') if steers => flight.ship.nudge_roll(1.0),

        // The same six, outside, on the camera.
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

        // The throttle is the up and down arrows, which is where it has always
        // been: only its letters went to the stick.
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

        // The outside camera, in and out.
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
    if !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no terminal on stdout; use --headless to write frames somewhere else",
        ));
    }

    // Not `terminal::size()` directly: tmux runs a `lock-command` against a tty
    // whose window size is not set yet, so it can report zero.
    let (cols, rows) = resolved_size(args);

    // Built before the terminal is taken over: a failed allocation aborts
    // outright — no unwind, no `Drop`, no panic hook — so anything installed
    // first would never be undone.
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

        // A signal asked for the process.
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
        // from.
        let dt = (frame_start - last).as_secs_f32().clamp(0.0, MAX_FRAME_DT);
        last = frame_start;
        // Smoothed so the readout is legible rather than flickering.
        fps += (1.0 / dt.max(1e-4) - fps) * 0.08;

        if args.unattended() {
            // The flight's own clock rather than the wall's, and that is now
            // the whole of what a pause needs from here.
            flight.fly_itself(args, flight.time);
        }

        if !paused {
            flight.advance(dt);
        }
        flight.draw(fps, paused, !args.unattended());
        flight.renderer.present(&mut out)?;

        // Spend what is left of the frame waiting on the event queue rather
        // than sleeping through it.
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
                    // The repaint follows the size the terminal last reported and the
                    // canvas follows `resize`, which answers no under `--size`.
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
    // cannot be read back off the picture.
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

    /// A notch of the wheel. The column and row are not read by anything here —
    /// nothing in this program is aimed at — so they are a fixed nowhere.
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
        // Regression: `time` was an `f32` accumulator.
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
        // Regression: `advance` is public and took whatever it was given.
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
        // sixteen milliseconds.
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
        // the property it is for.
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
        // `--stars 0` used to be for.
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
        // otherwise be comparing is mostly how dark the opening is.
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
        use clap::CommandFactory;
        let help = cli::Args::command().render_help().to_string();
        // The key *column* of the controls block, not the help at large: a
        // single letter turns up all over a page that also spells `--magnitude
        // <MAG>`, so `contains` answers yes to keys the block never names.
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
            // they can both be black instead.
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
                    // One key, one axis: the stick must not cross-couple, and W
                    // and S must no longer touch the throttle they used to be.
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
        // settled frame is byte for byte the frame the fade is not there for.
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
        // The whole of the design in one test.
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

        // A cut does not step.
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
        // both are here.
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
        // still the outgoing camera.
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
        // dip out of a cockpit nobody asked to see.
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
                // rise.
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
        // snapshot PNG already draws between the scene and the chrome.
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
        // the hull leans a few degrees.
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
        // band of sky streamed past it dead level.
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
            // The cut `c` armed, landed.
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
        // while nobody was in it.
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
        // limit came back somewhere else.
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
        // The other half of the claim above, stated on its own feet.
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
        // The sharp statement of what the rebuild is for.
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
        // It used to matter *which* sky the keys grew — there were two, and the
        // answer was whichever one was being looked at.
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
        // Regression in waiting: `q` quit before it was a control.
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
        // reads off the end of it.
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
        // watched the *first* answer.
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
        // Two ways into one control.
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
        // nothing at all if they do not differ anywhere.
        assert!(
            near.iter().zip(&far).any(|(a, b)| a != b),
            "the two zooms drew the same frame, so nothing below was measured"
        );

        // Generous: the hull at the widest zoom, and the bubble that would hold
        // it.
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
        // both come out 480x272.
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
        // in CI.
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
        // test for the whole camera.
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
        // other half of
        // `the_stick_flies_the_camera_outside_and_the_ship_inside` — in here
        // the six keys fly the ship, so none of them may move the camera, and
        // the camera survives a round trip through `C` because it is state
        // rather than a mode.
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
        // sky, not a throttle bar and a list of keys.
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
        // cross.
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
        // cannot be read back off the picture.
        let args = args_for(&["--seed", "4321", "--size", "40x12"]);
        assert_eq!(Flight::new(&args, 40, 12).seed(), 4321);

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
        // documented — a paused demo goes on flying itself and repainting.
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
        // clock either, because `advance` is not called.
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
        // carrying.
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
        let gap = [
            wrap_signed(flight.orbit_target.azimuth - flight.orbit.azimuth),
            wrap_signed(flight.orbit_target.elevation - flight.orbit.elevation),
            wrap_signed(flight.orbit_target.roll - flight.orbit.roll),
        ];
        assert!(
            gap.iter().all(|angle| angle.abs() < 0.05),
            "the pause opened a {gap:?} gap between the camera and where it is being asked to go"
        );

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
        let args = args_for(&["--magnitude", "3.5", "--size", "60x20"]);
        let mut flight = outside(&args);
        let mut paused = false;

        // A full loop of the elevation on its own, watched the whole way.
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
        // not depend on how the frames happened to land.
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
        // scrolling is how a list is read.
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
