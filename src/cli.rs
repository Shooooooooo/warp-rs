//! The command line, and the sizes it is allowed to ask for.
//!
//! Every number here is bounded. A canvas is four buffers wide and a failed
//! allocation aborts the process outright — no unwind, no `Drop`, no panic
//! hook — which interactively means being dumped back into a shell still in
//! raw mode on the alternate screen. So the limits are enforced at parse time,
//! where the answer is an error message instead.

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
const MAX_CELLS: usize = 2_000_000;
/// And no single dimension past this, so the error names the obvious mistake
/// rather than quoting a product.
const MAX_DIM: u16 = 10_000;
/// Ceiling on a star pool, however the pool was asked for.
///
/// Two doors reach it and this is the only number behind both: `--stars` here
/// and the `+` key. That key used to have a ceiling of its own, 20 000, which
/// sat *under* what this file already allowed — so `--stars 100000` and a
/// single press shrank the pool by four fifths, which is not what that key
/// says. There was a third door once, an automatic count in [`crate::app`] that
/// sized the pool from the canvas; it is gone, and [`DEFAULT_STARS`] is what
/// stands where it stood.
///
/// It is also the one bound in this file not enforced only at parse time, and
/// for a reason rather than by omission: a pool is the one thing on the list
/// that can still be resized after the command line has been read. A `Star` is
/// 40 bytes, so this is 40 MB of it — an allocation rather than an abort on
/// anything this will run on.
pub const MAX_STARS: usize = 1_000_000;
/// How many stars a flight opens with when the command line does not say.
///
/// This used to be a *density* — stars per subpixel, at 0.02 — so the sky was a
/// function of the window, and the number here was the sentinel `0` that asked
/// for it. What that bought was an even-looking field at every size; what it
/// cost was that no two terminals flew the same sky, and that a flight already
/// under way had its pool rebuilt underneath it every time the window moved.
/// The density's own answers say how wide that spread was: 19 stars at 40x12,
/// 76 at 80x24, 172 at 120x36, 480 at 200x60, 1080 at 300x90. A fixed count is
/// a picture the notes, the reference frames and two people on two machines can
/// all be talking about.
///
/// 256 sits inside that range and toward its thin end on purpose. The pool is
/// the same on every canvas now, so it has to be a number that a small window
/// can carry without becoming a wash — and the field is drawn as the segment
/// each star swept, so lighting the drive turns every one of them into a streak
/// several times its own length. Density is what a large window loses, and
/// depth parallax is what it keeps; the streaks lengthen with the frame either
/// way.
pub const DEFAULT_STARS: usize = 256;
/// So a default the parser would refuse is a compile error rather than a
/// command line that fails on every invocation. `default_value_t` is spelled
/// and then read back through the `value_parser` like anything typed at the
/// prompt, so a default past the ceiling would not be found until the binary
/// was run — which is the one way a number in this file can be wrong without
/// anything here saying so.
const _: () = assert!(
    DEFAULT_STARS <= MAX_STARS,
    "the default star count is past its own ceiling"
);
/// Ceiling on the two counts that are spent rather than allocated — `--frames`
/// and `--warmup`. Nothing runs out of memory over these; a `u32` of them is
/// simply a process that never comes back, and at sixty a second this is
/// already four hours of flying. Bounded anyway, because a number on this
/// command line that answers "how many" and has no limit is how the two above
/// went wrong.
const MAX_COUNT: u32 = 1_000_000;
/// Ceiling on `--scale`. This one *is* an allocation, and it enters squared:
/// the image is the canvas magnified on both axes, so at the snapshot's
/// default 240x68 — 240 by 136 subpixels — sixteen comes to 3840 by 2176, or
/// 25 MB of RGB. Past that the useful range has long since been left behind;
/// the README's two snapshots, which are the whole of `docs/`, are both taken
/// at 2.
#[cfg(feature = "snapshot")]
const MAX_SCALE: usize = 16;

#[derive(Parser, Debug)]
#[command(
    name = "warp",
    version,
    about = "Fly a starship through the universe at warp, in your terminal"
)]
pub struct Args {
    /// How many stars to keep in flight. 0 flies an empty sky.
    ///
    /// Zero is a count like any other here rather than the error the other
    /// zeroes on this command line are, and the reason is that the renderer
    /// draws it: the hull, the bubble, the drive, the vignette and the panel
    /// are all still there and only the sky behind them goes, which is the one
    /// way to look at any of them with nothing streaming past. `--frames 0` and
    /// `--scale 0` are refused because they are an allocation of nothing; this
    /// is a picture.
    ///
    /// It used to be the sentinel that asked for a count taken off the canvas,
    /// so the number reaching [`crate::app`] was never this one. The help text
    /// says what it does now rather than leaving it to be found, because a
    /// command line carrying `--stars 0` from before means something else.
    #[arg(
        long,
        default_value_t = DEFAULT_STARS,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(0..=MAX_STARS as u64)
    )]
    pub stars: usize,

    /// Frame rate cap.
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

    /// Colour depth. Auto-detected from COLORTERM and TERM by default.
    #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
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
    ///
    /// `allow_hyphen_values` because half the range starts with a minus sign,
    /// and without it clap reads `--orbit -75,10` as a flag it does not know
    /// rather than as a camera behind the ship. The equals form works either
    /// way; nobody should have to find that out.
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

    /// How finely a hull's outline is measured, in samples per subpixel on each
    /// axis. `1` is the hard-edged rasteriser this replaced.
    ///
    /// Only the hull is affected, and only from outside. Everything else on the
    /// canvas is laid down by a splat that already spreads its light over four
    /// subpixels, so there is nothing here for it to do.
    ///
    /// A number rather than an on and an off, because the number is what
    /// actually varies and because `1` is worth being able to ask for: it is
    /// not an approximation of the old picture, it is the old picture.
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Auto,
    Truecolor,
    #[value(name = "256")]
    Ansi256,
    Ascii,
}

