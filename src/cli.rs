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
pub(crate) const MAX_CELLS: usize = 2_000_000;
/// And no single dimension past this, so the error names the obvious mistake
/// rather than quoting a product.
const MAX_DIM: u16 = 10_000;
/// The faintest star a sky holds, and the two ends the flag is held to.
///
/// This replaced a literal count, and the change is not a rename. `--stars N`
/// asked for a number of objects, which is not a thing anybody can look up or
/// check against a sky; a limiting magnitude is how star counts have always
/// been quoted, it is a property of the observer rather than of the window, and
/// the number of stars on screen falls out of it and the field of view instead
/// of being capped by anybody.
///
/// 6.0 rather than the 6.5 a dark sky really reaches, and the half magnitude is
/// paid to the grid rather than to taste. The cockpit's window is 89 degrees
/// across, so a true naked-eye sky puts about a thousand stars in it — which is
/// honest, and which a terminal cannot resolve: at 120x36 that is a canvas of
/// 8 640 subpixels, the faint end of the count law is most of the thousand, and
/// the picture comes out an even wash rather than a sky. What makes a sky read
/// as one is the contrast between a handful of standouts and a dust behind
/// them, and half a magnitude of headroom is what leaves room for it. This is
/// about 4 600 stars over the sphere and 550 in the window, against the 256 the
/// old fixed count drew on every terminal alike. Both ends were shot and looked
/// at before this was settled.
///
/// The bounds are the allocation guard this file applies to every other number,
/// moved one step back: the pool grows as `10^(0.6 m)`, so bounding the
/// observer bounds the count. At 9.5 that is 574 000 stars of 24 bytes, which
/// is under the 40 MB the old `MAX_STARS` of a million allowed and is already
/// far past what a terminal can usefully draw. At the bottom end the whole
/// sphere holds under one star, which is how an empty sky is asked for now that
/// `--stars 0` is gone — and an empty sky is worth asking for, being the only
/// way to see the tunnel, the bubble and the hull with nothing streaming past
/// them.
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

