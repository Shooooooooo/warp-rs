//! Where the ship has been, so a long exposure can be drawn along it.
//!
//! The exposure in [`crate::universe`] is the track a star swept while the
//! shutter was open, and until this module existed it was worked out by
//! rewinding the ship along the nose it is pointed down *now*. That is exact
//! for a straight run and wrong the moment the stick moves: the ship was
//! somewhere else, it was pointed somewhere else, and the camera is bolted to
//! the hull, so the film turned with it. Three seconds of exposure against a
//! yaw stop of 0.85 rad/s is 146° of attitude that nothing in the tree
//! remembered.
//!
//! So it is remembered, and remembered rather than reconstructed. Working the
//! past out from the present is the fault [`crate::universe::Universe`]'s own
//! exposure length already shipped once — a number recomputed each frame from
//! the speed the ship happens to be doing reaches backward into the past faster
//! than time is passing — and an attitude is worse, because a rate that has
//! decayed to nothing leaves no trace at all of the turn it just flew.
//!
//! Two decisions carry this module.
//!
//! **It is measured in light years, not in seconds.** The exposure is a
//! distance; the ring is walked by arc length and knows nothing about the
//! clock. That is also what makes it exact across a frame rate: the same
//! flight at 10 and at 120 fps records the same track at different densities
//! and hands back the same poses.
//!
//! **The straight run is bitwise.** [`Track::straight_run`] is how far back the
//! recorded attitude is bit-for-bit the current one, and an exposure that does
//! not reach past it needs none of this — the caller takes its old arithmetic
//! and the picture is unchanged to the last bit. A ship nobody steers holds
//! [`crate::ship::LEVEL_AXES`] exactly, so that is the whole of a straight
//! flight, and the reference frames go on being the frames they were. An
//! epsilon there would be a threshold nobody chose with the reference frames
//! sitting on one side of it.

/// Poses the ring holds.
///
/// 1024 of them is 8.5 seconds at `app::SIM_STEP` and about 70 KB, allocated
/// once when the sky is built and never again. Sized in *samples* rather than
/// in seconds because what has to be covered is a distance and the exchange
/// rate is the speed: the exposure settles at `TRAIL_SECONDS · warp · v`, and
/// during a spool-up that runs ahead of the distance actually flown in the last
/// `TRAIL_SECONDS` — `Ship::toggle_warp` snaps 3.4 c on and the ease climbs to
/// 2000 c behind it, so `3·v` outruns `∫v` for as long as `v` is still
/// climbing. The headroom is for that, and [`Track::flown`] is the backstop:
/// the exposure is clamped to it, so it can never reach back past a flight the
/// ring no longer holds.
const TRACK_SAMPLES: usize = 1024;

