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
    /// Cumulative arc length at this sample, monotone and never reset.
    ///
    /// `f64` beside an `f32` everything else here, and that is the one place in
    /// this arithmetic that cancels badly: the tail is found by subtracting the
    /// exposure's length from the distance flown so far, two large numbers
    /// leaving a small one, at full warp after a flight that has been up for
    /// days. It only ever grows, too, which is the same argument
    /// [`crate::app::Flight::time`] makes for itself.
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

    /// How far back the recorded attitude is bit-for-bit the attitude being
    /// flown now, in light years.
    ///
    /// Bitwise, and not within a tolerance, because what it decides is whether
    /// the exposure takes the arithmetic every reference frame is pinned
    /// through. [`crate::ship::Ship::steer`] runs `orthonormalise` every step,
    /// and at [`crate::ship::LEVEL_AXES`] every dot product in it is a hard
    /// zero and every norm a hard one — so a flight nobody steers holds one
    /// attitude to the last bit for as long as it flies, which is what
    /// `a straight flight has turned` over in `ship.rs` already says. The four
    /// `--engage` reference flights take the fast path from the first frame to
    /// the last because of it.
    ///
    /// Note what this does *not* claim. Away from level, `orthonormalise` is
    /// not known to have a fixed point, so a ship that has once been turned may
    /// never come back to the fast path however straight it then flies. That
    /// costs a few matrix products a frame and nothing whatever in the picture.
    ///
    /// It is an accumulator in its own right rather than something read off the
    /// ring, and deliberately: an attitude unchanged for longer than the ring is
    /// long is still unchanged, and capping this at what the ring remembers
    /// would push a long exposure onto the curved path for no reason but the
    /// buffer's size.
    pub fn straight_run(&self) -> f32 {
        self.straight as f32
    }

    /// How much the attitude turned over the last `back` light years of track,
    /// in radians of total variation.
    ///
    /// Total variation and not the angle between the two ends, which is the
    /// distinction a weave makes expensive to get wrong: a ship that yaws a
    /// radian one way and a radian back has its nose where it started, so the
    /// net rotation is nearly the identity — and the arc its stars swept is two
    /// radians wide. A curve chosen by the net angle would be drawn as a chord
    /// straight across it. Summed per step and carried cumulatively, which also
    /// makes it monotone in how far back you look and so searchable the same way
    /// the arc length is.
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
    ///
    /// **Saturating** at the oldest sample the ring still holds, rather than
    /// extrapolated past it and rather than answered by clamping the exposure
    /// to what the ring remembers. Those are not the same guard and the
    /// difference is worth the sentence: the exposure is an `f32` sum of
    /// `speed · dt` and the ring's span is an `f64` sum of the same steps, so a
    /// `min` between them lands within a rounding of the number it was given
    /// and shaves an ulp off it either way — and an ulp on that length is
    /// applied to every star in the sky, which repaints every warp reference
    /// frame to say nothing at all. Saturating the *lookup* costs no arithmetic
    /// on any path, and a drawn exposure that fell short of the one asked for
    /// would move its tail outward, which is the direction a tail is allowed to
    /// move. The ring is sized so it cannot happen; this is what it does if it
    /// ever does.
    ///
    /// The pose between two samples is interpolated rather than rounded to the
    /// nearer of them. Rounding would step the tail of every streak in the sky
    /// as the walk crossed a sample boundary, and the walk crosses one every
    /// step by construction — the exposure grows by exactly the distance the
    /// ship flew — so it would be a jitter at 120 a second rather than an
    /// occasional one. The attitude is lofted the same way and pulled back to
    /// orthonormal afterwards, since a blend of two bases is not a basis: over
    /// one sim step the two differ by 0.007 of a radian at the yaw stop, so
    /// what the pullback has to correct is nothing.
    pub fn pose_at(&self, back: f32) -> ([f64; 3], [[f32; 3]; 3]) {
        let Some(newest) = self.newest() else {
            return ([0.0; 3], crate::ship::LEVEL_AXES);
        };
        // A NaN reach and a zero one mean the same thing: there is no exposure
        // to walk, so the pose is the one being flown. Spelled out rather than
        // as a negated comparison, which is the same test and reads as if the
        // NaN were an afterthought.
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

    /// Where a cumulative arc length falls in the ring: the index of the first
    /// sample at or past it, and how far between that one and the one before it.
    ///
    /// Binary search, because `flown` is monotone by construction and the ring
    /// holds a thousand samples that a frame walks a dozen times. An index of
    /// zero means the walk wanted further back than the ring remembers, which
    /// is the saturating case both callers answer for themselves.
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
///
/// From the trace of `a·bᵀ`, which for a rotation is `1 + 2cos θ`, so
/// `3 - trace` is `4 sin²(θ/2)` and the square root of it is `2 sin(θ/2)` —
/// which is `θ` to within a part in ten million over a sim step, and is what is
/// taken rather than an `acos` that would be ill-conditioned exactly where
/// every sample of a straight flight sits. Negatives are folded away: two
/// bases that differ by nothing can still put the trace a hair over three.
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
        // A screensaver is left up for days. The ring is allocated once, and
        // what it holds stops growing the moment it is full.
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
        // an answer to something that cannot happen. The exposure settles at
        // `TRAIL_SECONDS` of flight and can transiently run ahead of the
        // distance covered in the last `TRAIL_SECONDS` — the drive snaps 3.4 c
        // on and the ease climbs to 2000 c behind it, so `3·v` outruns the
        // integral of `v` for as long as `v` is still climbing. What the ring
        // has to hold is the worst of that with room over.
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
    fn a_turn_that_goes_out_and_comes_back_still_swept_the_arc_it_swept() {
        // Total variation and not the angle between the two ends, which is what
        // a weave makes expensive to get wrong. Yaw one way, then the other, so
        // the nose finishes where it started: the net rotation is nearly
        // nothing and the arc the sky swept is the sum of the two legs. A curve
        // chosen off the net angle would be drawn as a chord straight across it.
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
        assert_eq!(track.straight_run(), 0.0);
        assert_eq!(track.pose_at(5.0), ([0.0; 3], LEVEL_AXES));
    }
}
