//! Flight model: throttle, the warp drive, steering, and the transient effects
//! that make a change of speed feel like something actually happened.
//!
//! Terminals report key *presses*, and all but a few report nothing when a key
//! comes back up, so every control here is impulse-driven: a keypress nudges a
//! value and the value decays back on its own. Holding a key down produces
//! auto-repeat, which reads as sustained input without needing a key-up event.
//!
//! "All but a few" rather than "none": terminals speaking the kitty protocol do
//! send releases, and `app::handle_key` discards them, because acting on one
//! counts a single press twice. Which is the same design either way — a model
//! that needs a key-up would be wrong on every other terminal.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// Top sublight speed, in world units per second.
pub const CRUISE_MAX: f32 = 42.0;
/// Top speed with the warp drive engaged.
pub const WARP_MAX: f32 = 780.0;
/// Velocity, in multiples of c, at `CRUISE_MAX`.
const CRUISE_MAX_C: f32 = 0.9;
/// Velocity, in multiples of c, at `WARP_MAX`.
const WARP_MAX_C: f32 = 2000.0;

/// The warp factor full throttle reads, and the one the drive catches at.
///
/// The first is not a choice — it is what `WARP_MAX_C` already comes to on the
/// TNG scale, named here so the throttle can be shaped against it. The second
/// is: lighting the drive used to land the ship at warp 1.4, which is 3.4 c and
/// a six-hundredth of full-warp speed, so the flash and the shake fired over a
/// sky that had not started moving. Catching at warp 5 puts a tenth of the
/// full-warp rate on screen in the frame the key is pressed, which is about
/// what the old renderer's entry gave and is a great deal more honest about it.
const WARP_MAX_FACTOR: f32 = 9.779_267;
const WARP_ENTRY_FACTOR: f32 = 5.0;

/// Flight time is compressed so the sky moves: one second at the stick is ten
/// weeks underway. At full warp that is about 383 ly per second.
///
/// **This is the one number that sets how fast anything looks**, and it is set
/// against the renderer rather than against the calendar. Apparent motion is
/// speed over distance, and the distances are real now — so there is exactly
/// one scale here, and it fixes both ends of the throttle at once. Turn it up
/// and warp sweeps *and* impulse creeps; turn it down and impulse freezes *and*
/// warp dies.
///
/// It was one day, which put a typical star at 0.054 subpixels a frame at full
/// warp against the 3.64 the old renderer drew — sixty-eight times too slow,
/// and the reason warp stopped feeling like warp. Ten weeks puts it at 3.4,
/// which is where the tunnel reads as motion rather than as a still. What that
/// costs at the other end is written down rather than hidden: the median star
/// moves three subpixels in thirty seconds of *full* impulse and the nearest
/// twenty, where before they moved a twentieth of that. The sky still reads as
/// fixed — at the default throttle the median star moves half a subpixel in
/// thirty seconds — but it is no longer frozen to the bit, and nearby stars
/// having visible proper motion is the truth rather than a concession.
const TIME_COMPRESSION: f32 = 6_048_000.0;
const SECONDS_PER_YEAR: f32 = 31_557_600.0;

/// How far one multiple of c carries the ship in a second at the stick, in
/// light years.
///
/// The two constants above once belonged to the odometer alone, which meant the
/// distance the panel reported and the distance the sky was moved by were
/// separate inventions — [`Ship::speed`] is in world units and ran the stars,
/// and this ran the readout, and nothing made them agree. They are one scale
/// now: [`crate::universe`] measures its volume in light years and moves it by
/// [`Ship::velocity_ly_per_s`], so a flight that reads 3 ly on the panel really
/// has put 3 ly of sky behind it.
pub const LY_PER_C_SECOND: f32 = TIME_COMPRESSION / SECONDS_PER_YEAR;

const ACCEL_K: f32 = 1.6;
const DECEL_K: f32 = 2.4;
/// Dropping out of warp bleeds speed much harder than a normal throttle-down.
const DROPOUT_K: f32 = 6.0;

const THROTTLE_STEP: f32 = 0.07;
const YAW_IMPULSE: f32 = 0.42;
const PITCH_IMPULSE: f32 = 0.30;
/// Roll is the axis a ship turns fastest about, and a barrel roll is the whole
/// point of having it on the stick, so it gets the loosest limits of the three:
/// held down, `MAX_ROLL_RATE` comes out at about three and a half seconds for a
/// full revolution.
const ROLL_IMPULSE: f32 = 0.60;
pub const MAX_YAW_RATE: f32 = 0.85;
pub const MAX_PITCH_RATE: f32 = 0.60;
const MAX_ROLL_RATE: f32 = 1.80;
const STEER_DAMPING: f32 = 2.6;

