//! The command line, and the sizes it is allowed to ask for.

use crate::canvas;
use crate::models;
use crate::term::ColorMode;
use crate::view::{Orbit, ViewMode};
use clap::{Parser, ValueEnum};
use crossterm::terminal;

/// Fallback canvas size when there is no terminal to measure.
const FALLBACK_SIZE: (u16, u16) = (160, 48);

/// Ceiling on the canvas, in terminal cells. A cell costs about 54 bytes across
/// the four buffers a frame needs — two subpixels of HDR float, two of resolved
/// RGB, and a front and back cell — so this is roughly 110 MB. Far past any
/// real terminal, and small enough that it allocates instead of aborting.
pub(crate) const MAX_CELLS: usize = 2_000_000;
/// And no single dimension past this, so the error names the obvious mistake
/// rather than quoting a product.
const MAX_DIM: u16 = 10_000;
/// The faintest star a sky holds, and the two ends the flag is held to.
pub const DEFAULT_MAGNITUDE: f32 = 6.0;
pub const MIN_MAGNITUDE: f32 = -2.0;
pub const MAX_MAGNITUDE: f32 = 9.5;
/// So a default the parser would refuse is a compile error rather than a
/// command line that fails on every invocation. `default_value_t` is spelled
/// and then read back through the `value_parser` like anything typed at the
/// prompt, so a default past the ceiling would not be found until the binary
/// was run — which is the one way a number in this file can be wrong without
/// anything here saying so.
const _: () = assert!(
    DEFAULT_MAGNITUDE >= MIN_MAGNITUDE && DEFAULT_MAGNITUDE <= MAX_MAGNITUDE,
    "the default limiting magnitude is outside its own bounds"
);
/// How long a camera change takes, in seconds, dipping through black and coming
/// back — and, because the program opens at the bottom of a cut it was never on
/// the other side of, how long the sky takes to arrive at the start of a run.
pub const DEFAULT_FADE: f32 = 0.6;
/// The ceiling on `--fade`, for the reason every other number in this file has
/// one: a flag that answers "how long" with nothing holding it is how `--stars`
/// and `--magnitude` both went wrong. Nothing here allocates — what an absurd
/// value costs is a picture rather than an address space, since `--fade 1e9` is
/// a program that is simply always black — so the bound is generous rather than
/// tight. Five seconds is already longer than a good many `--demo` runs.
const MAX_FADE: f32 = 5.0;
/// The same compile-time guard [`DEFAULT_MAGNITUDE`] carries, and for the same
/// reason: a default the parser would refuse fails on every invocation rather
/// than at build time.
const _: () = assert!(
    DEFAULT_FADE >= 0.0 && DEFAULT_FADE <= MAX_FADE,
    "the default fade is outside its own bounds"
);
/// Ceiling on the two counts that are spent rather than allocated — `--frames`
/// and `--warmup`. Nothing runs out of memory over these; a `u32` of them is
/// simply a process that never comes back, and at sixty a second this is
/// already four hours of flying. Bounded anyway, because a number on this
/// command line that answers "how many" and has no limit is how the two above
/// went wrong.
const MAX_COUNT: u32 = 1_000_000;
/// Ceiling on `--scale`. This one *is* an allocation, and it enters squared:
/// the image is the canvas magnified on both axes, so at the snapshot's default
/// 240x68 — 240 by 136 subpixels — sixteen comes to 3840 by 2176, or 25 MB of
/// RGB. Past that the useful range has long since been left behind; the
/// README's two snapshots, which are the whole of `docs/`, are both taken at 2.
#[cfg(feature = "snapshot")]
const MAX_SCALE: usize = 16;

// The keys, printed under the flags rather than squeezed into the frame.
const CONTROLS: &str = "\
Controls:
  SPACE          engage or drop out of warp
  UP, DOWN       throttle
  C              change camera: the cockpit, or a chase view from outside
  + -            a fainter or a brighter sky, half a magnitude at a time
  P              pause
  R              back to the view and the throttle the flags asked for
  M              the ship picker (from outside)
  ESC, Ctrl-C    quit

  From the cockpit:
  W A S D        pitch and yaw; I and K pitch too
  Q E            roll

  From outside:
  W A S D Q E    walk the camera round the ship
  [ ] or wheel   push it in or out";

