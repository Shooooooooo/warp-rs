//! Where the ship has been, so a long exposure can be drawn along it.

/// Poses the ring holds.
const TRACK_SAMPLES: usize = 1024;

/// One pose the ship held, and how far it had flown by then.
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Light years, in the inertial world frame. `f64` for the reason
    /// [`crate::ship::Ship::position`] gives.
    origin: [f64; 3],
    /// Starboard, down, nose — [`crate::ship::Ship::axes`] as it stood.
    axes: [[f32; 3]; 3],
    /// Cumulative arc length at this sample, monotone and never reset.
    flown: f64,
    /// Cumulative attitude change, in radians of total variation. Monotone for
    /// the same reason and searched the same way — see [`Track::turn_over`].
    turn: f64,
}

/// The flown track, as a ring of poses walked by arc length.
#[derive(Debug)]
pub struct Track {
    ring: Vec<Sample>,
    /// Where the next sample goes. The newest is at `head - 1`.
    head: usize,
    len: usize,
    /// Arc length back over which the recorded attitude is bit-for-bit what it
    /// is now. See the module doc: this is what lets the exposure take its old
    /// arithmetic unchanged while the ship is flying straight, which is what
    /// keeps the reference frames still.
    straight: f64,
}

impl Default for Track {
    fn default() -> Self {
        Self::new()
    }
}

impl Track {
    pub fn new() -> Self {
        Self {
            // Allocated in full here and never grown, so nothing about flying
            // touches the allocator — and so the one allocation happens while
            // the sky is being built, which is before the terminal has been
            // taken over.
            ring: Vec::with_capacity(TRACK_SAMPLES),
            head: 0,
            len: 0,
            straight: 0.0,
        }
    }

    /// Add the pose the ship is holding now.
    pub fn record(&mut self, origin: [f64; 3], axes: [[f32; 3]; 3]) {
        let (step, swept, was_straight) = match self.newest() {
            Some(last) => {
                let (dx, dy, dz) = (
                    origin[0] - last.origin[0],
                    origin[1] - last.origin[1],
                    origin[2] - last.origin[2],
                );
                (
                    (dx * dx + dy * dy + dz * dz).sqrt(),
                    turned_by(last.axes, axes) as f64,
                    last.axes == axes,
                )
            }
            None => (0.0, 0.0, true),
        };
        let step = if step.is_finite() { step } else { 0.0 };
        let (flown, turn) = self
            .newest()
            .map_or((0.0, 0.0), |last| (last.flown + step, last.turn + swept));
        // Bitwise, deliberately: see [`Self::straight_run`].
        self.straight = if was_straight {
            self.straight + step
        } else {
            step
        };

        let sample = Sample {
            origin,
            axes,
            flown,
            turn,
        };
        if self.ring.len() < TRACK_SAMPLES {
            self.ring.push(sample);
        } else {
            self.ring[self.head] = sample;
        }
        self.head = (self.head + 1) % TRACK_SAMPLES;
        self.len = (self.len + 1).min(TRACK_SAMPLES);
    }

    /// How much track the ring still holds, in light years.
    pub fn flown(&self) -> f64 {
        match (self.oldest(), self.newest()) {
            (Some(first), Some(last)) => last.flown - first.flown,
            _ => 0.0,
        }
    }

    /// How far back the recorded attitude is bit-for-bit the attitude being
    /// flown now, in light years.
    pub fn straight_run(&self) -> f32 {
        if self.straight >= self.flown() {
            f32::INFINITY
        } else {
            self.straight as f32
        }
    }

    /// How much the attitude turned over the last `back` light years of track,
    /// in radians of total variation.
    pub fn turn_over(&self, back: f32) -> f32 {
        let Some(newest) = self.newest() else {
            return 0.0;
        };
        if back.is_nan() || back <= 0.0 {
            return 0.0;
        }
        let (i, t) = self.walk_to(newest.flown - back as f64);
        let turn = if i == 0 {
            self.at(0).turn
        } else {
            let (before, after) = (self.at(i - 1), self.at(i.min(self.len - 1)));
            before.turn + (after.turn - before.turn) * t
        };
        ((newest.turn - turn) as f32).max(0.0)
    }