/// How far the ship leans into a turn, in radians per rad/s of yaw.
const BANK_PER_YAW: f32 = 0.42;
/// How quickly that lean catches up with the yaw rate.
const BANK_LAG: f32 = 5.0;

const SHAKE_DECAY: f32 = 3.4;
const FLASH_DECAY: f32 = 5.5;
/// How far the pitch axis can be pushed before it stops, in radians.
const PITCH_LIMIT: f32 = FRAC_PI_2 * 0.85;

/// The attitude a ship launches at: starboard along `+x`, down along `+y`, the
/// nose along `+z`. Every entry is an exact zero or an exact one, so a flight
/// that never touches the stick projects the sky through a transform that is
/// the identity to the bit.
const LEVEL_AXES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The player's ship: everything about how it is currently moving.
#[derive(Debug, Clone)]
pub struct Ship {
    /// Pilot's throttle setting, 0..=1.
    pub throttle: f32,
    /// Actual speed in world units per second, easing toward the throttle.
    pub speed: f32,
    /// Whether the warp drive is lit.
    pub warp_engaged: bool,
    /// Angular rates in rad/s, decaying toward zero. Positive is starboard,
    /// nose-down, and right-wing-down respectively.
    pub yaw_rate: f32,
    pub pitch_rate: f32,
    pub roll_rate: f32,
    /// Accumulated attitude, for the instrument panel.
    pub heading: f32,
    pub pitch: f32,
    /// Roll the pilot has flown the ship to, in radians, folded into
    /// `[-PI, PI)`. Positive is a roll to starboard. Unlike heading and pitch
    /// it has nothing to level against — there is no horizon out here — so it
    /// stays wherever it is put until it is rolled back or the ship is reset.
    pub roll: f32,
    /// Bank angle, lagging behind the yaw rate so turns feel like turns. This
    /// is the lean into a turn and nothing else: it centres itself, and is kept
    /// apart from `roll` so the two never fight over one accumulator.
    pub bank: f32,
    /// Decaying 0..=1 transients driven by engaging/disengaging warp.
    pub shake: f32,
    pub flash: f32,
    /// Odometer, in light years.
    pub distance_ly: f64,
    /// The hull's own axes in the inertial world frame: starboard, down, and
    /// the nose, in that order and in the ship's own `+z` out the nose, `+x`
    /// to starboard, `+y` down convention.
    ///
    /// The attitude lives here rather than in the sky because there is only
    /// one sky now and two cameras looking at it, so a rotation applied to the
    /// stars would have to be undone for whichever camera did not want it.
    /// Stepped by exactly the arithmetic the cockpit's field used to apply to
    /// every star — see [`Ship::steer`] — so the view from the seat is the one
    /// it always was.
    ///
    /// Note what this is *not*: [`Self::heading`], [`Self::pitch`] and
    /// [`Self::roll`] stay where they are and go on being what they were, which
    /// is instrument readings. A compass is not an attitude, and deriving one
    /// from the other is the bug the sky already shipped twice.
    pub axes: [[f32; 3]; 3],
    /// Where the ship is, in light years, in that same inertial frame.
    ///
    /// `f64`, and not as a matter of taste. At full impulse the ship covers
    /// 2.1e-5 ly in a sim step against stars a thousand light years out, which
    /// is a relative change of 2e-8 where `f32` resolves 1.2e-7 — so a position
    /// carried on the stars themselves and decremented in place would round the
    /// whole far sky to a standstill permanently rather than merely slowly.
    /// Holding the stars still and moving the ship puts the small number in the
    /// accumulator where it belongs, and the subtraction is done here before
    /// anything is handed to the renderer.
    pub position: [f64; 3],
    /// Set while the drive is spinning down after a disengage.
    dropping_out: bool,
}

impl Default for Ship {
    fn default() -> Self {
        Self::new()
    }
}

impl Ship {
    pub fn new() -> Self {
        Self {
            throttle: 0.18,
            speed: 0.0,
            warp_engaged: false,
            yaw_rate: 0.0,
            pitch_rate: 0.0,
            roll_rate: 0.0,
            heading: 0.0,
            pitch: 0.0,
            roll: 0.0,
            bank: 0.0,
            shake: 0.0,
            flash: 0.0,
            distance_ly: 0.0,
            axes: LEVEL_AXES,
            position: [0.0; 3],
            dropping_out: false,
        }
    }