/// One pose the ship held, and how far it had flown by then.
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Light years, in the inertial world frame. `f64` for the reason
    /// [`crate::ship::Ship::position`] gives.
    origin: [f64; 3],
    /// Starboard, down, nose — [`crate::ship::Ship::axes`] as it stood.
    axes: [[f32; 3]; 3],
    /// Cumulative arc length at this sample, monotone and never reset. `f64`
    /// because it only ever grows and a screensaver is left up for days.
    flown: f64,
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
    ///
    /// The step is measured from the positions themselves rather than taken
    /// from the `speed · dt` beside them, and that is what makes `R` harmless
    /// without this module knowing anything about it: `Ship::reset` teleports
    /// the ship back to the origin mid-flight, and a jump counted as flown
    /// distance is a jump the exposure can never reach back across — it snaps
    /// to zero with the speed at the same instant and can only grow again as
    /// fast as the ship flies. Counted as `speed · dt` the ring would hold two
    /// poses light years apart at adjacent arc lengths, and a walk would land
    /// between them.
    pub fn record(&mut self, origin: [f64; 3], axes: [[f32; 3]; 3]) {
        let (step, was_straight) = match self.newest() {
            Some(last) => {
                let (dx, dy, dz) = (
                    origin[0] - last.origin[0],
                    origin[1] - last.origin[1],
                    origin[2] - last.origin[2],
                );
                ((dx * dx + dy * dy + dz * dz).sqrt(), last.axes == axes)
            }
            None => (0.0, true),
        };
        let step = if step.is_finite() { step } else { 0.0 };
        let flown = self.newest().map_or(0.0, |last| last.flown) + step;
        // Bitwise, deliberately: see the module doc.
        self.straight = if was_straight { self.straight + step } else { step };

        let sample = Sample {
            origin,
            axes,
            flown,
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
    ///
    /// The exposure is clamped to this, which is the honest statement of what a
    /// finite ring can promise: a renderer may draw the flight it remembers and
    /// not one light year more.
    pub fn flown(&self) -> f64 {
        match (self.oldest(), self.newest()) {
            (Some(first), Some(last)) => last.flown - first.flown,
            _ => 0.0,
        }
    }

    /// How far back the attitude is bit-for-bit the one being flown now.
    pub fn straight_run(&self) -> f32 {
        // Held to what the ring can still speak for. An attitude unchanged for
        // longer than the ring is long is still straight over the whole of it,
        // and the exposure is clamped to the same number, so the comparison the
        // caller makes stays true.
        self.straight.min(self.flown()) as f32
    }

    /// The pose the ship held `back` light years ago along its own track.
    ///
    /// Clamped to the oldest sample the ring still holds rather than
    /// extrapolated past it, which cannot bite while the exposure is clamped to
    /// [`Self::flown`] and is the right answer if it ever did: the last thing
    /// this should do is invent a flight.
    ///
    /// The two samples either side are *interpolated between* on the position
    /// and the nearer one is taken whole on the attitude. Rounding a position
    /// to the nearer sample would step the tail of every streak in the sky
    /// forward and back as the walk crossed a sample boundary, which is a
    /// visible jitter at 120 samples a second; an attitude is a basis and
    /// blending two of them is not one, so the near one is taken instead — the
    /// two are a sim step apart, which at the yaw stop is 0.007 of a radian.
    pub fn pose_at(&self, back: f32) -> ([f64; 3], [[f32; 3]; 3]) {
        let Some(newest) = self.newest() else {
            return ([0.0; 3], crate::ship::LEVEL_AXES);
        };
        if !(back > 0.0) {
            return (newest.origin, newest.axes);
        }
        let want = newest.flown - back as f64;
        // Binary search over the ring in flight order. `flown` is monotone, so
        // the partition point is the first sample at or past where the walk
        // wants to be.
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
            let oldest = self.at(0);
            return (oldest.origin, oldest.axes);
        }
        let (before, after) = (self.at(lo - 1), self.at(lo.min(self.len - 1)));
        let span = after.flown - before.flown;
        let t = if span > 0.0 {
            ((want - before.flown) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let origin = [
            before.origin[0] + (after.origin[0] - before.origin[0]) * t,
            before.origin[1] + (after.origin[1] - before.origin[1]) * t,
            before.origin[2] + (after.origin[2] - before.origin[2]) * t,
        ];
        let axes = if t < 0.5 { before.axes } else { after.axes };
        (origin, axes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship::{Ship, LEVEL_AXES};

    /// Fly a ship, recording as the sky does, and hand back the track and a
    /// straight log of every pose for a test to check the ring against.
    fn flown(steps: usize, steer: bool) -> (Track, Vec<([f64; 3], [[f32; 3]; 3], f64)>) {
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
            let after = log.partition_point(|(_, _, f)| *f < want).min(log.len() - 1);
            let before = after.saturating_sub(1);
            for axis in 0..3 {
                let (a, b) = (log[before].0[axis], log[after].0[axis]);
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let slack = (hi - lo).max(1e-9) + 1e-9;
                assert!(
                    origin[axis] >= lo - slack && origin[axis] <= hi + slack,
                    "walking back {back} ly landed at {} on axis {axis}, outside {lo}..{hi}",
                    origin[axis]
                );
            }
            checked += 1;
        }
        assert!(checked == 64, "only {checked} distances were walked");
    }

    #[test]
    fn walking_back_nothing_lands_on_the_ship() {
        // Exactly, not nearly. A zero-length exposure is a star drawn as a
        // point where it stands, and the head of every streak is projected from
        // this pose whatever the tail is doing.
        let (track, log) = flown(200, true);
        let (origin, axes) = track.pose_at(0.0);
        let last = log[log.len() - 1];
        assert_eq!(origin, last.0, "the head of the track moved");
        assert_eq!(axes, last.1, "the attitude at the head moved");
        // And a nonsense distance is the same answer rather than a panic.
        for back in [-1.0, f32::NAN] {
            assert_eq!(track.pose_at(back).0, last.0);
        }
    }

    #[test]
    fn a_ship_that_never_turns_is_straight_all_the_way_back() {
        // The fast path's whole warrant. An unsteered flight holds `LEVEL_AXES`
        // to the bit, so the straight run is the entire track and the exposure
        // — which is clamped to the same number — can never reach past it.
        let (track, _) = flown(600, false);
        assert_eq!(
            track.straight_run(),
            track.flown() as f32,
            "a flight nobody steered was not straight all the way back"
        );

        // And one nudge collapses it. Not to zero: the step the turn happened
        // on is still one step of straight flight.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        let mut track = Track::new();
        for _ in 0..600 {
            ship.update(1.0 / 120.0);
            track.record(ship.position, ship.axes);
        }
        assert_eq!(ship.axes, LEVEL_AXES, "the ship steered itself");
        let before = track.straight_run();
        ship.nudge_yaw(1.0);
        ship.update(1.0 / 120.0);
        track.record(ship.position, ship.axes);
        let after = track.straight_run();
        assert!(
            after < before * 0.01,
            "a turn left {after} of straight run behind it, against {before}"
        );
        assert!(after > 0.0, "the step the turn happened on was flown");
    }

    #[test]
    fn the_ring_never_grows() {
        // A screensaver is left up for days. The ring is allocated once and
        // what it holds stops growing; only the distance it spans is bounded by
        // the flight rather than by the ring.
        let (track, _) = flown(4000, true);
        assert_eq!(track.ring.len(), TRACK_SAMPLES);
        assert_eq!(track.ring.capacity(), TRACK_SAMPLES);
        assert_eq!(track.len, TRACK_SAMPLES);
        let span = track.flown();
        assert!(span > 0.0, "a long flight remembered no track at all");
        assert!(
            span < 40.0,
            "the ring is holding {span} ly, which is more than it should"
        );
    }

    #[test]
    fn a_reset_is_not_a_flight() {
        // `R` teleports the ship back to the origin mid-flight. The jump is
        // counted as flown distance, so the exposure — which restarts at zero
        // with the speed and grows only as fast as the ship — can never reach
        // back across it. What that has to mean here is that walking back any
        // distance the flight since the reset has actually earned lands on the
        // near side of the jump.
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
    fn an_empty_track_answers_rather_than_panicking() {
        // Nothing has flown yet on the first frame, and the sky is drawn on it.
        let track = Track::new();
        assert_eq!(track.flown(), 0.0);
        assert_eq!(track.straight_run(), 0.0);
        assert_eq!(track.pose_at(5.0), ([0.0; 3], LEVEL_AXES));
    }
}
