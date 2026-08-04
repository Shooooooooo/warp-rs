//! The instrument panel.
//!
//! Drawn last, over the composed frame, so it reads as glass in front of the
//! stars rather than something painted into the sky. Every element checks the
//! terminal it has been given: on a small window the panel sheds detail
//! instead of overflowing.

use crate::ship::Ship;
use crate::term::Screen;

const LABEL: (u8, u8, u8) = (96, 176, 208);
const VALUE: (u8, u8, u8) = (226, 240, 255);
const ACCENT: (u8, u8, u8) = (255, 186, 92);
const WARN: (u8, u8, u8) = (255, 122, 96);
const DIM: (u8, u8, u8) = (92, 108, 130);
const RULE: (u8, u8, u8) = (58, 92, 118);

/// Below this many columns the panel drops to a single status line.
const MIN_COLS: usize = 46;
/// Below this many rows there is no space for the lower panel.
const MIN_ROWS: usize = 12;

pub struct Readout<'a> {
    pub ship: &'a Ship,
    pub fps: f32,
    pub stars: usize,
    pub paused: bool,
}

pub fn draw(screen: &mut Screen, r: &Readout) {
    let (cols, rows) = screen.dims();

    if cols < MIN_COLS || rows < MIN_ROWS {
        draw_compact(screen, r, cols, rows);
        return;
    }

    draw_reticle(screen, cols, rows);
    draw_nav_panel(screen, r);
    draw_status_line(screen, r, cols, rows);
    draw_throttle(screen, r, rows);
    draw_hints(screen, cols, rows);
}

/// Everything the panel says, squeezed onto one line for a tiny window.
fn draw_compact(screen: &mut Screen, r: &Readout, cols: usize, rows: usize) {
    let line = format!("{} {}", velocity_text(r.ship), warp_text(r.ship));
    screen.overlay(0, 0, &truncate(&line, cols), VALUE);
    if rows > 1 {
        let thr = format!("THR {:>3.0}%", r.ship.throttle * 100.0);
        screen.overlay(0, rows - 1, &truncate(&thr, cols), ACCENT);
    }
}

/// Corner brackets around the vanishing point — where you are actually going.
fn draw_reticle(screen: &mut Screen, cols: usize, rows: usize) {
    let (cx, cy) = (cols / 2, rows / 2);
    let (dx, dy) = (9usize, 3usize);
    if cx < dx + 1 || cy < dy + 1 || cx + dx >= cols || cy + dy >= rows {
        return;
    }
    for (x, y, ch) in [
        (cx - dx, cy - dy, '\u{250C}'),
        (cx + dx, cy - dy, '\u{2510}'),
        (cx - dx, cy + dy, '\u{2514}'),
        (cx + dx, cy + dy, '\u{2518}'),
    ] {
        screen.overlay(x, y, &ch.to_string(), RULE);
    }
}

fn draw_nav_panel(screen: &mut Screen, r: &Readout) {
    let ship = r.ship;
    let rows = [
        ("VELOCITY", velocity_text(ship), VALUE),
        ("WARP", warp_text(ship), if ship.warp_engaged { ACCENT } else { DIM }),
        ("DISTANCE", format!("{} ly", distance_text(ship.distance_ly)), VALUE),
        ("HEADING", heading_text(ship), VALUE),
    ];

    screen.overlay(2, 1, "\u{250C} NAV", LABEL);
    for (i, (label, value, color)) in rows.iter().enumerate() {
        let row = 2 + i;
        screen.overlay(2, row, "\u{2502}", RULE);
        screen.overlay(4, row, label, LABEL);
        screen.overlay(15, row, value, *color);
    }
    screen.overlay(2, 2 + rows.len(), "\u{2514}", RULE);
}

/// The headline: what the drive is doing right now.
fn draw_status_line(screen: &mut Screen, r: &Readout, cols: usize, rows: usize) {
    let ship = r.ship;
    let (text, color) = if r.paused {
        ("\u{2016} ALL STOP \u{2016}".to_string(), WARN)
    } else if ship.warp_engaged {
        (
            format!("\u{27E8} WARP DRIVE ENGAGED \u{2014} FACTOR {:.2} \u{27E9}", ship.warp_factor()),
            // Flash the banner along with the engage transient.
            if ship.flash > 0.35 { (255, 255, 255) } else { ACCENT },
        )
    } else if ship.speed > 1.0 {
        ("\u{27E8} IMPULSE \u{27E9}".to_string(), LABEL)
    } else {
        ("\u{27E8} STATION KEEPING \u{27E9}".to_string(), DIM)
    };

    let text = truncate(&text, cols);
    let col = cols.saturating_sub(text.chars().count()) / 2;
    screen.overlay(col, rows.saturating_sub(3), &text, color);

    // Right-hand corner: how hard the machine is working.
    let stats = format!("STARS {:>5}   {:>3.0} FPS", r.stars, r.fps);
    let col = cols.saturating_sub(stats.chars().count() + 2);
    screen.overlay(col, 1, &stats, DIM);
}

fn draw_throttle(screen: &mut Screen, r: &Readout, rows: usize) {
    const WIDTH: usize = 16;
    let filled = (r.ship.throttle * WIDTH as f32).round() as usize;
    let bar: String = (0..WIDTH)
        .map(|i| if i < filled { '\u{2588}' } else { '\u{2591}' })
        .collect();
    let color = if r.ship.warp_engaged { ACCENT } else { LABEL };

    let row = rows.saturating_sub(2);
    screen.overlay(2, row, "THR", LABEL);
    screen.overlay(6, row, &bar, color);
    screen.overlay(6 + WIDTH + 1, row, &format!("{:>3.0}%", r.ship.throttle * 100.0), VALUE);
}

