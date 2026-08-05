//! The hand on the stick when there is nobody at it.
//!
//! Both `--demo` and `--screensaver` fly this way, and a screensaver is left
//! running for days, so the cycle has to keep coming round for all of them —
//! hence the `f64` clock, and the tests that start one a week in.

use crate::ship::Ship;

/// A repeating run-up to warp: a long cruise with a lazy weave, then a drop
/// back to impulse and a coast before the next one.
#[derive(Default)]
pub struct Autopilot {
    phase: usize,
}

impl Autopilot {
    /// Length of one full run-up-and-drop-out cycle, in seconds.
    pub const CYCLE: f64 = 46.0;

    /// `elapsed` is `f64` for the same reason `Flight::time` is: a screensaver
    /// runs for days, and this has to keep cycling for all of them.
    pub fn update(&mut self, ship: &mut Ship, elapsed: f64) {
        // Inside one cycle, so everything downstream is small enough for `f32`.
        let t = (elapsed % Self::CYCLE) as f32;
        let phase = match t {
            t if t < 6.0 => 0,  // sublight, easing the throttle up
            t if t < 32.0 => 1, // at warp
            t if t < 40.0 => 2, // dropping out
            _ => 3,             // coasting before the next run
        };

        if phase != self.phase {
            match phase {
                1 => {
                    ship.throttle = 0.55;
                    if !ship.warp_engaged {
                        ship.toggle_warp();
                    }
                }
                2 => {
                    if ship.warp_engaged {
                        ship.toggle_warp();
                    }
                }
                _ => {}
            }
            self.phase = phase;
        }

        match phase {
            0 => ship.throttle = (0.15 + t * 0.10).min(0.80),
            1 => {
                ship.throttle = (0.55 + (t - 6.0) * 0.025).min(1.0);
                // A slow weave, so the view is never quite static. Evaluated in
                // `f64` so the argument reduction stays exact indefinitely.
                ship.nudge_yaw((elapsed * 0.31).sin() as f32 * 0.003);
                ship.nudge_pitch((elapsed * 0.19).cos() as f32 * 0.002);
            }
            2 => ship.throttle = (ship.throttle - 0.004).max(0.25),
            _ => ship.throttle = (ship.throttle - 0.002).max(0.15),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_autopilot_completes_a_cycle_without_getting_stuck() {
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let dt = 1.0 / 60.0;
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
        assert!(peak > 100.0, "the autopilot never got up to speed: {peak} c");
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
            assert!(peak > 100.0, "never got up to speed {days:.1} days in: {peak} c");
        }
    }
}