impl ColorArg {
    pub fn resolve(self) -> ColorMode {
        match self {
            ColorArg::Auto => ColorMode::detect(),
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
/// Degrees rather than radians because this is the one place a person types the
/// number, and a right angle is `90` to everyone and `1.5707964` to nobody.
/// [`Orbit::held`] does the bounding, which is the same bounding a keypress
/// gets — the clamp lives with the geometry rather than with each of the two
/// ways of reaching it.
///
/// Not bounded here beyond that, and it does not need to be: an angle has no
/// end to run away past, so unlike every other number on this command line the
/// failure mode of a preposterous one is a picture, not an allocation.
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
        assert_eq!(args.stars, 256);
        assert!(args.demo.is_none() && !args.headless && !args.engage);
    }

    #[test]
    fn demo_takes_an_optional_duration() {
        assert_eq!(args_for(&["--demo"]).demo, Some(45.0));
        assert_eq!(args_for(&["--demo", "12.5"]).demo, Some(12.5));
    }

    #[test]
    fn a_demo_has_to_last_a_positive_length_of_time() {
        // Regression: `--demo` was the one number that went unchecked. A
        // negative deadline is already past on the first frame, so the flight
        // ended before it drew anything; a NaN one is never reached, so the
        // flag silently became an indefinite autopilot instead.
        for bad in ["-5", "0", "nan", "inf", "-inf", "banana"] {
            assert!(
                Args::try_parse_from(["warp", &format!("--demo={bad}")]).is_err(),
                "`--demo={bad}` should not parse"
            );
        }
        assert_eq!(args_for(&["--demo", "0.5"]).demo, Some(0.5));
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
        // said could not exist. `--frames` and `--warmup` took a whole `u32`,
        // which is a run that never ends; `--scale` took a whole `usize` and
        // is worse than slow, because the image is the canvas magnified on
        // both axes and the buffer for it grows as the square. `--scale 0` was
        // legal too, and was patched over three separate times at the far end
        // by a `.max(1)` rather than refused here where the answer can be a
        // message.
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
        // rest. Zero is the interesting end: it is not "no anti-aliasing", it
        // is a hull measured on no samples at all, and one is what means that.
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
    fn size_and_star_counts_are_bounded() {
        // Regression: neither was bounded. `--stars 500000000` asked for 20 GB
        // and `--size 60000x60000` for 86 GB, and a failed allocation aborts
        // the process — no unwind, no `Drop`, no panic hook — so interactively
        // it left the terminal in raw mode on the alternate screen.
        assert!(Args::try_parse_from(["warp", "--stars", "500000000"]).is_err());
        assert!(Args::try_parse_from(["warp", "--stars", "1000001"]).is_err());
        assert!(Args::try_parse_from(["warp", "--stars", "1000000"]).is_ok());
        assert!(
            Args::try_parse_from(["warp", "--stars", "0"]).is_ok(),
            "an empty sky is a count like any other"
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
    /// [`ViewMode::label`] spells the second. Nothing makes them agree, so
    /// this does. Walks `ViewMode::ALL`, so a third camera is covered the day
    /// it is added rather than the day somebody notices.
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
        // refused: all three angles go all the way round, so there is no
        // number here to reject, only one to fold.
        let over = args_for(&["--orbit", "720,400,-900"]).orbit;
        for angle in [over.azimuth, over.elevation, over.roll] {
            assert!(
                angle.abs() <= std::f32::consts::PI,
                "an angle got away: {over:?}"
            );
        }
        // And 400 degrees of elevation is 40, not a quarter turn: the fold has
        // to keep the angle it was given rather than stop at the top.
        assert!(
            (over.elevation - 40.0f32.to_radians()).abs() < 1e-5,
            "the elevation was clipped rather than folded: {over:?}"
        );

        // And behind the ship, which is where half the range is and where a
        // leading minus sign would otherwise be read as a flag.
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
    fn colour_modes_resolve() {
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