fn draw_hints(screen: &mut Screen, cols: usize, rows: usize) {
    let hint = "SPACE warp  \u{2191}\u{2193} throttle  \u{2190}\u{2192}IK steer  P pause  R reset  Q quit";
    if hint.chars().count() + 2 > cols {
        return;
    }
    let col = cols.saturating_sub(hint.chars().count() + 2);
    screen.overlay(col, rows.saturating_sub(2), hint, DIM);
}

fn velocity_text(ship: &Ship) -> String {
    let v = ship.velocity_c();
    if v < 1.0 {
        format!("{v:.3} c")
    } else if v < 100.0 {
        format!("{v:.1} c")
    } else {
        format!("{v:.0} c")
    }
}

fn warp_text(ship: &Ship) -> String {
    let w = ship.warp_factor();
    if w <= 0.0 {
        "\u{2014}\u{2014}\u{2014}".to_string()
    } else {
        format!("FACTOR {w:.2}")
    }
}

fn distance_text(ly: f64) -> String {
    if ly < 1.0 {
        format!("{ly:.3}")
    } else if ly < 1000.0 {
        format!("{ly:.2}")
    } else {
        format!("{ly:.0}")
    }
}

fn heading_text(ship: &Ship) -> String {
    let deg = ship.heading.to_degrees().rem_euclid(360.0);
    let pitch = ship.pitch.to_degrees();
    format!("{deg:>5.1}\u{b0} / {pitch:>+5.1}\u{b0}")
}

/// Character-aware truncation — the panel is full of multi-byte glyphs.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::ColorMode;

    fn readout(ship: &Ship) -> Readout<'_> {
        Readout { ship, fps: 60.0, stars: 4000, paused: false }
    }

    fn blank(cols: usize, rows: usize) -> Screen {
        let mut screen = Screen::new(cols, rows, ColorMode::Truecolor);
        screen.compose(&vec![[0, 0, 0]; cols * rows * 2]);
        screen
    }

    #[test]
    fn the_panel_fits_a_range_of_terminal_sizes() {
        let ship = Ship::new();
        // Including sizes far below anything usable: the panel must degrade,
        // never panic or write outside the grid.
        for (cols, rows) in [(1, 1), (2, 3), (20, 8), (46, 12), (80, 24), (200, 60), (400, 120)] {
            let mut screen = blank(cols, rows);
            draw(&mut screen, &readout(&ship));
            assert_eq!(screen.dims(), (cols, rows));
        }
    }

    #[test]
    fn the_panel_renders_at_every_point_in_the_flight_envelope() {
        let mut ship = Ship::new();
        let mut screen = blank(100, 30);
        draw(&mut screen, &readout(&ship));

        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..1200 {
            ship.update(1.0 / 60.0);
            draw(&mut screen, &readout(&ship));
        }
        ship.toggle_warp();
        for _ in 0..1200 {
            ship.update(1.0 / 60.0);
            draw(&mut screen, &readout(&ship));
        }
    }

    #[test]
    fn readouts_reflect_the_flight_state() {
        let mut ship = Ship::new();
        ship.throttle = 0.0;
        assert!(velocity_text(&ship).ends_with(" c"));
        assert_eq!(warp_text(&ship), "\u{2014}\u{2014}\u{2014}");

        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..1200 {
            ship.update(1.0 / 60.0);
        }
        assert!(warp_text(&ship).starts_with("FACTOR 9"), "{}", warp_text(&ship));
        assert!(ship.distance_ly > 0.0);
    }

    #[test]
    fn distance_and_heading_formatting_stays_compact() {
        for ly in [0.0, 0.5, 12.5, 4200.0, 1.0e6] {
            assert!(distance_text(ly).len() <= 8, "{}", distance_text(ly));
        }
        let mut ship = Ship::new();
        ship.heading = -1.0; // must wrap into 0..360 rather than print negative
        assert!(!heading_text(&ship).starts_with('-'));
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        let text = "\u{27E8} WARP \u{27E9}";
        assert_eq!(truncate(text, 3).chars().count(), 3);
        assert_eq!(truncate(text, 999), text);
    }

    #[test]
    fn the_status_banner_reports_the_drive_state() {
        // Over a black frame the shadow colour never changes, so each word
        // lands in the output as one contiguous run.
        let flushed = |ship: &Ship, paused: bool| {
            let mut screen = blank(120, 34);
            draw(&mut screen, &Readout { ship, fps: 60.0, stars: 900, paused });
            let mut out = Vec::new();
            screen.flush(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };

        let mut ship = Ship::new();
        ship.throttle = 0.0;
        ship.speed = 0.0;
        assert!(flushed(&ship, false).contains("KEEPING"), "expected station keeping");
        assert!(flushed(&ship, true).contains("STOP"), "expected the paused banner");

        ship.speed = 20.0;
        assert!(flushed(&ship, false).contains("IMPULSE"), "expected impulse");

        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..900 {
            ship.update(1.0 / 60.0);
        }
        let text = flushed(&ship, false);
        assert!(text.contains("ENGAGED"), "expected the warp banner");
        assert!(text.contains("FACTOR"), "the banner should quote a warp factor");
    }

    #[test]
    fn the_panel_writes_something_visible() {
        let ship = Ship::new();
        let mut screen = blank(100, 30);
        draw(&mut screen, &readout(&ship));
        let mut out = Vec::new();
        screen.flush(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("VELOCITY") && text.contains("THR"));
    }
}
