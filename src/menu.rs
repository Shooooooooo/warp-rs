//! The ship picker.
//!
//! A dialogue drawn over the finished frame, after the panel, because it is in
//! front of the scene rather than painted on the glass in front of it. It is
//! modal: while it is up the keyboard drives the list, and every way out of it
//! leaves the flight running.
//!
//! Moving the highlight flies the ship it is on. A list of five names tells you
//! nothing about five ships, so the picker previews rather than describes —
//! which is also why opening it takes the camera outside.

use crate::models;
use crate::term::{ColorMode, Screen};

const FRAME: (u8, u8, u8) = (96, 176, 208);
const TITLE: (u8, u8, u8) = (226, 240, 255);
const CHOSEN: (u8, u8, u8) = (255, 186, 92);
const REST: (u8, u8, u8) = (150, 168, 190);
const BLURB: (u8, u8, u8) = (92, 108, 130);

/// Narrowest and shortest terminal the box will draw itself in. Below this it
/// sheds the blurbs, and below *that* it gives up and says so on one line,
/// rather than overflowing a window it cannot fit.
const MIN_COLS: usize = 30;
const MIN_ROWS: usize = 9;
/// The width the box wants, before it is clamped to the terminal.
const WANTED_COLS: usize = 44;

/// The characters the box is drawn from, in a face for terminals that take
/// Unicode and one for terminals that do not. Every substitute is one column
/// wide, so the two lay out identically.
struct Glyphs {
    corner: [char; 4],
    horizontal: char,
    vertical: char,
    cursor: char,
}

impl Glyphs {
    const UNICODE: Glyphs = Glyphs {
        corner: ['\u{250C}', '\u{2510}', '\u{2514}', '\u{2518}'],
        horizontal: '\u{2500}',
        vertical: '\u{2502}',
        cursor: '\u{25B8}',
    };

    const ASCII: Glyphs = Glyphs {
        corner: ['+', '+', '+', '+'],
        horizontal: '-',
        vertical: '|',
        cursor: '>',
    };

    fn for_mode(mode: ColorMode) -> &'static Glyphs {
        match mode {
            ColorMode::Ascii => &Self::ASCII,
            ColorMode::Truecolor | ColorMode::Ansi256 => &Self::UNICODE,
        }
    }
}

/// The picker's state.
pub struct Menu {
    /// Where the highlight is. The renderer flies *this* ship, so moving it
    /// previews the choice.
    cursor: usize,
}

