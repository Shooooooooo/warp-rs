//! The ship picker.
//!
//! A dialogue drawn over the finished frame, after the panel, because it is in
//! front of the scene rather than painted on the glass in front of it. It is
//! modal: while it is up the keyboard drives the list, and every way out of it
//! leaves the flight running.
//!
//! Moving the highlight flies the ship it is on. A list of names tells you
//! nothing about the ships they belong to, so the picker previews rather than
//! describes — which is also why opening it takes the camera outside.

use crate::models;
use crate::term::{ColorMode, Screen};

const FRAME: (u8, u8, u8) = (96, 176, 208);
const TITLE: (u8, u8, u8) = (226, 240, 255);
const CHOSEN: (u8, u8, u8) = (255, 186, 92);
const REST: (u8, u8, u8) = (150, 168, 190);
const BLURB: (u8, u8, u8) = (92, 108, 130);

/// Narrowest and shortest terminal the box will draw itself in. Below this it
/// gives up and says what is selected on one line, rather than overflowing a
/// window it cannot fit.
const MIN_COLS: usize = 30;
const MIN_ROWS: usize = 9;

/// Rows the box spends on itself whatever the list is doing: the two frame
/// lines, the title and the rule under it, and the blank and the footer above
/// the closing one. `MIN_ROWS` is set above this with room for three ships.
const CHROME_ROWS: usize = 6;
/// Columns it spends the same way: the two frame lines and the space inside
/// each of them.
const CHROME_COLS: usize = 4;

/// Blurb columns below which there is nothing worth reading, so the row hands
/// them back to the name.
const MIN_BLURB: usize = 14;

/// What the box says about getting out of it, widest first: the first that
/// fits is the one drawn, the same way the panel picks a hint tier.
///
/// It used to be chosen on the same threshold as the blurbs, which is ten
/// columns short of what the long one needs — so between 38 and 47 columns the
/// box offered `ESC keep the one you hav`.
const FOOTERS: [&str; 2] = [
    "ENTER fly it   ESC keep the one you have",
    "ENTER fly   ESC keep",
];

/// The characters the box is drawn from, in a face for terminals that take
/// Unicode and one for terminals that do not. Every substitute is one column
/// wide, so the two lay out identically.
struct Glyphs {
    corner: [char; 4],
    horizontal: char,
    vertical: char,
    cursor: char,
    /// Stands at the end of a blurb the box had to cut short.
    cut: char,
}

impl Glyphs {
    const UNICODE: Glyphs = Glyphs {
        corner: ['\u{250C}', '\u{2510}', '\u{2514}', '\u{2518}'],
        horizontal: '\u{2500}',
        vertical: '\u{2502}',
        cursor: '\u{25B8}',
        cut: '\u{2026}',
    };

