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

/// Where the throttle readout starts, and how wide its bar is.
const THROTTLE_COL: usize = 2;
const THROTTLE_BAR: usize = 16;

/// Control hints, widest first: the first that fits is the one drawn, so a
/// narrow window sheds detail rather than losing the line entirely.
const HINTS: [&str; 3] = [
    "SPACE warp  \u{2191}\u{2193} throttle  WASD steer  QE roll  P pause  R reset  ESC quit",
    "SPACE warp  \u{2191}\u{2193} throttle  WASD steer  QE roll  ESC quit",
    "SPACE warp  WASD steer  QE roll  ESC quit",
];

/// The three instrument rows, counted up from the bottom. Each owns its row
/// outright — the hints used to share the throttle's, right-aligned, and
/// quietly overwrote it on any terminal narrow enough for the two to meet.
fn status_row(rows: usize) -> usize {
    rows.saturating_sub(3)
}
fn throttle_row(rows: usize) -> usize {
    rows.saturating_sub(2)
}
fn hint_row(rows: usize) -> usize {
    rows.saturating_sub(1)
}

pub struct Readout<'a> {
    pub ship: &'a Ship,
    pub fps: f32,
    pub stars: usize,
    pub paused: bool,
    /// Whether to show the control hints. A screensaver quits on any key, so
    /// listing which keys do what would be a lie.
    pub hints: bool,
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
    if r.hints {
        draw_hints(screen, cols, rows);
    }
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
        // A mark, not a readout: it sits in the scene, so it lightens what is
        // behind it instead of casting the panel's shadow. These brackets land
        // inside the tunnel glare, where a shadow would read as four dark
        // notches punched into the brightest part of the frame.
        screen.overlay_mark(x, y, &ch.to_string(), RULE);
    }
}

fn draw_nav_panel(screen: &mut Screen, r: &Readout) {
    let ship = r.ship;
    let rows = [
        ("VELOCITY", velocity_text(ship), VALUE),
        (
            "WARP",
            warp_text(ship),
            if ship.warp_engaged { ACCENT } else { DIM },
        ),
        (
            "DISTANCE",
            format!("{} ly", distance_text(ship.distance_ly)),
            VALUE,
        ),
        ("HEADING", heading_text(ship), VALUE),
        // A roll against a starfield is only visible while it is happening —
        // the sky has no up — so the number is the only thing that says where
        // the ship ended up.
        ("ROLL", roll_text(ship), VALUE),
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
            format!(
                "\u{27E8} WARP DRIVE ENGAGED \u{2014} FACTOR {:.2} \u{27E9}",
                ship.warp_factor()
            ),
            // Flash the banner along with the engage transient.
            if ship.flash > 0.35 {
                (255, 255, 255)
            } else {
                ACCENT
            },
        )
    } else if ship.speed > 1.0 {
        ("\u{27E8} IMPULSE \u{27E9}".to_string(), LABEL)
    } else {
        ("\u{27E8} STATION KEEPING \u{27E9}".to_string(), DIM)
    };

    let text = truncate(&text, cols);
    let col = cols.saturating_sub(text.chars().count()) / 2;
    screen.overlay(col, status_row(rows), &text, color);

    // Right-hand corner: how hard the machine is working.
    let stats = format!("STARS {:>5}   {:>3.0} FPS", r.stars, r.fps);
    let col = cols.saturating_sub(stats.chars().count() + 2);
    screen.overlay(col, 1, &stats, DIM);
}

fn draw_throttle(screen: &mut Screen, r: &Readout, rows: usize) {
    let filled = (r.ship.throttle * THROTTLE_BAR as f32).round() as usize;
    let bar: String = (0..THROTTLE_BAR)
        .map(|i| if i < filled { '\u{2588}' } else { '\u{2591}' })
        .collect();
    let color = if r.ship.warp_engaged { ACCENT } else { LABEL };

    let row = throttle_row(rows);
    let bar_col = THROTTLE_COL + 4;
    screen.overlay(THROTTLE_COL, row, "THR", LABEL);
    screen.overlay(bar_col, row, &bar, color);
    let pct = format!("{:>3.0}%", r.ship.throttle * 100.0);
    screen.overlay(bar_col + THROTTLE_BAR + 1, row, &pct, VALUE);
}