/// Every flag the command line takes, and the bounds each one is held to.
#[derive(Parser, Debug)]
#[command(
    name = "warp",
    version,
    about = "Fly a starship through the universe at warp, in your terminal",
    long_about = None,
    after_help = CONTROLS
)]
pub struct Args {
    /// How faint a star the sky holds, as a limiting visual magnitude.
    // `allow_hyphen_values` because the bottom of the range is negative and
    // without it clap reads `--magnitude -2` as a flag it does not know rather
    // than as an empty sky.
    #[arg(
        long,
        value_name = "MAG",
        default_value_t = DEFAULT_MAGNITUDE,
        allow_hyphen_values = true,
        value_parser = magnitude
    )]
    pub magnitude: f32,

    /// Gone, and refused by name rather than by silence.
    #[arg(long, value_name = "N", hide = true, value_parser = no_star_count)]
    pub stars: Option<String>,

    /// Frame rate cap — and, in `--headless` and `--snapshot`, the simulation
    /// timestep, so it changes the flight rather than only how often it is
    /// drawn.
    // Two meanings, and the help says both because leaving the second one out
    // is how somebody sets `--fps 10` on a headless run to save time and gets a
    // twelve-second flight instead of a two-second one, with every hash moved.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=240))]
    pub fps: u32,

    /// Fly on autopilot for this many seconds, then exit. Needs no keyboard.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "45",
        value_name = "SECS",
        // Unchecked, `--demo=-5` quit before drawing a single frame and
        // `--demo=nan` never reached its deadline at all — a screensaver
        // without the any-key-quits contract that makes one safe to use.
        value_parser = positive,
        // Both fly themselves, but only one of them has a deadline.
        conflicts_with = "screensaver",
    )]
    pub demo: Option<f32>,

    /// Screensaver: fly on autopilot indefinitely and quit on *any* key, so it
    /// can be dropped straight into tmux's `lock-command`.
    #[arg(long, conflicts_with = "headless")]
    pub screensaver: bool,

    /// Print frames to stdout instead of taking over the terminal.
    #[arg(long)]
    pub headless: bool,

    /// Frames to print in headless mode.
    #[arg(
        long,
        default_value_t = 1,
        value_name = "N",
        requires = "headless",
        value_parser = clap::value_parser!(u32).range(1..=MAX_COUNT as i64)
    )]
    pub frames: u32,

    /// Colour depth. 24-bit unless one of the narrower modes is named.
    // There was an `auto` here that read `COLORTERM` and then `TERM`, and it
    // guessed against the renderer: a terminal exporting no `COLORTERM` got the
    // 256-colour palette whatever it could really do, which is most terminals,
    // so the mode the canvas is designed for was the one it least often opened
    // in.
    #[arg(long, value_enum, default_value_t = ColorArg::Truecolor)]
    pub color: ColorArg,

    /// Seed for the sky. Omit for a different one every run.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Force a size, e.g. 200x50. Defaults to the terminal's.
    #[arg(long, value_name = "COLSxROWS", value_parser = parse_size)]
    pub size: Option<(u16, u16)>,

    /// Throttle to start at, 0..=1.
    #[arg(long, default_value_t = 0.18, value_parser = unit_interval)]
    pub throttle: f32,

    /// Start with the warp drive already lit.
    #[arg(long)]
    pub engage: bool,

    /// Which camera to fly behind. `C` cycles them at the stick.
    #[arg(long, value_enum, default_value_t = ViewArg::Cockpit)]
    pub view: ViewArg,

    /// Which ship to fly. Only visible from outside; `M` opens the picker.
    #[arg(long, value_name = "NAME", default_value = "enterprise", value_parser = parse_ship)]
    pub ship: usize,

    /// Where to park the outside camera, in degrees: round the ship, then over
    /// it, then its own roll. `WASD` and `QE` fly it from there.
    // `allow_hyphen_values` because half the range starts with a minus sign,
    // and without it clap reads `--orbit -75,10` as a flag it does not know
    // rather than as a camera behind the ship.
    #[arg(
        long,
        value_name = "AZ,EL[,ROLL]",
        default_value = "0,0",
        allow_hyphen_values = true,
        value_parser = parse_orbit
    )]
    pub orbit: Orbit,

    /// Tonemap exposure. Higher is brighter.
    #[arg(long, default_value_t = 1.9, value_parser = positive)]
    pub exposure: f32,

    /// Seconds a camera change takes, dipping through black on the way. The sky
    /// arrives over the tail of one at the start of a run. `0` changes camera
    /// instantly and opens on a lit sky, which is also what `--snapshot
    /// --warmup 0` wants, since without it the shot is taken at the bottom of
    /// the dip and comes out black.
    // `allow_hyphen_values` for the reason `--magnitude` carries it: without it
    // clap answers `--fade -1` with "unexpected argument", where the parser
    // below answers with the range.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = DEFAULT_FADE,
        allow_hyphen_values = true,
        value_parser = fade_seconds
    )]
    pub fade: f32,

    /// How finely a hull's outline is measured, in samples per subpixel on each
    /// axis. `1` is a hard-edged outline, with no anti-aliasing at all.
    #[arg(
        long,
        default_value_t = canvas::HULL_SAMPLES,
        value_name = "N",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new()
            .range(1..=canvas::MAX_HULL_SAMPLES as u64)
    )]
    pub aa: usize,

    /// Write a PNG of one frame and exit. The instrument panel is not drawn.
    #[cfg(feature = "snapshot")]
    #[arg(long, value_name = "FILE", conflicts_with = "headless")]
    pub snapshot: Option<std::path::PathBuf>,

    /// Frames to simulate before taking the snapshot.
    #[cfg(feature = "snapshot")]
    #[arg(
        long,
        default_value_t = 300,
        value_name = "N",
        requires = "snapshot",
        value_parser = clap::value_parser!(u32).range(0..=MAX_COUNT as i64)
    )]
    pub warmup: u32,

    /// Magnification of the snapshot image.
    #[cfg(feature = "snapshot")]
    #[arg(
        long,
        default_value_t = 3,
        value_name = "N",
        requires = "snapshot",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=MAX_SCALE as u64)
    )]
    pub scale: usize,
}