    /// The pose the ship held `back` light years ago along its own track.
    pub fn pose_at(&self, back: f32) -> ([f64; 3], [[f32; 3]; 3]) {
        let Some(newest) = self.newest() else {
            return ([0.0; 3], crate::ship::LEVEL_AXES);
        };
        // A NaN reach and a zero one mean the same thing: there is no exposure
        // to walk, so the pose is the one being flown.
        if back.is_nan() || back <= 0.0 {
            return (newest.origin, newest.axes);
        }
        let (i, t) = self.walk_to(newest.flown - back as f64);
        if i == 0 {
            let oldest = self.at(0);
            return (oldest.origin, oldest.axes);
        }
        let (before, after) = (self.at(i - 1), self.at(i.min(self.len - 1)));
        let origin = [
            before.origin[0] + (after.origin[0] - before.origin[0]) * t,
            before.origin[1] + (after.origin[1] - before.origin[1]) * t,
            before.origin[2] + (after.origin[2] - before.origin[2]) * t,
        ];
        let mut axes = [[0.0f32; 3]; 3];
        let blend = t as f32;
        for (axis, (a, b)) in axes.iter_mut().zip(before.axes.iter().zip(after.axes)) {
            for (place, (from, to)) in axis.iter_mut().zip(a.iter().zip(b)) {
                *place = from + (to - from) * blend;
            }
        }
        (origin, crate::ship::orthonormalise(axes))
    }