// The keys, printed under the flags rather than squeezed into the frame.
//
// The hint line along the bottom of the panel is the only place a running
// program says what the keys are, and it is chosen by width: the widest tier is
// 89 characters, so it wants 91 columns, and on the eighty-column terminal most
// people have it falls to the next one — which names four keys of a dozen and
// mentions neither camera nor picker nor sky. Under `MIN_COLS` the panel goes
// compact and there is no printed way to quit at all.
//
// Widening the tiers is the obvious answer and the wrong one twice over. It
// costs terminal columns the narrow tiers do not have — appending eight
// characters to the widest cockpit tier takes it from 89 to 97, so terminals
// between about 91 and 98 columns would shed it and lose the *throttle* to gain
// the sky — and five of the ten reference flights are rendered at 120 columns
// with a panel on them, so their hashes would move to say it. It was all ten
// until the flights that fly themselves stopped drawing one. Help text costs
// no columns at all and moves nothing, and `--help` is where somebody who
// cannot see a control reflexively looks.
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
///
/// One thing about the comments in here is not a matter of taste. Clap's derive
/// publishes a `///` block as help text — a field's as that flag's entry, and
/// this struct's as the whole program's `--long_about` — so a doc comment in
/// this file is addressed to whoever is *running* the program, where the house
/// style everywhere else in the tree is an essay addressed to whoever is
/// editing it next. The two collided on three flags and the user won the
/// argument: `--color` was explaining to the world at large why an `auto` mode
/// it can no longer ask for went away, and closing with a comparison to
/// `--stars`, which is a flag it cannot ask for either. Notes to the next
/// editor go in `//` blocks between the doc comment and the `#[arg(…)]`, where
/// clap does not read them and the next editor still cannot miss them.
///
/// `long_about = None` below is that rule applied to this very comment, which
/// otherwise prints in full above the usage line. It is what lets the struct
/// keep a doc comment for `cargo doc` without addressing it to the wrong
/// reader; clap falls back to `about` for both lengths of help.
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
    ///
    /// Higher is more stars: each magnitude is about four times as many, the
    /// way a darker site shows more of them. How many land on screen is then
    /// the field of view's business and nothing caps it.
    ///
    // `allow_hyphen_values` because the bottom of the range is negative and
    // without it clap reads `--magnitude -2` as a flag it does not know rather
    // than as an empty sky. `--orbit` needs it for the same reason and says
    // so; this is the second place that has come up.
    #[arg(
        long,
        value_name = "MAG",
        default_value_t = DEFAULT_MAGNITUDE,
        allow_hyphen_values = true,
        value_parser = magnitude
    )]
    pub magnitude: f32,

    /// Gone, and refused by name rather than by silence.
    ///
    /// A shell history or a script carrying `--stars 600` deserves to be told
    /// what replaced it, where clap's own answer to a flag it does not know is
    /// "unexpected argument" and a shrug. This is the shape `--color auto` is
    /// turned away with, for the same reason: the value that used to work is
    /// the one worth naming.
    #[arg(long, value_name = "N", hide = true, value_parser = no_star_count)]
    pub stars: Option<String>,

    /// Frame rate cap — and, in `--headless` and `--snapshot`, the simulation
    /// timestep, so it changes the flight rather than only how often it is
    /// drawn.
    ///
    /// Interactively it is a cap and nothing else, and only while nothing is
    /// being typed: a keypress is answered the moment it arrives rather than
    /// when the budget runs out.
    // Two meanings, and the help says both because leaving the second one out
    // is how somebody sets `--fps 10` on a headless run to save time and gets a
    // twelve-second flight instead of a two-second one, with every hash moved.
    // The maintainer's half of this is in CLAUDE.md; what is here is the half a
    // person running the program needs.
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
    ///
    /// The environment is not consulted: the narrower modes are asked for
    /// rather than fallen into, so a terminal that cannot read a 24-bit
    /// sequence needs telling here.
    // There was an `auto` here that read `COLORTERM` and then `TERM`, and it
    // guessed against the renderer: a terminal exporting no `COLORTERM` got the
    // 256-colour palette whatever it could really do, which is most terminals,
    // so the mode the canvas is designed for was the one it least often opened
    // in. With the default at 24-bit an `auto` would be a second answer to a
    // question already answered, and it is the answer that guesses — so it went
    // rather than being demoted. A terminal that cannot read the sequences is
    // the user's own call now, the way `--stars` is a count no window may
    // overrule.
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
    ///
    // `allow_hyphen_values` because half the range starts with a minus sign,
    // and without it clap reads `--orbit -75,10` as a flag it does not know
    // rather than as a camera behind the ship. The equals form works either
    // way; nobody should have to find that out.
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

// A `///` here rather than the `//` this file otherwise insists on, and the
// exception is real rather than an oversight: clap's derive publishes doc
// comments on the struct and on its *fields*, which is why a note to the next
// editor cannot be one there. It does not read an inherent `impl`, so this is
// documentation in the ordinary sense and `cargo doc` is the only thing that
// prints it.
impl Args {
    /// Whether there is nobody at the controls: `--demo` and `--screensaver`.
    ///
    /// One predicate rather than the four spellings that grew up around this
    /// question, because the ones that have to agree about a flight had already
    /// come apart. `--snapshot` conflicts only with `--headless`, so
    /// `--snapshot shot.png --screensaver` parses — and `run_snapshot` asked
    /// `demo.is_some()`, so it warmed up a flight nobody was flying without the
    /// autopilot flying it.
    ///
    /// It gates two things: whether the autopilot works the throttle and the
    /// camera, and whether the frame gets an instrument panel over it.
    pub fn unattended(&self) -> bool {
        self.demo.is_some() || self.screensaver
    }
}

