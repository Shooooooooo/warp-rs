//! warp-rs — fly a starship through the universe at warp, in your terminal.

mod canvas;
mod hud;
mod render;
mod ship;
mod starfield;
mod term;

#[cfg(feature = "snapshot")]
mod snapshot;

use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{terminal, QueueableCommand};
use hud::Readout;
use render::Renderer;
use ship::Ship;
use starfield::StarField;
use std::io::{self, BufWriter, Write};
use std::time::{Duration, Instant};
use term::{ColorMode, RawGuard};

/// Physics runs on a fixed step so the flight model behaves the same whether
/// the terminal can keep up or not.
const SIM_STEP: f32 = 1.0 / 120.0;
/// A stalled process must not fast-forward the universe on the next frame.
const MAX_FRAME_DT: f32 = 0.25;
/// Stars per subpixel when the count is chosen automatically.
const AUTO_DENSITY: f32 = 0.05;
const AUTO_MIN_STARS: usize = 300;
const AUTO_MAX_STARS: usize = 20_000;
/// Fallback canvas size when there is no terminal to measure.
const FALLBACK_SIZE: (u16, u16) = (160, 48);

/// Ceiling on the canvas, in terminal cells. A cell costs about 54 bytes across
/// the four buffers a frame needs — two subpixels of HDR float, two of resolved
/// RGB, and a front and back cell — so this is roughly 110 MB. Far past any real
/// terminal, and small enough that it allocates instead of aborting.
const MAX_CELLS: usize = 2_000_000;
/// And no single dimension past this, so the error names the obvious mistake
/// rather than quoting a product.
const MAX_DIM: u16 = 10_000;
/// Ceiling on `--stars`, fifty times the automatic maximum. A `Star` is 40
/// bytes, so this is 40 MB of pool.
const MAX_STARS: usize = 1_000_000;

#[derive(Parser, Debug)]
#[command(
    name = "warp",
    version,
    about = "Fly a starship through the universe at warp, in your terminal"
)]
struct Args {
    /// How many stars to keep in flight. 0 suits the count to the terminal.
    #[arg(
        long,
        default_value_t = 0,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(0..=MAX_STARS as u64)
    )]
    stars: usize,

    /// Frame rate cap.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=240))]
    fps: u32,

    /// Fly on autopilot for this many seconds, then exit. Needs no keyboard.
    #[arg(long, num_args = 0..=1, default_missing_value = "45", value_name = "SECS")]
    demo: Option<f32>,

    /// Screensaver: fly on autopilot indefinitely and quit on *any* key, so it
    /// can be dropped straight into tmux's `lock-command`.
    #[arg(long)]
    screensaver: bool,

    /// Print frames to stdout instead of taking over the terminal.
    #[arg(long)]
    headless: bool,

    /// Frames to print in headless mode.
    #[arg(long, default_value_t = 1, value_name = "N")]
    frames: u32,

    /// Colour depth. Auto-detected from COLORTERM and TERM by default.
    #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
    color: ColorArg,

    /// Seed for the sky. Omit for a different one every run.
    #[arg(long)]
    seed: Option<u64>,

    /// Force a size, e.g. 200x50. Defaults to the terminal's.
    #[arg(long, value_name = "COLSxROWS", value_parser = parse_size)]
    size: Option<(u16, u16)>,

    /// Throttle to start at, 0..=1.
    #[arg(long, default_value_t = 0.18, value_parser = unit_interval)]
    throttle: f32,

    /// Start with the warp drive already lit.
    #[arg(long)]
    engage: bool,

    /// Tonemap exposure. Higher is brighter.
    #[arg(long, default_value_t = 1.9, value_parser = positive)]
    exposure: f32,

    /// Write a PNG of one frame and exit. The instrument panel is not drawn.
    #[cfg(feature = "snapshot")]
    #[arg(long, value_name = "FILE")]
    snapshot: Option<std::path::PathBuf>,

    /// Frames to simulate before taking the snapshot.
    #[cfg(feature = "snapshot")]
    #[arg(long, default_value_t = 300, value_name = "N")]
    warmup: u32,

    /// Magnification of the snapshot image.
    #[cfg(feature = "snapshot")]
    #[arg(long, default_value_t = 3, value_name = "N")]
    scale: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ColorArg {
    Auto,
    Truecolor,
    #[value(name = "256")]
    Ansi256,
    Ascii,
}