    /// How far back the track had turned by `swept` radians, in light years,
    /// never further than `limit`.
    pub fn back_at_turn(&self, swept: f32, limit: f32) -> f32 {
        let Some(newest) = self.newest() else {
            return 0.0;
        };
        if swept.is_nan() || swept <= 0.0 {
            return 0.0;
        }
        let want = newest.turn - swept as f64;
        let (mut lo, mut hi) = (0usize, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.at(mid).turn < want {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return limit;
        }
        let (before, after) = (self.at(lo - 1), self.at(lo.min(self.len - 1)));
        let span = after.turn - before.turn;
        let t = if span > 0.0 {
            ((want - before.turn) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let flown = before.flown + (after.flown - before.flown) * t;
        ((newest.flown - flown) as f32).clamp(0.0, limit)
    }

    /// Where a cumulative arc length falls in the ring: the index of the first
    /// sample at or past it, and how far between that one and the one before
    /// it.
    fn walk_to(&self, want: f64) -> (usize, f64) {
        let (mut lo, mut hi) = (0usize, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.at(mid).flown < want {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return (0, 0.0);
        }
        let (before, after) = (self.at(lo - 1), self.at(lo.min(self.len - 1)));
        let span = after.flown - before.flown;
        let t = if span > 0.0 {
            ((want - before.flown) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (lo, t)
    }

    /// The `i`th oldest sample the ring holds.
    fn at(&self, i: usize) -> &Sample {
        let start = (self.head + TRACK_SAMPLES - self.len) % TRACK_SAMPLES;
        &self.ring[(start + i) % TRACK_SAMPLES]
    }

    fn oldest(&self) -> Option<&Sample> {
        (self.len > 0).then(|| self.at(0))
    }

    fn newest(&self) -> Option<&Sample> {
        (self.len > 0).then(|| self.at(self.len - 1))
    }
}

/// How far one attitude turned into another, in radians.
fn turned_by(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> f32 {
    let mut trace = 0.0;
    for (row, other) in a.iter().zip(b) {
        for (x, y) in row.iter().zip(other) {
            trace += x * y;
        }
    }
    (3.0 - trace).max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship::{Ship, LEVEL_AXES};

    /// A pose the ship really held, and how far it had flown by then: what the
    /// ring is checked against, kept in a plain growing list so the check does
    /// not lean on the thing under test.
    type Logged = ([f64; 3], [[f32; 3]; 3], f64);

    /// Fly a ship, recording as the sky does, and hand back the track and a
    /// straight log of every pose for a test to check the ring against.
    fn flown(steps: usize, steer: bool) -> (Track, Vec<Logged>) {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut track = Track::new();
        let mut log = Vec::new();
        let mut total = 0.0f64;
        let mut last = ship.position;
        for step in 0..steps {
            if steer {
                ship.nudge_yaw(0.4);
                if step % 3 == 0 {
                    ship.nudge_pitch(-0.3);
                }
            }
            ship.update(1.0 / 120.0);
            let d = [
                ship.position[0] - last[0],
                ship.position[1] - last[1],
                ship.position[2] - last[2],
            ];
            total += (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            last = ship.position;
            track.record(ship.position, ship.axes);
            log.push((ship.position, ship.axes, total));
        }
        (track, log)
    }

    #[test]
    fn the_track_remembers_where_the_ship_has_been() {
        // The whole of what this module promises, asked against a log kept
        // beside it rather than against the ring's own arithmetic: walking back
        // `d` light years has to land where the ship really was after flying
        // the first `total - d` of its track.
        let (track, log) = flown(400, true);
        let end = log[log.len() - 1].2;
        let mut checked = 0;
        for i in 0..64 {
            let back = track.flown() as f32 * i as f32 / 64.0;
            let (origin, _) = track.pose_at(back);
            let want = end - back as f64;
            // The logged pose either side of that distance; the walk is
            // interpolated, so it has to sit between them.
            let after = log
                .partition_point(|(_, _, f)| *f < want)
                .min(log.len() - 1);
            let before = after.saturating_sub(1);
            for (axis, (walked, (a, b))) in origin
                .iter()
                .zip(log[before].0.iter().zip(log[after].0))
                .enumerate()
            {
                let (lo, hi) = if *a <= b { (*a, b) } else { (b, *a) };
                let slack = (hi - lo).max(1e-9) + 1e-9;
                assert!(
                    *walked >= lo - slack && *walked <= hi + slack,
                    "walking back {back} ly landed at {walked} on axis {axis}, outside {lo}..{hi}"
                );
            }
            checked += 1;
        }
        assert!(checked == 64, "only {checked} distances were walked");
    }

    #[test]
    fn walking_back_nothing_lands_on_the_ship() {
        let (track, log) = flown(200, true);
        let (origin, axes) = track.pose_at(0.0);
        let last = log[log.len() - 1];
        assert_eq!(origin, last.0, "the head of the track moved");
        assert_eq!(axes, last.1, "the attitude at the head moved");
        for back in [-1.0, f32::NAN] {
            assert_eq!(track.pose_at(back).0, last.0);
        }
    }

    #[test]
    fn a_ship_that_never_turns_is_straight_all_the_way_back() {
        // The fast path's whole warrant.
        let (track, _) = flown(600, false);
        assert_eq!(
            track.straight_run(),
            f32::INFINITY,
            "a flight nobody steered was not straight all the way back"
        );

        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut track = Track::new();
        for _ in 0..600 {
            ship.update(1.0 / 120.0);
            track.record(ship.position, ship.axes);
        }
        assert_eq!(ship.axes, LEVEL_AXES, "the ship steered itself");
        assert_eq!(track.straight_run(), f32::INFINITY);
        ship.nudge_yaw(1.0);
        ship.update(1.0 / 120.0);
        track.record(ship.position, ship.axes);
        let after = track.straight_run();
        assert!(
            after < track.flown() as f32 * 0.01,
            "a turn left {after} of straight run behind it, of {}",
            track.flown()
        );
        assert!(after > 0.0, "the step the turn happened on was flown");
    }

    #[test]
    fn the_ring_never_grows() {
        // A screensaver is left up for days.
        let (track, _) = flown(4000, true);
        assert_eq!(track.ring.len(), TRACK_SAMPLES);
        assert_eq!(track.ring.capacity(), TRACK_SAMPLES);
        assert_eq!(track.len, TRACK_SAMPLES);
        let long = track.flown();
        let (longer, _) = flown(12_000, true);
        assert!(
            (longer.flown() - long).abs() < long * 0.05,
            "three times the flight remembered {} ly against {long}",
            longer.flown()
        );
    }

    #[test]
    fn the_ring_holds_more_track_than_an_exposure_can_ask_for() {
        // The reason `pose_at` saturating rather than clamping the exposure is
        // an answer to something that cannot happen.
        let (track, _) = flown(4000, false);
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..2000 {
            ship.update(1.0 / 120.0);
        }
        let settled = 3.0 * ship.velocity_ly_per_s() as f64;
        assert!(
            track.flown() > settled * 2.5,
            "the ring holds {} ly against an exposure that settles at {settled}",
            track.flown()
        );
    }

    #[test]
    fn a_reset_is_not_a_flight() {
        // `R` teleports the ship back to the origin mid-flight.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut track = Track::new();
        for _ in 0..600 {
            ship.nudge_yaw(0.4);
            ship.update(1.0 / 120.0);
            track.record(ship.position, ship.axes);
        }
        let jumped_from = ship.position;
        ship.reset();
        ship.throttle = 1.0;
        ship.toggle_warp();
        track.record(ship.position, ship.axes);

        let mut earned = 0.0f64;
        let mut last = ship.position;
        for _ in 0..300 {
            ship.nudge_yaw(0.4);
            ship.update(1.0 / 120.0);
            let d = [
                ship.position[0] - last[0],
                ship.position[1] - last[1],
                ship.position[2] - last[2],
            ];
            earned += (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            last = ship.position;
            track.record(ship.position, ship.axes);

            let (origin, _) = track.pose_at(earned as f32);
            let strayed = origin
                .iter()
                .zip(jumped_from)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f64, f64::max);
            assert!(
                strayed > 1.0,
                "the exposure walked back across the reset, to {origin:?}"
            );
        }
    }

    #[test]
    fn the_run_a_turn_leaves_behind_it_is_not_measured_against_the_exposure() {
        // The bug the infinity in `straight_run` exists for, as the property
        // that rules it out.
        let (track, _) = flown(240, false);
        let flown_far = track.flown() as f32;
        assert!(
            track.straight_run() > flown_far * 2.0,
            "a straight flight of {flown_far} ly answered {}",
            track.straight_run()
        );
    }

    #[test]
    fn a_turn_that_goes_out_and_comes_back_still_swept_the_arc_it_swept() {
        // Total variation and not the angle between the two ends, which is what
        // a weave makes expensive to get wrong.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut track = Track::new();
        let opened = ship.axes;
        for step in 0..600 {
            ship.nudge_yaw(if step < 300 { 0.5 } else { -0.5 });
            ship.update(1.0 / 120.0);
            track.record(ship.position, ship.axes);
        }
        let net = turned_by(opened, ship.axes);
        let swept = track.turn_over(track.flown() as f32);
        assert!(
            net < 0.15,
            "the ship was supposed to come back to its heading, and turned {net}"
        );
        assert!(
            swept > net * 5.0,
            "a there-and-back turn swept {swept} against a net {net}"
        );
    }

    #[test]
    fn nothing_turns_over_no_distance_at_all() {
        // The other end of it: asking how much a zero-length exposure swept has
        // to be zero rather than the whole flight's turn, or the station count
        // would be picked off a swing nothing was drawn over.
        let (track, _) = flown(400, true);
        assert_eq!(track.turn_over(0.0), 0.0);
        assert_eq!(track.turn_over(-1.0), 0.0);
        assert!(track.turn_over(track.flown() as f32) > 0.0);
    }

    #[test]
    fn an_empty_track_answers_rather_than_panicking() {
        // Nothing has flown yet on the first frame, and the sky is drawn on it.
        let track = Track::new();
        assert_eq!(track.flown(), 0.0);
        // Infinity rather than zero: nothing has turned, so nothing an exposure
        // could ask about reaches back past a turn.
        assert_eq!(track.straight_run(), f32::INFINITY);
        assert_eq!(track.pose_at(5.0), ([0.0; 3], LEVEL_AXES));
    }
}
