//! What a flight does when there is nobody flying it.
//!
//! Not a hand on the stick, which is what this used to be and what the name
//! still suggests: it works the throttle, lights and shuts down the drive, and
//! walks the camera round the hull. The stick it leaves alone.
//!
//! Both `--demo` and `--screensaver` fly this way, and a screensaver is left
//! running for days, so the cycle has to keep coming round for all of them —
//! hence the `f64` clock, and the tests that start one a week in.
//!
//! Three things are asked of everything in here and all of them are easier to
//! get wrong than they look.
//!
//! **The same flight at any frame rate.** Every value below is a closed form of
//! the clock rather than something accumulated a frame at a time, and there is
//! no exception left. There was one — the stick, which is impulse-driven
//! because a terminal reports presses and not releases — and nothing here
//! touches the stick now. So the property is structural rather than earned:
//! this module is handed no step and could not read one, and a flight sampled
//! at ten frames a second and the same flight sampled at five hundred are two
//! readings of one function of the clock.
//!
//! Both halves of how that used to be got wrong are worth keeping, because
//! neither fault needs a weave to happen again. The throttle used to come back
//! down by a fixed step per *frame*, so the eight-second drop-out shed 1.92 of
//! throttle at 60 fps, 3.84 at 120, and 0.32 at `--fps 10`, where it never
//! reached its floor at all. And the identity that governs anything
//! impulse-driven, which nothing in this module has a use for any more and
//! which [`crate::app::Flight::nudge_stick`] still points at: [`Ship::nudge_yaw`]
//! adds an impulse and [`Ship::update`] damps the rate it lands on, so a
//! per-frame impulse `a` against a decay of `exp(-k·dt)` sweeps exactly `a/k`
//! of angle per *frame* whatever `dt` is — which makes an unscaled impulse
//! sweep an angle proportional to the frame rate, and an impulse proportional
//! to `dt` sweep a fixed angle per second. The weave was the first of those
//! and then the second, and forty seconds of it swept a sixth of its 60 fps arc
//! at ten frames a second before the `dt` went in.
//!
//! **A ship nobody is flying holds its nose still.** It used to weave, on the
//! argument that the sublight half of the cycle has no streaks to carry the eye
//! and so needed something moving in it. That was the wrong place to find the
//! motion. The ship carries the attitude the whole sky is projected through, so
//! a wander nobody asked for swung the field from both cameras, leaned the hull
//! into itself, and put a curve in the exposure of every near star — a picture
//! drifting off its heading rather than a ship flying somewhere. What carries
//! the eye instead is what a viewer can tell they are being shown: the camera,
//! which walks the whole way round the hull, and the drive, which lights and
//! shuts down twice a minute. The nose is left exactly where the flight opened
//! on it, to the bit, and
//! `a_ship_nobody_is_flying_is_pointed_exactly_where_it_started` says so.
//!
//! What that costs is worth having measured rather than argued, because the
//! weave's own case was about exactly this and it was not wrong about the
//! symptom. Over a sublight second of `--demo` on a 90x26 terminal, 5 cells of
//! 2340 change from frame to frame — the twinkle, and nothing else. The same
//! second from `--view side` changes 266, because the camera is walking; the
//! same second at warp changes 481. So the stillness is real and it lands on
//! the *cockpit* at sublight, which is twenty of the forty-six seconds and no
//! other part of the cycle. That is the trade taken deliberately: a picture
//! that holds still is a ship flying somewhere, and a picture that wanders is a
//! ship that cannot hold a heading. Widening the weave was the answer to it
//! once, and the answer now is that a sky whose whole point is to hold still at
//! impulse should be allowed to.
//!
//! **A flight that does not repeat itself.** A screensaver is watched for
//! minutes at a time, and a schedule that comes round unchanged every
//! forty-six seconds is read as a loop however pretty each pass is. The cycle's
//! skeleton is fixed and its *content* is not: the throttle the run-up climbs
//! to and where the camera is watching from are each carried by a wave on its
//! own period, and no two of those periods — nor any of them and
//! [`Autopilot::CYCLE`] — share a factor.

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
///
/// Per *second*, and that is the whole of the fix: these were 0.004 and 0.002
/// per frame, which is the same thing only at 60 fps. The figures here are
/// those times sixty, so the flight at the rate the reference was recorded at
/// is the flight that was there before.
const DROPOUT_RATE: f32 = 0.24;
const DROPOUT_FLOOR: f32 = 0.25;
const COAST_RATE: f32 = 0.12;
const COAST_FLOOR: f32 = 0.15;

/// The lowest peak throttle a cycle is allowed to climb to, and how far above
/// it the wave can carry one.
///
/// The floor is not taste: three tests require every cycle to reach 100 c, and
/// 0.78 settles at about 490 c — warp 6.4 against the warp 3.98 they ask for,
/// which is a fivefold margin. The top of the swing is full throttle, warp 9.8,
/// so consecutive cycles are plainly different flights rather than the same one
/// with a wobble in it.
const PEAK_FLOOR: f32 = 0.78;
const PEAK_SWING: f32 = 0.22;
const PEAK_PERIOD: f64 = 71.0;

