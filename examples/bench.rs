//! A stopwatch on the render loop.
//!
//! ```sh
//! cargo run --release --example bench            # a default sweep
//! cargo run --release --example bench 200 60 20000
//! cargo run --release --example bench 200 60 20000 side 256
//! ```
//!
//! Reports where a frame goes: simulating the flight, drawing it, and getting
//! it out. The interesting number is the sum against the frame budget — 16.7 ms
//! at the default 60 fps — and how it moves when the renderer is changed.
//!
//! The colour mode is **pinned** rather than left to the default, for the
//! reason `tests/flight.rs` pins its own: a sweep that takes whatever the flag
//! happens to default to measures a different thing the day that default
//! changes, and the numbers written down in `CLAUDE.md` are compared across
//! exactly such changes. It made a real difference — the same case came out at
//! 6.87, 7.31 and 6.66 ms of drawing in ascii, 256 and truecolor — so the mode
//! is a column here rather than an assumption. The pin used to be against
//! detection reading `TERM`, which was a stronger-sounding reason for a weaker
//! property: two machines disagreeing is easier to notice than one machine
//! quietly measuring something else than it did last week.
//!
//! A `0` in the stars column leaves the flag off, so the case measures whatever
//! a default run draws. That used to be a number derived from the canvas and is
//! a fixed count now, which changes how those rows *read* rather than what they
//! measure: they are one pool at three canvases, so what stands between them is
//! the cost of the canvas with the sky held still. The rows naming a number are
//! the ones that vary the pool, and they are the ones a figure written down in
//! `CLAUDE.md` can be compared against across this change.

use clap::Parser;
use std::time::Instant;
use warp_rs::app::Flight;
use warp_rs::cli::Args;

/// Frames to time. Enough to average out a scheduler hiccup, few enough that
/// the whole sweep is a couple of seconds.
const FRAMES: usize = 240;
/// Frames flown before the clock starts: streaks are only their full length
/// once the drive has spooled up, and that is the case worth measuring.
const WARMUP: usize = 300;

/// One row of the sweep: a canvas, how faint a sky over it, whether the drive
/// is lit, and which camera in which colour mode. A named type because six
/// fields in a tuple is where clippy stops reading, and because the third one
/// stopped being a plain count.
type Case<'a> = (usize, usize, Option<f32>, bool, &'a str, &'a str);

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cases: Vec<Case<'_>> = if argv.len() >= 3 {
        let n = |i: usize| {
            argv[i]
                .parse()
                .expect("expected: [cols] [rows] [magnitude] [view] [color]")
        };
        let view = argv
            .get(3)
            .map_or("cockpit", |v| if v == "side" { "side" } else { "cockpit" });
        let color = argv.get(4).map_or("truecolor", String::as_str);
        // The third argument is a limiting magnitude now rather than a count,
        // and it is a *column* rather than an assumption for the reason the
        // colour mode is: a sweep that rides the default measures something
        // else the day the default moves.
        let mag: Option<f32> = argv.get(2).and_then(|a| a.parse().ok());
        vec![(n(0), n(1), mag, true, view, color)]
    } else {
        vec![
            (80, 24, None, false, "cockpit", "truecolor"),
            (80, 24, None, true, "cockpit", "truecolor"),
            (200, 60, None, true, "cockpit", "truecolor"),
            (200, 60, Some(8.0), true, "cockpit", "truecolor"),
            // The outside view at warp is the expensive frame in the program:
            // every streak near the ship is chopped into arcs and drawn twice,
            // once for each image the lens forms of it.
            (200, 60, None, true, "side", "truecolor"),
            (200, 60, Some(8.0), true, "side", "truecolor"),
            // And the cockpit at the same size and pool in the palette
            // spelling — the fourth row's flight, not the two `side` ones
            // directly above, which is what this comment claimed for as long as
            // it said "the same frame". It composes a cell differently from
            // every row above it, an index per colour change against three
            // spelled-out decimals, so it is the one case here that is not
            // truecolor. It earned the row when this was the mode most
            // terminals fell into and keeps it now that none do: the quantiser
            // is still on the frame path of everyone who asks for it, and a
            // column nobody times is a column nobody notices regressing.
            (200, 60, Some(8.0), true, "cockpit", "256"),
        ]
    };

    println!(
        "{:>9}  {:>8}  {:>9}  {:>7}  {:>8}  {:>8}  {:>8}  {:>8}  {:>6}",
        "size", "view", "color", "stars", "sim ms", "draw ms", "write ms", "total ms", "fps"
    );
    for (cols, rows, magnitude, warp, view, color) in cases {
        run(cols, rows, magnitude, warp, view, color);
    }
}

fn run(cols: usize, rows: usize, magnitude: Option<f32>, warp: bool, view: &str, color: &str) {
    let mut argv = vec![
        "warp".to_string(),
        "--seed".into(),
        "1".into(),
        "--view".into(),
        view.into(),
        // Pinned, never detected: see the note at the top of this file.
        "--color".into(),
        color.into(),
    ];
    if let Some(mag) = magnitude {
        argv.extend(["--magnitude".to_string(), mag.to_string()]);
    }
    if warp {
        argv.extend(["--engage".to_string(), "--throttle".into(), "1.0".into()]);
    }
    let args = Args::try_parse_from(&argv).expect("arguments should parse");

    let mut flight = Flight::new(&args, cols, rows);
    let dt = 1.0 / 60.0;
    for _ in 0..WARMUP {
        flight.advance(dt);
    }

    let (mut sim, mut draw, mut write) = (0.0, 0.0, 0.0);
    let mut out = Vec::with_capacity(1 << 20);
    for _ in 0..FRAMES {
        let a = Instant::now();
        flight.advance(dt);
        let b = Instant::now();
        flight.draw(60.0, false, true);
        let c = Instant::now();
        out.clear();
        flight
            .present_plain(&mut out)
            .expect("writing to a Vec cannot fail");
        let d = Instant::now();

        sim += (b - a).as_secs_f64();
        draw += (c - b).as_secs_f64();
        write += (d - c).as_secs_f64();
    }

    let ms = |total: f64| total * 1000.0 / FRAMES as f64;
    let total = ms(sim) + ms(draw) + ms(write);
    println!(
        "{:>9}  {:>8}  {:>9}  {:>7}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>6.0}",
        format!("{cols}x{rows}"),
        view,
        color,
        flight.stars(),
        ms(sim),
        ms(draw),
        ms(write),
        total,
        1000.0 / total,
    );
}
