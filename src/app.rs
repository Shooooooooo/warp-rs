//! Running a flight: the loops that drive one, and the state they drive.
//!
//! Three of them, sharing a `Flight` and differing only in where the frames
//! end up — an interactive terminal, stdout, or a PNG.

use crate::autopilot::Autopilot;
use crate::cli::{resolved_size, Args};
use crate::exterior::ExteriorField;
use crate::hud::Readout;
use crate::menu::{self, Menu};
use crate::models::{self, ShipModel};
use crate::render::{Exterior, Renderer};
use crate::ship::Ship;
#[cfg(feature = "snapshot")]
use crate::snapshot;
use crate::starfield::StarField;
use crate::term::RawGuard;
use crate::view::ViewMode;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{terminal, QueueableCommand};
use std::io::{self, BufWriter, Write};
use std::time::{Duration, Instant};

/// Physics runs on a fixed step so the flight model behaves the same whether
/// the terminal can keep up or not.
const SIM_STEP: f32 = 1.0 / 120.0;
/// A stalled process must not fast-forward the universe on the next frame.
const MAX_FRAME_DT: f32 = 0.25;
/// Stars per subpixel when the count is chosen automatically.
const AUTO_DENSITY: f32 = 0.05;
const AUTO_MIN_STARS: usize = 300;
const AUTO_MAX_STARS: usize = 20_000;
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

        let renderer = Renderer::new(cols, rows, args.color.resolve(), args.exposure);
        let cam = renderer.camera(&ship, 0.0);
        let seed = seed(args);
        let field = StarField::new(star_count(args, &renderer), seed, &cam);

        let mut flight = Self {
            ship,
            field,
            exterior: None,
            renderer,
            autopilot: Autopilot::default(),
            view: ViewMode::Cockpit,
            model: args.ship,
            menu: None,
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
    pub fn cycle_view(&mut self, args: &Args) {
        self.set_view(self.view.next(), args);
    }

    fn set_view(&mut self, view: ViewMode, args: &Args) {
        self.view = view;
        if view == ViewMode::Side && self.exterior.is_none() {
            let cam = self.renderer.exterior_camera(&self.ship, self.time);
            // Its own generator, from a seed of its own, so the cockpit's
            // stream is untouched whenever this happens — and so the two skies
            // are not the same pattern seen twice.
            self.exterior = Some(ExteriorField::new(
                star_count(args, &self.renderer),
                self.seed ^ EXTERIOR_SEED,
                &cam,
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
        self.time += dt as f64;
        self.accumulator += dt;
        while self.accumulator >= SIM_STEP {
            self.ship.update(SIM_STEP);
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
                        field.update(SIM_STEP, self.ship.speed, &cam);
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
    fn resize_pool(&mut self, scale: f32, floor: usize) {
        let wanted = |len: usize| ((len as f32 * scale) as usize).clamp(floor, AUTO_MAX_STARS);
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
        if args.stars == 0 {
            let count = star_count(args, &self.renderer);
            self.field.resize_pool(count);
            if let Some(field) = &mut self.exterior {
                field.resize_pool(count);
            }
        }
        true
    }
}

fn star_count(args: &Args, renderer: &Renderer) -> usize {
    if args.stars > 0 {
        return args.stars;
    }
    let (w, h) = renderer.canvas_dims();
    (((w * h) as f32 * AUTO_DENSITY) as usize).clamp(AUTO_MIN_STARS, AUTO_MAX_STARS)
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
    match key.code {
        // `q` is on the stick, so it cannot also be the way out: nothing a
        // pilot reaches for mid-turn should end the flight.
        KeyCode::Char('c' | 'd') if ctrl => return Action::Quit,
        KeyCode::Esc => return Action::Quit,

        // The stick. WASD for the two axes that point the nose, QE for the one
        // that turns the sky about it; `i`/`k` pitch too, as they always have.
        KeyCode::Char('w' | 'W' | 'i' | 'I') => flight.ship.nudge_pitch(-1.0),
        KeyCode::Char('s' | 'S' | 'k' | 'K') => flight.ship.nudge_pitch(1.0),
        KeyCode::Left | KeyCode::Char('a' | 'A') => flight.ship.nudge_yaw(-1.0),
        KeyCode::Right | KeyCode::Char('d' | 'D') => flight.ship.nudge_yaw(1.0),
        KeyCode::Char('q' | 'Q') => flight.ship.nudge_roll(-1.0),
        KeyCode::Char('e' | 'E') => flight.ship.nudge_roll(1.0),

        // The throttle is the arrows, which is where it has always been: only
        // its letters went to the stick.
        KeyCode::Up => flight.ship.nudge_throttle(1.0),
        KeyCode::Down => flight.ship.nudge_throttle(-1.0),

        KeyCode::Char(' ') => {
            flight.ship.toggle_warp();
        }
        KeyCode::Char('p' | 'P') => *paused = !*paused,
        KeyCode::Char('r' | 'R') => {
            flight.ship.reset();
            flight.ship.throttle = args.throttle;
            *paused = false;
        }
        KeyCode::Char('+' | '=') => flight.resize_pool(1.25, 64),
        KeyCode::Char('-' | '_') => flight.resize_pool(0.8, 64),

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

    let _guard = RawGuard::new()?;
    let mut out = BufWriter::with_capacity(1 << 20, io::stdout());
    out.queue(terminal::Clear(terminal::ClearType::All))?;

    let mut paused = false;
    let mut fps = args.fps as f32;
    let frame_budget = Duration::from_secs_f32(1.0 / args.fps as f32);
    let start = Instant::now();
    let mut last = start;

    'flying: loop {
        let frame_start = Instant::now();

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
        loop {
            // Saturating, not checked: a frame that has already run over its
            // budget has nothing left to wait, but a terminal too slow to keep
            // up still has to be quittable, so the queue is drained either way.
            let remaining = frame_budget.saturating_sub(frame_start.elapsed());
            if !event::poll(remaining)? {
                break;
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
                }
                // Only repaint if the size really changed: terminals emit
                // resize events that settle on the size we already have, and
                // clearing on those makes the field blink for no reason.
                Event::Resize(cols, rows) => {
                    let changed = flight.resize(args, cols as usize, rows as usize);
                    if changed {
                        out.queue(terminal::Clear(terminal::ClearType::All))?;
                        flight.renderer.screen().force_redraw();
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Render frames to stdout with a fixed timestep. No raw mode, no alternate
/// screen — the same seed always produces the same bytes.
fn run_headless(args: &Args) -> io::Result<()> {
    let (cols, rows) = resolved_size(args);
    let mut flight = Flight::new(args, cols as usize, rows as usize);
    let mut out = BufWriter::with_capacity(1 << 20, io::stdout());
    let dt = 1.0 / args.fps as f32;

    for frame in 0..args.frames {
        if args.demo.is_some() {
            flight
                .autopilot
                .update(&mut flight.ship, frame as f64 * dt as f64);
        }
        flight.advance(dt);
        flight.draw(args.fps as f32, false, true);
        flight.present_plain(&mut out)?;
    }
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
        w * args.scale.max(1),
        h * args.scale.max(1),
        flight.ship.velocity_c()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args_for;
    use crate::term::ColorMode;

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
    fn the_automatic_star_count_scales_with_the_canvas() {
        let args = args_for(&[]);
        let small = Renderer::new(40, 12, ColorMode::Truecolor, 1.0);
        let large = Renderer::new(300, 90, ColorMode::Truecolor, 1.0);
        let (a, b) = (star_count(&args, &small), star_count(&args, &large));
        assert!(a >= AUTO_MIN_STARS && b <= AUTO_MAX_STARS);
        assert!(b > a, "a bigger window should hold more stars: {a} vs {b}");

        let explicit = args_for(&["--stars", "1234"]);
        assert_eq!(star_count(&explicit, &large), 1234);
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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);

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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);

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
                    // One key, one axis: the stick must not cross-couple, and
                    // W and S must no longer touch the throttle they used to be.
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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);

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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);

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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
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
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);

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
    fn a_flight_survives_being_resized_underneath_it() {
        let args = args_for(&[]);
        let mut flight = Flight::new(&args, 80, 24);
        for (cols, rows) in [(80, 24), (250, 70), (8, 3), (1, 1), (120, 40)] {
            flight.resize(&args, cols, rows);
            for _ in 0..20 {
                flight.advance(1.0 / 60.0);
                flight.draw(60.0, false, true);
            }
            flight.renderer.present(&mut Vec::new()).unwrap();
            assert!(!flight.field.is_empty());
        }
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
    fn resizing_retunes_the_automatic_star_count_but_not_an_explicit_one() {
        let args = args_for(&[]);
        let mut flight = Flight::new(&args, 40, 12);
        let small = flight.field.len();
        flight.resize(&args, 300, 90);
        assert!(
            flight.field.len() > small,
            "a bigger window should hold more stars"
        );

        let args = args_for(&["--stars", "777"]);
        let mut flight = Flight::new(&args, 40, 12);
        flight.resize(&args, 300, 90);
        assert_eq!(
            flight.field.len(),
            777,
            "an explicit count is not a suggestion"
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