/// How long the camera takes to walk all the way round the ship, in seconds.
///
/// It walks rather than wanders, because the whole subject of the view from
/// outside is the ship and there is no one angle that is the ship. A turn every
/// three quarters of a minute is about eight and a half degrees a second — fast
/// enough that the shot has plainly moved over the span anyone watches it for,
/// and still under a twentieth of what a held camera key sweeps, which is what
/// keeps it a shot rather than a spin.
///
/// It was 137 seconds, and that was too slow to read as motion at all: a
/// screensaver got a third of the way round the hull per run to warp, so the
/// shot looked parked unless you left it and came back. Deliberately *not* 47,
/// which would sit one second off the cycle and slip by only eight degrees a
/// pass — very nearly the same shot every time round, which is the one thing
/// this whole set of periods is chosen to avoid.
const CAMERA_TURN: f64 = 43.0;

/// How far the camera lifts over the hull and drops under it, in radians, and
/// how long that takes.
///
/// It crosses zero rather than sitting off it, so the camera passes *through*
/// the beam — which is where every exact identity in the view from outside
/// lives — instead of parking on one side of it.
///
/// The period is kept in proportion to [`CAMERA_TURN`] rather than tuned on its
/// own: all three axes were sped up by about three together, because an orbit
/// walking briskly against a lift still taking a minute and a half reads as one
/// axis running away from the others rather than as a camera being flown. The
/// *amplitude* is untouched — the camera covers the same ground, sooner.
const CAMERA_LIFT: f32 = 0.55;
const CAMERA_LIFT_PERIOD: f64 = 29.0;

/// How much closer and further off the camera drifts, as a factor either way,
/// and how long that takes.
///
/// Geometric, because every other zoom in the tree is: `view::ZOOM_STEP` is a
/// multiplier and `Flight::nudge_zoom` divides it out again. A factor of 1.4
/// spans 0.71 to 1.39 of the default, which is comfortably inside the stops
/// the zoom is held to — and it is clamped to them anyway, since a constant
/// that drifts out of range should cost a framing rather than a panic.
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
    ///
    /// The skeleton is fixed even though the flight is not. Every wave above
    /// has a period that shares no factor with it — 29, 37, 43 and 71 are prime
    /// and 46 is 2·23 — so the schedule as a whole comes round on their
    /// product, 150 689 974 seconds, which is a little under four and three
    /// quarter years.
    ///
    /// That is down from seven hundred years, and the arithmetic is worth
    /// having written down because the figure moved without anybody aiming at
    /// it: the 149 in that product was the weave's envelope, and it went with
    /// the weave. What the figure has to be is longer than a machine stays up,
    /// and four and a half years still is by a wide margin. If it ever wants
    /// lengthening again the lever is another period rather than a longer one —
    /// a fifth coprime wave multiplies, where stretching one of these four only
    /// scales.
    pub const CYCLE: f64 = 46.0;

    /// Fly the ship for one frame: the throttle, and the drive.
    ///
    /// `elapsed` is `f64` for the same reason `Flight::time` is: a screensaver
    /// runs for days, and this has to keep cycling for all of them.
    ///
    /// There is deliberately no step to hand it, and the absence is the
    /// property rather than an omission. Every arm below is a function of that
    /// clock alone, so this cannot be made to fly a different flight by being
    /// called more often — where a `dt` in the signature is an invitation to
    /// accumulate something, and the last thing that did was the stick.
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
            // Only the drive is switched here. The throttle used to be set to
            // its entry value alongside, which the arm below then wrote again
            // with the same number — the ramp starts from exactly that.
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
        // find. The last two used to walk it down a step a frame, which is a
        // different flight at every frame rate and no flight at all at ten.
        let peak = peak_throttle(elapsed);
        ship.throttle = match phase {
            0 => IDLE_THROTTLE + t * RUNUP_RATE,
            1 => (ENTRY_THROTTLE + (t - LIGHT_AT) * CLIMB_RATE).min(peak),
            2 => (peak - DROPOUT_RATE * (t - DROP_AT)).max(DROPOUT_FLOOR),
            _ => (DROPOUT_FLOOR - COAST_RATE * (t - COAST_AT)).max(COAST_FLOOR),
        };

        // And nothing touches the stick. A weave used to run the whole cycle
        // here, put in because the run-up, the drop-out and the coast — twenty
        // of the forty-six seconds — are sublight, have no streaks to carry the
        // eye, and were perfectly still without it. The motion was real and it
        // was found in the wrong place: the sky is projected through the ship's
        // own attitude, so a wander nobody asked for swings the whole field
        // from both cameras and curves the exposure of every near star. What is
        // left moving is what a viewer can tell they are being shown — the
        // camera walking round the hull, the drive lighting and shutting down —
        // and the nose stays exactly where the flight opened on it.
    }

    /// Where the camera should be watching from, as an offset from wherever
    /// `--orbit` parked it, and what to multiply the zoom by.
    ///
    /// A closed form of the clock and nothing else — no state, no `Flight`, and
    /// nothing accumulated — so it is frame-rate independent by construction,
    /// a reference flight reproduces it exactly, and it can be asked what it
    /// intends without flying anything.
    ///
    /// An *offset* rather than an answer, because `--orbit` is a starting angle
    /// the way `--throttle` is a starting throttle and an autopilot that
    /// overwrote it would make the flag mean nothing. At `elapsed` of zero all
    /// three angles are exactly zero and the factor is exactly one, so the
    /// frame a flight opens on is the one it always opened on.
    pub fn camera(&self, elapsed: f64) -> (Orbit, f32) {
        (
            Orbit {
                azimuth: ramp(elapsed, CAMERA_TURN),
                elevation: CAMERA_LIFT * wave(elapsed, CAMERA_LIFT_PERIOD),
                // Left alone deliberately. Roll turns the hull and the sky
                // together, and a picture that slowly rotates under a viewer
                // who did not ask for it reads as a fault rather than as a
                // shot — where a camera walking round a ship reads as a camera
                // walking round a ship.
                roll: 0.0,
            },
            CAMERA_ZOOM_SWING.powf(wave(elapsed, CAMERA_ZOOM_PERIOD)),
        )
    }
}

