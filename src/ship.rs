//! Flight model: throttle, the warp drive, steering, and the transient effects
//! that make a change of speed feel like something actually happened.

use std::f32::consts::{PI, TAU};

/// Top sublight speed, in world units per second.
pub const CRUISE_MAX: f32 = 42.0;
/// Top speed with the warp drive engaged.
pub const WARP_MAX: f32 = 780.0;
/// Speed floor the drive snaps to the instant it engages.
const WARP_ENTRY: f32 = 170.0;

/// Velocity, in multiples of c, at `CRUISE_MAX`.
const CRUISE_MAX_C: f32 = 0.9;
/// Velocity, in multiples of c, at `WARP_MAX`.
const WARP_MAX_C: f32 = 2000.0;

/// Flight time is compressed so the odometer moves: one second at the stick is
/// one day underway. At full warp that reads out around 5 ly per second.
const TIME_COMPRESSION: f32 = 86_400.0;
const SECONDS_PER_YEAR: f32 = 31_557_600.0;

/// How far one multiple of c carries the ship in a second at the stick, in
/// light years.
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

/// The attitude a ship launches at: starboard along `+x`, down along `+y`, the
/// nose along `+z`. Every entry is an exact zero or an exact one, so a flight
/// that never touches the stick projects the sky through a transform that is
/// the identity to the bit.
pub(crate) const LEVEL_AXES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

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
    /// The compass, for the instrument panel: a bearing about the ship's own
    /// vertical and the nose's angle to the horizontal, both in radians and
    /// both read straight off [`Self::axes`] each step.
    pub heading: f32,
    pub pitch: f32,
    /// Roll the pilot has flown the ship to, in radians, folded into `[-PI,
    /// PI)`. Positive is a roll to starboard. Unlike heading and pitch it has
    /// nothing to level against — there is no horizon out here — so it stays
    /// wherever it is put until it is rolled back or the ship is reset.
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
    /// the nose, in that order and in the ship's own `+z` out the nose, `+x` to
    /// starboard, `+y` down convention.
    pub axes: [[f32; 3]; 3],
    /// Where the ship is, in light years, in that same inertial frame.
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
            self.speed = self.speed.max(WARP_ENTRY);
            self.shake = 1.0;
            self.flash = 1.0;
        } else {
            self.dropping_out = true;
            self.shake = 0.55;
        }
        self.warp_engaged
    }

    /// True while the drive is spinning down after a disengage.
    pub fn dropping_out(&self) -> bool {
        self.dropping_out
    }

    /// Where the throttle is asking the ship to end up.
    fn target_speed(&self) -> f32 {
        if self.warp_engaged {
            WARP_ENTRY + (WARP_MAX - WARP_ENTRY) * self.throttle
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

        // Roll is flown rather than read off: it turns the ship about the very
        // axis it is travelling along, so it moves the profile without moving
        // the nose and there is nothing in the attitude to recover it from.
        self.roll = wrap_signed(self.roll + self.roll_rate * dt);

        // Bank chases the yaw rate rather than tracking it exactly, so rolling
        // into and out of a turn has some lag to it.
        let target_bank = -self.yaw_rate * BANK_PER_YAW;
        self.bank += (target_bank - self.bank) * (1.0 - (-BANK_LAG * dt).exp());

        self.shake *= (-SHAKE_DECAY * dt).exp();
        self.flash *= (-FLASH_DECAY * dt).exp();

        self.steer(dt);
        // Read off the attitude that was just flown rather than integrated
        // beside it, which is the whole of the fix for a compass that lost
        // calibration and never got it back.
        let nose = self.axes[2];
        self.heading = nose[0].atan2(nose[2]).rem_euclid(TAU);
        self.pitch = nose[1].clamp(-1.0, 1.0).asin();
        self.coast(dt);

        // `velocity_ly_per_s` rather than the two constants spelled out again,
        // which is the whole of what [`LY_PER_C_SECOND`] was introduced for:
        // the distance the panel reports and the distance the sky is moved by
        // have to be one scale, and two spellings of one scale is how two
        // numbers come apart.
        self.distance_ly += (self.velocity_ly_per_s() * dt) as f64;
    }

    /// Turn the hull by this step's rates.
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
    pub fn warp_intensity(&self) -> f32 {
        ((self.speed - CRUISE_MAX) / (WARP_MAX - CRUISE_MAX)).clamp(0.0, 1.0)
    }
}

/// A basis pulled back to orthonormal and right-handed, and sent home to
/// [`LEVEL_AXES`] if it cannot be.
pub(crate) fn orthonormalise(a: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
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
pub fn wrap_signed(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
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
        // rather than been throttled back, so it has to be true for exactly the
        // stretch the steep bleed applies over — a flag that never came on and
        // a flag that never went off would both leave the plume looking right
        // for most of the flight.
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
        // The compass is an angle to the horizontal, so it lives inside a
        // quarter turn by construction rather than by a clamp.
        assert!(
            ship.pitch.abs() <= std::f32::consts::FRAC_PI_2 + 1e-6,
            "pitch {}",
            ship.pitch
        );
    }

    #[test]
    fn the_compass_agrees_with_the_ship_it_is_bolted_to() {
        // Regression, and the report it came from is worth keeping: `pitch` was
        // an integral of the pitch rate with a clamp on top, so it discarded
        // input the attitude went on taking and never got it back.
        let mut ship = Ship::new();
        let dt = 1.0 / 120.0;
        let hold = |ship: &mut Ship, dir: f32, seconds: f32| {
            for _ in 0..(seconds / dt) as usize {
                ship.nudge_pitch(dir);
                ship.update(dt);
            }
        };

        hold(&mut ship, -1.0, 5.0);
        hold(&mut ship, 1.0, 5.0);
        for _ in 0..600 {
            ship.update(dt);
        }
        let flown = ship.axes[2][1].asin();
        assert!(
            (ship.pitch - flown).abs() < 1e-5,
            "the ship is at {:.1} degrees and the compass says {:.1}",
            flown.to_degrees(),
            ship.pitch.to_degrees()
        );

        let mut ship = Ship::new();
        for step in 0..1200 {
            ship.nudge_roll(1.0);
            ship.nudge_yaw(1.0);
            ship.nudge_pitch(if step % 200 < 100 { -1.0 } else { 1.0 });
            ship.update(dt);
            let nose = ship.axes[2];
            assert!(
                (ship.pitch - nose[1].asin()).abs() < 1e-5
                    && (ship.heading - nose[0].atan2(nose[2]).rem_euclid(TAU)).abs() < 1e-5,
                "the compass drifted off the ship at step {step}"
            );
        }
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
        // not.
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
        // first turn.
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
        assert!(
            straight <= ship.distance_ly as f32 * 1.001,
            "the ship has covered more ground than it has flown"
        );
    }

    #[test]
    fn the_odometer_and_the_sky_agree_about_a_light_year() {
        // The panel's distance and the distance the stars are moved by used to
        // be separate inventions, and this is the test that says they are not.
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
    fn full_impulse_is_a_crawl_and_full_warp_is_not() {
        // The numbers the whole rebuild turns on.
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
            (0.0022..0.0025).contains(&impulse),
            "full impulse is {impulse} ly/s"
        );
        assert!((5.0..5.6).contains(&warp), "full warp is {warp} ly/s");
        assert!(
            warp / impulse > 2000.0,
            "the sky's answer to the throttle is flatter than the dial's"
        );
    }
}