impl Menu {
    pub fn new(selected: usize) -> Self {
        Self {
            cursor: selected.min(models::models().len().saturating_sub(1)),
        }
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Move the highlight, wrapping at both ends. A list this short is a ring:
    /// stopping dead at the last entry only makes the reader press the key
    /// again to find out that it has.
    pub fn move_cursor(&mut self, delta: isize) {
        let count = models::models().len() as isize;
        if count <= 0 {
            return;
        }
        self.cursor = (self.cursor as isize + delta).rem_euclid(count) as usize;
    }
}

pub fn draw(screen: &mut Screen, menu: &Menu) {
    let (cols, rows) = screen.dims();
    let g = Glyphs::for_mode(screen.color_mode());
    let ships = models::models();

    if cols < MIN_COLS || rows < MIN_ROWS {
        // No room for a box. Say what is selected and what moves it, on the
        // one line there is, rather than drawing a frame over the whole window.
        let line = format!(" SHIP {} ", ships[menu.cursor].name.to_uppercase());
        screen.overlay_panel(0, 0, &truncate(&line, cols), CHOSEN);
        return;
    }

    let width = WANTED_COLS.min(cols.saturating_sub(4)).max(MIN_COLS - 4);
    // Title, rule, a row per ship, rule, footer — plus the two frame lines.
    let height = ships.len() + 6;
    let (left, top) = ((cols - width) / 2, (rows.saturating_sub(height)) / 2);
    // Blurbs are the first thing to go: the names are the point.
    let room_for_blurbs = width >= 34;

    let rule: String = std::iter::repeat_n(g.horizontal, width - 2).collect();
    let blank = " ".repeat(width - 2);
    let line = |body: &str| format!("{} {body} {}", g.vertical, g.vertical);

    let mut row = top;
    let put = |screen: &mut Screen, text: &str, color: (u8, u8, u8), row: &mut usize| {
        if *row < rows {
            screen.overlay_panel(left, *row, &truncate(text, cols - left), color);
        }
        *row += 1;
    };

    put(
        screen,
        &format!("{}{}{}", g.corner[0], rule, g.corner[1]),
        FRAME,
        &mut row,
    );
    put(
        screen,
        &line(&pad("SELECT SHIP", width - 4)),
        TITLE,
        &mut row,
    );
    put(screen, &line(&"-".repeat(width - 4)), FRAME, &mut row);

    for (i, ship) in ships.iter().enumerate() {
        let mark = if i == menu.cursor { g.cursor } else { ' ' };
        let name = ship.name.to_uppercase();
        let body = if room_for_blurbs {
            format!("{mark} {name:<9} {}", ship.blurb)
        } else {
            format!("{mark} {name}")
        };
        let color = if i == menu.cursor { CHOSEN } else { REST };
        put(screen, &line(&pad(&body, width - 4)), color, &mut row);
    }

    put(screen, &line(&blank[..width - 4]), FRAME, &mut row);
    let footer = if room_for_blurbs {
        "ENTER fly it   ESC keep the one you have"
    } else {
        "ENTER fly   ESC keep"
    };
    put(screen, &line(&pad(footer, width - 4)), BLURB, &mut row);
    put(
        screen,
        &format!("{}{}{}", g.corner[2], rule, g.corner[3]),
        FRAME,
        &mut row,
    );
}

/// Pad or cut to exactly `width` characters, counting characters rather than
/// bytes: the box is full of multi-byte glyphs and its right-hand edge has to
/// line up with its left.
fn pad(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    for _ in out.chars().count()..width {
        out.push(' ');
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::ColorMode;

    fn lit(cols: usize, rows: usize, mode: ColorMode) -> Screen {
        let mut screen = Screen::new(cols, rows, mode);
        // A frame with something blazing in every cell, which is the case a
        // dialogue has to stay readable over.
        screen.compose(&vec![[220, 230, 255]; cols * rows * 2]);
        screen
    }

    #[test]
    fn the_highlight_wraps_at_both_ends() {
        let last = models::models().len() - 1;
        let mut menu = Menu::new(0);
        menu.move_cursor(-1);
        assert_eq!(menu.cursor(), last, "up from the first should come round");
        menu.move_cursor(1);
        assert_eq!(menu.cursor(), 0);
        for i in 0..models::models().len() {
            assert_eq!(menu.cursor(), i);
            menu.move_cursor(1);
        }
        assert_eq!(menu.cursor(), 0, "down from the last should come round");
    }

    #[test]
    fn a_selection_out_of_range_is_brought_back_in() {
        assert_eq!(Menu::new(9_999).cursor(), models::models().len() - 1);
    }

    #[test]
    fn the_popup_covers_the_sky_behind_it() {
        // The regression this exists for: `Screen::stamp` skips spaces so the
        // starfield shows through the panel's text, which is right for an
        // instrument and quite wrong for a dialogue — the gaps between words
        // would be full of stars.
        let (cols, rows) = (120usize, 34usize);
        let mut screen = lit(cols, rows, ColorMode::Truecolor);
        let menu = Menu::new(1);
        draw(&mut screen, &menu);

        let width = WANTED_COLS;
        let height = models::models().len() + 6;
        let (left, top) = ((cols - width) / 2, (rows - height) / 2);
        for row in top..top + height {
            for col in left..left + width {
                let (fg, bg) = screen.cell_colors(col, row);
                let (fg, bg) = (fg.expect("truecolor"), bg.expect("truecolor"));
                assert!(
                    bg.0 < 120 && bg.1 < 120 && bg.2 < 160,
                    "the sky is still blazing behind ({col}, {row}): {bg:?}"
                );
                // The top half of a cell is the foreground, and it has to go
                // down too — except where a glyph has been written in its own
                // colour, which is the point of the box.
                assert!(
                    fg.0 < 120
                        || fg == CHOSEN
                        || fg == FRAME
                        || fg == TITLE
                        || fg == REST
                        || fg == BLURB,
                    "a streak showed through the top of ({col}, {row}): {fg:?}"
                );
            }
        }
    }

    #[test]
    fn the_popup_names_every_ship_and_marks_the_one_under_the_cursor() {
        let mut screen = lit(120, 34, ColorMode::Truecolor);
        let menu = Menu::new(2);
        draw(&mut screen, &menu);
        let text: String = (0..34).map(|r| screen.row_text(r)).collect();
        for ship in models::models() {
            assert!(
                text.contains(&ship.name.to_uppercase()),
                "{} is not in the list",
                ship.name
            );
        }
        assert!(
            text.contains(Glyphs::UNICODE.cursor),
            "nothing is highlighted"
        );
    }

    #[test]
    fn the_popup_is_actually_ascii_in_ascii_mode() {
        // The same contract the instrument panel keeps: `--color ascii` is for
        // a terminal that cannot be sent colour, and such a terminal is
        // generally not one to send box-drawing characters to either.
        for (cols, rows) in [(120usize, 34usize), (46, 12), (30, 9), (20, 6), (2, 2)] {
            let mut screen = lit(cols, rows, ColorMode::Ascii);
            draw(&mut screen, &Menu::new(0));
            for row in 0..rows {
                let text = screen.row_text(row);
                assert!(
                    text.is_ascii(),
                    "row {row} of {cols}x{rows} left ASCII: {text:?}"
                );
            }
        }
    }

    #[test]
    fn the_popup_fits_whatever_terminal_it_is_given() {
        for (cols, rows) in [
            (1usize, 1usize),
            (2, 3),
            (20, 6),
            (30, 9),
            (46, 12),
            (80, 24),
            (400, 120),
        ] {
            for mode in [ColorMode::Truecolor, ColorMode::Ansi256, ColorMode::Ascii] {
                let mut screen = lit(cols, rows, mode);
                draw(&mut screen, &Menu::new(models::models().len() - 1));
                assert_eq!(screen.dims(), (cols, rows));
                // Whatever it drew, every row is still exactly as wide as the
                // terminal — nothing ran off the end of a line.
                for row in 0..rows {
                    assert_eq!(screen.row_text(row).chars().count(), cols);
                }
            }
        }
    }

    #[test]
    fn a_narrow_box_sheds_the_blurbs_rather_than_the_names() {
        let mut wide = lit(120, 34, ColorMode::Truecolor);
        draw(&mut wide, &Menu::new(0));
        let wide_text: String = (0..34).map(|r| wide.row_text(r)).collect();
        assert!(wide_text.contains("Interceptor"), "the blurbs are missing");

        let mut narrow = lit(32, 16, ColorMode::Truecolor);
        draw(&mut narrow, &Menu::new(0));
        let narrow_text: String = (0..16).map(|r| narrow.row_text(r)).collect();
        assert!(
            !narrow_text.contains("Interceptor"),
            "the blurb did not fit"
        );
        assert!(narrow_text.contains("DART"), "the name has to survive");
    }
}
