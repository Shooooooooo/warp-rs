//! The instrument panel.

use crate::ship::Ship;
use crate::term::{truncate, ColorMode, Screen};
use crate::view::ViewMode;

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
    "SPACE warp  \u{2191}\u{2193} throttle  WASD steer  QE roll  C view  M ships  P pause  R reset  ESC quit",
    "SPACE warp  \u{2191}\u{2193} throttle  WASD steer  QE roll  ESC quit",
    "SPACE warp  WASD steer  QE roll  ESC quit",
];

/// The same hints without the arrows, for a terminal being drawn in ASCII.
const ASCII_HINTS: [&str; 3] = [
    "SPACE warp  UP/DN throttle  WASD steer  QE roll  C view  M ships  P pause  R reset  ESC quit",
    "SPACE warp  UP/DN throttle  WASD steer  QE roll  ESC quit",
    "SPACE warp  WASD steer  QE roll  ESC quit",
];

/// And the same again for the view from outside, where those same six keys fly
/// the camera instead of the ship.
const SIDE_HINTS: [&str; 3] = [
    "SPACE warp  \u{2191}\u{2193} throttle  WASDQE cam  [] zoom  C view  M ships  P pause  R reset  ESC quit",
    "SPACE warp  \u{2191}\u{2193} throttle  WASDQE cam  C view  ESC quit",
    "SPACE warp  WASDQE cam  C view  ESC quit",
];

const ASCII_SIDE_HINTS: [&str; 3] = [
    "SPACE warp  UP/DN throttle  WASDQE cam  [] zoom  C view  M ships  P pause  R reset  ESC quit",
    "SPACE warp  UP/DN throttle  WASDQE cam  C view  ESC quit",
    "SPACE warp  WASDQE cam  C view  ESC quit",
];

/// The characters the panel is drawn from.
struct Glyphs {
    /// The nav panel's frame: the corner it opens on, the corner it closes on,
    /// and the rule down its left-hand side.
    frame_top: char,
    frame_bottom: char,
    vrule: char,
    /// The reticle's four corners, clockwise from the top left.
    reticle: [char; 4],
    /// Wraps the status banner.
    open: char,
    close: char,
    /// Inside the warp banner, and — three of it — the placeholder a cold drive
    /// leaves in the one-line panel a narrow window gets.
    dash: char,
    /// Wraps the paused banner.
    stop: char,
    bar_full: char,
    bar_empty: char,
    degree: char,
    hints: &'static [&'static str; 3],
    side_hints: &'static [&'static str; 3],
}

impl Glyphs {
    const UNICODE: Glyphs = Glyphs {
        frame_top: '\u{250C}',
        frame_bottom: '\u{2514}',
        vrule: '\u{2502}',
        reticle: ['\u{250C}', '\u{2510}', '\u{2514}', '\u{2518}'],
        open: '\u{27E8}',
        close: '\u{27E9}',
        dash: '\u{2014}',
        stop: '\u{2016}',
        bar_full: '\u{2588}',
        bar_empty: '\u{2591}',
        degree: '\u{B0}',
        hints: &HINTS,
        side_hints: &SIDE_HINTS,
    };

    /// Chosen against [`crate::term`]'s brightness ramp as much as against the
    /// alphabet: in this mode the panel has no colour to set it apart from the
    /// starfield, so a glyph the ramp also draws — `#`, `.`, `+`, `*` — reads
    /// as a bright star rather than as an instrument.
    const ASCII: Glyphs = Glyphs {
        frame_top: '+',
        frame_bottom: '+',
        vrule: '|',
        reticle: ['[', ']', '[', ']'],
        open: '<',
        close: '>',
        dash: '-',
        stop: '|',
        bar_full: '|',
        bar_empty: '_',
        // Nothing in ASCII means "degrees".
        degree: '*',
        hints: &ASCII_HINTS,
        side_hints: &ASCII_SIDE_HINTS,
    };

