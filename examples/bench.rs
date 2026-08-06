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
//! The colour mode is **pinned** rather than detected, for the reason
//! `tests/flight.rs` pins its own: `--color auto` reads `TERM`, so an
//! unpinned sweep measures whatever the shell happens to export and two runs on
//! two machines are not comparable. It made a real difference — the same case
//! came out at 6.87, 7.31 and 6.66 ms of drawing in ascii, 256 and truecolor —
//! so the mode is a column here rather than an assumption.

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

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cases: Vec<(usize, usize, usize, bool, &str, &str)> = if argv.len() >= 3 {
        let n = |i: usize| {
            argv[i]
                .parse()
                .expect("expected: [cols] [rows] [stars] [view] [color]")
        };
        let view = argv
            .get(3)
            .map_or("cockpit", |v| if v == "side" { "side" } else { "cockpit" });
        let color = argv.get(4).map_or("truecolor", String::as_str);
        vec![(n(0), n(1), n(2), true, view, color)]
    } else {
        vec![
            (80, 24, 0, false, "cockpit", "truecolor"),
            (80, 24, 0, true, "cockpit", "truecolor"),
            (200, 60, 0, true, "cockpit", "truecolor"),
            (200, 60, 20_000, true, "cockpit", "truecolor"),
            // The outside view at warp is the expensive frame in the program:
            // every streak near the ship is chopped into arcs and drawn twice,
            // once for each image the lens forms of it.
            (200, 60, 0, true, "side", "truecolor"),
            (200, 60, 20_000, true, "side", "truecolor"),
            // And the same frame in the mode most terminals actually get, since
            // `ColorMode::detect` answers 256 for anything with a `TERM` entry
            // and no `COLORTERM`. It composes a cell differently from the two
            // above, so it is the one case here that is not truecolor.
            (200, 60, 20_000, true, "cockpit", "256"),
        ]
    };

    println!(
        "{:>9}  {:>8}  {:>9}  {:>7}  {:>8}  {:>8}  {:>8}  {:>8}  {:>6}",
        "size", "view", "color", "stars", "sim ms", "draw ms", "write ms", "total ms", "fps"
    );
    for (cols, rows, stars, warp, view, color) in cases {
        run(cols, rows, stars, warp, view, color);
    }
}

fn run(cols: usize, rows: usize, stars: usize, warp: bool, view: &str, color: &str) {
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
    if stars > 0 {
        argv.extend(["--stars".to_string(), stars.to_string()]);
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
