//! Driving a flight from outside the crate.
//!
//! The unit tests can reach into private state; these deliberately cannot.
//! They fly through the same surface any other program would have to use, so
//! they fail if the library stops being usable without the binary — which is
//! the whole reason `main.rs` is fifteen lines.

use clap::Parser;
use std::time::{Duration, Instant};
use warp_rs::app::Flight;
use warp_rs::cli::Args;

/// Fly `frames` frames at a fixed timestep and hand back the bytes.
///
/// The colour depth is pinned rather than auto-detected: `--color auto` reads
/// `TERM`, which CI runners do not set, and the bytes these tests assert on are
/// the truecolor ones. A caller that cares can pass its own `--color`.
fn fly(argv: &[&str], cols: usize, rows: usize, frames: usize) -> Vec<u8> {
    let mut full = vec!["warp", "--color", "truecolor"];
    full.extend_from_slice(argv);
    let args = Args::try_parse_from(full).expect("arguments should parse");

    let mut flight = Flight::new(&args, cols, rows);
    let mut out = Vec::new();
    for _ in 0..frames {
        flight.advance(1.0 / 60.0);
        flight.draw(60.0, false, true);
        flight
            .present_plain(&mut out)
            .expect("writing to a Vec cannot fail");
    }
    out
}

#[test]
fn a_flight_needs_nothing_but_the_library() {
    let out = fly(&["--seed", "5", "--stars", "600"], 60, 20, 20);
    assert!(!out.is_empty());
    // Twenty frames of twenty rows, each row ending in a newline.
    assert_eq!(out.iter().filter(|b| **b == b'\n').count(), 20 * 20);
    // The glyphs, with the colour codes between them taken back out — the sky
    // shows through the panel, so a star behind a word puts a colour code in
    // the middle of it.
    let glyphs: String = String::from_utf8_lossy(&out)
        .split('\u{1b}')
        .map(|chunk| chunk.split_once('m').map_or(chunk, |(_, rest)| rest))
        .collect();
    assert!(glyphs.contains('\u{2580}'), "no half blocks came out");
    assert!(glyphs.contains("VELOCITY"), "the panel never drew");
}

#[test]
fn the_seed_is_the_whole_of_the_state() {
    let a = fly(&["--seed", "5", "--stars", "600"], 60, 20, 20);
    let b = fly(&["--seed", "5", "--stars", "600"], 60, 20, 20);
    let c = fly(&["--seed", "6", "--stars", "600"], 60, 20, 20);
    assert_eq!(a, b, "a seeded flight has to be reproducible");
    assert_ne!(a, c, "two seeds should not give the same sky");
}

#[test]
fn the_view_from_outside_can_be_flown_from_the_library_alone() {
    // The whole outside view — camera, star band, hull, lens — reached through
    // nothing but the surface any other program would have to use.
    let out = fly(
        &[
            "--seed", "5", "--stars", "600", "--view", "side", "--ship", "hauler",
        ],
        60,
        20,
        20,
    );
    assert_eq!(out.iter().filter(|b| **b == b'\n').count(), 20 * 20);
    let glyphs: String = String::from_utf8_lossy(&out)
        .split('\u{1b}')
        .map(|chunk| chunk.split_once('m').map_or(chunk, |(_, rest)| rest))
        .collect();
    assert!(glyphs.contains('\u{2580}'), "no half blocks came out");
    assert!(glyphs.contains("VELOCITY"), "the panel never drew");
    assert!(
        glyphs.contains("HAULER"),
        "the panel does not say what it is"
    );
}

#[test]
fn the_seed_is_still_the_whole_of_the_state_from_outside() {
    let flags = |seed: &'static str| -> Vec<&'static str> {
        vec![
            "--seed", seed, "--stars", "600", "--view", "side", "--engage",
        ]
    };
    let a = fly(&flags("5"), 60, 20, 20);
    let b = fly(&flags("5"), 60, 20, 20);
    let c = fly(&flags("6"), 60, 20, 20);
    assert_eq!(a, b, "a seeded flight has to be reproducible out here too");
    assert_ne!(a, c, "two seeds should not give the same sky");
}

#[test]
fn every_ship_can_be_flown_from_the_command_line() {
    // A ship the picker offers but the command line will not take is a ship
    // that cannot be screenshotted, which is most of what these flags are for.
    for ship in ["dart", "hauler", "needle", "beetle", "trident"] {
        let out = fly(
            &[
                "--seed", "1", "--stars", "200", "--view", "side", "--ship", ship,
            ],
            60,
            20,
            2,
        );
        assert!(!out.is_empty(), "{ship} would not fly");
    }
}

#[test]
fn a_flight_survives_a_step_the_caller_should_not_have_asked_for() {
    // `Flight::advance` is public, and the whole point of this file is that a
    // flight can be driven from out here. The guard against a `dt` that is not
    // a frame used to live in the interactive loop instead, so a caller at this
    // level got neither half of it: an enormous step was unbounded work, and a
    // NaN step stopped the simulation for good while frames went on drawing.
    let args = Args::try_parse_from([
        "warp",
        "--color",
        "truecolor",
        "--seed",
        "5",
        "--stars",
        "600",
        "--throttle",
        "1.0",
    ])
    .expect("arguments should parse");
    let mut flight = Flight::new(&args, 60, 20);
    let frame = |flight: &mut Flight| {
        flight.draw(60.0, false, true);
        let mut out = Vec::new();
        flight
            .present_plain(&mut out)
            .expect("writing to a Vec cannot fail");
        out
    };

    for _ in 0..30 {
        flight.advance(1.0 / 60.0);
    }
    flight.advance(f32::NAN);

    let before = frame(&mut flight);
    for _ in 0..60 {
        flight.advance(1.0 / 60.0);
    }
    assert_ne!(
        before,
        frame(&mut flight),
        "one bad step froze the flight for the rest of its life"
    );

    // And a step far past anything a frame could be returns rather than
    // grinding: unclamped, this one is a hundred billion simulation steps.
    let started = Instant::now();
    flight.advance(1e9);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "an absurd dt was taken literally"
    );
}

#[test]
fn a_frame_does_not_repeat_a_colour_it_is_already_using() {
    // A cell carries about 40 bytes of escape codes, and a starfield is mostly
    // long runs of black, so this is most of the size of a frame. Checked from
    // out here because it is a property of the bytes, not of the buffers.
    let out = fly(&["--seed", "5", "--stars", "600"], 60, 20, 1);
    let text = String::from_utf8_lossy(&out);

    for (row, line) in text.lines().enumerate() {
        let mut last: Option<&str> = None;
        for code in line.split('\u{1b}').skip(1) {
            let Some((code, _)) = code.split_once('m') else {
                continue;
            };
            assert!(last != Some(code), "row {row} set `{code}` twice in a row");
            last = Some(code);
        }
    }
}
