//! The command line, and the sizes it is allowed to ask for.
//!
//! Every number here is bounded. A canvas is four buffers wide and a failed
//! allocation aborts the process outright — no unwind, no `Drop`, no panic
//! hook — which interactively means being dumped back into a shell still in
//! raw mode on the alternate screen. So the limits are enforced at parse time,
//! where the answer is an error message instead.

use crate::term::ColorMode;
use clap::{Parser, ValueEnum};
use crossterm::terminal;

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
pub struct Args {
    /// How many stars to keep in flight. 0 suits the count to the terminal.
    #[arg(
        long,
        default_value_t = 0,
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
    #[arg(long, default_value_t = 1, value_name = "N", requires = "headless")]
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

    /// Tonemap exposure. Higher is brighter.
    #[arg(long, default_value_t = 1.9, value_parser = positive)]
    pub exposure: f32,

    /// Write a PNG of one frame and exit. The instrument panel is not drawn.
    #[cfg(feature = "snapshot")]
    #[arg(long, value_name = "FILE", conflicts_with = "headless")]
    pub snapshot: Option<std::path::PathBuf>,

    /// Frames to simulate before taking the snapshot.
    #[cfg(feature = "snapshot")]
    #[arg(long, default_value_t = 300, value_name = "N", requires = "snapshot")]
    pub warmup: u32,

    /// Magnification of the snapshot image.
    #[cfg(feature = "snapshot")]
    #[arg(long, default_value_t = 3, value_name = "N", requires = "snapshot")]
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
        assert_eq!(args.stars, 0);
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
        assert!(
            Args::try_parse_from(["warp", "--stars", "0"]).is_ok(),
            "0 still means auto"
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
    fn color_modes_resolve() {
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