// A `///` here rather than the `//` this file otherwise insists on, and the
// exception is real rather than an oversight: clap's derive publishes doc
// comments on the struct and on its *fields*, which is why a note to the next
// editor cannot be one there.
impl Args {
    /// Whether there is nobody at the controls: `--demo` and `--screensaver`.
    pub fn unattended(&self) -> bool {
        self.demo.is_some() || self.screensaver
    }
}

/// How much colour to spell a cell in, as the command line names it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Truecolor,
    #[value(name = "256")]
    Ansi256,
    Ascii,
}

impl ColorArg {
    pub fn resolve(self) -> ColorMode {
        match self {
            ColorArg::Truecolor => ColorMode::Truecolor,
            ColorArg::Ansi256 => ColorMode::Ansi256,
            ColorArg::Ascii => ColorMode::Ascii,
        }
    }
}

/// Which camera the flight starts behind.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ViewArg {
    Cockpit,
    Side,
}

impl ViewArg {
    pub fn resolve(self) -> ViewMode {
        match self {
            ViewArg::Cockpit => ViewMode::Cockpit,
            ViewArg::Side => ViewMode::Side,
        }
    }
}

/// A ship by name, as an index into [`crate::models::models`]. Parsed here
/// rather than carried as a string so an unknown name is an error message at
/// the command line instead of a silent fall back to the default.
fn parse_ship(text: &str) -> Result<usize, String> {
    models::by_name(text).ok_or_else(|| {
        let known: Vec<&str> = models::models().iter().map(|m| m.name).collect();
        format!("`{text}` is not a ship. Try one of: {}", known.join(", "))
    })
}

/// Two or three angles in degrees, comma-separated: round the ship, over it,
/// and the camera's own roll.
///
/// The one number on this command line with no bound on it: an angle has no
/// end to run away past, so a preposterous one costs a picture rather than an
/// allocation. [`Orbit::held`] folds it with exactly the fold a keypress gets.
fn parse_orbit(text: &str) -> Result<Orbit, String> {
    let mut angles = [0.0f32; 3];
    let given: Vec<&str> = text.split(',').collect();
    if given.len() < 2 || given.len() > 3 {
        return Err(format!("expected AZ,EL or AZ,EL,ROLL, got `{text}`"));
    }
    for (angle, field) in angles.iter_mut().zip(&given) {
        *angle = field
            .trim()
            .parse::<f32>()
            .map_err(|_| format!("`{field}` is not an angle in degrees"))?;
        if !angle.is_finite() {
            return Err(format!("`{field}` is not an angle in degrees"));
        }
        *angle = angle.to_radians();
    }
    Ok(Orbit {
        azimuth: angles[0],
        elevation: angles[1],
        roll: angles[2],
    }
    .held())
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
    let v: f32 = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number"))?;
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("expected a value between 0 and 1, got {v}"))
    }
}