    fn for_mode(mode: ColorMode) -> &'static Glyphs {
        match mode {
            ColorMode::Ascii => &Self::ASCII,
            ColorMode::Truecolor | ColorMode::Ansi256 => &Self::UNICODE,
        }
    }

    /// The hints for the view being flown: they name different keys, because in
    /// the two views different keys do anything.
    fn hints_for(&self, view: ViewMode) -> &'static [&'static str; 3] {
        match view {
            ViewMode::Cockpit => self.hints,
            ViewMode::Side => self.side_hints,
        }
    }
}

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
    /// How faint the faintest star the sky holds is, which is what was asked
    /// for and what `+` and `-` move. It is shown because a key that changes a
    /// number nobody can see is a key nobody believes. The count it comes to
    /// used to be shown beside it and is not any more: of the two, this is the
    /// one that was asked for rather than arrived at.
    pub magnitude: f32,
    pub paused: bool,
    /// Whether to put any glass in front of the frame at all.
    pub panel: bool,
    /// Which camera the frame under the glass was drawn with. The panel is the
    /// same either way except where the view itself has moved something: the
    /// reticle marks where the nose is pointed, which is only a thing you can
    /// see from behind it, and the ship's name is only worth reading when you
    /// are looking at the ship.
    pub view: ViewMode,
    /// What the ship is, for the row that names it.
    pub model: &'a str,
}

pub fn draw(screen: &mut Screen, r: &Readout) {
    // Above the size check rather than below it: the compact layout is this
    // same panel on a window with no room for it, so a flight nobody is flying
    // draws neither.
    if !r.panel {
        return;
    }
    let (cols, rows) = screen.dims();
    let g = Glyphs::for_mode(screen.color_mode());

    if cols < MIN_COLS || rows < MIN_ROWS {
        draw_compact(screen, r, cols, rows, g);
        return;
    }

    if r.view == ViewMode::Cockpit {
        draw_reticle(screen, cols, rows, g, nav_bottom_row(r.view));
    }
    draw_nav_panel(screen, r, g);
    draw_status_line(screen, r, cols, rows, g);
    draw_throttle(screen, r, rows, g);
    draw_hints(screen, cols, rows, g, r.view);
}

/// Everything the panel says, squeezed onto one line for a tiny window.
fn draw_compact(screen: &mut Screen, r: &Readout, cols: usize, rows: usize, g: &Glyphs) {
    let line = format!("{} {}", velocity_text(r.ship), warp_text(r.ship, g));
    screen.overlay(0, 0, &truncate(&line, cols), VALUE);
    if rows > 1 {
        let thr = format!("THR {:>3.0}%", r.ship.throttle * 100.0);
        screen.overlay(0, rows - 1, &truncate(&thr, cols), ACCENT);
    }
}

/// How many rows the NAV panel has, and where its closing rule therefore lands.
fn nav_rows(view: ViewMode) -> usize {
    // VELOCITY, DISTANCE, HEADING and ROLL, and SHIP only from outside.
    4 + usize::from(view == ViewMode::Side)
}

fn nav_bottom_row(view: ViewMode) -> usize {
    2 + nav_rows(view)
}

/// Corner brackets around the vanishing point — where you are actually going.
fn draw_reticle(screen: &mut Screen, cols: usize, rows: usize, g: &Glyphs, nav_bottom: usize) {
    let (cx, cy) = (cols / 2, rows / 2);
    let (dx, dy) = (9usize, 3usize);
    if cx < dx + 1 || cy < dy + 1 || cx + dx >= cols || cy + dy >= rows {
        return;
    }
    // And it clears the instrument panel, which is the same kind of refusal as
    // the one above and was missing for as long as there has been a reticle.
    if cy - dy <= nav_bottom {
        return;
    }
    for (x, y, ch) in [
        (cx - dx, cy - dy, g.reticle[0]),
        (cx + dx, cy - dy, g.reticle[1]),
        (cx - dx, cy + dy, g.reticle[2]),
        (cx + dx, cy + dy, g.reticle[3]),
    ] {
        // A mark, not a readout: it sits in the scene, so it lightens what is
        // behind it instead of casting the panel's shadow.
        screen.overlay_mark(x, y, &ch.to_string(), RULE);
    }
}

