//! Driving a flight from outside the crate.

use clap::Parser;
use std::time::{Duration, Instant};
use warp_rs::app::Flight;
use warp_rs::cli::Args;

/// Fly `frames` frames at a fixed timestep and hand back the bytes.
fn fly(argv: &[&str], cols: usize, rows: usize, frames: usize) -> Vec<u8> {
    let mut full = vec!["warp", "--color", "truecolor", "--fade", "0"];
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
    let out = fly(&["--seed", "5", "--magnitude", "4.5"], 60, 20, 20);
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
fn a_flight_can_be_left_to_fly_itself() {
    // The autopilot reached the way another program would have to reach it.
    let args = Args::try_parse_from([
        "warp",
        "--color",
        "truecolor",
        // Pinned for the reason `fly` above pins it.
        "--fade",
        "0",
        "--demo",
        "--view",
        "side",
        "--seed",
        "5",
        "--magnitude",
        "4",
    ])
    .expect("arguments should parse");
    let mut flight = Flight::new(&args, 60, 20);
    let opened = flight.orbit_target();

    let mut out = Vec::new();
    for frame in 0..600 {
        flight.fly_itself(&args, frame as f64 / 60.0);
        flight.advance(1.0 / 60.0);
        flight.draw(60.0, false, true);
        flight
            .present_plain(&mut out)
            .expect("writing to a Vec cannot fail");
    }

    assert!(!out.is_empty(), "ten seconds of autopilot drew nothing");
    assert_ne!(
        flight.orbit_target(),
        opened,
        "the autopilot never moved the camera it was given"
    );
    // And it really flew: ten seconds is past the run-up, so the drive is lit.
    let glyphs: String = String::from_utf8_lossy(&out)
        .split('\u{1b}')
        .map(|chunk| chunk.split_once('m').map_or(chunk, |(_, rest)| rest))
        .collect::<String>()
        .replace('\u{2580}', " ");
    assert!(
        glyphs.contains("WARP DRIVE ENGAGED"),
        "the panel never showed the drive"
    );

    // And the panel is a decision the caller makes, which is the other half of
    // what this file is for.
    let mut bare = Vec::new();
    flight.draw(60.0, false, false);
    flight
        .present_plain(&mut bare)
        .expect("writing to a Vec cannot fail");
    let bare = String::from_utf8_lossy(&bare)
        .split('\u{1b}')
        .map(|chunk| chunk.split_once('m').map_or(chunk, |(_, rest)| rest))
        .collect::<String>();
    assert!(
        !bare.contains("VELOCITY") && !bare.contains("THR"),
        "a frame asked for without a panel drew one anyway"
    );
}

#[test]
fn a_turn_at_warp_can_be_flown_from_the_library_alone() {
    // The stick, through the surface another program would have to use.
    let fly = |steer: bool| {
        let args = Args::try_parse_from([
            "warp",
            "--seed",
            "12",
            "--magnitude",
            "5.5",
            "--size",
            "60x20",
            "--engage",
            "--throttle",
            "1.0",
            "--color",
            "truecolor",
            // Pinned for the reason `fly` above pins it.
            "--fade",
            "0",
        ])
        .expect("arguments should parse");
        let mut flight = Flight::new(&args, 60, 20);
        let mut out = Vec::new();
        for _ in 0..300 {
            if steer {
                flight.nudge_stick(1.0, -0.35, 0.0);
            }
            flight.advance(1.0 / 60.0);
        }
        flight.draw(60.0, false, true);
        flight.present_plain(&mut out).expect("writing to a Vec");
        out
    };
    let (turned, straight) = (fly(true), fly(false));
    assert!(
        turned != straight,
        "a turn at warp drew the frame a straight flight drew"
    );
    assert!(!turned.is_empty(), "the turning flight drew nothing at all");
}

#[test]
fn the_seed_is_the_whole_of_the_state() {
    let a = fly(&["--seed", "5", "--magnitude", "4.5"], 60, 20, 20);
    let b = fly(&["--seed", "5", "--magnitude", "4.5"], 60, 20, 20);
    let c = fly(&["--seed", "6", "--magnitude", "4.5"], 60, 20, 20);
    assert_eq!(a, b, "a seeded flight has to be reproducible");
    assert_ne!(a, c, "two seeds should not give the same sky");
}

#[test]
fn the_view_from_outside_can_be_flown_from_the_library_alone() {
    // The whole outside view — camera, star band, hull, lens — reached through
    // nothing but the surface any other program would have to use.
    let out = fly(
        &[
            "--seed",
            "5",
            "--magnitude",
            "4.5",
            "--view",
            "side",
            "--ship",
            "normandy",
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
        glyphs.contains("NORMANDY"),
        "the panel does not say what it is"
    );
}

#[test]
fn the_seed_is_still_the_whole_of_the_state_from_outside() {
    let flags = |seed: &'static str| -> Vec<&'static str> {
        vec![
            "--seed",
            seed,
            "--magnitude",
            "4.5",
            "--view",
            "side",
            "--engage",
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
    for ship in warp_rs::models::models().iter().map(|m| m.name) {
        let out = fly(
            &[
                "--seed",
                "1",
                "--magnitude",
                "3.5",
                "--view",
                "side",
                "--ship",
                ship,
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
    // flight can be driven from out here.
    let args = Args::try_parse_from([
        "warp",
        "--color",
        "truecolor",
        // Pinned for the reason `fly` above pins it.
        "--fade",
        "0",
        "--seed",
        "5",
        "--magnitude",
        "4.5",
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
    // long runs of black, so this is most of the size of a frame.
    let out = fly(&["--seed", "5", "--magnitude", "4.5"], 60, 20, 1);
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

#[test]
fn a_fade_can_be_asked_for_from_the_command_line() {
    // The flag's whole contract, stated where another program would have to
    // read it.
    let frames = |fade: &str| -> Vec<Vec<u8>> {
        let args = Args::try_parse_from([
            "warp",
            "--color",
            "truecolor",
            "--seed",
            "4",
            "--magnitude",
            "4.5",
            "--size",
            "60x20",
            "--fade",
            fade,
        ])
        .expect("arguments should parse");
        let mut flight = Flight::new(&args, 60, 20);
        let mut out = Vec::new();
        for _ in 0..120 {
            flight.advance(1.0 / 60.0);
            flight.draw(60.0, false, false);
            let mut frame = Vec::new();
            flight
                .present_plain(&mut frame)
                .expect("writing to a Vec cannot fail");
            out.push(frame);
        }
        out
    };

    let (plain, faded) = (frames("0"), frames("0.5"));
    assert_ne!(
        plain[0], faded[0],
        "half a second of fade left the opening frame exactly as it was"
    );
    // A shot opens at the bottom of the dip, so the rise is the whole of the
    // fade less its fall: half a second of `--fade` is settled by frame 21 at a
    // sixtieth of a second a frame.
    for (i, (a, b)) in plain.iter().zip(&faded).enumerate().skip(30) {
        assert_eq!(
            a, b,
            "frame {i} is past the fade and still differs from the flight that had none"
        );
    }
}
