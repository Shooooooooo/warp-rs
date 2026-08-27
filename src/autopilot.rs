//! What a flight does when there is nobody flying it.

use crate::ship::Ship;
use crate::view::Orbit;
use std::f64::consts::TAU;

/// When the drive lights, when it shuts down, and when the coast before the
/// next run begins, in seconds into a cycle.
const LIGHT_AT: f32 = 6.0;
const DROP_AT: f32 = 32.0;
const COAST_AT: f32 = 40.0;

/// Throttle the run-up starts from, and how fast it walks up before the drive
/// lights. Six seconds of it reaches 0.75, which used to be followed by a
/// `.min(0.80)` that could therefore never bind.
const IDLE_THROTTLE: f32 = 0.15;
const RUNUP_RATE: f32 = 0.10;

/// Where the throttle sits the moment the drive lights, and how fast it climbs
/// from there toward whatever this cycle's peak turns out to be.
const ENTRY_THROTTLE: f32 = 0.55;
const CLIMB_RATE: f32 = 0.025;

/// How fast the throttle comes back once the drive is shut down, and the floor
/// it settles on; then the same pair again for the coast that follows.
const DROPOUT_RATE: f32 = 0.24;
const DROPOUT_FLOOR: f32 = 0.25;
const COAST_RATE: f32 = 0.12;
const COAST_FLOOR: f32 = 0.15;

/// The lowest peak throttle a cycle is allowed to climb to, and how far above
/// it the wave can carry one.
const PEAK_FLOOR: f32 = 0.78;
const PEAK_SWING: f32 = 0.22;
const PEAK_PERIOD: f64 = 71.0;

/// How long the camera takes to walk all the way round the ship, in seconds.
const CAMERA_TURN: f64 = 43.0;

/// How far the camera lifts over the hull and drops under it, in radians, and
/// how long that takes.
const CAMERA_LIFT: f32 = 0.55;
const CAMERA_LIFT_PERIOD: f64 = 29.0;

/// How much closer and further off the camera drifts, as a factor either way,
/// and how long that takes.
const CAMERA_ZOOM_SWING: f32 = 1.4;
const CAMERA_ZOOM_PERIOD: f64 = 37.0;

/// A repeating run-up to warp: a long cruise with a lazy weave, then a drop
/// back to impulse and a coast before the next one — flown a little differently
/// every time, and watched from somewhere new.
#[derive(Default)]
pub struct Autopilot {
    phase: usize,
}

impl Autopilot {
    /// Length of one full run-up-and-drop-out cycle, in seconds.
    pub const CYCLE: f64 = 46.0;

    /// Fly the ship for one frame: the throttle, and the drive.
    pub fn update(&mut self, ship: &mut Ship, elapsed: f64) {
        // Inside one cycle, so everything downstream is small enough for `f32`.
        let t = (elapsed % Self::CYCLE) as f32;
        let phase = match t {
            t if t < LIGHT_AT => 0, // sublight, easing the throttle up
            t if t < DROP_AT => 1,  // at warp
            t if t < COAST_AT => 2, // dropping out
            _ => 3,                 // coasting before the next run
        };

        if phase != self.phase {
            // Only the drive is switched here.
            match phase {
                1 if !ship.warp_engaged => {
                    ship.toggle_warp();
                }
                2 if ship.warp_engaged => {
                    ship.toggle_warp();
                }
                _ => {}
            }
            self.phase = phase;
        }

        // All four arms are functions of `t` rather than of the throttle they
        // find.
        let peak = peak_throttle(elapsed);
        ship.throttle = match phase {
            0 => IDLE_THROTTLE + t * RUNUP_RATE,
            1 => (ENTRY_THROTTLE + (t - LIGHT_AT) * CLIMB_RATE).min(peak),
            2 => (peak - DROPOUT_RATE * (t - DROP_AT)).max(DROPOUT_FLOOR),
            _ => (DROPOUT_FLOOR - COAST_RATE * (t - COAST_AT)).max(COAST_FLOOR),
        };

        // And nothing touches the stick.
    }

    /// Where the camera should be watching from, as an offset from wherever
    /// `--orbit` parked it, and what to multiply the zoom by.
    pub fn camera(&self, elapsed: f64) -> (Orbit, f32) {
        (
            Orbit {
                azimuth: ramp(elapsed, CAMERA_TURN),
                elevation: CAMERA_LIFT * wave(elapsed, CAMERA_LIFT_PERIOD),
                // Left alone deliberately.
                roll: 0.0,
            },
            CAMERA_ZOOM_SWING.powf(wave(elapsed, CAMERA_ZOOM_PERIOD)),
        )
    }
}

/// The throttle this moment's cycle is climbing toward.
fn peak_throttle(elapsed: f64) -> f32 {
    PEAK_FLOOR + PEAK_SWING * up(wave(elapsed, PEAK_PERIOD))
}

/// A wave from `-1` to `1` that comes round once every `period` seconds.
fn wave(elapsed: f64, period: f64) -> f32 {
    ((elapsed % period) * (TAU / period)).sin() as f32
}