/// The throttle this moment's cycle is climbing toward.
///
/// Sampled at the clock rather than latched at the top of a cycle, so it is a
/// function of `elapsed` like everything else here and there is no state to
/// come back wrong after a pause.
fn peak_throttle(elapsed: f64) -> f32 {
    PEAK_FLOOR + PEAK_SWING * up(wave(elapsed, PEAK_PERIOD))
}

/// A wave from `-1` to `1` that comes round once every `period` seconds.
///
/// The clock is folded onto one period *before* the angle is worked out. For a
/// wave that is only tidiness — `sin` reduces its own argument exactly, and the
/// existing test that starts a flight ten million seconds in says so — but
/// [`ramp`] below shares the fold and genuinely needs it, and two spellings of
/// one idea is how the two drift apart.
fn wave(elapsed: f64, period: f64) -> f32 {
    ((elapsed % period) * (TAU / period)).sin() as f32
}

/// The same fold, handed back as the angle itself: `0` to `TAU` once every
/// `period` seconds.
///
/// This one has to fold. It is a ramp rather than a wave, so unfolded it would
/// be over a million radians by the time a screensaver had been up a season —
/// and as an `f32` at that magnitude consecutive frames round to the same
/// angle, so a camera that is supposed to be gliding would stutter instead.
/// Note which way speeding the camera up moved that: a shorter period is *more*
/// radians a season, not fewer, so the fold matters more than it did.
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
    ///
    /// The ship is stepped on a fixed accumulator rather than at the frame
    /// rate, which is what `Flight::advance` does and is the whole point of
    /// flying it this way here. It mattered more when this returned a swept
    /// heading — `Ship::update` gains heading by a right-Riemann sum, so a
    /// tenth-of-a-second step under-counts a turn by 12% where a five-hundredth
    /// under-counts it by 0.3%, a fact about `Ship` at a step nothing calls it
    /// at that arrived here looking like a frame-rate dependence. Nothing turns
    /// now, so that trap is asleep rather than gone; the accumulator stays
    /// because a test of the autopilot should fly the loop the autopilot flies.
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
        // can be. This module used to lean on the stick every frame — a lazy
        // wander on the yaw and the pitch — and the sky is projected through
        // `Ship::axes`, so what that produced was the whole field swinging
        // under a viewer who had asked for a flight rather than a drift.
        //
        // Exactly, not nearly, and that is the useful half. An attitude held to
        // the bit is what lets ten reference hashes say that a change to the
        // steering path really was the identity it looked like, and it is what
        // makes every `--demo` flight in `tests/golden.rs` a control for that
        // path rather than a contrast against it.
        //
        // Three frame rates, kept from the test this replaces. Five hundred is
        // not a display rate: it is what the interactive loop reaches when the
        // frame cap is abandoned to answer a key, and it is where an impulse
        // that had crept back in unscaled would show first and worst.
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

        // Asked of the spread rather than of a number: the throttle the
        // drop-out starts from wanders from cycle to cycle, so where the floor
        // is reached is a fact about *this* cycle. What may not vary is the
        // frame rate it was flown at, and the coarsest of the three can only
        // notice the crossing to within its own tenth of a second.
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
        // The exact inverse of the test that stood here, and the inversion was
        // the point rather than a casualty. That one asserted the sky is never
        // still for a whole second at a stretch, because a weave that lived
        // inside the warp leg left the run-up, the drop-out and the coast —
        // twenty of the forty-six seconds — with every rate decaying to nothing.
        //
        // Widening the weave to the whole cycle was the answer then. Taking it
        // out is the answer now: a flight nobody is flying holds the heading it
        // was given, and what carries the eye is the camera and the drive.
        // Stated over two cycles a frame at a time rather than at the end, so a
        // rate that was nudged and damped back to nothing between two samples
        // cannot slip past.
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
        // is doing real work. Held well clear of the 100 c three other tests
        // ask for: what has to be true is that the drive is plainly lit every
        // pass, not that it scrapes past a bound.
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
