//! Assembling a frame: stars, then the tunnel, then the glass.
//!
//! Everything above the starfield is deliberately driven by the same 0..=1
//! warp ramp the streaks use, so the whole image tightens up together as the
//! drive spools rather than each effect arriving on its own schedule.

use crate::canvas::Canvas;
use crate::hud::{self, Readout};
use crate::ship::Ship;
use crate::starfield::{Camera, StarField};
use crate::term::{ColorMode, Screen};
use std::io::{self, Write};

/// Colour of the glare down the throat of the tunnel.
const CORE_COLOR: [f32; 3] = [0.62, 0.80, 1.00];
/// Shake displacement at full intensity, as a fraction of canvas height.
const SHAKE_AMPLITUDE: f32 = 0.045;

pub struct Renderer {
    canvas: Canvas,
    screen: Screen,
    exposure: f32,
    gamma: f32,
    /// Scratch buffer for resolved pixels, reused across frames.
    pixels: Vec<[u8; 3]>,
}

impl Renderer {
    pub fn new(cols: usize, rows: usize, mode: ColorMode, exposure: f32) -> Self {
        Self {
            // Two subpixel rows per terminal row: that is the half-block trick.
            canvas: Canvas::new(cols, rows * 2),
            screen: Screen::new(cols, rows, mode),
            exposure,
            gamma: 2.2,
            pixels: Vec::new(),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.canvas.resize(cols, rows * 2);
        self.screen.resize(cols, rows);
    }

    /// Canvas dimensions in subpixels.
    pub fn canvas_dims(&self) -> (usize, usize) {
        self.canvas.dims()
    }

    pub fn screen(&mut self) -> &mut Screen {
        &mut self.screen
    }

    /// Build the camera for this instant, including shake and bank.
    pub fn camera(&self, ship: &Ship, time: f32) -> Camera {
        let (w, h) = self.canvas.dims();
        let mut cam = Camera::new(w, h);
        if ship.shake > 0.0 {
            // Two incommensurate frequencies so the wobble never looks like a
            // clean sine, without needing a noise table.
            let amp = h as f32 * SHAKE_AMPLITUDE * ship.shake;
            cam.cx += amp * ((time * 31.0).sin() + (time * 17.3).sin() * 0.6);
            cam.cy += amp * ((time * 27.7).cos() + (time * 13.1).sin() * 0.6);
        }
        cam.bank = ship.bank;
        cam
    }

    /// Draw one frame into the cell grid. Nothing reaches the terminal yet.
    pub fn render(&mut self, field: &StarField, ship: &Ship, cam: &Camera, time: f32, hud: &Readout) {
        let warp = ship.warp_intensity();

        self.canvas.clear();
        for streak in field.streaks(cam, warp, time) {
            self.canvas.draw_streak(&streak);
        }

        // The tunnel: glare down the throat, and a vignette closing in around
        // it. Two overlaid glows — a tight core inside a wide halo — read as
        // light; a single blob reads as an object. Both ramp with the cube of
        // the warp ramp so nothing is hanging in the middle of the view until
        // the ship is genuinely moving.
        if warp > 0.0 {
            let (_, h) = self.canvas.dims();
            let glare = warp * warp * warp;
            self.canvas
                .add_glow(cam.cx, cam.cy, h as f32 * 0.10, CORE_COLOR, glare * 1.6);
            self.canvas
                .add_glow(cam.cx, cam.cy, h as f32 * 0.50, CORE_COLOR, glare * 0.45);
        }
        self.canvas
            .apply_vignette(cam.cx, cam.cy, 0.22 + 0.48 * warp);

        // The flash sits on top of the vignette so it whites out the whole
        // frame, edges included, the way a drive catching should.
        if ship.flash > 0.0 {
            self.canvas
                .add_flash([1.0, 1.0, 1.0], ship.flash.powf(1.6) * 0.85);
        }

        self.pixels = self.canvas.resolve(self.exposure, self.gamma);
        self.screen.compose(&self.pixels);
        hud::draw(&mut self.screen, hud);
    }

    /// Push the frame to an interactive terminal, writing only what changed.
    pub fn present(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.screen.flush(out)
    }

    /// Write the frame as a self-contained block of text, for piping.
    pub fn present_plain(&self, out: &mut impl Write) -> io::Result<()> {
        self.screen.write_plain(out)
    }

    /// The resolved 8-bit pixels of the last rendered frame.
    #[cfg_attr(not(feature = "snapshot"), allow(dead_code))]
    pub fn pixels(&self) -> &[[u8; 3]] {
        &self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readout(ship: &Ship) -> Readout<'_> {
        Readout { ship, fps: 60.0, stars: 1000, paused: false }
    }

    #[test]
    fn a_frame_renders_end_to_end_at_every_speed() {
        let mut renderer = Renderer::new(80, 24, ColorMode::Truecolor, 1.25);
        let (w, h) = renderer.canvas_dims();
        assert_eq!((w, h), (80, 48), "two subpixel rows per terminal row");

        let mut ship = Ship::new();
        let cam = renderer.camera(&ship, 0.0);
        let mut field = StarField::new(600, 42, &cam);
        let mut time = 0.0;

        ship.throttle = 1.0;
        ship.toggle_warp();
        for frame in 0..240 {
            time += 1.0 / 60.0;
            ship.update(1.0 / 60.0);
            let cam = renderer.camera(&ship, time);
            field.update(1.0 / 60.0, ship.speed, ship.yaw_rate, ship.pitch_rate, &cam);
            renderer.render(&field, &ship, &cam, time, &readout(&ship));

            if frame == 120 {
                // Something must actually be lit up by now.
                assert!(
                    renderer.pixels().iter().any(|p| p.iter().any(|v| *v > 40)),
                    "the frame came out black"
                );
            }
        }
        let mut out = Vec::new();
        renderer.present(&mut out).unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn warp_brightens_the_middle_of_the_frame() {
        let sample = |engaged: bool| {
            let mut renderer = Renderer::new(80, 24, ColorMode::Truecolor, 1.25);
            let mut ship = Ship::new();
            ship.throttle = 1.0;
            if engaged {
                ship.toggle_warp();
            }
            let cam = renderer.camera(&ship, 0.0);
            let mut field = StarField::new(1500, 11, &cam);
            let mut time = 0.0;
            for _ in 0..300 {
                time += 1.0 / 60.0;
                ship.update(1.0 / 60.0);
                let cam = renderer.camera(&ship, time);
                field.update(1.0 / 60.0, ship.speed, 0.0, 0.0, &cam);
                renderer.render(&field, &ship, &cam, time, &readout(&ship));
            }
            let (w, h) = renderer.canvas_dims();
            let px = renderer.pixels();
            // Average brightness over the middle ninth of the frame.
            let mut total = 0u64;
            let mut n = 0u64;
            for y in h / 3..2 * h / 3 {
                for x in w / 3..2 * w / 3 {
                    let p = px[y * w + x];
                    total += (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
                    n += 1;
                }
            }
            total as f64 / n as f64
        };
        let cruising = sample(false);
        let warping = sample(true);
        assert!(warping > cruising * 1.5, "cruise {cruising:.1} vs warp {warping:.1}");
    }

    #[test]
    fn resizing_mid_flight_is_survivable() {
        let mut renderer = Renderer::new(80, 24, ColorMode::Truecolor, 1.25);
        let mut ship = Ship::new();
        ship.throttle = 0.8;
        let cam = renderer.camera(&ship, 0.0);
        let mut field = StarField::new(400, 5, &cam);

        for (i, (cols, rows)) in [(80, 24), (200, 60), (12, 4), (1, 1), (120, 40)]
            .into_iter()
            .enumerate()
        {
            renderer.resize(cols, rows);
            let (w, h) = renderer.canvas_dims();
            assert_eq!((w, h), (cols.max(1), (rows * 2).max(1)));
            let cam = renderer.camera(&ship, i as f32);
            field.retarget(&cam);
            for _ in 0..30 {
                ship.update(1.0 / 60.0);
                field.update(1.0 / 60.0, ship.speed, 0.2, 0.0, &cam);
                renderer.render(&field, &ship, &cam, i as f32, &readout(&ship));
            }
            renderer.present(&mut Vec::new()).unwrap();
        }
    }

    #[test]
    fn shake_moves_the_vanishing_point_and_settles() {
        let mut ship = Ship::new();
        let renderer = Renderer::new(80, 24, ColorMode::Truecolor, 1.25);
        let still = renderer.camera(&ship, 1.0);
        ship.toggle_warp();
        let shaking = renderer.camera(&ship, 1.0);
        assert!(
            (shaking.cx - still.cx).abs() + (shaking.cy - still.cy).abs() > 0.1,
            "engaging warp should kick the view"
        );
        for _ in 0..600 {
            ship.update(1.0 / 60.0);
        }
        let settled = renderer.camera(&ship, 1.0);
        assert!((settled.cx - still.cx).abs() < 0.05);
    }
}