fn draw_hints(screen: &mut Screen, cols: usize, rows: usize) {
    let Some(hint) = HINTS.iter().find(|h| h.chars().count() + 2 <= cols) else {
        return;
    };
    let col = cols - (hint.chars().count() + 2);
    screen.overlay(col, hint_row(rows), hint, DIM);
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

/// Signed, the way a bank indicator reads: starboard positive, and never more
/// than half a turn away from level in either direction.
fn roll_text(ship: &Ship) -> String {
    format!("{:>+6.1}\u{b0}", ship.roll.to_degrees())
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
    use std::f32::consts::PI;

    fn readout(ship: &Ship) -> Readout<'_> {
        Readout {
            ship,
            fps: 60.0,
            stars: 4000,
            paused: false,
            hints: true,
        }
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
        for (cols, rows) in [
            (1, 1),
            (2, 3),
            (20, 8),
            (46, 12),
            (80, 24),
            (200, 60),
            (400, 120),
        ] {
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
        assert!(
            warp_text(&ship).starts_with("FACTOR 9"),
            "{}",
            warp_text(&ship)
        );
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
    fn the_roll_readout_is_signed_and_stays_in_its_column() {
        let mut ship = Ship::new();
        assert_eq!(roll_text(&ship).trim(), "+0.0\u{b0}");
        for roll in [-PI, -1.0, 0.0, 1.0, PI - 0.001] {
            ship.roll = roll;
            let text = roll_text(&ship);
            assert_eq!(text.chars().count(), 7, "{text:?} is the wrong width");
            assert!(
                text.contains('+') || text.contains('-'),
                "{text:?} should carry a sign"
            );
        }
        ship.roll = -1.0;
        assert!(roll_text(&ship).contains('-'), "port should read negative");
        ship.roll = 1.0;
        assert!(roll_text(&ship).contains('+'), "starboard should read plus");
    }

    #[test]
    fn the_panel_reports_the_roll_it_was_flown_to() {
        let mut screen = blank(120, 34);
        let mut ship = Ship::new();
        for _ in 0..40 {
            ship.nudge_roll(1.0);
            ship.update(1.0 / 60.0);
        }
        draw(&mut screen, &readout(&ship));
        let mut out = Vec::new();
        screen.flush(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("ROLL"), "the panel lost its roll row");
        // The sky itself gives a static roll away not at all, so the number is
        // the only report there is: it has to be the one that was flown.
        let degrees = format!("{:.1}", ship.roll.to_degrees());
        assert!(text.contains(&degrees), "expected {degrees} in the panel");
    }

    #[test]
    fn the_hints_name_the_keys_that_exist() {
        // The hints are the only place the controls are written down, so a key
        // that has moved must move here too — and `q` no longer quits.
        for hint in HINTS {
            assert!(hint.contains("WASD") && hint.contains("QE"), "{hint:?}");
            assert!(hint.contains("ESC quit"), "{hint:?}");
            assert!(!hint.contains("Q quit"), "{hint:?}");
            assert!(!hint.contains("IK"), "{hint:?}");
        }
        // Widest first, so the first that fits is the most detailed that fits.
        let widths: Vec<usize> = HINTS.iter().map(|h| h.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] > w[1]),
            "hints are out of order: {widths:?}"
        );
        assert!(
            widths.last().is_some_and(|w| w + 2 <= MIN_COLS),
            "the shortest hint does not fit the narrowest panel: {widths:?}"
        );
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        let text = "\u{27E8} WARP \u{27E9}";
        assert_eq!(truncate(text, 3).chars().count(), 3);
        assert_eq!(truncate(text, 999), text);
    }

    #[test]
    fn the_reticle_never_darkens_the_frame_behind_it() {
        // Regression: the reticle went through the panel's text path, so it
        // shadowed its own cells — and it sits inside the tunnel glare, so at
        // warp it punched four dark notches into the brightest part of the
        // view. Over a uniformly lit frame no reticle cell may come out
        // dimmer than what was composed.
        let (cols, rows) = (120usize, 34usize);
        let lit = [200u8, 210, 230];
        let mut screen = Screen::new(cols, rows, ColorMode::Truecolor);
        screen.compose(&vec![lit; cols * rows * 2]);

        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..900 {
            ship.update(1.0 / 60.0);
        }
        draw(&mut screen, &readout(&ship));

        let (cx, cy) = (cols / 2, rows / 2);
        for (x, y) in [
            (cx - 9, cy - 3),
            (cx + 9, cy - 3),
            (cx - 9, cy + 3),
            (cx + 9, cy + 3),
        ] {
            let (fg, bg) = screen.cell_colors(x, y);
            let (fg, bg) = (fg.expect("truecolor cell"), bg.expect("truecolor cell"));
            assert_eq!(
                bg,
                (lit[0], lit[1], lit[2]),
                "reticle at ({x},{y}) dimmed its backdrop"
            );
            for (got, under) in [(fg.0, lit[0]), (fg.1, lit[1]), (fg.2, lit[2])] {
                assert!(
                    got >= under,
                    "reticle at ({x},{y}) dimmed a channel: {got} < {under}"
                );
            }
        }
    }

    #[test]
    fn the_hints_never_eat_the_throttle_readout() {
        // Regression: the hints were right-aligned onto the throttle's own row
        // and only checked that they fit the terminal, not that they cleared
        // the bar. At every width from 63 to 89 — the default 80 among them —
        // they overwrote the end of the bar and the whole percentage:
        //
        //   THR ███░░░░░░░░░░SPACE warp  ↑↓ throttle  ←→IK steer  P pause ...
        let mut ship = Ship::new();
        ship.throttle = 0.5;
        let rows = 24;

        for cols in MIN_COLS..=200 {
            let mut screen = blank(cols, rows);
            draw(&mut screen, &readout(&ship));
            let row = screen.row_text(throttle_row(rows));
            let cells: Vec<char> = row.chars().collect();

            let bar_col = THROTTLE_COL + 4;
            for (i, ch) in cells[bar_col..bar_col + THROTTLE_BAR].iter().enumerate() {
                assert!(
                    *ch == '\u{2588}' || *ch == '\u{2591}',
                    "bar cell {i} became {ch:?} at {cols} columns"
                );
            }
            assert!(
                row.contains("50%"),
                "the percentage went missing at {cols} columns"
            );
            assert!(
                !row.contains("pause"),
                "hints landed on the throttle row at {cols} columns"
            );
        }
    }

    #[test]
    fn the_hints_shed_detail_rather_than_vanishing() {
        // The full hint needs 63 columns. Below that a shorter one still has
        // to name the keys that matter, down to the panel's own minimum.
        let ship = Ship::new();
        let rows = 24;
        for cols in MIN_COLS..=120 {
            let mut screen = blank(cols, rows);
            draw(&mut screen, &readout(&ship));
            let hints = screen.row_text(hint_row(rows));
            assert!(
                hints.contains("warp") && hints.contains("quit"),
                "no usable hint at {cols} columns: {hints:?}"
            );
            // Whatever was chosen has to have fitted: nothing may be clipped
            // off the right-hand edge.
            assert!(
                !hints.ends_with("quit"),
                "the hint ran into the last column at {cols} columns"
            );
        }
    }

    #[test]
    fn control_hints_can_be_suppressed() {
        // In screensaver mode any key quits, so advertising "SPACE warp" and
        // friends would be telling the viewer something untrue.
        let render = |hints: bool| {
            let mut screen = blank(120, 34);
            let ship = Ship::new();
            draw(
                &mut screen,
                &Readout {
                    ship: &ship,
                    fps: 60.0,
                    stars: 900,
                    paused: false,
                    hints,
                },
            );
            let mut out = Vec::new();
            screen.flush(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };
        assert!(
            render(true).contains("pause"),
            "hints should be there by default"
        );
        let bare = render(false);
        assert!(!bare.contains("pause"), "hints should be gone");
        // Everything else still draws.
        assert!(bare.contains("VELOCITY") && bare.contains("THR"));
    }

    #[test]
    fn the_status_banner_reports_the_drive_state() {
        // Over a black frame the shadow colour never changes, so each word
        // lands in the output as one contiguous run.
        let flushed = |ship: &Ship, paused: bool| {
            let mut screen = blank(120, 34);
            draw(
                &mut screen,
                &Readout {
                    ship,
                    fps: 60.0,
                    stars: 900,
                    paused,
                    hints: true,
                },
            );
            let mut out = Vec::new();
            screen.flush(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };

        let mut ship = Ship::new();
        ship.throttle = 0.0;
        ship.speed = 0.0;
        assert!(
            flushed(&ship, false).contains("KEEPING"),
            "expected station keeping"
        );
        assert!(
            flushed(&ship, true).contains("STOP"),
            "expected the paused banner"
        );

        ship.speed = 20.0;
        assert!(
            flushed(&ship, false).contains("IMPULSE"),
            "expected impulse"
        );

        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..900 {
            ship.update(1.0 / 60.0);
        }
        let text = flushed(&ship, false);
        assert!(text.contains("ENGAGED"), "expected the warp banner");
        assert!(
            text.contains("FACTOR"),
            "the banner should quote a warp factor"
        );
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