    /// Back to the launch state, keeping nothing but the ship itself.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn nudge_throttle(&mut self, dir: f32) {
        self.throttle = (self.throttle + dir * THROTTLE_STEP).clamp(0.0, 1.0);
    }

    pub fn nudge_yaw(&mut self, dir: f32) {
        self.yaw_rate = (self.yaw_rate + dir * YAW_IMPULSE).clamp(-MAX_YAW_RATE, MAX_YAW_RATE);
    }

    pub fn nudge_pitch(&mut self, dir: f32) {
        self.pitch_rate =
            (self.pitch_rate + dir * PITCH_IMPULSE).clamp(-MAX_PITCH_RATE, MAX_PITCH_RATE);
    }

    /// Roll about the nose. Positive drops the starboard wing, the same way a
    /// right-hand turn leans.
    pub fn nudge_roll(&mut self, dir: f32) {
        self.roll_rate = (self.roll_rate + dir * ROLL_IMPULSE).clamp(-MAX_ROLL_RATE, MAX_ROLL_RATE);
    }

    /// Light the drive or shut it down. Returns the new state.
    pub fn toggle_warp(&mut self) -> bool {
        self.warp_engaged = !self.warp_engaged;
        if self.warp_engaged {
            self.dropping_out = false;
            self.speed = self.speed.max(speed_for_warp_factor(WARP_ENTRY_FACTOR));
            self.shake = 1.0;
            self.flash = 1.0;
        } else {
            self.dropping_out = true;
            self.shake = 0.55;
        }
        self.warp_engaged
    }

    /// True while the drive is spinning down after a disengage.
    ///
    /// The flight model needs nobody to know this — the steeper bleed off
    /// `speed` is the whole of its effect in here. What wants it is the trail
    /// out of the bells, which should visibly quit when the drive is switched
    /// off rather than fade out with the speed: seen from outside, a dropout
    /// and a hand easing the throttle back are the same falling number, and
    /// the flag is the only thing that tells them apart.
    pub fn dropping_out(&self) -> bool {
        self.dropping_out
    }

    /// Where the throttle is asking the ship to end up.
    ///
    /// Above light the stick picks a **warp factor**, not a speed, and that is
    /// the whole of the difference. It used to be linear in [`Self::speed`],
    /// which `speed_to_c` then put through three orders of magnitude — so the
    /// bottom sixty percent of the throttle bought the bottom four percent of
    /// the sky's motion and the stick was dead for most of its travel. A factor
    /// is what a pilot asks for anyway ("go to warp five"), and each notch is
    /// then a proportionate change rather than a rounding error at one end and
    /// a leap at the other.
    fn target_speed(&self) -> f32 {
        if self.warp_engaged {
            let factor = WARP_ENTRY_FACTOR + (WARP_MAX_FACTOR - WARP_ENTRY_FACTOR) * self.throttle;
            speed_for_warp_factor(factor)
        } else {
            CRUISE_MAX * self.throttle
        }
    }

    pub fn update(&mut self, dt: f32) {
        let target = self.target_speed();
        let k = if target >= self.speed {
            ACCEL_K
        } else if self.dropping_out {
            DROPOUT_K
        } else {
            DECEL_K
        };
        self.speed += (target - self.speed) * (1.0 - (-k * dt).exp());
        // The hard dropout curve only applies until the ship is sublight again.
        if self.dropping_out && self.speed <= CRUISE_MAX * 1.02 {
            self.dropping_out = false;
        }

        let damp = (-STEER_DAMPING * dt).exp();
        self.yaw_rate *= damp;
        self.pitch_rate *= damp;
        self.roll_rate *= damp;

        self.heading = (self.heading + self.yaw_rate * dt).rem_euclid(TAU);
        self.pitch = (self.pitch + self.pitch_rate * dt).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        // Pitch stops short of straight up because there is no way back over
        // the top; roll has no such limit, because going all the way round is
        // the point of it.
        self.roll = wrap_signed(self.roll + self.roll_rate * dt);

        // Bank chases the yaw rate rather than tracking it exactly, so rolling
        // into and out of a turn has some lag to it.
        let target_bank = -self.yaw_rate * BANK_PER_YAW;
        self.bank += (target_bank - self.bank) * (1.0 - (-BANK_LAG * dt).exp());

        self.shake *= (-SHAKE_DECAY * dt).exp();
        self.flash *= (-FLASH_DECAY * dt).exp();

        self.steer(dt);
        self.coast(dt);

        self.distance_ly += (self.velocity_c() * dt * TIME_COMPRESSION / SECONDS_PER_YEAR) as f64;
    }

    /// Turn the hull by this step's rates.
    ///
    /// This is the rotation the cockpit star field used to apply to
    /// every star, moved off the sky and onto the ship and otherwise untouched
    /// — same three axes, same order, same sines. It has to be the same one:
    /// the sky used to turn the opposite way from the ship, so a star was
    /// rotated by `S` while the hull it is seen from was implicitly rotated by
    /// `Sᵀ`, and a star's place in the seat is `Aᵀ(w − P)`. Substituting the
    /// axes for the coordinates gives `A ← A·Sᵀ`, which is the same three lines
    /// with a vector where each scalar was.
    ///
    /// Yaw about the vertical, pitch about the horizontal, roll about the nose,
    /// each a rotation of the ship's *own* frame — so roll ninety degrees and
    /// the pitch axis has come round to where the yaw axis was, exactly as it
    /// would in something with wings.
    fn steer(&mut self, dt: f32) {
        let (sy, cy) = (self.yaw_rate * dt).sin_cos();
        let (sp, cp) = (self.pitch_rate * dt).sin_cos();
        // Negated to match the sky's own spelling, where dropping the starboard
        // wing swung the stars anticlockwise.
        let (sr, cr) = (-self.roll_rate * dt).sin_cos();

        let [right, down, nose] = self.axes;
        let (right, nose) = (mix(right, cy, nose, -sy), mix(right, sy, nose, cy));
        let (down, nose) = (mix(down, cp, nose, -sp), mix(down, sp, nose, cp));
        let (right, down) = (mix(right, cr, down, -sr), mix(right, sr, down, cr));
        self.axes = orthonormalise([right, down, nose]);
    }

    /// Move the ship along its nose by this step.
    ///
    /// In light years, from [`Self::velocity_ly_per_s`], and accumulated in
    /// `f64` — see [`Self::position`] for why that is the whole point.
    fn coast(&mut self, dt: f32) {
        let step = (self.velocity_ly_per_s() * dt) as f64;
        for (place, nose) in self.position.iter_mut().zip(self.axes[2]) {
            *place += nose as f64 * step;
        }
    }

    /// Current velocity as a multiple of the speed of light.
    pub fn velocity_c(&self) -> f32 {
        speed_to_c(self.speed)
    }

    /// The same velocity in the units the sky is measured in.
    ///
    /// One spelling, deliberately, because this is the number that used to have
    /// two: the panel's odometer ran on the compressed clock and the stars ran
    /// on [`Self::speed`], which is 42 world units at 0.9 c and 780 at 2000 c —
    /// so the sky at impulse streamed at a twentieth of its warp rate while the
    /// dial read a two-thousandth of it, and the stars had to be parked a
    /// ship's length from the canopy for that to look like anything at all.
    pub fn velocity_ly_per_s(&self) -> f32 {
        self.velocity_c() * LY_PER_C_SECOND
    }

    /// Warp factor on the TNG scale, where v = w^(10/3) · c. Zero below light.
    pub fn warp_factor(&self) -> f32 {
        let v = self.velocity_c();
        if v >= 1.0 {
            v.powf(0.3)
        } else {
            0.0
        }
    }

    /// 0..=1 ramp across the superluminal range, used to drive the visuals.
    ///
    /// Measured on the warp *factor* rather than on [`Self::speed`], so it is
    /// exactly zero at light speed and exactly one at maximum warp — which is
    /// the statement the glare, the vignette, the bubble and the exposure all
    /// want, and a cleaner one than the ramp on an internal speed variable that
    /// stood here and began at 0.9 c.
    ///
    /// It had to move when the throttle became a factor. The drive catches at
    /// warp 5, which is 566 of `speed`'s 780, so the old spelling would have
    /// jumped to 0.71 the instant the key was pressed and left every effect
    /// with a quarter of its range to play in. This gives 0.46 at the catch and
    /// climbs the rest of the way with the stick.
    pub fn warp_intensity(&self) -> f32 {
        ((self.warp_factor() - 1.0) / (WARP_MAX_FACTOR - 1.0)).clamp(0.0, 1.0)
    }
}

