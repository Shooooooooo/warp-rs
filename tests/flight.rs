//! Driving a flight from outside the crate.
//!
//! The unit tests can reach into private state; these deliberately cannot.
//! They fly through the same surface any other program would have to use, so
//! they fail if the library stops being usable without the binary — which is
//! the whole reason `main.rs` is fifteen lines.

use clap::Parser;
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
    // The glyphs, with the colour codes between them taken back out — the
    // panel's shadow means all but the rarest pair of neighbours differ.
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
