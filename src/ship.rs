//! Flight model: throttle, the warp drive, steering, and the transient effects
//! that make a change of speed feel like something actually happened.
//!
//! Terminals report key *presses*, not releases, so every control here is
//! impulse-driven: a keypress nudges a value and the value decays back on its
//! own. Holding a key down produces auto-repeat, which reads as sustained
//! input without needing a key-up event.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

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

        self.distance_ly += (self.velocity_c() * dt * TIME_COMPRESSION / SECONDS_PER_YEAR) as f64;
    }

    /// Current velocity as a multiple of the speed of light.
    pub fn velocity_c(&self) -> f32 {
        speed_to_c(self.speed)
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

/// Fold an angle into `[-PI, PI)` — the range a bank indicator reads in, and
/// the one that keeps a ship that has rolled the same way for an hour from
/// accumulating an angle too coarse to steer with.
fn wrap_signed(angle: f32) -> f32 {
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
}
