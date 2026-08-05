//! A stopwatch on the render loop.
//!
//! ```sh
//! cargo run --release --example bench            # a default sweep
//! cargo run --release --example bench 200 60 20000
//! ```
//!
//! Reports where a frame goes: simulating the flight, drawing it, and getting
//! it out. The interesting number is the sum against the frame budget — 16.7 ms
//! at the default 60 fps — and how it moves when the renderer is changed.

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
    let cases: Vec<(usize, usize, usize, bool)> = if argv.len() >= 3 {
        let n = |i: usize| argv[i].parse().expect("expected: [cols] [rows] [stars]");
        vec![(n(0), n(1), n(2), true)]
    } else {
        vec![
            (80, 24, 0, false),
            (80, 24, 0, true),
            (200, 60, 0, true),
            (200, 60, 20_000, true),
        ]
    };

    println!(
        "{:>9}  {:>7}  {:>8}  {:>8}  {:>8}  {:>8}  {:>6}",
        "size", "stars", "sim ms", "draw ms", "write ms", "total ms", "fps"
    );
    for (cols, rows, stars, warp) in cases {
        run(cols, rows, stars, warp);
    }
}

fn run(cols: usize, rows: usize, stars: usize, warp: bool) {
    let mut argv = vec!["warp".to_string(), "--seed".into(), "1".into()];
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
        "{:>9}  {:>7}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>6.0}",
        format!("{cols}x{rows}"),
        flight.stars(),
        ms(sim),
        ms(draw),
        ms(write),
        total,
        1000.0 / total,
    );
}
