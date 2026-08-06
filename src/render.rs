//! Assembling a frame: stars, then the tunnel, then the glass.
//!
//! Everything above the starfield is deliberately driven by the same 0..=1
//! warp ramp the streaks use, so the whole image tightens up together as the
//! drive spools rather than each effect arriving on its own schedule.

use crate::canvas::{Canvas, Tonemap};
use crate::exterior::ExteriorField;
use crate::hud::{self, Readout};
use crate::lens::Lens;
use crate::models::{self, ShipModel};
use crate::ship::Ship;
use crate::starfield::{Camera, StarField};
use crate::term::{ColorMode, Screen};
use crate::view::{self, Orbit, SIDE_FOCAL};
use std::io::{self, Write};

/// Colour of the glare down the throat of the tunnel.
const CORE_COLOR: [f32; 3] = [0.62, 0.80, 1.00];
/// Shake displacement at full intensity, as a fraction of canvas height.
const SHAKE_AMPLITUDE: f32 = 0.045;
/// Display gamma the tonemap decodes for. Fixed: it describes the terminal,
/// not a taste setting, and `--exposure` is the knob for brightness.
const GAMMA: f32 = 2.2;

/// Everything the view from outside draws, beyond the camera and the panel.
///
/// A bundle rather than four more arguments: `render_exterior` would otherwise
/// be one past the point where clippy — which CI runs as an error — starts
/// asking what all of them are for.
pub struct Exterior<'a> {
    /// Taken by mutable reference because bending a streak needs somewhere to
    /// put it, and that scratch is kept with the field rather than allocated
    /// afresh for every star of every frame.
    pub field: &'a mut ExteriorField,
    pub ship: &'a Ship,
    pub model: &'a ShipModel,
    pub time: f64,
    /// How far in or out the camera has been pushed, as a multiple of the
    /// default framing. It rides in here rather than on the [`Camera`] because
    /// the camera has not moved: the zoom is a dolly on the *ship*, and the
    /// sky — which is laid out against the camera's focal length and cached
    /// against it — is meant to sit perfectly still while it happens.
    pub zoom: f32,
    /// Which way round the ship the camera has been swung. It rides in here
    /// for the same reason the zoom does and with the opposite consequence:
    /// the [`Camera`] still has no pose, so this is the only thing that knows
    /// the eye has moved — but unlike the dolly, an orbit is *meant* to take
    /// the sky with it, and the star band is handed it separately in
    /// `Flight::advance` so that it can.
    pub orbit: Orbit,
}

pub struct Renderer {
    canvas: Canvas,
    screen: Screen,
    /// The exposure and gamma curve, baked into a table once at startup.
    tonemap: Tonemap,
    /// Scratch buffer for resolved pixels, reused across frames.
    pixels: Vec<[u8; 3]>,
}

impl Renderer {
    pub fn new(cols: usize, rows: usize, mode: ColorMode, exposure: f32) -> Self {
        // Clamp *before* doubling. `Canvas` and `Screen` each clamp a zero
        // dimension up to one on their own, and handed a zero the two
        // disagree about how big the pixel buffer is: the canvas becomes one
        // row, the screen wants two. A terminal that reports zero rows — which
        // is what tmux hands a `lock-command` — then crashes `compose`.
        let (cols, rows) = (cols.max(1), rows.max(1));
        Self {
            // Two subpixel rows per terminal row: that is the half-block trick.
            canvas: Canvas::new(cols, rows * 2),
            screen: Screen::new(cols, rows, mode),
            tonemap: Tonemap::new(exposure, GAMMA),
            pixels: Vec::with_capacity(cols * rows * 2),
        }
    }