/// The same fold, handed back as the angle itself: `0` to `TAU` once every
/// `period` seconds.
fn ramp(elapsed: f64, period: f64) -> f32 {
    ((elapsed % period) * (TAU / period)) as f32
}

/// A wave lifted into `0..=1`, which is what a knob with a floor wants.
fn up(wave: f32) -> f32 {
    0.5 + 0.5 * wave
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{ZOOM_MAX, ZOOM_MIN};

    /// One sim step, spelled here because `app::SIM_STEP` is private to the
    /// module that owns the loop and this has to be the same flight it flies.
    const SIM_STEP: f32 = 1.0 / 120.0;

    /// Fly the autopilot at `fps` for `seconds` and report the peak velocity it
    /// reached, and where it left the ship.
    fn fly(fps: f64, seconds: f64, start: f64) -> (f32, Ship) {
        let dt = 1.0 / fps;
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let (mut peak, mut acc) = (0.0f32, 0.0f32);
        for frame in 0..(seconds * fps) as usize {
            autopilot.update(&mut ship, start + frame as f64 * dt);
            acc += dt as f32;
            while acc >= SIM_STEP {
                ship.update(SIM_STEP);
                acc -= SIM_STEP;
            }
            peak = peak.max(ship.velocity_c());
        }
        (peak, ship)
    }

    #[test]
    fn the_autopilot_completes_a_cycle_without_getting_stuck() {
        let dt = 1.0 / 60.0;
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let mut peak: f32 = 0.0;
        let mut engaged_at_some_point = false;

        for frame in 0..(Autopilot::CYCLE / dt as f64) as usize {
            autopilot.update(&mut ship, frame as f64 * dt as f64);
            ship.update(dt);
            peak = peak.max(ship.velocity_c());
            engaged_at_some_point |= ship.warp_engaged;
            assert!((0.0..=1.0).contains(&ship.throttle));
        }
        assert!(engaged_at_some_point, "the autopilot never lit the drive");
        assert!(
            peak > 100.0,
            "the autopilot never got up to speed: {peak} c"
        );
        assert!(!ship.warp_engaged, "the cycle should end back at impulse");
        assert!(ship.velocity_c() < 1.0, "it should be sublight by the end");
    }

    #[test]
    fn the_autopilot_still_cycles_after_days_aloft() {
        // Same clock, same problem: `elapsed % CYCLE` needs the resolution to
        // land in distinct phases one frame apart.
        let dt = 1.0 / 60.0f64;
        for start in [0.0f64, 86_400.0, 604_800.0, 10_000_000.0] {
            let mut ship = Ship::new();
            let mut autopilot = Autopilot::default();
            let (mut peak, mut engaged) = (0.0f32, false);

            for frame in 0..(2.0 * Autopilot::CYCLE / dt) as usize {
                autopilot.update(&mut ship, start + frame as f64 * dt);
                ship.update(dt as f32);
                peak = peak.max(ship.velocity_c());
                engaged |= ship.warp_engaged;
            }
            let days = start / 86_400.0;
            assert!(engaged, "the drive never lit {days:.1} days in");
            assert!(
                peak > 100.0,
                "never got up to speed {days:.1} days in: {peak} c"
            );
        }
    }

    #[test]
    fn a_ship_nobody_is_flying_is_pointed_exactly_where_it_started() {
        // The property the weave was taken out for, stated as strongly as it
        // can be.
        for fps in [10.0f64, 60.0, 500.0] {
            let (_, ship) = fly(fps, 2.0 * Autopilot::CYCLE, 0.0);
            assert_eq!(
                ship.axes,
                crate::ship::LEVEL_AXES,
                "two cycles at {fps} frames a second moved the nose to {:?}",
                ship.axes
            );
            for (name, rate) in [
                ("yaw", ship.yaw_rate),
                ("pitch", ship.pitch_rate),
                ("roll", ship.roll_rate),
                ("bank", ship.bank),
            ] {
                assert_eq!(
                    rate, 0.0,
                    "the autopilot left {rate} on the {name} at {fps} frames a second"
                );
            }
        }
    }

    #[test]
    fn the_throttle_comes_down_at_the_same_time_whatever_the_frame_rate() {
        // The other half of the same fault, and the one that was visible: the
        // drop-out walked the throttle down a fixed step per frame, so at
        // `--fps 10` — which is what the `steer.txt` reference flies at — the
        // eight seconds it has shed 0.32 of throttle instead of the 1.92 it
        // sheds at sixty, and the floor was never reached at all.
        let rates = [10.0f64, 60.0, 120.0];
        let floored: Vec<f64> = rates
            .into_iter()
            .map(|fps| {
                let dt = 1.0 / fps;
                let mut ship = Ship::new();
                let mut autopilot = Autopilot::default();
                let mut acc = 0.0f32;
                for frame in 0..(Autopilot::CYCLE * fps) as usize {
                    let elapsed = frame as f64 * dt;
                    autopilot.update(&mut ship, elapsed);
                    acc += dt as f32;
                    while acc >= SIM_STEP {
                        ship.update(SIM_STEP);
                        acc -= SIM_STEP;
                    }
                    if elapsed > DROP_AT as f64 && ship.throttle <= DROPOUT_FLOOR + 1e-4 {
                        return elapsed;
                    }
                }
                panic!("the drop-out never reached its floor at {fps} frames a second")
            })
            .collect();

        // Asked of the spread rather than of a number: the throttle the drop-
        // out starts from wanders from cycle to cycle, so where the floor is
        // reached is a fact about *this* cycle.
        let lo = floored.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = floored.iter().cloned().fold(0.0f64, f64::max);
        assert!(
            hi - lo < 0.15,
            "the drop-out reaches its floor at {floored:?} seconds in at \
             {rates:?} frames a second"
        );
    }

    #[test]
    fn the_stick_is_never_touched_at_all() {
        let dt = 1.0 / 60.0;
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        for frame in 0..(2.0 * Autopilot::CYCLE / dt as f64) as usize {
            autopilot.update(&mut ship, frame as f64 * dt as f64);
            ship.update(dt);
            assert_eq!(
                (ship.yaw_rate, ship.pitch_rate, ship.roll_rate),
                (0.0, 0.0, 0.0),
                "the autopilot put the stick over at frame {frame}"
            );
        }
    }

    #[test]
    fn no_two_cycles_are_flown_the_same_way() {
        // A screensaver is watched for minutes, and a schedule that comes round
        // unchanged is read as a loop however pretty one pass is.
        let peaks: Vec<f32> = (0..6)
            .map(|cycle| fly(60.0, Autopilot::CYCLE, cycle as f64 * Autopilot::CYCLE).0)
            .collect();
        for (i, a) in peaks.iter().enumerate() {
            for b in &peaks[i + 1..] {
                assert!(
                    (a - b).abs() > 0.05 * a.max(*b),
                    "two cycles both peaked at about the same speed: {peaks:?}"
                );
            }
        }
    }

    #[test]
    fn every_cycle_still_reaches_warp_however_the_schedule_wanders() {
        // The floor under the wandering peak, asked over enough cycles to walk
        // the whole of its period and from far enough in that the `f64` clock
        // is doing real work.
        for start in [0.0f64, 604_800.0] {
            let mut worst = f32::INFINITY;
            for cycle in 0..40 {
                let at = start + cycle as f64 * Autopilot::CYCLE;
                worst = worst.min(fly(60.0, Autopilot::CYCLE, at).0);
            }
            assert!(
                worst > 300.0,
                "the slowest cycle {start} s in only reached {worst} c"
            );
        }
    }

    #[test]
    fn the_camera_starts_exactly_where_the_flag_parked_it() {
        // Bitwise, not nearly: the offset is laid on top of `--orbit` and the
        // factor multiplies the zoom, so anything but an exact identity here
        // would move the opening frame of every reference flight — including
        // the four that carry no autopilot at all, if this ever reached them.
        let (swing, zoom) = Autopilot::default().camera(0.0);
        assert_eq!(swing, Orbit::LEVEL, "the camera does not open on the beam");
        assert_eq!(
            zoom, 1.0,
            "the camera does not open at the framing asked for"
        );
    }

    #[test]
    fn the_camera_goes_all_the_way_round_a_ship_nobody_is_flying() {
        // The gap this closes: `--screensaver --view side` used to park the
        // camera wherever `--orbit` left it and never move it again, so the one
        // view whose whole subject is the ship was very nearly a still life.
        let autopilot = Autopilot::default();
        let (mut seen_ahead, mut seen_astern) = (false, false);
        let (mut lifted, mut dropped) = (false, false);
        for frame in 0..(300.0 * 60.0) as usize {
            let (swing, _) = autopilot.camera(frame as f64 / 60.0);
            let az = crate::ship::wrap_signed(swing.azimuth);
            seen_ahead |= az > 1.4;
            seen_astern |= az < -1.4;
            lifted |= swing.elevation > 0.5;
            dropped |= swing.elevation < -0.5;
        }
        assert!(
            seen_ahead && seen_astern,
            "the camera never gets both ahead of the ship and astern of it"
        );
        assert!(
            lifted && dropped,
            "the camera never gets both over the hull and under it"
        );
    }

    #[test]
    fn the_camera_never_asks_for_a_zoom_it_is_not_allowed() {
        // The clamp in `Flight::fly_itself` is the guard that matters; this is
        // the statement that it is a guard rather than a working part, so the
        // ship is never quietly pinned against a stop for half a period.
        let autopilot = Autopilot::default();
        let (mut lo, mut hi) = (f32::INFINITY, 0.0f32);
        for frame in 0..(400.0 * 30.0) as usize {
            let (_, zoom) = autopilot.camera(frame as f64 / 30.0);
            lo = lo.min(zoom);
            hi = hi.max(zoom);
        }
        assert!(
            lo > ZOOM_MIN && hi < ZOOM_MAX,
            "the camera asks for zooms from {lo} to {hi}, outside {ZOOM_MIN}..{ZOOM_MAX}"
        );
    }
}