fn draw_nav_panel(screen: &mut Screen, r: &Readout, g: &Glyphs) {
    let ship = r.ship;
    let mut rows = vec![
        ("VELOCITY", velocity_text(ship), VALUE),
        (
            "DISTANCE",
            format!("{} ly", distance_text(ship.distance_ly)),
            VALUE,
        ),
        ("HEADING", heading_text(ship, g), VALUE),
        // A roll against a starfield is only visible while it is happening —
        // the sky has no up — so the number is the only thing that says where
        // the ship ended up.
        ("ROLL", roll_text(ship, g), VALUE),
    ];
    // Only from outside: in the cockpit you are sitting in it, and a row that
    // never changes is a row the panel does not need.
    if r.view == ViewMode::Side {
        rows.push(("SHIP", r.model.to_uppercase(), ACCENT));
    }

    // The one place the row list and `nav_rows` could drift apart, so it is
    // also the place they are held together.
    debug_assert_eq!(
        rows.len(),
        nav_rows(r.view),
        "the NAV panel grew a row that `nav_rows` does not know about"
    );

    screen.overlay(2, 1, &format!("{} NAV", g.frame_top), LABEL);
    for (i, (label, value, color)) in rows.iter().enumerate() {
        let row = 2 + i;
        screen.overlay(2, row, &g.vrule.to_string(), RULE);
        screen.overlay(4, row, label, LABEL);
        screen.overlay(15, row, value, *color);
    }
    screen.overlay(2, nav_bottom_row(r.view), &g.frame_bottom.to_string(), RULE);
}