/// A basis pulled back to orthonormal and right-handed, and sent home to
/// [`LEVEL_AXES`] if it cannot be.
///
/// Run every step rather than every so often, because "every so often" is a
/// second thing to get right and this costs three square roots against a sim
/// step that touches the whole sky. What it is for is the standard this tree
/// holds everything else to: a screensaver is left up for days, a step is three
/// rotations each rounding at about 1e-7, and a random walk over the ten
/// million steps in a day arrives at 3e-4 of skew — small, and it does not stop
/// growing. It is also exact where it matters most: at [`LEVEL_AXES`] every dot
/// product is a hard zero and every norm a hard one, so a flight nobody steers
/// is projected through the identity to the bit.
///
/// The nose is taken as the cross product rather than orthogonalised in its own
/// right, which is what keeps the determinant at `+1`. A basis that drifted
/// into its own mirror image would look very nearly right and invert every
/// facing test in [`crate::models`].
fn orthonormalise(a: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [right, down, _] = a;
    let Some(right) = unit(right) else {
        return LEVEL_AXES;
    };
    let along = dot(down, right);
    let down = [
        down[0] - along * right[0],
        down[1] - along * right[1],
        down[2] - along * right[2],
    ];
    let Some(down) = unit(down) else {
        return LEVEL_AXES;
    };
    let nose = [
        right[1] * down[2] - right[2] * down[1],
        right[2] * down[0] - right[0] * down[2],
        right[0] * down[1] - right[1] * down[0],
    ];
    [right, down, nose]
}