/// A limiting magnitude, held to the range the pool can be allocated in.
fn magnitude(text: &str) -> Result<f32, String> {
    let v: f32 = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number"))?;
    // Not `clamp`: a NaN passes straight through that and would ask the sky for
    // a pool of `NaN as usize` stars, which is a silent zero.
    if v.is_finite() && (MIN_MAGNITUDE..=MAX_MAGNITUDE).contains(&v) {
        Ok(v)
    } else {
        Err(format!(
            "expected a limiting magnitude between {MIN_MAGNITUDE} and {MAX_MAGNITUDE}, got {text}"
        ))
    }
}

/// How long a cut takes, held to a range rather than clamped into one.
fn fade_seconds(text: &str) -> Result<f32, String> {
    let v: f32 = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number"))?;
    // Not `clamp`, for the reason `magnitude` is not: a NaN goes straight
    // through it, and a NaN here reads as `fade <= 0.0` further down — which is
    // "no fade at all", a silent success rather than a refusal.
    if v.is_finite() && (0.0..=MAX_FADE).contains(&v) {
        Ok(v)
    } else {
        Err(format!(
            "expected a fade of between 0 and {MAX_FADE} seconds, got {text}"
        ))
    }
}

/// Always an error, and the error is the point.
fn no_star_count(text: &str) -> Result<String, String> {
    Err(format!(
        "`--stars {text}` is gone: a sky is asked for by how faint a star it \
         holds, not by how many. Try `--magnitude {DEFAULT_MAGNITUDE}` \
         (between {MIN_MAGNITUDE} and {MAX_MAGNITUDE}); each magnitude is \
         about four times as many stars"
    ))
}

fn positive(text: &str) -> Result<f32, String> {
    let v: f32 = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number"))?;
    if v > 0.0 && v.is_finite() {
        Ok(v)
    } else {
        Err(format!("expected a positive number, got {v}"))
    }
}

/// The size to render at: whatever was asked for, else the terminal's, else a
/// sensible default for a process with no terminal at all.
pub(crate) fn resolved_size(args: &Args) -> (u16, u16) {
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
pub(crate) fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    let (cols, rows) = (cols.clamp(1, MAX_DIM), rows.clamp(1, MAX_DIM));
    let cells = cols as usize * rows as usize;
    if cells <= MAX_CELLS {
        return (cols, rows);
    }
    let shrink = (MAX_CELLS as f64 / cells as f64).sqrt();
    let scaled = |v: u16| ((v as f64 * shrink).floor() as u16).max(1);
    (scaled(cols), scaled(rows))
}