/// The headline: what the drive is doing right now.
fn draw_status_line(screen: &mut Screen, r: &Readout, cols: usize, rows: usize, g: &Glyphs) {
    let ship = r.ship;
    let (open, close, stop) = (g.open, g.close, g.stop);
    let (text, color) = if r.paused {
        (format!("{stop} ALL STOP {stop}"), WARN)
    } else if ship.warp_engaged {
        (
            format!(
                "{open} WARP DRIVE ENGAGED {} FACTOR {:.2} {close}",
                g.dash,
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
        (format!("{open} IMPULSE {close}"), LABEL)
    } else {
        (format!("{open} STATION KEEPING {close}"), DIM)
    };

    let text = truncate(&text, cols);
    let col = cols.saturating_sub(text.chars().count()) / 2;
    screen.overlay(col, status_row(rows), &text, color);

    // Right-hand corner: what was asked of the sky, and how hard the machine is
    // working for it.
    let stats = format!("MAG {:>4.1}  {:>3.0} FPS", r.magnitude, r.fps);
    let col = cols.saturating_sub(stats.chars().count() + 2);
    screen.overlay(col, 1, &stats, DIM);
}

fn draw_throttle(screen: &mut Screen, r: &Readout, rows: usize, g: &Glyphs) {
    let filled = (r.ship.throttle * THROTTLE_BAR as f32).round() as usize;
    let bar: String = (0..THROTTLE_BAR)
        .map(|i| if i < filled { g.bar_full } else { g.bar_empty })
        .collect();
    let color = if r.ship.warp_engaged { ACCENT } else { LABEL };

    let row = throttle_row(rows);
    let bar_col = THROTTLE_COL + 4;
    screen.overlay(THROTTLE_COL, row, "THR", LABEL);
    screen.overlay(bar_col, row, &bar, color);
    let pct = format!("{:>3.0}%", r.ship.throttle * 100.0);
    screen.overlay(bar_col + THROTTLE_BAR + 1, row, &pct, VALUE);
}

fn draw_hints(screen: &mut Screen, cols: usize, rows: usize, g: &Glyphs, view: ViewMode) {
    let Some(hint) = g
        .hints_for(view)
        .iter()
        .find(|h| h.chars().count() + 2 <= cols)
    else {
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

fn warp_text(ship: &Ship, g: &Glyphs) -> String {
    let w = ship.warp_factor();
    if w <= 0.0 {
        String::from_iter([g.dash; 3])
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

fn heading_text(ship: &Ship, g: &Glyphs) -> String {
    let deg = ship.heading.to_degrees().rem_euclid(360.0);
    let pitch = ship.pitch.to_degrees();
    let d = g.degree;
    format!("{deg:>5.1}{d} / {pitch:>+5.1}{d}")
}

/// Signed, the way a bank indicator reads: starboard positive, and never more
/// than half a turn away from level in either direction.
fn roll_text(ship: &Ship, g: &Glyphs) -> String {
    let d = g.degree;
    format!("{:>+6.1}{d}", ship.roll.to_degrees())
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
            magnitude: 6.0,
            paused: false,
            panel: true,
            view: ViewMode::Cockpit,
            model: "normandy",
        }
    }

    fn blank(cols: usize, rows: usize) -> Screen {
        blank_in(cols, rows, ColorMode::Truecolor)
    }

    fn blank_in(cols: usize, rows: usize, mode: ColorMode) -> Screen {
        let mut screen = Screen::new(cols, rows, mode);
        screen.compose(&vec![[0, 0, 0]; cols * rows * 2]);
        screen
    }

    /// The glyph set the readout helpers were written against.
    fn uni() -> &'static Glyphs {
        &Glyphs::UNICODE
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
            // `draw` never resizes, so the row widths below are the real check:
            // the panel has to degrade, not run off the end of a line.
            assert_eq!(screen.dims(), (cols, rows));
            for row in 0..rows {
                assert_eq!(
                    screen.row_text(row).chars().count(),
                    cols,
                    "row {row} of {cols}x{rows} is not the width it was given"
                );
            }
        }
    }

    #[test]
    fn the_panel_renders_at_every_point_in_the_flight_envelope() {
        let mut ship = Ship::new();
        let mut screen = blank(100, 30);
        let drawn = |screen: &Screen| {
            (0..30)
                .flat_map(|row| screen.row_text(row).chars().collect::<Vec<_>>())
                .filter(|ch| !ch.is_whitespace() && *ch != '\u{2580}')
                .count()
        };

        draw(&mut screen, &readout(&ship));
        assert!(drawn(&screen) > 20, "the panel drew nothing at rest");

        ship.throttle = 1.0;
        ship.toggle_warp();
        for frame in 0..1200 {
            ship.update(1.0 / 60.0);
            draw(&mut screen, &readout(&ship));
            assert!(
                drawn(&screen) > 20,
                "the panel emptied at frame {frame} on the way up, at {:.1} c",
                ship.velocity_c()
            );
        }
        ship.toggle_warp();
        for frame in 0..1200 {
            ship.update(1.0 / 60.0);
            draw(&mut screen, &readout(&ship));
            assert!(
                drawn(&screen) > 20,
                "the panel emptied at frame {frame} on the way down, at {:.1} c",
                ship.velocity_c()
            );
        }
    }

    #[test]
    fn readouts_reflect_the_flight_state() {
        let mut ship = Ship::new();
        ship.throttle = 0.0;
        assert!(velocity_text(&ship).ends_with(" c"));
        assert_eq!(warp_text(&ship, uni()), "\u{2014}\u{2014}\u{2014}");

        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..1200 {
            ship.update(1.0 / 60.0);
        }
        assert!(
            warp_text(&ship, uni()).starts_with("FACTOR 9"),
            "{}",
            warp_text(&ship, uni())
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
        assert!(!heading_text(&ship, uni()).starts_with('-'));
    }

    #[test]
    fn the_roll_readout_is_signed_and_stays_in_its_column() {
        let mut ship = Ship::new();
        assert_eq!(roll_text(&ship, uni()).trim(), "+0.0\u{b0}");
        for roll in [-PI, -1.0, 0.0, 1.0, PI - 0.001] {
            ship.roll = roll;
            let text = roll_text(&ship, uni());
            assert_eq!(text.chars().count(), 7, "{text:?} is the wrong width");
            assert!(
                text.contains('+') || text.contains('-'),
                "{text:?} should carry a sign"
            );
        }
        ship.roll = -1.0;
        assert!(
            roll_text(&ship, uni()).contains('-'),
            "port should read negative"
        );
        ship.roll = 1.0;
        assert!(
            roll_text(&ship, uni()).contains('+'),
            "starboard should read plus"
        );
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
        for set in [&HINTS, &ASCII_HINTS, &SIDE_HINTS, &ASCII_SIDE_HINTS] {
            for hint in set {
                assert!(hint.contains("QE"), "{hint:?} does not name the roll");
                assert!(hint.contains("ESC quit"), "{hint:?}");
                assert!(!hint.contains("Q quit"), "{hint:?}");
                assert!(!hint.contains("IK"), "{hint:?}");
            }
            // Widest first, so the first that fits is the most detailed one.
            let widths: Vec<usize> = set.iter().map(|h| h.chars().count()).collect();
            assert!(
                widths.windows(2).all(|w| w[0] > w[1]),
                "hints are out of order: {widths:?}"
            );
            assert!(
                widths.last().is_some_and(|w| w + 2 <= MIN_COLS),
                "the shortest hint does not fit the narrowest panel: {widths:?}"
            );
            // The camera is on the widest tier only.
            assert!(
                set[0].contains("C view") && set[0].contains("M ships"),
                "the widest hint does not name the camera: {:?}",
                set[0]
            );
        }

        for set in [&HINTS, &ASCII_HINTS] {
            assert!(set.iter().all(|h| h.contains("WASD steer")), "{set:?}");
        }
        for set in [&SIDE_HINTS, &ASCII_SIDE_HINTS] {
            assert!(
                set.iter().all(|h| h.contains("WASDQE cam")),
                "the outside view does not name the camera it flies: {set:?}"
            );
            assert!(
                set.iter().all(|h| !h.contains("steer")),
                "the outside view offers a stick that does not fly the ship: {set:?}"
            );
            assert!(
                set[0].contains("[] zoom"),
                "the outside view does not name the zoom: {:?}",
                set[0]
            );
        }
        for set in [&HINTS, &ASCII_HINTS] {
            assert!(
                set.iter().all(|h| !h.contains("zoom")),
                "the cockpit advertises a zoom it has not got: {set:?}"
            );
        }
    }

    #[test]
    fn the_hint_line_follows_the_view_it_is_drawn_over() {
        let ship = Ship::new();
        let read = |view| {
            let mut screen = blank(120, 34);
            draw(
                &mut screen,
                &Readout {
                    ship: &ship,
                    fps: 60.0,
                    magnitude: 6.0,
                    paused: false,
                    panel: true,
                    view,
                    model: "enterprise",
                },
            );
            screen.row_text(hint_row(34))
        };
        // Word by word rather than phrase by phrase: `overlay` is transparent
        // and leaves the half-block showing through the gaps between words, so
        // a row read back has the sky in its spaces.
        let cockpit = read(ViewMode::Cockpit);
        let side = read(ViewMode::Side);
        assert!(
            cockpit.contains("WASD") && cockpit.contains("steer"),
            "{cockpit}"
        );
        assert!(
            !cockpit.contains("cam") && !cockpit.contains("zoom"),
            "the cockpit offered a camera there is nothing to point at: {cockpit}"
        );
        assert!(
            side.contains("WASDQE") && side.contains("cam"),
            "the outside view did not offer the camera it flies: {side}"
        );
        assert!(
            !side.contains("steer"),
            "the outside view offered a stick that does not fly the ship: {side}"
        );
    }

    #[test]
    fn the_ascii_panel_is_actually_ascii() {
        // Regression: `--color ascii` is for a terminal that cannot be sent
        // colour, but the panel went on drawing box rules, block bars, angle
        // brackets, em dashes, degree signs and arrow keys at it — every one of
        // them multi-byte.
        let mut ship = Ship::new();
        ship.throttle = 0.5;
        ship.toggle_warp();
        for _ in 0..900 {
            ship.update(1.0 / 60.0);
        }

        // Wide enough for the full panel, and narrow enough for the compact
        // one, and paused as well as under way: every branch that writes text.
        for (cols, rows) in [(120, 34), (80, 24), (46, 12), (20, 6), (2, 2)] {
            for paused in [false, true] {
                for view in ViewMode::ALL {
                    let mut screen = blank_in(cols, rows, ColorMode::Ascii);
                    draw(
                        &mut screen,
                        &Readout {
                            ship: &ship,
                            fps: 60.0,
                            magnitude: 6.0,
                            paused,
                            panel: true,
                            view,
                            model: "normandy",
                        },
                    );
                    for row in 0..rows {
                        let text = screen.row_text(row);
                        assert!(
                            text.is_ascii(),
                            "row {row} of {cols}x{rows} in the {} view left ASCII: {text:?}",
                            view.label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_ascii_shapes_stay_clear_of_the_brightness_ramp() {
        // In this mode the panel has no colour to distinguish it from the sky,
        // so the marks that are read as shapes rather than words must not be
        // characters the starfield also draws.
        let ramp: Vec<char> = crate::term::ASCII_RAMP.iter().map(|b| *b as char).collect();
        let g = &Glyphs::ASCII;
        let mut shapes = vec![g.bar_full, g.bar_empty, g.vrule];
        shapes.extend_from_slice(&g.reticle);
        for ch in shapes {
            assert!(
                !ramp.contains(&ch),
                "{ch:?} is in the brightness ramp, so it reads as a star"
            );
        }
        assert!(
            g.bar_full != g.bar_empty,
            "a bar needs two ends to tell apart"
        );
    }

    #[test]
    fn the_two_faces_of_the_panel_lay_out_identically() {
        // The ASCII substitutes are all one column wide, which is what lets the
        // same layout code serve both: a wider stand-in would push the readouts
        // out of their columns on exactly the terminals least able to spare the
        // room.
        let ship = Ship::new();
        // The hints are excluded from the sweep rather than switched off, and
        // the distinction is the whole of this test's soundness.
        let quiet = Readout {
            ship: &ship,
            fps: 60.0,
            magnitude: 6.0,
            paused: false,
            panel: true,
            view: ViewMode::Cockpit,
            model: "normandy",
        };
        // Which cells the panel stamped over the composed frame.
        let footprint = |mode, cols: usize, rows: usize| -> Vec<Vec<bool>> {
            let bare = blank_in(cols, rows, mode);
            let mut drawn = blank_in(cols, rows, mode);
            draw(&mut drawn, &quiet);
            // The hint row is masked out at the sizes that draw one.
            let masked = (cols >= MIN_COLS && rows >= MIN_ROWS).then(|| hint_row(rows));
            (0..rows)
                .map(|row| {
                    if Some(row) == masked {
                        return vec![false; cols];
                    }
                    bare.row_text(row)
                        .chars()
                        .zip(drawn.row_text(row).chars())
                        .map(|(before, after)| before != after)
                        .collect()
                })
                .collect()
        };

        for (cols, rows) in [(120, 34), (80, 24), (46, 12), (20, 6)] {
            let truecolor = footprint(ColorMode::Truecolor, cols, rows);
            // Two empty footprints agree beautifully, and this test spent a
            // while able to produce a pair of them.
            assert!(
                truecolor.iter().flatten().any(|stamped| *stamped),
                "the panel stamped nothing at all at {cols}x{rows}"
            );
            assert_eq!(
                truecolor,
                footprint(ColorMode::Ascii, cols, rows),
                "the two faces disagree about the layout at {cols}x{rows}"
            );
        }
    }

    #[test]
    fn the_reticle_never_darkens_the_frame_behind_it() {
        // Regression: the reticle went through the panel's text path, so it
        // shadowed its own cells — and it sits inside the tunnel glare, so at
        // warp it punched four dark notches into the brightest part of the
        // view.
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
            // Said before the colours are looked at, because
            // `Backdrop::Lighten` leaves `bg` alone and takes the brighter of
            // the two foregrounds — so a cell the reticle never reached carries
            // the backdrop in both and satisfies everything below.
            assert!(
                uni()
                    .reticle
                    .contains(&screen.row_text(y).chars().nth(x).expect("a cell")),
                "no reticle at ({x},{y}), so there is nothing here to have dimmed"
            );
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
        // the bar.
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
    fn the_reticle_never_lands_in_the_instrument_panel() {
        // Regression, and the sibling of `the_hints_never_eat_the_throttle_
        // readout` above: that one sweeps every *width* because the hints once
        // overwrote the throttle bar across a band of them.
        let mut ship = Ship::new();
        ship.throttle = 1.0;

        for mode in [ColorMode::Truecolor, ColorMode::Ascii] {
            let g = Glyphs::for_mode(mode);
            for cols in [MIN_COLS, 60, 80, 120, 200] {
                for rows in MIN_ROWS..=60 {
                    let mut screen = blank_in(cols, rows, mode);
                    draw(&mut screen, &readout(&ship));

                    // Where the panel ends, asked of the picture rather than of
                    // the row count, so another NAV row moves this with it.
                    let Some(bottom) = (2..rows)
                        .find(|row| screen.row_text(*row).chars().nth(2) == Some(g.frame_bottom))
                    else {
                        panic!("the panel did not close at {cols}x{rows}");
                    };

                    for row in 1..=bottom {
                        let text = screen.row_text(row);
                        // From column 4: the frame itself lives at column 2 and
                        // is drawn in `┌` and `└`, which are two of the four
                        // glyphs being looked for.
                        for (col, ch) in text.chars().enumerate().skip(4) {
                            assert!(
                                !g.reticle.contains(&ch),
                                "the reticle put {ch:?} at column {col} of NAV \
                                 row {row}, inside a panel closing at {bottom}, \
                                 at {cols}x{rows} in {mode:?}:\n{text}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_reticle_is_still_drawn_where_there_is_room_for_it() {
        // The other half, and the reason the test above cannot stand alone: a
        // `draw_reticle` that returned immediately would satisfy it completely.
        let ship = Ship::new();
        let g = uni();
        let (cols, rows) = (120, 36);
        let mut screen = blank(cols, rows);
        draw(&mut screen, &readout(&ship));

        let found: usize = (0..rows)
            .map(|row| {
                screen
                    .row_text(row)
                    .chars()
                    .enumerate()
                    .filter(|(col, ch)| *col >= 4 && g.reticle.contains(ch))
                    .count()
            })
            .sum();
        assert_eq!(
            found, 4,
            "the reticle should draw all four corners at {cols}x{rows}"
        );
    }

    #[test]
    fn the_reticle_comes_back_as_soon_as_the_panel_leaves_it_room() {
        // The threshold, pinned at the two heights either side of it rather
        // than left to fall out of the panel's own arithmetic.
        let ship = Ship::new();
        let g = uni();
        let brackets = |rows: usize| -> usize {
            let mut screen = blank(80, rows);
            draw(&mut screen, &readout(&ship));
            (0..rows)
                .map(|row| {
                    screen
                        .row_text(row)
                        .chars()
                        .enumerate()
                        .filter(|(col, ch)| *col >= 4 && g.reticle.contains(ch))
                        .count()
                })
                .sum()
        };
        assert_eq!(
            brackets(20),
            4,
            "the reticle should fit a twenty-row window"
        );
        assert_eq!(brackets(19), 0, "nineteen rows has no room for a reticle");
    }

    #[test]
    fn the_hints_shed_detail_rather_than_vanishing() {
        // Each tier needs its own width plus the two columns that keep it off
        // the right-hand edge, so the widest fits only a wide terminal and most
        // of this range gets a shorter one.
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
    fn the_panel_can_be_suppressed_outright() {
        // A flight nobody is flying gets no glass in front of it: `--demo` and
        // `--screensaver` want the sky, and instruments are readings for
        // somebody at the controls.
        let render = |panel: bool| {
            let mut screen = blank(120, 34);
            let ship = Ship::new();
            draw(
                &mut screen,
                &Readout {
                    ship: &ship,
                    fps: 60.0,
                    magnitude: 6.0,
                    paused: false,
                    panel,
                    view: ViewMode::Cockpit,
                    model: "normandy",
                },
            );
            let mut out = Vec::new();
            screen.flush(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };
        // The frame as `compose` left it, handed to nothing.
        let mut untouched = blank(120, 34);
        let mut out = Vec::new();
        untouched.flush(&mut out).unwrap();
        let untouched = String::from_utf8_lossy(&out).into_owned();

        assert_eq!(
            render(false),
            untouched,
            "a suppressed panel still put something on the frame"
        );
        let drawn = render(true);
        assert_ne!(drawn, untouched, "the panel drew nothing with `panel` set");
        for word in ["VELOCITY", "THR", "pause"] {
            assert!(drawn.contains(word), "the panel is missing {word}");
        }
    }

    #[test]
    fn the_corner_reports_the_sky_it_was_asked_for_and_the_rate_it_is_drawn_at() {
        // Row 1's right-hand end had nothing looking at it at all: the only
        // assertions that reached that row were the width sweep and the ASCII
        // sweep, neither of which reads a word of it, so the ten reference
        // frames were the whole of its coverage — and a hash says a frame
        // changed without saying what about it was meant to.
        let ship = Ship::new();
        let mut screen = blank(120, 34);
        draw(
            &mut screen,
            &Readout {
                ship: &ship,
                fps: 42.0,
                magnitude: 5.5,
                paused: false,
                panel: true,
                view: ViewMode::Cockpit,
                model: "normandy",
            },
        );
        // Read with the backdrop taken out rather than word by word, because
        // what is being pinned is that the row holds these two fields and
        // nothing else — a `contains` apiece would go on passing with a third
        // number put back between them.
        let ink: String = screen
            .row_text(1)
            .chars()
            .filter(|c| *c != '\u{2580}')
            .collect();
        assert_eq!(
            ink, "\u{250C}NAVMAG5.542FPS",
            "row 1 is not the NAV header and the two corner fields"
        );
    }

    #[test]
    fn the_panel_quotes_the_warp_factor_once() {
        // The NAV panel used to carry a `WARP` row reading `FACTOR 9.78` while
        // the status banner said `WARP DRIVE ENGAGED — FACTOR 9.78` across the
        // bottom of the same frame: one number, twice, several rows apart, and
        // the quieter of the two was the one taking a row out of a budget
        // CLAUDE.md describes as tight.
        let mut ship = Ship::new();
        ship.throttle = 1.0;
        ship.toggle_warp();
        for _ in 0..900 {
            ship.update(1.0 / 60.0);
        }
        assert!(ship.warp_factor() > 0.0, "the drive never lit");

        for view in ViewMode::ALL {
            let (cols, rows) = (120, 34);
            let mut screen = blank(cols, rows);
            draw(
                &mut screen,
                &Readout {
                    ship: &ship,
                    fps: 60.0,
                    magnitude: 6.0,
                    paused: false,
                    panel: true,
                    view,
                    model: "normandy",
                },
            );
            let frame: String = (0..rows).map(|row| screen.row_text(row)).collect();
            assert_eq!(
                frame.matches("FACTOR").count(),
                1,
                "the {} view quotes the warp factor more than once",
                view.label()
            );
        }
    }

    #[test]
    fn the_status_banner_reports_the_drive_state() {
        // Over a black frame nothing behind the panel changes colour, so each
        // word lands in the output as one contiguous run.
        let flushed = |ship: &Ship, paused: bool| {
            let mut screen = blank(120, 34);
            draw(
                &mut screen,
                &Readout {
                    ship,
                    fps: 60.0,
                    magnitude: 6.0,
                    paused,
                    panel: true,
                    view: ViewMode::Cockpit,
                    model: "normandy",
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
}