impl ColorArg {
    fn resolve(self) -> ColorMode {
        match self {
            ColorArg::Auto => ColorMode::detect(),
            ColorArg::Truecolor => ColorMode::Truecolor,
            ColorArg::Ansi256 => ColorMode::Ansi256,
            ColorArg::Ascii => ColorMode::Ascii,
        }
    }
}

fn parse_size(text: &str) -> Result<(u16, u16), String> {
    let (w, h) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected COLSxROWS, got `{text}`"))?;
    let parse = |s: &str, what: &str| {
        s.trim()
            .parse::<u16>()
            .map_err(|_| format!("`{s}` is not a valid number of {what}"))
            .and_then(|v| match v {
                0 => Err(format!("{what} must not be zero")),
                v if v > MAX_DIM => Err(format!("{v} {what} is past the limit of {MAX_DIM}")),
                v => Ok(v),
            })
    };
    let (cols, rows) = (parse(w, "columns")?, parse(h, "rows")?);
    // Checked as a product too: each dimension can be reasonable while the area
    // is not, and it is the area that decides how much gets allocated.
    if cols as usize * rows as usize > MAX_CELLS {
        return Err(format!(
            "{cols}x{rows} is {} cells, past the limit of {MAX_CELLS}",
            cols as usize * rows as usize
        ));
    }
    Ok((cols, rows))
}

fn unit_interval(text: &str) -> Result<f32, String> {
    let v: f32 = text.parse().map_err(|_| format!("`{text}` is not a number"))?;
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("expected a value between 0 and 1, got {v}"))
    }
}

fn positive(text: &str) -> Result<f32, String> {
    let v: f32 = text.parse().map_err(|_| format!("`{text}` is not a number"))?;
    if v > 0.0 && v.is_finite() {
        Ok(v)
    } else {
        Err(format!("expected a positive number, got {v}"))
    }
}