/// `u·p + v·q`, componentwise — one plane of a rotation applied to a pair of
/// axes rather than to a pair of coordinates.
///
/// Spelled with a negated multiplier rather than a subtraction at the call
/// sites above, which is the same number: `a + (-b)` is `a - b` exactly, and so
/// is `v·(-q)` against `-(v·q)`.
fn mix(u: [f32; 3], p: f32, v: [f32; 3], q: f32) -> [f32; 3] {
    [
        u[0] * p + v[0] * q,
        u[1] * p + v[1] * q,
        u[2] * p + v[2] * q,
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `v` scaled to unit length, or `None` if it has no direction to report.
fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot(v, v).sqrt();
    if !len.is_finite() || len <= f32::MIN_POSITIVE {
        return None;
    }
    // Multiplied by the reciprocal rather than divided three times, and the
    // reciprocal of an exact one is an exact one, so a basis already unit long
    // comes back bit for bit.
    let inv = 1.0 / len;
    Some([v[0] * inv, v[1] * inv, v[2] * inv])
}

/// Fold an angle into `[-PI, PI)` — the range a bank indicator reads in, and
/// the one that keeps a ship that has rolled the same way for an hour from
/// accumulating an angle too coarse to steer with.
///
/// Public because the camera outside has the same problem and no business
/// growing a second copy of the answer: [`crate::view::Orbit`] swings all the
/// way round twice over, and a held key is a held key whether it is turning a
/// ship or an eye.
pub fn wrap_signed(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

/// The speed that flies at a given warp factor — [`speed_to_c`] run backwards.
///
/// The flight model still eases [`Ship::speed`], because that is what the
/// shake, the plume, the glare and the lens all read; this is only how the
/// throttle says where it should end up. Clamped to the warp range at both
/// ends, so a factor below light or past maximum asks for a speed the rest of
/// the model already knows what to do with.
fn speed_for_warp_factor(factor: f32) -> f32 {
    let v = factor.max(1.0).powf(10.0 / 3.0);
    let t = (v / CRUISE_MAX_C).ln() / (WARP_MAX_C / CRUISE_MAX_C).ln();
    CRUISE_MAX + (WARP_MAX - CRUISE_MAX) * t.clamp(0.0, 1.0)
}

/// Maps world speed to a velocity in multiples of c: linear up to `CRUISE_MAX`,
/// then exponential, so the warp range spans three orders of magnitude without
/// a discontinuity at the light barrier.
fn speed_to_c(speed: f32) -> f32 {
    if speed <= CRUISE_MAX {
        CRUISE_MAX_C * (speed / CRUISE_MAX).max(0.0)
    } else {
        let t = ((speed - CRUISE_MAX) / (WARP_MAX - CRUISE_MAX)).clamp(0.0, 1.0);
        (CRUISE_MAX_C.ln() + t * (WARP_MAX_C.ln() - CRUISE_MAX_C.ln())).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_curve_is_monotonic_and_continuous() {
        let mut prev = -1.0;
        for i in 0..=1000 {
            let speed = WARP_MAX * i as f32 / 1000.0;
            let v = speed_to_c(speed);
            assert!(v > prev, "velocity must increase with speed at {speed}");
            prev = v;
        }
        // No jump across the crossover point.
        let below = speed_to_c(CRUISE_MAX - 0.001);
        let above = speed_to_c(CRUISE_MAX + 0.001);
        assert!((above - below).abs() < 0.01, "{below} vs {above}");
    }

    #[test]
    fn sublight_stays_sublight() {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        for _ in 0..2000 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.velocity_c() < 1.0, "got {}", ship.velocity_c());
        assert_eq!(ship.warp_factor(), 0.0);
        assert_eq!(ship.warp_intensity(), 0.0);
    }

    #[test]
    fn warp_reaches_factor_nine_at_full_throttle() {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..2000 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.warp_factor() > 9.0, "got {}", ship.warp_factor());
        assert!((ship.warp_intensity() - 1.0).abs() < 0.01);
        assert!(ship.distance_ly > 0.0);
    }

    #[test]
    fn engaging_warp_kicks_the_transients_and_they_decay() {
        let mut ship = Ship::new();
        assert!(ship.toggle_warp());
        assert_eq!(ship.shake, 1.0);
        assert_eq!(ship.flash, 1.0);
        for _ in 0..300 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.shake < 0.01 && ship.flash < 0.01);
    }

    #[test]
    fn dropping_out_of_warp_returns_to_sublight() {
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(!ship.toggle_warp());
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.velocity_c() < 1.0, "got {}", ship.velocity_c());
    }

    #[test]
    fn a_disengaged_drive_says_it_is_spinning_down() {
        // The trail out of the bells reads this to know the drive has quit
        // rather than been throttled back, so it has to be true for exactly
        // the stretch the steep bleed applies over — a flag that never came on
        // and a flag that never went off would both leave the plume looking
        // right for most of the flight.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        assert!(!ship.dropping_out(), "lighting the drive is not a dropout");
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(!ship.dropping_out(), "cruising at warp is not a dropout");

        assert!(!ship.toggle_warp());
        assert!(ship.dropping_out(), "shutting the drive down is one");
        ship.update(1.0 / 60.0);
        assert!(ship.dropping_out(), "and it lasts longer than a frame");

        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(
            !ship.dropping_out(),
            "it should let go once the ship is sublight again"
        );
    }

    #[test]
    fn steering_impulses_are_clamped_and_decay() {
        let mut ship = Ship::new();
        for _ in 0..50 {
            ship.nudge_yaw(1.0);
            ship.nudge_pitch(-1.0);
            ship.nudge_roll(1.0);
        }
        assert!(ship.yaw_rate <= MAX_YAW_RATE);
        assert!(ship.pitch_rate >= -MAX_PITCH_RATE);
        assert!(ship.roll_rate <= MAX_ROLL_RATE);
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.yaw_rate.abs() < 0.01 && ship.pitch_rate.abs() < 0.01);
        assert!(ship.roll_rate.abs() < 0.01);
        assert!(ship.pitch >= -PITCH_LIMIT);
    }

    #[test]
    fn roll_goes_all_the_way_round_and_stays_where_it_is_put() {
        let mut ship = Ship::new();
        // Held down long enough for a full revolution and then some: unlike
        // pitch there is no limit to stop against, only the wrap.
        let mut swept = 0.0f32;
        for _ in 0..600 {
            ship.nudge_roll(1.0);
            ship.update(1.0 / 60.0);
            swept += ship.roll_rate / 60.0;
        }
        assert!(
            swept > TAU,
            "a held roll should go right round: {swept} rad"
        );
        assert!(
            (-PI..PI).contains(&ship.roll),
            "roll must stay folded: {}",
            ship.roll
        );

        // Let go and the ship coasts to a stop, as the other axes do — and then
        // holds the attitude it stopped at rather than levelling out.
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.roll_rate.abs() < 1e-4, "the roll rate must decay");
        let held = ship.roll;
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(
            (ship.roll - held).abs() < 1e-4,
            "{held} drifted to {}",
            ship.roll
        );
    }

    #[test]
    fn roll_and_the_lean_into_a_turn_are_separate_things() {
        // `bank` is cosmetic and centres itself; `roll` is attitude and does
        // not. A hard turn must not leave the ship rolled, and a roll must not
        // show up as a turn.
        let mut ship = Ship::new();
        for _ in 0..120 {
            ship.nudge_yaw(1.0);
            ship.update(1.0 / 60.0);
        }
        assert!(ship.bank.abs() > 0.05, "a turn should lean: {}", ship.bank);
        assert_eq!(ship.roll, 0.0, "a turn is not a roll");

        // Short of half a turn, so the sign is still the one that was flown.
        let mut ship = Ship::new();
        for _ in 0..30 {
            ship.nudge_roll(-1.0);
            ship.update(1.0 / 60.0);
        }
        assert!(ship.roll < -0.1, "Q should roll to port: {}", ship.roll);
        assert_eq!(ship.heading, 0.0, "a roll is not a turn");
        assert_eq!(ship.bank, 0.0, "a roll is not a lean");
    }

    #[test]
    fn wrapping_folds_an_angle_onto_the_indicator() {
        assert_eq!(wrap_signed(0.0), 0.0, "level has to stay exactly level");
        for turns in [-3.0f32, -1.0, 1.0, 2.0] {
            for angle in [0.0f32, 0.5, -0.5, 3.0, -3.0] {
                let folded = wrap_signed(angle + turns * TAU);
                assert!((-PI..PI).contains(&folded), "{folded}");
                assert!(
                    (folded - angle).abs() < 1e-3,
                    "{angle} + {turns} turns folded to {folded}"
                );
            }
        }
    }

    #[test]
    fn throttle_stays_in_range() {
        let mut ship = Ship::new();
        for _ in 0..100 {
            ship.nudge_throttle(1.0);
        }
        assert_eq!(ship.throttle, 1.0);
        for _ in 0..100 {
            ship.nudge_throttle(-1.0);
        }
        assert_eq!(ship.throttle, 0.0);
    }

    /// Fly `steps` of `dt` with the stick worked on all three axes, so the
    /// basis is composed out of rotations that do not commute rather than out
    /// of one repeated turn.
    fn flown(steps: usize, dt: f32) -> Ship {
        let mut ship = Ship::new();
        ship.warp_engaged = true;
        ship.throttle = 1.0;
        for i in 0..steps {
            match i % 7 {
                0 | 3 => ship.nudge_yaw(1.0),
                1 | 4 => ship.nudge_pitch(-1.0),
                2 => ship.nudge_roll(1.0),
                5 => ship.nudge_yaw(-1.0),
                _ => {}
            }
            ship.update(dt);
        }
        ship
    }

    #[test]
    fn the_attitude_stays_orthonormal_and_right_handed() {
        // A screensaver is left up for days, and a step is three rotations each
        // rounding at about 1e-7 — so this is about the drift, not about the
        // first turn. Right-handedness is checked as well as orthogonality
        // because a basis that walked into its own mirror image would look very
        // nearly right and invert every facing test in `crate::models`.
        let ship = flown(200_000, 1.0 / 120.0);
        let [right, down, nose] = ship.axes;
        for (name, axis) in [("right", right), ("down", down), ("nose", nose)] {
            let len = dot(axis, axis).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "the {name} axis is {len} long after a long flight"
            );
        }
        for (a, b, name) in [
            (right, down, "right and down"),
            (right, nose, "right and the nose"),
            (down, nose, "down and the nose"),
        ] {
            assert!(
                dot(a, b).abs() < 1e-5,
                "{name} have drifted {} out of square",
                dot(a, b)
            );
        }
        let cross = [
            right[1] * down[2] - right[2] * down[1],
            right[2] * down[0] - right[0] * down[2],
            right[0] * down[1] - right[1] * down[0],
        ];
        assert!(
            dot(cross, nose) > 0.99,
            "the basis has turned into its own mirror image"
        );
    }

    #[test]
    fn a_ship_nobody_steers_is_pointed_exactly_where_it_started() {
        // Exactly, not nearly. The whole sky is projected through this, so an
        // ulp here is a repainted frame — and the reference flights that carry
        // no autopilot hold every rate at a hard zero from the first frame to
        // the last, which is what makes them able to say a change was the
        // identity it looked like.
        let mut ship = Ship::new();
        ship.warp_engaged = true;
        for _ in 0..600 {
            ship.update(1.0 / 120.0);
        }
        assert_eq!(ship.axes, LEVEL_AXES, "a straight flight has turned");
    }

    #[test]
    fn a_ship_flies_where_its_nose_points() {
        let ship = flown(400, 1.0 / 120.0);
        let nose = ship.axes[2];
        let travelled = dot(
            [
                ship.position[0] as f32,
                ship.position[1] as f32,
                ship.position[2] as f32,
            ],
            nose,
        );
        let straight = (ship.position[0] * ship.position[0]
            + ship.position[1] * ship.position[1]
            + ship.position[2] * ship.position[2])
            .sqrt() as f32;
        assert!(travelled > 0.0, "the ship has flown backwards");
        // Not equal: the track curves while the stick is being worked, so the
        // straight-line distance from the start is shorter than the odometer.
        // What is asserted is that the two are the same order of thing, which
        // is what fails if the nose and the travel ever come apart.
        assert!(
            straight <= ship.distance_ly as f32 * 1.001,
            "the ship has covered more ground than it has flown"
        );
    }

    #[test]
    fn the_odometer_and_the_sky_agree_about_a_light_year() {
        // The panel's distance and the distance the stars are moved by used to
        // be separate inventions, and this is the test that says they are not.
        // Flown straight, so the track is the odometer.
        let mut ship = Ship::new();
        ship.warp_engaged = true;
        ship.throttle = 1.0;
        for _ in 0..1200 {
            ship.update(1.0 / 120.0);
        }
        let flown = (ship.position[0] * ship.position[0]
            + ship.position[1] * ship.position[1]
            + ship.position[2] * ship.position[2])
            .sqrt();
        let drift = (flown - ship.distance_ly).abs() / ship.distance_ly;
        assert!(
            drift < 1e-3,
            "the panel says {} ly and the sky has moved {flown}",
            ship.distance_ly
        );
    }

    #[test]
    fn the_throttle_is_a_warp_factor() {
        // The stick's shape, and the fault it was given for. It used to be
        // linear in `speed`, which `speed_to_c` then put through three orders
        // of magnitude — so half throttle was 83 c, four percent of full-warp
        // speed, and the bottom half of the travel did nothing anybody could
        // see. A factor is what a pilot asks for and each notch is now worth a
        // proportionate change.
        let factor = |throttle: f32| {
            let mut ship = Ship::new();
            ship.warp_engaged = true;
            ship.throttle = throttle;
            for _ in 0..2000 {
                ship.update(1.0 / 120.0);
            }
            ship.warp_factor()
        };
        assert!(
            (factor(0.0) - WARP_ENTRY_FACTOR).abs() < 0.02,
            "the drive catches at warp {}",
            factor(0.0)
        );
        assert!(
            (factor(1.0) - WARP_MAX_FACTOR).abs() < 0.02,
            "full throttle reads warp {}",
            factor(1.0)
        );
        // Linear in between, which is the whole claim. Checked against the
        // straight line rather than against a table of numbers, so the two ends
        // above are the only figures written down twice.
        for step in 0..=10 {
            let throttle = step as f32 / 10.0;
            let want = WARP_ENTRY_FACTOR + (WARP_MAX_FACTOR - WARP_ENTRY_FACTOR) * throttle;
            let got = factor(throttle);
            assert!(
                (got - want).abs() < 0.02,
                "throttle {throttle} settles at warp {got} rather than {want}"
            );
        }
    }

    #[test]
    fn full_impulse_is_a_crawl_and_full_warp_is_not() {
        // The numbers the whole rebuild turns on, and the ones it was retuned
        // against when warp came out too slow. A star seventy light years out —
        // the median distance a magnitude-limited catalogue puts them at —
        // subtends `focal · v / d` subpixels a second, and on the 48-subpixel
        // canvas an 80x24 terminal gives that is a tenth of one at impulse
        // against 206 at warp. Frozen, and then not.
        //
        // The ratio is the load-bearing half and is asserted below: the sky and
        // the dial are one map, so it is 2000 against 0.9 and nothing else.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        for _ in 0..1200 {
            ship.update(1.0 / 120.0);
        }
        let impulse = ship.velocity_ly_per_s();
        ship.warp_engaged = true;
        for _ in 0..1200 {
            ship.update(1.0 / 120.0);
        }
        let warp = ship.velocity_ly_per_s();
        assert!(
            (0.16..0.18).contains(&impulse),
            "full impulse is {impulse} ly/s"
        );
        assert!((375.0..390.0).contains(&warp), "full warp is {warp} ly/s");
        assert!(
            warp / impulse > 2000.0,
            "the sky's answer to the throttle is flatter than the dial's"
        );
    }
}