    /// How finely a hull's outline is measured, per axis — `--aa`.
    ///
    /// A builder rather than a fifth argument to [`Self::new`]. There are
    /// seventeen renderers in this tree and sixteen of them are tests, which
    /// want the default and would otherwise all have to say so; the one caller
    /// that has an opinion is the one holding the command line.
    pub fn with_hull_samples(mut self, samples: usize) -> Self {
        self.canvas.set_hull_samples(samples);
        self
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
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
    pub fn camera(&self, ship: &Ship, time: f64) -> Camera {
        let (w, h) = self.canvas.dims();
        let mut cam = Camera::new(w, h);
        if ship.shake > 0.0 {
            // Two incommensurate frequencies so the wobble never looks like a
            // clean sine, without needing a noise table. Evaluated in `f64`:
            // there are only four of them per frame, and it means the argument
            // reduction stays exact however long the process has been up.
            let amp = h as f32 * SHAKE_AMPLITUDE * ship.shake;
            cam.cx += amp * ((time * 31.0).sin() + (time * 17.3).sin() * 0.6) as f32;
            cam.cy += amp * ((time * 27.7).cos() + (time * 13.1).sin() * 0.6) as f32;
        }
        cam.bank = ship.bank;
        cam
    }

    /// Build the camera for the view from outside: parked off the ship's
    /// starboard beam, looking across its track.
    ///
    /// The shake is spelled out again rather than shared with [`Self::camera`].
    /// That function's arithmetic is pinned byte for byte by the reference
    /// frames, and a helper extracted out of it is exactly the sort of
    /// "obviously equivalent" edit that moves a float by an ulp and repaints
    /// the whole sky.
    pub fn exterior_camera(&self, ship: &Ship, time: f64) -> Camera {
        let (w, h) = self.canvas.dims();
        let mut cam = Camera::new(w, h);
        // A longer lens than the cockpit's. It flattens the hull into a
        // profile, which is the shot; a wide one splays it into a
        // three-quarter view of a ship that is meant to be seen side-on.
        cam.focal = (h as f32).max(1.0) * SIDE_FOCAL;
        if ship.shake > 0.0 {
            let amp = h as f32 * SHAKE_AMPLITUDE * ship.shake;
            cam.cx += amp * ((time * 31.0).sin() + (time * 17.3).sin() * 0.6) as f32;
            cam.cy += amp * ((time * 27.7).cos() + (time * 13.1).sin() * 0.6) as f32;
        }
        // `bank` stays level. Out here the horizon is the direction of travel,
        // and a star stream that tilts into a turn reads as a skid — the lean
        // belongs to the hull, which has one of its own.
        cam
    }

    /// Draw one frame into the cell grid. Nothing reaches the terminal yet.
    pub fn render(
        &mut self,
        field: &StarField,
        ship: &Ship,
        cam: &Camera,
        time: f64,
        hud: &Readout,
    ) {
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
        //
        // Both are wide and weak rather than narrow and fierce, and that is
        // the whole of the tuning. The core was a tenth of the height across
        // at 1.6 and the halo half of it at 0.45, which put a hard white point
        // at the vanishing point inside a veil that reached the middle third
        // of the frame — the point too small to read as distance and the veil
        // wide enough to flatten the streaks carrying it. Spreading each over
        // two to three times the radius at a quarter of the strength trades
        // one for the other: the light arrives over a broad falloff instead of
        // as a blob with a wash round it, and the streaks stay legible through
        // all of it because no part of the pool is bright enough to bury them.
        //
        // The falloff stays quartic, which is what keeps this from reading as
        // a solid body hanging in the middle of the view. Flattening the curve
        // instead of widening the radius is the obvious alternative and it was
        // tried: at a squared falloff the pool acquires a visible rim and the
        // tunnel becomes a ball. `Canvas::add_glow` says the same thing from
        // the other end, and it is shared with the drive bells out in the side
        // view, so the exponent is not this call site's to change anyway.
        if warp > 0.0 {
            let (_, h) = self.canvas.dims();
            let glare = warp * warp * warp;
            self.canvas
                .add_glow(cam.cx, cam.cy, h as f32 * 0.30, CORE_COLOR, glare * 0.42);
            self.canvas
                .add_glow(cam.cx, cam.cy, h as f32 * 0.75, CORE_COLOR, glare * 0.14);
        }
        self.canvas
            .apply_vignette(cam.cx, cam.cy, 0.22 + 0.48 * warp);

        // The flash sits on top of the vignette so it whites out the whole
        // frame, edges included, the way a drive catching should.
        if ship.flash > 0.0 {
            self.canvas
                .add_flash([1.0, 1.0, 1.0], ship.flash.powf(1.6) * 0.85);
        }

        self.canvas.resolve_into(&self.tonemap, &mut self.pixels);
        self.screen.compose(&self.pixels);
        hud::draw(&mut self.screen, hud);
    }

    /// Draw one frame of the view from outside. Nothing reaches the terminal
    /// yet.
    ///
    /// The same order as [`Self::render`] — sky, then what is lit, then the
    /// glass — with the hull between them. There is no depth buffer and none
    /// is needed: [`crate::exterior`]'s near wall is beyond the furthest any
    /// hull reaches, so nothing can be in front of it, and the far side of the
    /// hull is culled on the winding of each plate. The clearance is kept as a
    /// `const` assertion in that module rather than quoted here as a distance,
    /// which is what it used to be and what had silently gone wrong: the
    /// number outlived the constants it was measured from by some margin, and
    /// a wrong figure guarding an invariant reads exactly like a right one.
    pub fn render_exterior(&mut self, scene: Exterior<'_>, cam: &Camera, hud: &Readout) {
        let Exterior {
            field,
            ship,
            model,
            time,
            zoom,
            orbit,
        } = scene;
        let warp = ship.warp_intensity();
        let (_, h) = self.canvas.dims();
        // Where the zoom lands: how far off to put the hull, and how big that
        // makes it. Two readings of one number, taken here rather than in each
        // of the two places that wants one, so they cannot disagree.
        let eye = view::Eye::new(orbit, zoom);
        let ship_half = view::ship_half_on_screen(h as f32, zoom);

        // The ship sits dead on the camera's axis, so its centre projects to
        // the vanishing point — shake included, which keeps the bend locked to
        // the hull rather than to the frame. Sized in ships rather than in
        // frames, so pushing the camera in and out carries the bubble with the
        // hull instead of leaving a fixed collar hanging in the middle.
        let lens = Lens::for_warp((cam.cx, cam.cy), warp, ship_half, orbit.nose_in_camera());

        self.canvas.clear();
        field.draw(&mut self.canvas, cam, warp, time, &lens);

        // A wash inside the shadow, so the region the lens has swept clear
        // reads as a bubble the ship is sitting in rather than as a hole
        // punched in the sky. Drawn to the shadow's own two axes and about the
        // bubble's own centre, both of which are now astern of the vanishing
        // point: a round wash sitting in an elongated hole gives the shape away
        // twice over, once at the ends where the hole runs on past the light
        // and once at the waist where the light spills out of it.
        if lens.is_on() {
            let glare = warp * warp * warp;
            let (rx, ry) = lens.shadow_axes();
            self.canvas.add_glow_oval(
                lens.center,
                (rx * 1.5, ry * 1.5),
                lens.turn(),
                CORE_COLOR,
                glare * 0.35,
            );
        }

        models::draw(&mut self.canvas, cam, ship, model, &eye, time);

        // A lighter vignette than the cockpit's: there is no tunnel to be
        // pulled down here, and the ship is off the centre of attention.
        self.canvas
            .apply_vignette(cam.cx, cam.cy, 0.18 + 0.30 * warp);
        if ship.flash > 0.0 {
            self.canvas
                .add_flash([1.0, 1.0, 1.0], ship.flash.powf(1.6) * 0.85);
        }

        self.canvas.resolve_into(&self.tonemap, &mut self.pixels);
        self.screen.compose(&self.pixels);
        hud::draw(&mut self.screen, hud);
    }

    /// Push the frame to an interactive terminal, writing only what changed.
    pub fn present(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.screen.flush(out)
    }

    /// Write the frame as a self-contained block of text, for piping.
    pub fn present_plain(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.screen.write_plain(out)
    }

    /// The resolved 8-bit pixels of the last rendered frame.
    pub fn pixels(&self) -> &[[u8; 3]] {
        &self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readout(ship: &Ship) -> Readout<'_> {
        Readout {
            ship,
            fps: 60.0,
            stars: 1000,
            paused: false,
            hints: true,
            view: crate::view::ViewMode::Cockpit,
            model: "dart",
        }
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
            field.update(
                1.0 / 60.0,
                ship.speed,
                ship.yaw_rate,
                ship.pitch_rate,
                ship.roll_rate,
                &cam,
            );
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
                field.update(1.0 / 60.0, ship.speed, 0.0, 0.0, 0.0, &cam);
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
        assert!(
            warping > cruising * 1.5,
            "cruise {cruising:.1} vs warp {warping:.1}"
        );
    }

    /// Fly the outside view for a while and hand back the renderer.
    fn fly_outside(
        cols: usize,
        rows: usize,
        model: usize,
        engaged: bool,
        frames: usize,
        stars: usize,
    ) -> Renderer {
        fly_outside_from(
            cols,
            rows,
            model,
            engaged,
            frames,
            stars,
            (view::ZOOM_DEFAULT, Orbit::LEVEL),
        )
    }

    /// The same, from a camera of your choosing: how far off it is parked, and
    /// which way round the ship it has been swung. The two travel together
    /// because they are the two halves of [`view::Eye`], and because one more
    /// loose argument here is one past where clippy stops reading.
    fn fly_outside_from(
        cols: usize,
        rows: usize,
        model: usize,
        engaged: bool,
        frames: usize,
        stars: usize,
        (zoom, orbit): (f32, Orbit),
    ) -> Renderer {
        let mut renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        if engaged {
            ship.toggle_warp();
        }
        let cam = renderer.exterior_camera(&ship, 0.0);
        let mut field = crate::exterior::ExteriorField::new(stars, 5, &cam, orbit);
        let mut time = 0.0;
        for _ in 0..frames {
            time += 1.0 / 60.0;
            ship.update(1.0 / 60.0);
            let cam = renderer.exterior_camera(&ship, time);
            field.update(1.0 / 60.0, ship.speed, &cam, orbit);
            let readout = readout(&ship);
            let scene = Exterior {
                field: &mut field,
                ship: &ship,
                model: &crate::models::models()[model],
                time,
                zoom,
                orbit,
            };
            renderer.render_exterior(scene, &cam, &readout);
        }
        renderer
    }

    #[test]
    fn the_outside_view_renders_end_to_end_for_every_ship() {
        for model in 0..crate::models::models().len() {
            for engaged in [false, true] {
                let renderer = fly_outside(80, 24, model, engaged, 90, 800);
                assert!(
                    renderer.pixels().iter().any(|p| p.iter().any(|v| *v > 40)),
                    "ship {model} came out black"
                );
            }
        }
    }

    #[test]
    fn warp_opens_a_hole_in_the_sky_around_the_ship() {
        // The lens, seen from the outside of the renderer: at warp the sky
        // immediately around the hull is swept clear and piles up beyond it,
        // and at impulse the two read much the same.
        //
        // The two bands are measured in *rings*, which is the third unit this
        // has been written in and the last one it needs. Fractions of the canvas
        // height only worked while the hull was a fixed fraction of it. Ships
        // survived the zoom but not the shape: the bubble is elongated and
        // seated astern now, so a circle drawn about the middle of the frame at
        // 1.1 ship-halves passes through the swept-clear disc ahead of the ship
        // and straight out through the bright rim above it. `Lens::offset` is
        // the bubble's own metric, so a band in it means the same thing at any
        // framing, any zoom and any shape the bubble is ever given.
        let sample = |engaged: bool| -> (f32, f32) {
            let renderer = fly_outside(120, 36, 0, engaged, 150, 2500);
            let (w, h) = renderer.canvas_dims();
            let px = renderer.pixels();
            let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
            let ship = view::ship_half_on_screen(h as f32, view::ZOOM_DEFAULT);
            // The bubble a lit drive makes, used as the ruler for both frames:
            // the point is to look at the same places in each, so this is built
            // at full warp whether the drive in the frame is lit or not.
            let bubble = Lens::for_warp((cx, cy), 1.0, ship, Orbit::LEVEL.nose_in_camera());
            let mean = |lo: f32, hi: f32| {
                let (mut total, mut n) = (0u64, 0u64);
                for y in 0..h {
                    for x in 0..w {
                        let m = bubble.offset((x as f32, y as f32));
                        if m >= lo && m < hi {
                            let p = px[y * w + x];
                            total += (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
                            n += 1;
                        }
                    }
                }
                total as f32 / n.max(1) as f32
            };
            // Inside the shadow, which is where the hull sits; and out past the
            // ring, where the sky the bubble pushed aside has piled up. The
            // shadow is 0.72 of the way out, so the first band is well inside
            // it and the second starts beyond the ring entirely.
            (mean(0.46, 0.63), mean(1.15, 1.55))
        };

        let (near_hull, far_out) = sample(true);
        assert!(
            near_hull < far_out * 0.7,
            "warp did not clear the sky around the ship: {near_hull} against {far_out}"
        );
        let (near_hull, far_out) = sample(false);
        assert!(
            near_hull > far_out * 0.7,
            "impulse should not bend anything: {near_hull} against {far_out}"
        );
    }

    #[test]
    fn a_zero_sized_terminal_does_not_crash_from_outside() {
        for (cols, rows) in [(0usize, 0usize), (80, 0), (0, 24), (1, 0), (2, 3)] {
            let renderer = fly_outside(cols, rows, 0, true, 5, 200);
            let (w, h) = renderer.canvas_dims();
            assert_eq!(renderer.pixels().len(), w * h);
        }
    }

    #[test]
    fn a_zero_sized_terminal_does_not_crash() {
        // Regression: tmux runs a lock-command against a tty whose size is not
        // established, so `terminal::size()` can hand back zero rows. Clamping
        // after doubling left the canvas one subpixel row tall while the
        // screen expected two, and `compose` indexed off the end.
        for (cols, rows) in [(0, 0), (80, 0), (0, 24), (1, 0)] {
            let mut renderer = Renderer::new(cols, rows, ColorMode::Truecolor, 1.9);
            let (w, h) = renderer.canvas_dims();
            let (sc, sr) = renderer.screen().dims();
            assert_eq!(
                (w, h),
                (sc, sr * 2),
                "canvas and screen disagree at {cols}x{rows}"
            );

            let mut ship = Ship::new();
            ship.throttle = 1.0;
            let cam = renderer.camera(&ship, 0.0);
            let mut field = StarField::new(50, 1, &cam);
            for _ in 0..5 {
                ship.update(1.0 / 60.0);
                field.update(1.0 / 60.0, ship.speed, 0.0, 0.0, 0.0, &cam);
                renderer.render(&field, &ship, &cam, 0.0, &readout(&ship));
            }
            renderer.present(&mut Vec::new()).unwrap();

            renderer.resize(cols, rows);
            let (w, h) = renderer.canvas_dims();
            let (sc, sr) = renderer.screen().dims();
            assert_eq!(
                (w, h),
                (sc, sr * 2),
                "resize to {cols}x{rows} desynced them"
            );
        }
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
            let cam = renderer.camera(&ship, i as f64);
            field.retarget(&cam);
            for _ in 0..30 {
                ship.update(1.0 / 60.0);
                field.update(1.0 / 60.0, ship.speed, 0.2, 0.0, 0.0, &cam);
                renderer.render(&field, &ship, &cam, i as f64, &readout(&ship));
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