fn main() {
    let args = Args::parse();
    if let Err(err) = run(&args) {
        // The guard has already put the terminal back by the time this prints.
        eprintln!("warp: {err}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> io::Result<()> {
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
struct Flight {
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
    fn new(args: &Args, cols: usize, rows: usize) -> Self {
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
    fn advance(&mut self, dt: f32) {
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

    fn draw(&mut self, fps: f32, paused: bool, hints: bool) {
        let cam = self.renderer.camera(&self.ship, self.time);
        let readout =
            Readout { ship: &self.ship, fps, stars: self.field.len(), paused, hints };
        self.renderer.render(&self.field, &self.ship, &cam, self.time, &readout);
    }

    /// Adapt to a new terminal size. Reports whether anything actually moved,
    /// so the caller can skip the repaint a resize otherwise forces.
    fn resize(&mut self, args: &Args, cols: usize, rows: usize) -> bool {
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

/// Flies the ship when nobody is at the stick: a repeating run-up to warp,
/// a long cruise with a lazy weave, and a drop back to impulse.
#[derive(Default)]
struct Autopilot {
    phase: usize,
}

impl Autopilot {
    /// Length of one full run-up-and-drop-out cycle, in seconds.
    const CYCLE: f64 = 46.0;

    /// `elapsed` is `f64` for the same reason `Flight::time` is: a screensaver
    /// runs for days, and this has to keep cycling for all of them.
    fn update(&mut self, ship: &mut Ship, elapsed: f64) {
        // Inside one cycle, so everything downstream is small enough for `f32`.
        let t = (elapsed % Self::CYCLE) as f32;
        let phase = match t {
            t if t < 6.0 => 0,  // sublight, easing the throttle up
            t if t < 32.0 => 1, // at warp
            t if t < 40.0 => 2, // dropping out
            _ => 3,             // coasting before the next run
        };

        if phase != self.phase {
            match phase {
                1 => {
                    ship.throttle = 0.55;
                    if !ship.warp_engaged {
                        ship.toggle_warp();
                    }
                }
                2 => {
                    if ship.warp_engaged {
                        ship.toggle_warp();
                    }
                }
                _ => {}
            }
            self.phase = phase;
        }

        match phase {
            0 => ship.throttle = (0.15 + t * 0.10).min(0.80),
            1 => {
                ship.throttle = (0.55 + (t - 6.0) * 0.025).min(1.0);
                // A slow weave, so the view is never quite static. Evaluated in
                // `f64` so the argument reduction stays exact indefinitely.
                ship.nudge_yaw((elapsed * 0.31).sin() as f32 * 0.003);
                ship.nudge_pitch((elapsed * 0.19).cos() as f32 * 0.002);
            }
            2 => ship.throttle = (ship.throttle - 0.004).max(0.25),
            _ => ship.throttle = (ship.throttle - 0.002).max(0.15),
        }
    }
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

        while event::poll(Duration::ZERO)? {
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
                    if flight.resize(args, cols as usize, rows as usize) {
                        out.queue(terminal::Clear(terminal::ClearType::All))?;
                        flight.renderer.screen().force_redraw();
                    }
                }
                _ => {}
            }
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

        if let Some(remaining) = frame_budget.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(remaining);
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
        flight.renderer.present_plain(&mut out)?;
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

/// The size to render at: whatever was asked for, else the terminal's, else a
/// sensible default for a process with no terminal at all.
fn resolved_size(args: &Args) -> (u16, u16) {
    let (cols, rows) = args
        .size
        .or_else(|| terminal::size().ok())
        .filter(|(c, r)| *c > 0 && *r > 0)
        .unwrap_or(FALLBACK_SIZE);
    // `--size` was vetted at parse time, but an ioctl answer arrives unvetted,
    // and a terminal that claims to be enormous should not be believed to the
    // point of exhausting memory over it.
    clamp_size(cols, rows)
}

/// Bring a size inside `MAX_DIM` and `MAX_CELLS`. Both axes scale by the same
/// factor, so clamping shrinks the view without squashing the field.
fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    let (cols, rows) = (cols.clamp(1, MAX_DIM), rows.clamp(1, MAX_DIM));
    let cells = cols as usize * rows as usize;
    if cells <= MAX_CELLS {
        return (cols, rows);
    }
    let shrink = (MAX_CELLS as f64 / cells as f64).sqrt();
    let scaled = |v: u16| ((v as f64 * shrink).floor() as u16).max(1);
    (scaled(cols), scaled(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_for(extra: &[&str]) -> Args {
        let mut argv = vec!["warp"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("arguments should parse")
    }

    #[test]
    fn defaults_are_sane() {
        let args = args_for(&[]);
        assert_eq!(args.fps, 60);
        assert_eq!(args.stars, 0);
        assert!(args.demo.is_none() && !args.headless && !args.engage);
    }

    #[test]
    fn demo_takes_an_optional_duration() {
        assert_eq!(args_for(&["--demo"]).demo, Some(45.0));
        assert_eq!(args_for(&["--demo", "12.5"]).demo, Some(12.5));
    }

    #[test]
    fn sizes_parse_and_bad_ones_are_rejected() {
        assert_eq!(parse_size("200x50").unwrap(), (200, 50));
        assert_eq!(parse_size(" 80 X 24 ").unwrap(), (80, 24));
        for bad in ["", "200", "x50", "200x", "0x50", "200x0", "-4x9", "axb"] {
            assert!(parse_size(bad).is_err(), "`{bad}` should not parse");
        }
    }

    #[test]
    fn numeric_arguments_are_range_checked() {
        assert!(Args::try_parse_from(["warp", "--throttle", "1.5"]).is_err());
        assert!(Args::try_parse_from(["warp", "--throttle", "-0.1"]).is_err());
        assert!(Args::try_parse_from(["warp", "--exposure", "0"]).is_err());
        assert!(Args::try_parse_from(["warp", "--fps", "0"]).is_err());
        assert!(Args::try_parse_from(["warp", "--fps", "999"]).is_err());
        assert!(Args::try_parse_from(["warp", "--throttle", "0.5"]).is_ok());
    }

    #[test]
    fn size_and_star_counts_are_bounded() {
        // Regression: neither was bounded. `--stars 500000000` asked for 20 GB
        // and `--size 60000x60000` for 86 GB, and a failed allocation aborts
        // the process — no unwind, no `Drop`, no panic hook — so interactively
        // it left the terminal in raw mode on the alternate screen.
        assert!(Args::try_parse_from(["warp", "--stars", "500000000"]).is_err());
        assert!(Args::try_parse_from(["warp", "--stars", "1000001"]).is_err());
        assert!(Args::try_parse_from(["warp", "--stars", "1000000"]).is_ok());
        assert!(Args::try_parse_from(["warp", "--stars", "0"]).is_ok(), "0 still means auto");

        assert!(parse_size("60000x60000").is_err());
        assert!(parse_size("20000x10").is_err(), "past the per-dimension limit");
        // Each dimension legal on its own, but the area is what gets allocated.
        assert!(parse_size("10000x10000").is_err());
        assert!(parse_size("2000x1000").is_ok(), "exactly the cell ceiling");
        assert!(parse_size("2000x1001").is_err(), "one row past it");
    }

    #[test]
    fn a_preposterous_terminal_is_clamped_rather_than_believed() {
        // `--size` is vetted when it is parsed, but an ioctl answer is not, and
        // a terminal claiming to be 65535 square would ask for about 100 GB.
        for (cols, rows) in [
            (0, 0),
            (1, 1),
            (80, 24),
            (u16::MAX, u16::MAX),
            (u16::MAX, 1),
            (1, u16::MAX),
            (10_000, 10_000),
            (3000, 900),
        ] {
            let (c, r) = clamp_size(cols, rows);
            let cells = c as usize * r as usize;
            assert!(c >= 1 && r >= 1, "{cols}x{rows} clamped to a zero dimension");
            assert!(c <= MAX_DIM && r <= MAX_DIM, "{cols}x{rows} -> {c}x{r}");
            assert!(cells <= MAX_CELLS, "{cols}x{rows} -> {c}x{r}, which is {cells} cells");
        }
        assert_eq!(clamp_size(80, 24), (80, 24), "a sane size is left alone");
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
    fn the_autopilot_still_cycles_after_days_aloft() {
        // Same clock, same problem: `elapsed % CYCLE` needs the resolution to
        // land in distinct phases one frame apart.
        let dt = 1.0 / 60.0f64;
        for start in [0.0f64, 86_400.0, 604_800.0, 10_000_000.0] {
            let mut ship = Ship::new();
            let mut autopilot = Autopilot::default();
            let (mut peak, mut engaged) = (0.0f32, false);

            for frame in 0..(2.0 * Autopilot::CYCLE / dt) as usize {
                autopilot.update(&mut ship, start + frame as f64 * dt);
                ship.update(dt as f32);
                peak = peak.max(ship.velocity_c());
                engaged |= ship.warp_engaged;
            }
            let days = start / 86_400.0;
            assert!(engaged, "the drive never lit {days:.1} days in");
            assert!(peak > 100.0, "never got up to speed {days:.1} days in: {peak} c");
        }
    }

    #[test]
    fn color_modes_resolve() {
        assert_eq!(args_for(&["--color", "256"]).color.resolve(), ColorMode::Ansi256);
        assert_eq!(args_for(&["--color", "ascii"]).color.resolve(), ColorMode::Ascii);
        assert_eq!(
            args_for(&["--color", "truecolor"]).color.resolve(),
            ColorMode::Truecolor
        );
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
    fn the_autopilot_completes_a_cycle_without_getting_stuck() {
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let dt = 1.0 / 60.0;
        let mut peak: f32 = 0.0;
        let mut engaged_at_some_point = false;

        for frame in 0..(Autopilot::CYCLE / dt as f64) as usize {
            autopilot.update(&mut ship, frame as f64 * dt as f64);
            ship.update(dt);
            peak = peak.max(ship.velocity_c());
            engaged_at_some_point |= ship.warp_engaged;
            assert!((0.0..=1.0).contains(&ship.throttle));
        }
        assert!(engaged_at_some_point, "the autopilot never lit the drive");
        assert!(peak > 100.0, "the autopilot never got up to speed: {peak} c");
        assert!(!ship.warp_engaged, "the cycle should end back at impulse");
        assert!(ship.velocity_c() < 1.0, "it should be sublight by the end");
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
            assert!(flight.field.len() >= 1);
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
            assert!(flight.field.len() >= 1, "the star pool emptied at {cols}x{rows}");
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