    const ASCII: Glyphs = Glyphs {
        corner: ['+', '+', '+', '+'],
        horizontal: '-',
        vertical: '|',
        cursor: '>',
        // One column, like every other substitute here, so the two faces lay
        // out identically. An ASCII ellipsis is three characters wide and
        // would push the row it ends out of the box.
        cut: '~',
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

    let width = wanted_cols()
        .min(cols - CHROME_COLS)
        .max(MIN_COLS - CHROME_COLS);
    // What a line has room for between the two frame characters and the spaces
    // inside them. Every row of the box is padded to exactly this, which is
    // what keeps the right-hand edge lined up with the left.
    let inner = width - CHROME_COLS;

    // The list is the only part of the box that can be shortened, so it is the
    // part that gives. It used to be laid out at its full height and clipped
    // against the terminal, which on a nine-row window cost the blank, the
    // footer and the closing rule — leaving a box with no bottom edge, and a
    // *modal* dialogue with nothing left saying how to get out of it.
    let shown = ships.len().min(rows - CHROME_ROWS);
    let height = shown + CHROME_ROWS;
    let (left, top) = ((cols - width) / 2, (rows - height) / 2);
    // Windowed on the cursor rather than taken from the top, so moving down the
    // list scrolls it instead of walking the highlight onto a ship that is not
    // being drawn.
    let first = menu
        .cursor
        .saturating_sub(shown / 2)
        .min(ships.len() - shown);

    let rule: String = std::iter::repeat_n(g.horizontal, width - 2).collect();
    let line = |text: &str| format!("{} {text} {}", g.vertical, g.vertical);

    let mut row = top;
    // No bounds check: the box is sized to the terminal above, so every row of
    // it has somewhere to go.
    let put = |screen: &mut Screen, text: &str, color: (u8, u8, u8), row: &mut usize| {
        screen.overlay_panel(left, *row, &truncate(text, cols - left), color);
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
        &line(&title(menu, ships.len(), shown, inner)),
        TITLE,
        &mut row,
    );
    put(screen, &line(&"-".repeat(inner)), FRAME, &mut row);

    for (i, ship) in ships.iter().enumerate().skip(first).take(shown) {
        let mark = if i == menu.cursor { g.cursor } else { ' ' };
        let color = if i == menu.cursor { CHOSEN } else { REST };
        put(
            screen,
            &line(&pad(&ship_row(ship, mark, inner, g), inner)),
            color,
            &mut row,
        );
    }

    put(screen, &line(&" ".repeat(inner)), FRAME, &mut row);
    put(screen, &line(&pad(footer(inner), inner)), BLURB, &mut row);
    put(
        screen,
        &format!("{}{}{}", g.corner[2], rule, g.corner[3]),
        FRAME,
        &mut row,
    );
}

/// The name column, as wide as the longest name there is.
///
/// Pinned at nine it was one character short of `enterprise`, so that one row's
/// blurb started a column later than every other row's and lost a character off
/// its end that none of the others did.
fn name_cols() -> usize {
    models::models()
        .iter()
        .map(|m| m.name.chars().count())
        .max()
        .unwrap_or(1)
}

/// Columns a list row spends before its blurb: the cursor, a space, the name
/// column, and the space after it.
fn label_cols() -> usize {
    name_cols() + 3
}

/// The width the box would like, before it is clamped to the terminal: enough
/// for the longest blurb there is.
///
/// Derived rather than pinned at a number. Pinned at 44 it was narrower than
/// the sentences it was framing, so five of the six blurbs were cut mid-word at
/// *every* terminal size, the widest included — which reads as a fault in the
/// renderer rather than as a box deciding what it has room for.
fn wanted_cols() -> usize {
    let blurb = models::models()
        .iter()
        .map(|m| m.blurb.chars().count())
        .max()
        .unwrap_or(0);
    CHROME_COLS + label_cols() + blurb
}

/// The title, carrying where in the list the cursor is whenever the box cannot
/// show all of it.
///
/// Shedding a blurb is a detail going quietly, and the rest of this module does
/// that without comment. Hiding whole ships is not the same thing: there would
/// otherwise be nothing at all to tell a six-ship hangar shortened to three from
/// a hangar that only has three ships in it.
fn title(menu: &Menu, total: usize, shown: usize, inner: usize) -> String {
    if shown >= total {
        return pad("SELECT SHIP", inner);
    }
    let counter = format!("{}/{}", menu.cursor + 1, total);
    let room = inner.saturating_sub(counter.chars().count());
    pad(&format!("{}{counter}", pad("SELECT SHIP", room)), inner)
}

/// One row of the list: the cursor, the name in its own column, and as much of
/// the blurb as the box has room for.
fn ship_row(ship: &models::ShipModel, mark: char, inner: usize, g: &Glyphs) -> String {
    let name = ship.name.to_uppercase();
    let names = name_cols();
    match inner.checked_sub(label_cols()).filter(|r| *r >= MIN_BLURB) {
        Some(room) => format!("{mark} {name:<names$} {}", clip(ship.blurb, room, g)),
        // Below that a blurb is a few words and an ellipsis, which says less
        // than the columns are worth. The names are the point.
        None => format!("{mark} {name}"),
    }
}

/// A blurb cut to fit, with a mark saying that it was cut. Cutting silently
/// fills the box with sentences that stop mid-word; the mark is the difference
/// between a fit and a fault.
fn clip(text: &str, room: usize, g: &Glyphs) -> String {
    if text.chars().count() <= room {
        return text.to_string();
    }
    let head: String = text.chars().take(room - 1).collect();
    format!("{}{}", head.trim_end(), g.cut)
}

/// The widest thing the box can say about getting out of itself in the room it
/// has. The last tier is held to `MIN_COLS` by a test, so there is always one.
fn footer(inner: usize) -> &'static str {
    FOOTERS
        .iter()
        .copied()
        .find(|f| f.chars().count() <= inner)
        .unwrap_or(FOOTERS[FOOTERS.len() - 1])
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

        let width = wanted_cols();
        let height = models::models().len() + CHROME_ROWS;
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

    /// The picker as text, a string per row of the terminal.
    ///
    /// In colour a dialogue leaves the half block standing where it writes a
    /// space, so the sky is still faintly there behind it rather than a hole
    /// being cut in the frame. Those are put back to spaces here, so what comes
    /// out is the text the box laid out rather than the text plus its backdrop.
    fn rendered(cols: usize, rows: usize, cursor: usize) -> Vec<String> {
        let mut screen = lit(cols, rows, ColorMode::Truecolor);
        draw(&mut screen, &Menu::new(cursor));
        (0..rows)
            .map(|r| screen.row_text(r).replace('\u{2580}', " "))
            .collect()
    }

