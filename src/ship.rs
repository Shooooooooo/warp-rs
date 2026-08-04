//! Flight model: throttle, the warp drive, steering, and the transient effects
//! that make a change of speed feel like something actually happened.
//!
//! Terminals report key *presses*, not releases, so every control here is
//! impulse-driven: a keypress nudges a value and the value decays back on its
//! own. Holding a key down produces auto-repeat, which reads as sustained
//! input without needing a key-up event.

use std::f32::consts::{FRAC_PI_2, TAU};

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
const MAX_YAW_RATE: f32 = 0.85;
const MAX_PITCH_RATE: f32 = 0.60;
const STEER_DAMPING: f32 = 2.6;

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
    /// Angular rates in rad/s, decaying toward zero.
    pub yaw_rate: f32,
    pub pitch_rate: f32,
    /// Accumulated attitude, for the instrument panel.
    pub heading: f32,
    pub pitch: f32,
    /// Bank angle, lagging behind the yaw rate so turns feel like turns.
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
            heading: 0.0,
            pitch: 0.0,
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
        // The hard dropout curve only applies until we've rejoined sublight.
        if self.dropping_out && self.speed <= CRUISE_MAX * 1.02 {
            self.dropping_out = false;
        }

        let damp = (-STEER_DAMPING * dt).exp();
        self.yaw_rate *= damp;
        self.pitch_rate *= damp;

        self.heading = (self.heading + self.yaw_rate * dt).rem_euclid(TAU);
        self.pitch = (self.pitch + self.pitch_rate * dt).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        // Bank chases the yaw rate rather than tracking it exactly, so rolling
        // into and out of a turn has some lag to it.
        let target_bank = -self.yaw_rate * 0.42;
        self.bank += (target_bank - self.bank) * (1.0 - (-5.0 * dt).exp());

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
        }
        assert!(ship.yaw_rate <= MAX_YAW_RATE);
        assert!(ship.pitch_rate >= -MAX_PITCH_RATE);
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.yaw_rate.abs() < 0.01 && ship.pitch_rate.abs() < 0.01);
        assert!(ship.pitch >= -PITCH_LIMIT);
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