/// Parse an argument list the way `main` does. Tests elsewhere in the crate
/// need a realistic `Args`, and hand-building one would skip the validation
/// that is half the point of this module.
#[cfg(test)]
pub(crate) fn args_for(extra: &[&str]) -> Args {
    let mut argv = vec!["warp"];
    argv.extend_from_slice(extra);
    Args::try_parse_from(argv).expect("arguments should parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let args = args_for(&[]);
        assert_eq!(args.fps, 60);
        // The literal rather than the constant, exactly as the frame rate above
        // it is: this is the one place the defaults are pinned as numbers, and
        // asserting a constant against itself pins nothing.
        assert_eq!(args.magnitude, DEFAULT_MAGNITUDE);
        assert!(args.demo.is_none() && !args.headless && !args.engage);
        // Pinned here as the flag's own default, beside the frame rate and the
        // star count.
        assert_eq!(args.color, ColorArg::Truecolor);
        assert_eq!(args.fade, DEFAULT_FADE);
    }

    #[test]
    fn demo_takes_an_optional_duration() {
        assert_eq!(args_for(&["--demo"]).demo, Some(45.0));
        assert_eq!(args_for(&["--demo", "12.5"]).demo, Some(12.5));
    }

    #[test]
    fn a_demo_has_to_last_a_positive_length_of_time() {
        // Regression: `--demo` was the one number that went unchecked.
        for bad in ["-5", "0", "nan", "inf", "-inf", "banana"] {
            assert!(
                Args::try_parse_from(["warp", &format!("--demo={bad}")]).is_err(),
                "`--demo={bad}` should not parse"
            );
        }
        assert_eq!(args_for(&["--demo", "0.5"]).demo, Some(0.5));
    }

    #[test]
    fn the_modes_that_fly_themselves_are_the_ones_that_say_so() {
        // One predicate answers this in four places now — two autopilot gates
        // and two panel gates — so it is worth pinning that it means what its
        // name says.
        for flying_itself in [
            vec!["--demo"],
            vec!["--demo", "12"],
            vec!["--screensaver"],
            vec!["--headless", "--demo"],
        ] {
            assert!(
                args_for(&flying_itself).unattended(),
                "{flying_itself:?} flies itself and does not say so"
            );
        }
        for flown in [vec![], vec!["--engage"], vec!["--throttle", "1.0"]] {
            assert!(
                !args_for(&flown).unattended(),
                "{flown:?} has a pilot and claims not to"
            );
        }
    }

    #[test]
    fn flags_that_contradict_each_other_are_refused() {
        // Each of these used to be accepted, with one flag quietly winning:
        // `--demo` capped the screensaver, `--headless` ignored it entirely,
        // and `--frames` did nothing at all outside headless mode.
        assert!(Args::try_parse_from(["warp", "--demo", "--screensaver"]).is_err());
        assert!(Args::try_parse_from(["warp", "--screensaver", "--headless"]).is_err());
        assert!(Args::try_parse_from(["warp", "--frames", "10"]).is_err());

        // The combinations that mean something still do.
        assert!(Args::try_parse_from(["warp", "--headless", "--frames", "10"]).is_ok());
        assert!(Args::try_parse_from(["warp", "--headless", "--demo"]).is_ok());
        assert!(Args::try_parse_from(["warp", "--screensaver"]).is_ok());
    }

    #[cfg(feature = "snapshot")]
    #[test]
    fn the_snapshot_flags_need_a_snapshot_to_take() {
        assert!(Args::try_parse_from(["warp", "--warmup", "10"]).is_err());
        assert!(Args::try_parse_from(["warp", "--scale", "2"]).is_err());
        assert!(Args::try_parse_from(["warp", "--snapshot", "a.png", "--headless"]).is_err());
        assert!(Args::try_parse_from(["warp", "--snapshot", "a.png", "--warmup", "10"]).is_ok());
    }

    #[test]
    fn the_counts_and_the_magnification_are_bounded_too() {
        // Regression: these three were the numbers this module's own header
        // said could not exist.
        let frames = |n: &str| Args::try_parse_from(["warp", "--headless", "--frames", n]);
        assert!(frames("1000001").is_err());
        assert!(frames("1000000").is_ok());
        assert!(frames("0").is_err(), "a flight of no frames is not one");

        #[cfg(feature = "snapshot")]
        {
            let shot = |flag: &str, n: &str| {
                Args::try_parse_from(["warp", "--snapshot", "a.png", flag, n])
            };
            assert!(shot("--warmup", "1000001").is_err());
            assert!(shot("--warmup", "1000000").is_ok());
            assert!(shot("--warmup", "0").is_ok(), "no warm-up is a legal ask");

            assert!(shot("--scale", "17").is_err());
            assert!(shot("--scale", "16").is_ok());
            assert!(
                shot("--scale", "0").is_err(),
                "a magnification of nothing is an image of nothing"
            );
        }
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

        // `--aa` is a sample grid entered squared, so it is bounded like the
        // rest.
        assert!(Args::try_parse_from(["warp", "--aa", "0"]).is_err());
        let past = (canvas::MAX_HULL_SAMPLES + 1).to_string();
        assert!(Args::try_parse_from(["warp", "--aa", &past]).is_err());
        assert!(Args::try_parse_from(["warp", "--aa", "1"]).is_ok());
        assert_eq!(
            args_for(&[]).aa,
            canvas::HULL_SAMPLES,
            "the flag's default and the canvas's have drifted apart"
        );
    }

    #[test]
    fn size_and_sky_are_bounded() {
        assert!(Args::try_parse_from(["warp", "--magnitude", "12"]).is_err());
        assert!(Args::try_parse_from(["warp", "--magnitude", "-8"]).is_err());
        assert!(Args::try_parse_from(["warp", "--magnitude", "nan"]).is_err());
        assert!(Args::try_parse_from(["warp", "--magnitude", "9.5"]).is_ok());
        assert!(
            Args::try_parse_from(["warp", "--magnitude", "-2"]).is_ok(),
            "an empty sky is a limit like any other"
        );

        assert!(parse_size("60000x60000").is_err());
        assert!(
            parse_size("20000x10").is_err(),
            "past the per-dimension limit"
        );
        // Each dimension legal on its own, but the area is what gets allocated.
        assert!(parse_size("10000x10000").is_err());
        assert!(parse_size("2000x1000").is_ok(), "exactly the cell ceiling");
        assert!(parse_size("2000x1001").is_err(), "one row past it");
    }

    #[test]
    fn the_star_count_flag_says_what_replaced_it() {
        // `--stars` is gone and clap's own answer to a flag it does not know is
        // "unexpected argument", which is no use to a shell history or a script
        // carrying one.
        let err = Args::try_parse_from(["warp", "--stars", "600"])
            .expect_err("a sky is not asked for by the number any more")
            .to_string();
        assert!(
            err.contains("--magnitude"),
            "the error does not name what replaced it: {err}"
        );
    }

    #[test]
    fn the_fade_is_bounded_rather_than_believed() {
        // The bound is not about memory — nothing here allocates — but a fade
        // is a number a person types and a NaN one would read as no fade at all
        // further down, which is the kind of silent success this file refuses
        // everything else by name to avoid.
        let fade = |v: &str| Args::try_parse_from(["warp", "--fade", v]);
        for refused in ["-1", "nan", "inf", "abc", "1e9"] {
            let err = fade(refused)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| panic!("`--fade {refused}` was believed"));
            assert!(
                err.contains("between 0 and") || err.contains("not a number"),
                "`--fade {refused}` was refused without saying what the range is: {err}"
            );
        }
        // Both ends are reached rather than approached, and the near one is the
        // whole of the escape hatch: `--fade 0` is the renderer this arrived
        // on, to the bit.
        for taken in ["0", "0.25"] {
            assert!(fade(taken).is_ok(), "`--fade {taken}` is a legal ask");
        }
        assert_eq!(
            fade(&MAX_FADE.to_string())
                .expect("the ceiling is reachable")
                .fade,
            MAX_FADE
        );
    }

    #[test]
    fn the_help_text_is_addressed_to_whoever_is_running_the_program() {
        // Clap's derive publishes `///` blocks as help, and the house style
        // writes `///` as an essay to the next editor, so the two collide every
        // time a flag grows a note.
        use clap::CommandFactory;
        let long = Args::command().render_long_help().to_string();
        let short = Args::command().render_help().to_string();
        for help in [long.as_str(), short.as_str()] {
            for leak in [
                // Flags and values the program will not accept.
                "--stars",
                "COLORTERM",
                // Names that mean something to rustc and nothing to a pilot.
                "allow_hyphen_values",
                "value_parser",
                "long_about",
                "#[arg",
            ] {
                assert!(
                    !help.contains(leak),
                    "`{leak}` reached the help text, which is addressed to \
                     whoever is running the program:\n{help}"
                );
            }
        }
        assert!(
            long.contains("limiting visual magnitude") && long.contains("Tonemap exposure"),
            "the help stopped describing the flags:\n{long}"
        );
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
            assert!(
                c >= 1 && r >= 1,
                "{cols}x{rows} clamped to a zero dimension"
            );
            assert!(c <= MAX_DIM && r <= MAX_DIM, "{cols}x{rows} -> {c}x{r}");
            assert!(
                cells <= MAX_CELLS,
                "{cols}x{rows} -> {c}x{r}, which is {cells} cells"
            );
        }
        assert_eq!(clamp_size(80, 24), (80, 24), "a sane size is left alone");
    }

    #[test]
    fn the_view_and_the_ship_can_be_chosen_at_the_command_line() {
        let plain = args_for(&[]);
        assert_eq!(
            plain.view.resolve(),
            ViewMode::Cockpit,
            "the default is inside"
        );
        assert_eq!(
            plain.ship,
            models::DEFAULT_MODEL,
            "the default ship is the one the list opens with"
        );

        assert_eq!(args_for(&["--view", "side"]).view.resolve(), ViewMode::Side);
        // Every ship the picker offers can also be named here, or one of them
        // is only reachable by hand.
        for (i, model) in models::models().iter().enumerate() {
            assert_eq!(args_for(&["--ship", model.name]).ship, i);
            assert_eq!(args_for(&["--ship", &model.name.to_uppercase()]).ship, i);
        }
    }

    /// `ViewArg` and `ViewMode` are two enums for one idea, and the names on
    /// the command line come from clap's derive over the first while
    /// [`ViewMode::label`] spells the second. Nothing makes them agree, so this
    /// does. Walks `ViewMode::ALL`, so a third camera is covered the day it is
    /// added rather than the day somebody notices.
    #[test]
    fn the_camera_can_be_parked_at_the_command_line() {
        // Degrees in, radians out, and held to the same range a keypress is
        // held to — the clamp lives with the geometry, so there is only one of
        // it however the number arrived.
        let args = args_for(&["--orbit", "90,45"]);
        assert!((args.orbit.azimuth - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!((args.orbit.elevation - std::f32::consts::FRAC_PI_4).abs() < 1e-5);
        assert_eq!(args.orbit.roll, 0.0, "an omitted roll is no roll");
        assert!(
            args_for(&[]).orbit.is_level(),
            "the shot does not open on the beam"
        );
        // Three angles, and the third is the camera's own roll.
        assert!(!args_for(&["--orbit", "0,0,30"]).orbit.is_level());
        // Wound round several times on every axis, it is folded rather than
        // refused: all three angles go all the way round, so there is no number
        // here to reject, only one to fold.
        let over = args_for(&["--orbit", "720,400,-900"]).orbit;
        for angle in [over.azimuth, over.elevation, over.roll] {
            assert!(
                angle.abs() <= std::f32::consts::PI,
                "an angle got away: {over:?}"
            );
        }
        assert!(
            (over.elevation - 40.0f32.to_radians()).abs() < 1e-5,
            "the elevation was clipped rather than folded: {over:?}"
        );

        let behind = args_for(&["--orbit", "-75,15"]).orbit;
        assert!(behind.azimuth < 0.0 && behind.elevation > 0.0, "{behind:?}");

        for bad in ["", "1", "1,2,3,4", "up,over", "1,nan", "1,inf", "--stars"] {
            assert!(
                Args::try_parse_from(["warp", "--orbit", bad]).is_err(),
                "`{bad}` was accepted as a camera angle"
            );
        }
    }

    #[test]
    fn the_command_line_takes_every_view_by_its_name() {
        for view in ViewMode::ALL {
            let parsed = Args::try_parse_from(["warp", "--view", view.label()])
                .unwrap_or_else(|e| panic!("`--view {}` was refused: {e}", view.label()));
            assert_eq!(
                parsed.view.resolve(),
                view,
                "`--view {}` flew a different camera",
                view.label()
            );
        }
    }

    #[test]
    fn an_unknown_ship_is_refused_and_the_message_says_what_there_is() {
        let err = Args::try_parse_from(["warp", "--ship", "millennium falcon"])
            .expect_err("that is not one of ours")
            .to_string();
        for model in models::models() {
            assert!(
                err.contains(model.name),
                "the error does not list {}",
                model.name
            );
        }
        assert!(Args::try_parse_from(["warp", "--view", "porthole"]).is_err());
    }

    #[test]
    fn an_unknown_colour_mode_is_refused_and_the_message_says_what_there_is() {
        // `auto` is the value worth naming, because it is the one that used to
        // work: a script or a shell history carrying it from before deserves
        // the modes that are left rather than a silent fall back to one of
        // them.
        let err = Args::try_parse_from(["warp", "--color", "auto"])
            .expect_err("the terminal is not asked what it can do any more")
            .to_string();
        for mode in ColorArg::value_variants() {
            let name = mode
                .to_possible_value()
                .expect("every colour mode is reachable by name");
            assert!(
                err.contains(name.get_name()),
                "the error does not offer `{}`: {err}",
                name.get_name()
            );
        }
    }

    #[test]
    fn the_default_is_24_bit_colour_whatever_the_terminal_says() {
        assert_eq!(args_for(&[]).color.resolve(), ColorMode::Truecolor);
    }

    #[test]
    fn colour_modes_resolve() {
        // Exhaustive now that `auto` is gone: three names, three modes, and
        // nothing between the flag and the writer that can choose a fourth.
        assert_eq!(
            args_for(&["--color", "256"]).color.resolve(),
            ColorMode::Ansi256
        );
        assert_eq!(
            args_for(&["--color", "ascii"]).color.resolve(),
            ColorMode::Ascii
        );
        assert_eq!(
            args_for(&["--color", "truecolor"]).color.resolve(),
            ColorMode::Truecolor
        );
    }
}