    #[test]
    fn the_box_always_closes_and_always_says_how_to_leave_it() {
        // Regression: the box was laid out at `ships.len() + 6` rows against a
        // MIN_ROWS of 9 and then clipped against the terminal, so on a short
        // window the blank, the footer and the closing rule fell off the bottom
        // — a box with no bottom edge. It is *modal*, so the footer it dropped
        // was the only thing left on screen naming the key that gets you out.
        let g = &Glyphs::UNICODE;
        for rows in MIN_ROWS..=40 {
            for cols in [MIN_COLS, 34, 40, 48, 63, 80, 200] {
                let text = rendered(cols, rows, 0).concat();
                assert!(
                    text.contains(g.corner[2]) && text.contains(g.corner[3]),
                    "the box has no bottom edge at {cols}x{rows}"
                );
                // Verbatim, not merely present: a footer clipped to
                // `ESC keep the one you hav` still contains the word.
                assert!(
                    FOOTERS.iter().any(|f| text.contains(f)),
                    "the way out was cut short at {cols}x{rows}"
                );
            }
        }
    }

    #[test]
    fn a_short_box_windows_the_list_and_says_that_it_did() {
        // Shedding a blurb can go quietly. Dropping whole ships cannot: without
        // the counter there is nothing to tell a hangar of six shortened to
        // three from a hangar that only has three in it.
        let all = models::models().len();
        let counter = format!("1/{all}");

        let short = rendered(80, MIN_ROWS, 0).concat();
        assert!(
            short.contains(&counter),
            "ships went missing without a word: {short:?}"
        );

        let tall = rendered(80, all + CHROME_ROWS, 0).concat();
        assert!(
            !tall.contains(&counter),
            "the counter showed up with the whole list on screen"
        );
    }

    #[test]
    fn the_ship_under_the_cursor_is_always_one_of_the_ones_drawn() {
        // The highlight walks the whole list whatever the box can show, so the
        // window has to follow it. A cursor on a row that is not being drawn is
        // a picker flying a ship you cannot see yourself choosing.
        let ships = models::models();
        for rows in MIN_ROWS..=(ships.len() + CHROME_ROWS + 4) {
            for (cursor, ship) in ships.iter().enumerate() {
                let text = rendered(80, rows, cursor);
                let marked = text
                    .iter()
                    .find(|r| r.contains(Glyphs::UNICODE.cursor))
                    .unwrap_or_else(|| panic!("nothing highlighted at {rows} rows"));
                let name = ship.name.to_uppercase();
                assert!(
                    marked.contains(&name),
                    "the window left {name} behind at {rows} rows: {marked:?}"
                );
            }
        }
    }

    #[test]
    fn the_shortest_footer_fits_the_narrowest_box() {
        // The same shape as the panel's hint tiers: widest first, so the first
        // that fits is the most detailed, and the last has to fit the smallest
        // box there is or `footer` has nothing to fall back on.
        let widths: Vec<usize> = FOOTERS.iter().map(|f| f.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] > w[1]),
            "the footers are out of order: {widths:?}"
        );
        // A box at `MIN_COLS` spends `CHROME_COLS` on its frame and the same
        // again centring itself, which leaves this.
        assert!(
            widths
                .last()
                .is_some_and(|w| *w <= MIN_COLS - CHROME_COLS * 2),
            "the shortest footer does not fit the narrowest box: {widths:?}"
        );
    }

    #[test]
    fn a_blurb_the_box_could_not_finish_says_that_it_was_cut() {
        // Regression: blurbs were cut by `pad`, which stops mid-word without a
        // word about it, so the box read as a renderer dropping characters
        // rather than as a box deciding what it had room for.
        let cut = Glyphs::UNICODE.cut;
        let tight = rendered(48, 20, 0).concat();
        assert!(
            tight.contains(cut),
            "a blurb was cut with nothing to say so: {tight:?}"
        );
    }

    #[test]
    fn a_box_with_the_room_finishes_every_sentence_in_it() {
        // Regression: the width was pinned at 44, which is narrower than the
        // blurbs it was framing — so five of the six were cut mid-word at
        // *every* terminal size, the widest included. The width is derived from
        // the blurbs now, so a terminal with the room shows them whole.
        let text = rendered(120, 34, 0).concat();
        for ship in models::models() {
            assert!(
                text.contains(ship.blurb),
                "{} is still cut short on a terminal with room to spare",
                ship.name
            );
        }
        assert!(
            !text.contains(Glyphs::UNICODE.cut),
            "something was cut in a box that had the room"
        );
    }
}
