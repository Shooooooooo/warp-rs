//! The animation on the README's front page, shot.
//!
//! ```sh
//! cargo run --release --features snapshot --example demo -- docs/demo.png
//! ```
//!
//! It is a flight flown through the library rather than the binary, because
//! the command line has no way to say "light the drive a second in, then go
//! outside": the cockpit at impulse, the drive lighting, the tunnel, then a cut
//! to the camera outside walking round the ship, and a cut back in that is
//! stopped at the bottom of its dip. A run opens out of black, so that is where
//! the loop joins itself.

use clap::Parser;
use std::path::PathBuf;
use warp_rs::app::Flight;
use warp_rs::cli::Args;
use warp_rs::snapshot::Animation;

/// Frames a second, which is also the flight's timestep.
const FPS: u16 = 30;
/// The canvas, in terminal cells, and how many pixels a subpixel is drawn as.
/// Four makes it 960 across, wider than the column GitHub gives a README, so
/// the page shrinks it to fit rather than stretching it soft.
const SIZE: (usize, usize) = (240, 68);
const SCALE: usize = 4;
/// When the drive lights, when the camera goes outside, and when it comes back
/// in, in seconds from the first frame.
const ENGAGE: f64 = 1.2;
const OUTSIDE: f64 = 4.0;
const INSIDE: f64 = 8.0;
/// How fast the camera walks round the ship and up over it while it is
/// outside, in notches of `D` and `W` a second.
const WALK: (f32, f32) = (2.0, 0.6);
/// The sky, the throttle, and where the camera outside starts: on the
/// starboard quarter, a little above, so the walk ends astern and over the top.
const ARGS: &[&str] = &[
    "warp",
    "--seed",
    "6",
    "--throttle",
    "1.0",
    "--orbit",
    "205,12,0",
    "--color",
    "truecolor",
];

fn main() {
    let path: PathBuf = std::env::args()
        .nth(1)
        .expect("expected: <file.png>")
        .into();
    let args = Args::try_parse_from(ARGS).expect("the demo's own arguments should parse");
    let (cols, rows) = SIZE;
    let mut flight = Flight::new(&args, cols, rows);
    let (w, h) = flight.canvas_dims();
    let mut film = Animation::new(w, h);
    let dt = 1.0 / f32::from(FPS);

    let (mut lit, mut outside, mut inside) = (false, false, false);
    let (mut frames, mut poster, mut darkest) = (0usize, 0usize, u8::MAX);
    loop {
        let t = frames as f64 / f64::from(FPS);
        if !lit && t >= ENGAGE {
            flight.toggle_warp();
            lit = true;
        }
        if !outside && t >= OUTSIDE {
            flight.cycle_view();
            outside = true;
        }
        if outside && !inside {
            if t >= INSIDE {
                // The last of the walk is the ship from astern and above, which
                // is the still to show anything that cannot animate.
                poster = frames - 1;
                flight.cycle_view();
                inside = true;
            } else {
                flight.nudge_orbit(WALK.0 * dt, WALK.1 * dt, 0.0);
            }
        }
        // Drawn before it is stepped, so a frame is the flight at `t` and the
        // first is the bottom of the dip the shot opens out of: black.
        flight.draw(f32::from(FPS), false, false);

        // Frames do not land exactly on the bottom of the closing dip, so the
        // loop stops on the darkest one it does land on.
        let peak = flight
            .pixels()
            .iter()
            .map(|p| p[0].max(p[1]).max(p[2]))
            .max()
            .unwrap_or(0);
        if inside {
            if peak > darkest {
                break;
            }
            darkest = peak;
        }
        film.push(flight.pixels());
        flight.advance(dt);
        frames += 1;
        assert!(
            t < INSIDE + 2.0,
            "the cut back inside never started coming up again"
        );
    }

    film.write(&path, SCALE, FPS, poster)
        .expect("the animation should write");
    let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
    eprintln!(
        "wrote {} ({}x{} px, {frames} frames at {FPS} fps, {:.2} MB)",
        path.display(),
        w * SCALE,
        h * SCALE,
        bytes as f64 / 1e6
    );
}
