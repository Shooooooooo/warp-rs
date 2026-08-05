//! Running a flight: the loops that drive one, and the state they drive.
//!
//! Three of them, sharing a `Flight` and differing only in where the frames
//! end up — an interactive terminal, stdout, or a PNG.

use crate::autopilot::Autopilot;
use crate::cli::{resolved_size, Args};
use crate::hud::Readout;
use crate::render::Renderer;
use crate::ship::Ship;
#[cfg(feature = "snapshot")]
use crate::snapshot;
use crate::starfield::StarField;
use crate::term::RawGuard;
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
    renderer: Renderer,
    autopilot: Autopilot,
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
        let field = StarField::new(star_count(args, &renderer), seed(args), &cam);

        Self { ship, field, renderer, autopilot: Autopilot::default(), time: 0.0, accumulator: 0.0 }
    }

    /// Advance by `dt` of wall time, in fixed physics steps.
    pub fn advance(&mut self, dt: f32) {
        self.time += dt as f64;
        self.accumulator += dt;
        while self.accumulator >= SIM_STEP {
            self.ship.update(SIM_STEP);
            let cam = self.renderer.camera(&self.ship, self.time);
            self.field.update(
                SIM_STEP,
                self.ship.speed,
                self.ship.yaw_rate,
                self.ship.pitch_rate,
                &cam,
            );
            self.accumulator -= SIM_STEP;
        }
    }

    pub fn draw(&mut self, fps: f32, paused: bool, hints: bool) {
        let cam = self.renderer.camera(&self.ship, self.time);
        let readout =
            Readout { ship: &self.ship, fps, stars: self.field.len(), paused, hints };
        self.renderer.render(&self.field, &self.ship, &cam, self.time, &readout);
    }

    /// How many stars are currently in flight.
    pub fn stars(&self) -> usize {
        self.field.len()
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
        if args.stars == 0 {
            self.field.resize_pool(star_count(args, &self.renderer));
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
    match key.code {
        KeyCode::Char('c' | 'd') if ctrl => return Action::Quit,
        KeyCode::Char('q' | 'Q') | KeyCode::Esc => return Action::Quit,

        KeyCode::Up | KeyCode::Char('w' | 'W') => flight.ship.nudge_throttle(1.0),
        KeyCode::Down | KeyCode::Char('s' | 'S') => flight.ship.nudge_throttle(-1.0),
        KeyCode::Left | KeyCode::Char('a' | 'A') => flight.ship.nudge_yaw(-1.0),
        KeyCode::Right | KeyCode::Char('d' | 'D') => flight.ship.nudge_yaw(1.0),
        KeyCode::Char('i' | 'I') => flight.ship.nudge_pitch(-1.0),
        KeyCode::Char('k' | 'K') => flight.ship.nudge_pitch(1.0),

        KeyCode::Char(' ') => {
            flight.ship.toggle_warp();
        }
        KeyCode::Char('p' | 'P') => *paused = !*paused,
        KeyCode::Char('r' | 'R') => {
            flight.ship.reset();
            flight.ship.throttle = args.throttle;
            *paused = false;
        }
        KeyCode::Char('+' | '=') => {
            let n = (flight.field.len() as f32 * 1.25) as usize;
            flight.field.resize_pool(n.min(AUTO_MAX_STARS));
        }
        KeyCode::Char('-' | '_') => {
            let n = (flight.field.len() as f32 * 0.8) as usize;
            flight.field.resize_pool(n.max(64));
        }
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
            flight.autopilot.update(&mut flight.ship, frame as f64 * dt as f64);
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
            flight.autopilot.update(&mut flight.ship, frame as f64 * dt as f64);
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
            assert!(flight.time > start, "the clock stopped at {start} s ({days:.1} days)");

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

        handle_key(press(KeyCode::Left), &mut flight, &args, &mut paused);
        assert!(flight.ship.yaw_rate < 0.0);
        handle_key(press(KeyCode::Char('i')), &mut flight, &args, &mut paused);
        assert!(flight.ship.pitch_rate < 0.0);

        handle_key(press(KeyCode::Char('p')), &mut flight, &args, &mut paused);
        assert!(paused);

        let stars = flight.field.len();
        handle_key(press(KeyCode::Char('+')), &mut flight, &args, &mut paused);
        assert!(flight.field.len() > stars);
        handle_key(press(KeyCode::Char('-')), &mut flight, &args, &mut paused);
        assert!(flight.field.len() < stars * 2);

        handle_key(press(KeyCode::Char('r')), &mut flight, &args, &mut paused);
        assert!(!flight.ship.warp_engaged && !paused);
        assert_eq!(flight.ship.throttle, args.throttle);
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
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert!(matches!(
                handle_key(key, &mut flight, &args, &mut paused),
                Action::Quit
            ));
        }
        // A plain 'c' is not a quit — it would be a nasty surprise if it were.
        assert!(matches!(
            handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
                &mut flight,
                &args,
                &mut paused
            ),
            Action::Continue
        ));
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
            assert!(!flight.field.is_empty(), "the star pool emptied at {cols}x{rows}");
            assert!(flight.ship.speed.is_finite());

            let (sc, sr) = flight.renderer.screen().dims();
            assert_eq!((cw, ch), (sc, sr * 2), "canvas and screen disagree at {cols}x{rows}");
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
        assert!(flight.resize(&args, 80, 24), "an unforced size follows the terminal");
        assert_eq!(flight.renderer.canvas_dims(), (80, 48));
        assert!(!flight.resize(&args, 80, 24), "settling on the same size is no change");
    }

    #[test]
    fn resizing_retunes_the_automatic_star_count_but_not_an_explicit_one() {
        let args = args_for(&[]);
        let mut flight = Flight::new(&args, 40, 12);
        let small = flight.field.len();
        flight.resize(&args, 300, 90);
        assert!(flight.field.len() > small, "a bigger window should hold more stars");

        let args = args_for(&["--stars", "777"]);
        let mut flight = Flight::new(&args, 40, 12);
        flight.resize(&args, 300, 90);
        assert_eq!(flight.field.len(), 777, "an explicit count is not a suggestion");
    }

    #[test]
    fn a_long_flight_stays_finite() {
        let args = args_for(&["--seed", "3", "--stars", "800", "--size", "60x20", "--engage"]);
        let mut flight = Flight::new(&args, 60, 20);
        let mut autopilot = Autopilot::default();
        for frame in 0..3000 {
            autopilot.update(&mut flight.ship, frame as f64 / 60.0);
            flight.advance(1.0 / 60.0);
        }
        flight.draw(60.0, false, true);
        assert!(flight.ship.speed.is_finite() && flight.ship.distance_ly.is_finite());
        assert!(flight.renderer.pixels().iter().any(|p| p.iter().any(|v| *v > 0)));
    }
}