/// How much colour to spell a cell in, as the command line names it.
///
/// A bare rename of [`ColorMode`] since `Auto` went, and kept anyway for the
/// two reasons [`ViewArg`] below is kept: `256` is not a Rust identifier, so
/// the wire name has to hang off a variant somewhere, and the alternative is
/// deriving `ValueEnum` on [`ColorMode`] itself, which puts clap inside
/// `crate::term`. The dependency rule this crate is written to is about the
/// tree rather than the manifest, and that would widen it for a rename.
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
        // star count. That it survives to the writer is a separate question and
        // has its own test below.
        assert_eq!(args.color, ColorArg::Truecolor);
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
    fn the_modes_that_fly_themselves_are_the_ones_that_say_so() {
        // One predicate answers this in four places now — two autopilot gates
        // and two panel gates — so it is worth pinning that it means what its
        // name says. The spelling it replaced was `demo.is_some()`, which is
        // right in the headless loop, where `--screensaver` cannot be set, and
        // wrong in the snapshot one, where it can.
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
    fn size_and_sky_are_bounded() {
        // Regression: neither was bounded. `--stars 500000000` asked for 20 GB
        // and `--size 60000x60000` for 86 GB, and a failed allocation aborts
        // the process — no unwind, no `Drop`, no panic hook — so interactively
        // it left the terminal in raw mode on the alternate screen.
        //
        // The star half is now bounded one door further back: the count is
        // derived from the limiting magnitude, which grows as `10^(0.6 m)`, so
        // holding the observer holds the allocation. Both ends are checked
        // because both are reachable — the top by a hand on `+` and the bottom
        // by a hand on `-`.
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
        // carrying one. The same courtesy `--color auto` is turned away with.
        let err = Args::try_parse_from(["warp", "--stars", "600"])
            .expect_err("a sky is not asked for by the number any more")
            .to_string();
        assert!(
            err.contains("--magnitude"),
            "the error does not name what replaced it: {err}"
        );
    }

    #[test]
    fn the_help_text_is_addressed_to_whoever_is_running_the_program() {
        // Clap's derive publishes `///` blocks as help, and the house style
        // writes `///` as an essay to the next editor, so the two collide every
        // time a flag grows a note. They had, on three: `--magnitude` and
        // `--orbit` each explained `allow_hyphen_values` to the world, and
        // `--color` published a paragraph about an `auto` mode that no longer
        // exists, ending on a comparison to `--stars`, which does not exist
        // either — so the help advertised two things it would refuse to do.
        //
        // Asked of the rendered help rather than of the source, because what
        // went wrong was not where the words were written but where they came
        // out. The struct's own doc comment is in this net too: it renders
        // above the usage line unless `long_about` turns it off, and it said
        // all of the above at once for one commit.
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
        // And the flags a person does need are still described. A help text
        // that leaked nothing because it said nothing would pass the loop
        // above.
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
    fn an_unknown_colour_mode_is_refused_and_the_message_says_what_there_is() {
        // `auto` is the value worth naming, because it is the one that used to
        // work: a script or a shell history carrying it from before deserves
        // the modes that are left rather than a silent fall back to one of
        // them. Asked of the variants rather than of three strings, for the
        // reason the ship test above walks the hangar — a mode written out in
        // quotes goes stale the day the list changes under it.
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
        // This used to answer a question about the shell that started the test
        // rather than about the program. `--color` defaulted to `auto`, so a
        // runner with no `TERM` got ascii, anything with a `TERM` entry and no
        // `COLORTERM` got 256, and only a terminal announcing itself got
        // truecolor — three answers depending on where the suite ran, which
        // the forty-odd tests in `app.rs` that build their `Args` without
        // `--color` inherited whole. Nothing reads the environment now. Asked
        // of the resolved mode rather than of the flag, because the flag being
        // `Truecolor` is a fact about clap and this is the one about the
        // renderer; the ambient environment is the fixture, and under the old
        // code it would have been red both here and on CI.
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
