//! The hand on the stick when there is nobody at it.
//!
//! Both `--demo` and `--screensaver` fly this way, and a screensaver is left
//! running for days, so the cycle has to keep coming round for all of them —
//! hence the `f64` clock, and the tests that start one a week in.
//!
//! Two things are asked of everything in here and both are easier to get wrong
//! than they look.
//!
//! **The same flight at any frame rate.** Every value below is a closed form of
//! the clock rather than something accumulated a frame at a time, and the one
//! place that cannot be — the stick, which is impulse-driven because a terminal
//! reports presses and not releases — is scaled by `dt` so that it is. The
//! throttle used to come back down by a fixed step per *frame*, which meant the
//! eight-second drop-out shed 1.92 of throttle at 60 fps, 3.84 at 120, and 0.32
//! at `--fps 10`, where it never reached its floor at all.
//!
//! **A flight that does not repeat itself.** A screensaver is watched for
//! minutes at a time, and a schedule that comes round unchanged every
//! forty-six seconds is read as a loop however pretty each pass is. The cycle's
//! skeleton is fixed and its *content* is not: the throttle the run-up climbs
//! to, how hard the weave leans on the stick, and where the camera is watching
//! from are each carried by a wave on its own period, and no two of those
//! periods — nor any of them and [`Autopilot::CYCLE`] — share a factor.

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
/// 0.30 settles at about 495 c — warp 6.4 against the warp 3.98 they ask for,
/// which is a fivefold margin. The top of the swing is full throttle, warp 9.8,
/// so consecutive cycles are plainly different flights rather than the same one
/// with a wobble in it.
///
/// The pair was 0.78 and 0.22, and it had to move when the throttle became a
/// warp factor rather than a speed. What this wants is a spread in *velocity* —
/// that is what a watcher sees — and the old numbers bought a fourfold one
/// because the stick was exponential in it. Against a factor the same 0.22 of
/// travel spans 1369 c to 2000 c, half a warp factor, and consecutive cycles
/// peaked within a few percent of each other: a screensaver reading as a loop,
/// which is the one thing this constant exists to prevent. These reach the same
/// 495 c at the bottom and so the same fourfold spread.
const PEAK_FLOOR: f32 = 0.30;
const PEAK_SWING: f32 = 0.70;
const PEAK_PERIOD: f64 = 83.0;

/// How hard the weave leans on the stick, in keypresses a second.
///
/// A keypress a second rather than a keypress a frame, and that is the fix
/// rather than a change of units. [`Ship::nudge_yaw`] adds an impulse and
/// [`Ship::update`] damps the rate it lands on, so in steady state a per-frame
/// impulse `a` against a decay of `exp(-k·dt)` sweeps exactly `a/k` of angle
/// per frame whatever `dt` is — and an impulse that does *not* scale with `dt`
/// therefore sweeps an angle proportional to the frame rate. It did, exactly:
/// forty seconds of weaving swept a sixth of its 60 fps arc at `--fps 10` and
/// eight and a third times it at the five hundred frames a second an uncapped
/// burst of input reaches, where the lean it holds is nearly six degrees
/// against three quarters of one. Scaled by `dt` the same forty seconds sweep
/// 36.75° of arc at every rate from 10 to 500, agreeing to five figures.
///
/// Sixty times the per-frame figures they replace, so the flight at 60 fps is
/// the one that was there before. What is left is the fixed `SIM_STEP` the
/// heading is summed over, which does not vary with the frame rate either.
const WEAVE_YAW: f32 = 0.18;
const WEAVE_PITCH: f32 = 0.12;

/// How long the weave takes to come round on each axis. Two periods rather than
/// one so the two never quite line up, and neither divides the cycle.
const WEAVE_YAW_PERIOD: f64 = 20.0;
const WEAVE_PITCH_PERIOD: f64 = 33.0;

/// How narrow the weave is allowed to get, as a fraction of its full width, and
/// how long that takes to come round.
///
/// It narrows and widens rather than holding one amplitude, which is half of
/// what keeps a long watch from reading as a loop. It never goes *above* full
/// width, and that ceiling is load-bearing: `models::a_ship_nobody_is_flying_
/// never_crosses_the_swap` measures how far the lean this produces carries the
/// drive toward the side it is drawn on, and full width sits about a ninth of
/// the way over against a bound of a quarter.
const WEAVE_FLOOR: f32 = 0.30;
const WEAVE_PERIOD: f64 = 149.0;

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
    /// has a period that shares no factor with it — 29, 37, 43, 71 and 149 are
    /// prime and 46 is 2·23 — so the schedule as a whole comes round on their
    /// product, which is some seven hundred years. That is down from twenty
    /// thousand, because speeding the camera up shortened three of the five;
    /// what the figure has to be is longer than a machine stays up, and it is.
    pub const CYCLE: f64 = 46.0;

    /// Fly the ship for one frame.
    ///
    /// `elapsed` is `f64` for the same reason `Flight::time` is: a screensaver
    /// runs for days, and this has to keep cycling for all of them. `dt` is
    /// what the stick is scaled by, and it is the *frame's* step rather than
    /// the simulation's — the stick is pressed once a frame, and it is a frame
    /// the impulse has to be worth the same amount of turn over.
    pub fn update(&mut self, ship: &mut Ship, elapsed: f64, dt: f32) {
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

        // The weave, and it runs the whole cycle rather than only the warp leg.
        // It used to sit inside the arm above, which left the run-up, the
        // drop-out and the coast — twenty of the forty-six seconds — with every
        // rate decaying to nothing and the sky perfectly still. That is the
        // sublight half, where there are no streaks to carry the eye either, so
        // it was the half that most needed something moving in it.
        let width = WEAVE_FLOOR + (1.0 - WEAVE_FLOOR) * up(wave(elapsed, WEAVE_PERIOD));
        ship.nudge_yaw(WEAVE_YAW * width * wave(elapsed, WEAVE_YAW_PERIOD) * dt);
        ship.nudge_pitch(WEAVE_PITCH * width * wave(elapsed, WEAVE_PITCH_PERIOD) * dt);
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

    /// Fly the autopilot at `fps` for `seconds` and report what it did: the
    /// heading it swept, the peak velocity it reached, and where it left the
    /// ship.
    ///
    /// The stick is worked once a *frame* and the ship is stepped on a fixed
    /// accumulator, which is `Flight::advance` and is the whole point. Stepping
    /// the ship at the frame rate instead measures something else and looks
    /// like a fault: `Ship::update` gains heading by a right-Riemann sum, so a
    /// tenth-of-a-second step under-counts the turn by 12% where a
    /// five-hundredth under-counts it by 0.3% — a fact about `Ship` at a step
    /// nothing ever calls it at, arriving here as a frame-rate dependence the
    /// autopilot does not have.
    fn fly(fps: f64, seconds: f64, start: f64) -> (f32, f32, Ship) {
        let dt = 1.0 / fps;
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let (mut swept, mut peak, mut acc) = (0.0f32, 0.0f32, 0.0f32);
        for frame in 0..(seconds * fps) as usize {
            autopilot.update(&mut ship, start + frame as f64 * dt, dt as f32);
            acc += dt as f32;
            while acc >= SIM_STEP {
                let before = ship.heading;
                ship.update(SIM_STEP);
                swept += crate::ship::wrap_signed(ship.heading - before).abs();
                acc -= SIM_STEP;
            }
            peak = peak.max(ship.velocity_c());
        }
        (swept, peak, ship)
    }

    #[test]
    fn the_autopilot_completes_a_cycle_without_getting_stuck() {
        let dt = 1.0 / 60.0;
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let mut peak: f32 = 0.0;
        let mut engaged_at_some_point = false;

        for frame in 0..(Autopilot::CYCLE / dt as f64) as usize {
            autopilot.update(&mut ship, frame as f64 * dt as f64, dt);
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
                autopilot.update(&mut ship, start + frame as f64 * dt, dt as f32);
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
    fn the_weave_sweeps_the_same_sky_whatever_the_frame_rate() {
        // The reported fault, and the reason for every `dt` in this module. The
        // stick is impulse-driven, so an impulse applied once a frame and
        // damped over the frame sweeps an angle proportional to the frame rate
        // unless it is scaled by the frame. It was exactly that: the same forty
        // seconds swept a sixth of its 60 fps arc at ten frames a second and
        // eight and a third times it at five hundred, which is one autopilot
        // flying a fiftyfold different turn.
        //
        // Five hundred is not a display rate. It is what the interactive loop
        // reaches when the frame cap is abandoned to answer a key — a stuck key
        // or a pasted burst — and it is the end of the range where the lean got
        // large enough to matter, which is the half of this that
        // `models::a_ship_nobody_is_flying_never_crosses_the_swap` measures.
        //
        // The arc rather than the net turn, because the weave comes back: a
        // signed sum could be small because nothing moved or because it moved
        // both ways.
        let swept: Vec<f32> = [10.0, 30.0, 60.0, 120.0, 500.0]
            .into_iter()
            .map(|fps| fly(fps, 40.0, 0.0).0.to_degrees())
            .collect();
        let lo = swept.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = swept.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            hi < lo * 1.01,
            "the same forty seconds of weaving sweeps {swept:?} degrees of \
             heading at ten to five hundred frames a second"
        );
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
                    autopilot.update(&mut ship, elapsed, dt as f32);
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
    fn the_stick_is_never_quite_still() {
        // What the module doc has always claimed and what was true for only
        // twenty-six of the forty-six seconds: the weave lived inside the warp
        // leg, so the run-up, the drop-out and the coast left every rate
        // decaying to zero and the sky dead.
        let dt = 1.0 / 60.0;
        let mut ship = Ship::new();
        let mut autopilot = Autopilot::default();
        let (mut still, mut worst) = (0, 0);
        for frame in 0..(2.0 * Autopilot::CYCLE / dt as f64) as usize {
            autopilot.update(&mut ship, frame as f64 * dt as f64, dt);
            ship.update(dt);
            if ship.yaw_rate.abs() < 1e-5 && ship.pitch_rate.abs() < 1e-5 {
                still += 1;
                worst = worst.max(still);
            } else {
                still = 0;
            }
        }
        assert!(
            worst < 60,
            "the sky holds perfectly still for {worst} frames at a stretch"
        );
    }

    #[test]
    fn no_two_cycles_are_flown_the_same_way() {
        // A screensaver is watched for minutes, and a schedule that comes round
        // unchanged is read as a loop however pretty one pass is.
        //
        // Asked as two questions rather than as one, and the change is a
        // consequence of the throttle becoming a warp factor. It used to demand
        // that all fifteen pairs of six cycles differ by five percent in peak
        // velocity, and that was reachable only because the stick was
        // exponential in velocity: a hand's width of throttle at the top was
        // worth a factor of two, so any two samples of the wave came out far
        // apart. Against a power law the top of the range is flat — and worse,
        // the peak is the *most* the throttle reaches during a leg, so every
        // cycle whose leg contains the wave's crest peaks at full warp. No
        // period makes six such samples pairwise distinct, and tuning for one
        // would be fitting the constant to the test.
        //
        // What the test is actually for survives both facts. A schedule reads
        // as a loop when the same flight comes round again, so: consecutive
        // cycles must differ, and the set of them must cover a real range.
        let peaks: Vec<f32> = (0..6)
            .map(|cycle| fly(60.0, Autopilot::CYCLE, cycle as f64 * Autopilot::CYCLE).1)
            .collect();
        for pair in peaks.windows(2) {
            assert!(
                (pair[0] - pair[1]).abs() > 0.05 * pair[0].max(pair[1]),
                "two cycles running peaked at about the same speed: {peaks:?}"
            );
        }
        let (lo, hi) = peaks
            .iter()
            .fold((f32::MAX, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        assert!(
            hi / lo > 2.0,
            "every cycle peaked at much the same speed: {peaks:?}"
        );
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
                worst = worst.min(fly(60.0, Autopilot::CYCLE, at).1);
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
