//! Getting pixels onto a terminal, and putting the terminal back afterward.
//!
//! Terminal cells are about twice as tall as they are wide, which would squash
//! a starfield into an ellipse. The fix is the upper half block, `▀`: set its
//! foreground to the top pixel and its background to the bottom one and a cell
//! becomes two stacked, roughly square pixels. That doubles vertical resolution
//! and keeps the field circular.
//!
//! Only cells that actually changed are re-emitted, and colour codes are only
//! re-sent when they differ from the last cell written.

use crossterm::{
    cursor,
    style::{Color, Print, SetBackgroundColor, SetForegroundColor},
    terminal, QueueableCommand,
};
use std::io::{self, Write};

/// Upper half block: foreground paints the top pixel, background the bottom.
const HALF_BLOCK: char = '\u{2580}';
/// Brightness ramp for terminals that cannot do colour at all. Visible to
/// the crate because the panel picks its ASCII face partly to avoid it: with
/// no colour to separate glass from sky, an instrument drawn in a character
/// the starfield also uses is one the eye cannot pick out.
pub(crate) const ASCII_RAMP: &[u8] = b" .,:;-=+*oO#%@";

/// How much colour the terminal can be trusted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// 24-bit colour. What the renderer is designed for.
    Truecolor,
    /// The xterm 256-colour palette; noticeably banded but recognisable.
    Ansi256,
    /// No colour: a brightness ramp of ASCII characters.
    Ascii,
}

impl ColorMode {
    /// Work out what the terminal can do from the environment.
    pub fn detect() -> Self {
        if let Ok(v) = std::env::var("COLORTERM") {
            if v.contains("truecolor") || v.contains("24bit") {
                return ColorMode::Truecolor;
            }
        }
        match std::env::var("TERM") {
            Err(_) => ColorMode::Ascii,
            Ok(term) if term.is_empty() || term == "dumb" => ColorMode::Ascii,
            // Anything else modern enough to have a TERM entry does 256.
            Ok(_) => ColorMode::Ansi256,
        }
    }
}

/// A cell's foreground and background, either of which may be the terminal's
/// own default rather than a colour we chose.
#[cfg(test)]
type CellColors = (Option<(u8, u8, u8)>, Option<(u8, u8, u8)>);

/// How an overlaid glyph treats the frame behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backdrop {
    /// Dim what is behind the glyph so text stays legible over a bright
    /// streak. For instrument readouts, where legibility wins.
    Shadow,
    /// Never darken anything: keep the background pixel, and lighten the glyph
    /// against the pixel it replaces.
    Lighten,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    /// `None` means the terminal's own default colour.
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
}

impl Cell {
    const BLANK: Cell = Cell {
        ch: ' ',
        fg: None,
        bg: None,
    };
}

/// Where a frame's bytes go, and how they are spelled.
///
/// Spelling them by hand is worth the code: a frame is tens of thousands of
/// escape sequences and crossterm writes each one through `fmt::Display`,
/// which came to more than everything the renderer does to produce them. The
/// command path stays for the one terminal whose commands are not sequences at
/// all — a Windows console without virtual terminal processing, where they are
/// WinAPI calls that no amount of bytes can stand in for.
enum Sink<'a, W: Write> {
    Ansi(&'a mut Vec<u8>),
    Commands(&'a mut W),
}

impl<W: Write> Sink<'_, W> {
    fn move_to(&mut self, col: usize, row: usize) -> io::Result<()> {
        match self {
            Sink::Ansi(buf) => {
                buf.extend_from_slice(b"\x1b[");
                push_decimal(buf, row + 1);
                buf.push(b';');
                push_decimal(buf, col + 1);
                buf.push(b'H');
                Ok(())
            }
            Sink::Commands(out) => {
                out.queue(cursor::MoveTo(col as u16, row as u16))?;
                Ok(())
            }
        }
    }

    fn set_color(&mut self, color: Option<(u8, u8, u8)>, ground: Ground) -> io::Result<()> {
        match self {
            Sink::Ansi(buf) => {
                match color {
                    Some((r, g, b)) => {
                        buf.extend_from_slice(ground.rgb_prefix());
                        push_decimal(buf, r as usize);
                        buf.push(b';');
                        push_decimal(buf, g as usize);
                        buf.push(b';');
                        push_decimal(buf, b as usize);
                        buf.push(b'm');
                    }
                    None => buf.extend_from_slice(ground.reset()),
                }
                Ok(())
            }
            Sink::Commands(out) => {
                let color = to_color(color);
                match ground {
                    Ground::Foreground => out.queue(SetForegroundColor(color))?,
                    Ground::Background => out.queue(SetBackgroundColor(color))?,
                };
                Ok(())
            }
        }
    }

    fn glyph(&mut self, ch: char) -> io::Result<()> {
        match self {
            Sink::Ansi(buf) => {
                buf.extend_from_slice(ch.encode_utf8(&mut [0u8; 4]).as_bytes());
                Ok(())
            }
            Sink::Commands(out) => {
                out.queue(Print(ch))?;
                Ok(())
            }
        }
    }
}

/// Which half of a cell's colour is being set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ground {
    Foreground,
    Background,
}

impl Ground {
    fn rgb_prefix(self) -> &'static [u8] {
        match self {
            Ground::Foreground => b"\x1b[38;2;",
            Ground::Background => b"\x1b[48;2;",
        }
    }

    /// Back to the terminal's own colour.
    fn reset(self) -> &'static [u8] {
        match self {
            Ground::Foreground => b"\x1b[39m",
            Ground::Background => b"\x1b[49m",
        }
    }
}

/// Decimal, straight into the buffer. This is the whole of the formatting the
/// fast path needs, and doing it here is most of why it is fast.
fn push_decimal(buf: &mut Vec<u8>, value: usize) {
    let mut digits = [0u8; 20]; // a `usize` cannot be longer than this
    let mut n = 0;
    let mut value = value;
    loop {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
        if value == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        buf.push(digits[n]);
    }
}

/// Whether escape sequences reach this terminal as escape sequences.
fn ansi_supported() -> bool {
    #[cfg(windows)]
    {
        crossterm::ansi_support::supports_ansi()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// A double-buffered grid of terminal cells.
pub struct Screen {
    cols: usize,
    rows: usize,
    mode: ColorMode,
    /// What the terminal is currently showing.
    front: Vec<Cell>,
    /// What we want it to show.
    back: Vec<Cell>,
    dirty: bool,
    /// A frame's bytes, reused across frames rather than reallocated.
    scratch: Vec<u8>,
    /// Whether this terminal can be written to directly. See `Sink`.
    ansi: bool,
}

impl Screen {
    pub fn new(cols: usize, rows: usize, mode: ColorMode) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Self {
            cols,
            rows,
            mode,
            front: vec![Cell::BLANK; cols * rows],
            back: vec![Cell::BLANK; cols * rows],
            dirty: true,
            scratch: Vec::new(),
            ansi: ansi_supported(),
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// How much colour this screen is being drawn with. The instrument panel
    /// asks, because a terminal that cannot be sent colour is generally one
    /// that cannot be sent box-drawing characters either.
    pub fn color_mode(&self) -> ColorMode {
        self.mode
    }

    /// Whether a colour set on a cell will actually be emitted. In
    /// [`ColorMode::Ascii`] every cell is colourless, so sending the codes at
    /// all — even the resets — puts escape sequences on a terminal that asked
    /// not to be sent any.
    fn colored(&self) -> bool {
        self.mode != ColorMode::Ascii
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.front = vec![Cell::BLANK; cols * rows];
        self.back = vec![Cell::BLANK; cols * rows];
        self.dirty = true;
    }

    /// Forget what the terminal is showing, so the next flush repaints
    /// everything. Needed after anything else has written to the screen.
    pub fn force_redraw(&mut self) {
        self.front.fill(Cell::BLANK);
        self.dirty = true;
    }

    /// Fold a `cols × 2·rows` pixel buffer into cells.
    pub fn compose(&mut self, pixels: &[[u8; 3]]) {
        debug_assert_eq!(pixels.len(), self.cols * self.rows * 2);
        for row in 0..self.rows {
            for col in 0..self.cols {
                let top = pixels[(row * 2) * self.cols + col];
                let bottom = pixels[(row * 2 + 1) * self.cols + col];
                self.back[row * self.cols + col] = self.pixel_pair(top, bottom);
            }
        }
        self.dirty = true;
    }

    fn pixel_pair(&self, top: [u8; 3], bottom: [u8; 3]) -> Cell {
        match self.mode {
            ColorMode::Truecolor => Cell {
                ch: HALF_BLOCK,
                fg: Some((top[0], top[1], top[2])),
                bg: Some((bottom[0], bottom[1], bottom[2])),
            },
            ColorMode::Ansi256 => Cell {
                ch: HALF_BLOCK,
                fg: Some(quantize_256(top)),
                bg: Some(quantize_256(bottom)),
            },
            ColorMode::Ascii => {
                // With no colour to work with, the two pixels have to collapse
                // into a single glyph, so average them and pick by brightness.
                let level = (luma(top) + luma(bottom)) * 0.5;
                let idx = (level * (ASCII_RAMP.len() - 1) as f32).round() as usize;
                Cell {
                    ch: ASCII_RAMP[idx.min(ASCII_RAMP.len() - 1)] as char,
                    fg: None,
                    bg: None,
                }
            }
        }
    }

    /// Stamp instrument text over the composed frame, shadowed so it stays
    /// readable. Anything off the edge is dropped.
    pub fn overlay(&mut self, col: usize, row: usize, text: &str, fg: (u8, u8, u8)) {
        self.stamp(col, row, text, fg, Backdrop::Shadow);
    }

    /// Stamp a mark that must never darken the frame behind it — for glyphs
    /// that belong in the scene rather than on the glass.
    pub fn overlay_mark(&mut self, col: usize, row: usize, text: &str, fg: (u8, u8, u8)) {
        self.stamp(col, row, text, fg, Backdrop::Lighten);
    }

    fn stamp(&mut self, col: usize, row: usize, text: &str, fg: (u8, u8, u8), how: Backdrop) {
        if row >= self.rows {
            return;
        }
        let mode = self.mode;
        let mark = match mode {
            ColorMode::Truecolor => Some(fg),
            ColorMode::Ansi256 => Some(quantize_256([fg.0, fg.1, fg.2])),
            ColorMode::Ascii => None,
        };
        for (i, ch) in text.chars().enumerate() {
            let Some(c) = col.checked_add(i).filter(|c| *c < self.cols) else {
                break;
            };
            // Keep the starfield showing through the gaps in the panel text.
            if ch == ' ' {
                continue;
            }
            let cell = &mut self.back[row * self.cols + c];
            cell.ch = ch;
            match how {
                Backdrop::Shadow => {
                    cell.fg = mark;
                    // Drop a shadow behind the glyph rather than a solid panel:
                    // the field still glows through, but text stays readable
                    // even when a streak happens to be blazing directly behind.
                    cell.bg = cell.bg.map(|(r, g, b)| (r / 4, g / 4, b / 4));
                }
                Backdrop::Lighten => {
                    // `compose` already ran, so the cell's own colours *are*
                    // the two pixels underneath: fg the top, bg the bottom.
                    // Taking the brighter of the two per channel means the
                    // mark only ever adds light, and the background is left
                    // exactly as the starfield drew it.
                    cell.fg = match (cell.fg, mark) {
                        (Some(under), Some(m)) => Some(lighten(under, m, mode)),
                        // Ascii mode carries no colour to blend.
                        _ => mark,
                    };
                }
            }
        }
        self.dirty = true;
    }

    /// Push the differences to the terminal. One write, one flush.
    pub fn flush(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if self.ansi {
            // The scratch buffer is taken out and put back so that building
            // the frame can borrow the two grids at the same time.
            let mut buf = std::mem::take(&mut self.scratch);
            buf.clear();
            let built = self.diff(Sink::<Vec<u8>>::Ansi(&mut buf));
            let written = built.and_then(|()| out.write_all(&buf));
            self.scratch = buf;
            written?;
        } else {
            self.diff(Sink::Commands(out))?;
        }

        out.flush()?;
        self.dirty = false;
        Ok(())
    }

    /// Emit every cell that differs from what the terminal is showing, and
    /// remember that it is showing them now.
    fn diff<W: Write>(&mut self, mut sink: Sink<W>) -> io::Result<()> {
        let colored = self.colored();
        let mut last_fg: Option<Option<(u8, u8, u8)>> = None;
        let mut last_bg: Option<Option<(u8, u8, u8)>> = None;
        // Where the terminal's cursor sits, if we know it.
        let mut cursor_at: Option<(usize, usize)> = None;

        for row in 0..self.rows {
            for col in 0..self.cols {
                let idx = row * self.cols + col;
                let cell = self.back[idx];
                if cell == self.front[idx] {
                    continue;
                }

                if cursor_at != Some((col, row)) {
                    sink.move_to(col, row)?;
                }
                if colored && last_fg != Some(cell.fg) {
                    sink.set_color(cell.fg, Ground::Foreground)?;
                    last_fg = Some(cell.fg);
                }
                if colored && last_bg != Some(cell.bg) {
                    sink.set_color(cell.bg, Ground::Background)?;
                    last_bg = Some(cell.bg);
                }
                sink.glyph(cell.ch)?;

                self.front[idx] = cell;
                cursor_at = Some((col + 1, row));
            }
        }
        Ok(())
    }

    /// A cell's foreground and background, for tests outside this module.
    #[cfg(test)]
    pub fn cell_colors(&self, col: usize, row: usize) -> CellColors {
        let cell = self.back[row * self.cols + col];
        (cell.fg, cell.bg)
    }

    /// The glyphs of one row, for tests that care about the panel's layout
    /// rather than its colours.
    #[cfg(test)]
    pub fn row_text(&self, row: usize) -> String {
        self.back[row * self.cols..(row + 1) * self.cols]
            .iter()
            .map(|c| c.ch)
            .collect()
    }

    /// Render the frame as plain text plus ANSI colour, for piping somewhere
    /// that is not an interactive terminal.
    ///
    /// Colours are re-sent only when they change, the same way `flush` does it:
    /// a cell carries about 40 bytes of escape codes, and a starfield is mostly
    /// long runs of black. Each row still ends on a reset, so a row is
    /// self-contained and never inherits state from the one above it.
    pub fn write_plain(&mut self, out: &mut impl Write) -> io::Result<()> {
        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        let built = self.plain_into(Sink::<Vec<u8>>::Ansi(&mut buf));
        let written = built.and_then(|()| out.write_all(&buf));
        self.scratch = buf;
        written?;
        out.flush()
    }

    fn plain_into<W: Write>(&self, mut sink: Sink<W>) -> io::Result<()> {
        let colored = self.colored();
        for row in 0..self.rows {
            let (mut last_fg, mut last_bg) = (None, None);
            for col in 0..self.cols {
                let cell = self.back[row * self.cols + col];
                if colored && last_fg != Some(cell.fg) {
                    sink.set_color(cell.fg, Ground::Foreground)?;
                    last_fg = Some(cell.fg);
                }
                if colored && last_bg != Some(cell.bg) {
                    sink.set_color(cell.bg, Ground::Background)?;
                    last_bg = Some(cell.bg);
                }
                sink.glyph(cell.ch)?;
            }
            // Nothing was coloured, so there is nothing to hand back.
            if colored {
                sink.set_color(None, Ground::Foreground)?;
                sink.set_color(None, Ground::Background)?;
            }
            sink.glyph('\n')?;
        }
        Ok(())
    }
}

fn to_color(rgb: Option<(u8, u8, u8)>) -> Color {
    match rgb {
        Some((r, g, b)) => Color::Rgb { r, g, b },
        None => Color::Reset,
    }
}

/// The brighter of two colours, channel by channel.
fn lighten(under: (u8, u8, u8), mark: (u8, u8, u8), mode: ColorMode) -> (u8, u8, u8) {
    let lit = (
        under.0.max(mark.0),
        under.1.max(mark.1),
        under.2.max(mark.2),
    );
    match mode {
        // A per-channel max of two palette entries need not itself be one: the
        // 24-step grey ramp and the 6×6×6 cube share no values, so mixing them
        // can land between both. Snap the result back onto the palette.
        ColorMode::Ansi256 => quantize_256([lit.0, lit.1, lit.2]),
        _ => lit,
    }
}

fn luma(rgb: [u8; 3]) -> f32 {
    (0.2126 * rgb[0] as f32 + 0.7152 * rgb[1] as f32 + 0.0722 * rgb[2] as f32) / 255.0
}

/// Snap a colour to the nearest xterm-256 palette entry, then hand back that
/// entry's RGB. Emitting the RGB rather than the index means one code path in
/// the writer; the point is that the *values* are restricted to the palette,
/// so a 256-colour terminal renders them exactly.
fn quantize_256(rgb: [u8; 3]) -> (u8, u8, u8) {
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

    let nearest_cube = |v: u8| {
        CUBE.iter()
            .copied()
            .min_by_key(|c| (*c as i32 - v as i32).abs())
            .unwrap()
    };
    let cube = [
        nearest_cube(rgb[0]),
        nearest_cube(rgb[1]),
        nearest_cube(rgb[2]),
    ];

    // The 24-step grey ramp is finer than the cube's grey diagonal, so near-grey
    // colours come out visibly better if we let it compete.
    let avg = (rgb[0] as u32 + rgb[1] as u32 + rgb[2] as u32) / 3;
    let step = ((avg as i32 - 8) as f32 / 10.0).round().clamp(0.0, 23.0) as i32;
    let grey = (8 + step * 10) as u8;
    let grey = [grey, grey, grey];

    let dist = |a: [u8; 3]| -> i32 {
        (0..3)
            .map(|i| {
                let d = a[i] as i32 - rgb[i] as i32;
                d * d
            })
            .sum()
    };

    let best = if dist(grey) < dist(cube) { grey } else { cube };
    (best[0], best[1], best[2])
}

/// Puts the terminal into raw, full-screen mode and — crucially — puts it back
/// on the way out, including when the way out is a panic.
pub struct RawGuard;

impl RawGuard {
    pub fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // Own the undoing before anything else can fail. Constructing the guard
        // last meant a `?` on any of the lines below returned with raw mode
        // still on and nothing left to switch it off.
        let guard = Self;

        let mut out = io::stdout();
        out.queue(terminal::EnterAlternateScreen)?;
        // No autowrap. The grid is painted with explicit cursor moves and never
        // relies on it, and leaving it on means a terminal that shrinks between
        // our idea of its width and the next flush shears the frame diagonally
        // instead of harmlessly clipping. It also keeps the bottom-right cell —
        // which every full repaint writes — from scrolling the alternate screen.
        out.queue(terminal::DisableLineWrap)?;
        out.queue(cursor::Hide)?;
        out.flush()?;

        // A panic mid-render would otherwise leave the user with an invisible
        // cursor in raw mode, unable to read the panic message they need.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));

        Ok(guard)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Undo everything `RawGuard::new` did. Safe to call more than once.
pub fn restore() {
    let mut out = io::stdout();
    let _ = out.queue(SetForegroundColor(Color::Reset));
    let _ = out.queue(SetBackgroundColor(Color::Reset));
    let _ = out.queue(cursor::Show);
    let _ = out.queue(terminal::EnableLineWrap);
    let _ = out.queue(terminal::LeaveAlternateScreen);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(cols: usize, rows: usize, fill: [u8; 3]) -> Vec<[u8; 3]> {
        vec![fill; cols * rows * 2]
    }

    /// What crossterm would put on the wire for a command.
    fn crossterm_ansi(command: impl crossterm::Command) -> String {
        let mut out = String::new();
        command
            .write_ansi(&mut out)
            .expect("writing to a String cannot fail");
        out
    }

    /// What we put on the wire instead.
    fn ours(write: impl FnOnce(&mut Sink<Vec<u8>>)) -> String {
        let mut buf = Vec::new();
        write(&mut Sink::<Vec<u8>>::Ansi(&mut buf));
        String::from_utf8(buf).expect("escape sequences are utf-8")
    }

    #[test]
    fn the_escapes_written_by_hand_are_the_ones_crossterm_writes() {
        // The fast path spells its own sequences rather than formatting
        // crossterm's, which is most of why it is fast. It is only allowed to
        // do that while the two agree to the byte.
        for (col, row) in [(0, 0), (7, 3), (79, 23), (199, 59), (1234, 999)] {
            assert_eq!(
                ours(|s| s.move_to(col, row).unwrap()),
                crossterm_ansi(cursor::MoveTo(col as u16, row as u16)),
                "MoveTo({col}, {row})"
            );
        }
        for rgb in [(0, 0, 0), (255, 255, 255), (1, 10, 100), (58, 92, 118)] {
            assert_eq!(
                ours(|s| s.set_color(Some(rgb), Ground::Foreground).unwrap()),
                crossterm_ansi(SetForegroundColor(to_color(Some(rgb)))),
                "foreground {rgb:?}"
            );
            assert_eq!(
                ours(|s| s.set_color(Some(rgb), Ground::Background).unwrap()),
                crossterm_ansi(SetBackgroundColor(to_color(Some(rgb)))),
                "background {rgb:?}"
            );
        }
        assert_eq!(
            ours(|s| s.set_color(None, Ground::Foreground).unwrap()),
            crossterm_ansi(SetForegroundColor(Color::Reset))
        );
        assert_eq!(
            ours(|s| s.set_color(None, Ground::Background).unwrap()),
            crossterm_ansi(SetBackgroundColor(Color::Reset))
        );
        for ch in [' ', HALF_BLOCK, '@', '\n', '\u{250C}'] {
            assert_eq!(
                ours(|s| s.glyph(ch).unwrap()),
                crossterm_ansi(Print(ch)),
                "{ch:?}"
            );
        }
    }

    #[test]
    fn numbers_come_out_in_decimal() {
        for v in [0, 1, 9, 10, 99, 100, 1023, 65_535, usize::MAX] {
            let mut buf = Vec::new();
            push_decimal(&mut buf, v);
            assert_eq!(String::from_utf8(buf).unwrap(), v.to_string());
        }
    }

    #[test]
    fn a_frame_composes_into_half_blocks() {
        let mut screen = Screen::new(4, 2, ColorMode::Truecolor);
        let mut px = pixels(4, 2, [0, 0, 0]);
        px[0] = [255, 0, 0]; // top-left top pixel
        px[4] = [0, 0, 255]; // top-left bottom pixel (row 1)
        screen.compose(&px);
        let cell = screen.back[0];
        assert_eq!(cell.ch, HALF_BLOCK);
        assert_eq!(cell.fg, Some((255, 0, 0)));
        assert_eq!(cell.bg, Some((0, 0, 255)));
    }

    #[test]
    fn only_changed_cells_are_written() {
        let mut screen = Screen::new(8, 4, ColorMode::Truecolor);
        let px = pixels(8, 4, [10, 20, 30]);
        screen.compose(&px);
        let mut first = Vec::new();
        screen.flush(&mut first).unwrap();
        assert!(!first.is_empty());

        // Composing the identical frame should produce no output at all.
        screen.compose(&px);
        let mut second = Vec::new();
        screen.flush(&mut second).unwrap();
        assert!(second.is_empty(), "an unchanged frame should cost nothing");

        // ...until we invalidate what we think the terminal is showing.
        screen.force_redraw();
        screen.compose(&px);
        let mut third = Vec::new();
        screen.flush(&mut third).unwrap();
        assert!(!third.is_empty());
    }

    #[test]
    fn overlay_text_survives_into_the_flushed_frame() {
        let mut screen = Screen::new(20, 3, ColorMode::Truecolor);
        screen.compose(&pixels(20, 3, [0, 0, 0]));
        screen.overlay(2, 1, "WARP 9", (255, 255, 255));
        let cell = |col: usize| screen.back[20 + col].ch; // row 1 of a 20-wide grid
        assert_eq!(cell(2), 'W');
        assert_eq!(cell(7), '9');
        // The space in "WARP 9" is left alone so stars show through.
        assert_eq!(cell(6), HALF_BLOCK);
    }

    #[test]
    fn overlay_shadows_what_is_behind_it() {
        // The panel's shadow is deliberate — it is what keeps a readout
        // legible when a streak is blazing directly behind it.
        let mut screen = Screen::new(8, 2, ColorMode::Truecolor);
        screen.compose(&pixels(8, 2, [200, 200, 200]));
        screen.overlay(0, 0, "X", (255, 255, 255));
        let (fg, bg) = screen.cell_colors(0, 0);
        assert_eq!(fg, Some((255, 255, 255)));
        assert_eq!(bg, Some((50, 50, 50)), "the backdrop should be dimmed");
    }

    #[test]
    fn a_mark_never_darkens_the_frame_behind_it() {
        // Regression: the reticle used the panel's shadow, so it punched dark
        // holes in the tunnel glare it sits inside of.
        let mut screen = Screen::new(8, 2, ColorMode::Truecolor);
        let bright = [240, 250, 255];
        screen.compose(&pixels(8, 2, bright));
        screen.overlay_mark(3, 0, "\u{250C}", (58, 92, 118));

        let (fg, bg) = screen.cell_colors(3, 0);
        let (fg, bg) = (fg.unwrap(), bg.unwrap());
        assert_eq!(bg, (240, 250, 255), "the background must be left alone");
        for (lit, under) in [(fg.0, bright[0]), (fg.1, bright[1]), (fg.2, bright[2])] {
            assert!(lit >= under, "the mark dimmed a channel: {lit} < {under}");
        }
    }

    #[test]
    fn a_mark_is_still_visible_against_empty_space() {
        // Never darkening must not mean never showing up.
        let mut screen = Screen::new(8, 2, ColorMode::Truecolor);
        screen.compose(&pixels(8, 2, [0, 0, 0]));
        screen.overlay_mark(3, 0, "\u{250C}", (58, 92, 118));
        let (fg, bg) = screen.cell_colors(3, 0);
        assert_eq!(
            fg,
            Some((58, 92, 118)),
            "over black the mark keeps its own colour"
        );
        assert_eq!(bg, Some((0, 0, 0)));
        assert_eq!(screen.back[3].ch, '\u{250C}');
    }

    #[test]
    fn a_lightened_mark_stays_on_the_256_palette() {
        // Per-channel max can land between the grey ramp and the colour cube,
        // so the blend has to be snapped back onto the palette.
        const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
        for level in [0, 18, 40, 90, 128, 200, 255] {
            let mut screen = Screen::new(4, 1, ColorMode::Ansi256);
            screen.compose(&pixels(4, 1, [level, level, level]));
            screen.overlay_mark(1, 0, "\u{250C}", (58, 92, 118));
            let (fg, _) = screen.cell_colors(1, 0);
            let c = fg.unwrap();
            let on_cube = [c.0, c.1, c.2].iter().all(|v| CUBE.contains(v));
            let on_grey = c.0 == c.1 && c.1 == c.2 && (c.0 as i32 - 8) % 10 == 0;
            assert!(on_cube || on_grey, "{c:?} is not a 256-palette colour");
        }
    }

    #[test]
    fn a_mark_in_ascii_mode_just_sets_the_glyph() {
        let mut screen = Screen::new(4, 1, ColorMode::Ascii);
        screen.compose(&pixels(4, 1, [255, 255, 255]));
        screen.overlay_mark(1, 0, "\u{250C}", (58, 92, 118));
        let (fg, bg) = screen.cell_colors(1, 0);
        assert_eq!(
            (fg, bg),
            (None, None),
            "ascii mode carries no colour to blend"
        );
        assert_eq!(screen.back[1].ch, '\u{250C}');
    }

    #[test]
    fn overlay_clips_instead_of_panicking() {
        let mut screen = Screen::new(10, 2, ColorMode::Truecolor);
        screen.compose(&pixels(10, 2, [0, 0, 0]));
        screen.overlay(8, 0, "far too long to fit", (255, 255, 255));
        screen.overlay(0, 99, "off the bottom", (255, 255, 255));
        screen.overlay(usize::MAX - 1, 0, "overflowing column", (255, 255, 255));
    }

    #[test]
    fn ascii_mode_uses_the_ramp_and_no_colour() {
        let mut screen = Screen::new(2, 1, ColorMode::Ascii);
        let mut px = pixels(2, 1, [0, 0, 0]);
        px[1] = [255, 255, 255];
        px[3] = [255, 255, 255];
        screen.compose(&px);
        assert_eq!(screen.back[0].ch, ' ');
        assert_eq!(screen.back[1].ch, '@');
        assert!(screen.back[1].fg.is_none() && screen.back[1].bg.is_none());
    }

    #[test]
    fn quantized_colours_are_palette_members_and_stay_close() {
        const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let is_member = |c: (u8, u8, u8)| {
            let cube_ok = [c.0, c.1, c.2].iter().all(|v| CUBE.contains(v));
            let grey_ok = c.0 == c.1 && c.1 == c.2 && (c.0 as i32 - 8) % 10 == 0;
            cube_ok || grey_ok
        };
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let q = quantize_256([r, g, b]);
                    assert!(is_member(q), "{q:?} is not in the 256 palette");
                    let err = (q.0 as i32 - r as i32).abs().max(
                        (q.1 as i32 - g as i32)
                            .abs()
                            .max((q.2 as i32 - b as i32).abs()),
                    );
                    // The palette's widest gap is 0..95, so the worst honest
                    // per-channel error is 48. Anything past that is a bug.
                    assert!(err <= 48, "quantizing {r},{g},{b} drifted to {q:?}");
                }
            }
        }
        // Pure black and white must survive exactly.
        assert_eq!(quantize_256([0, 0, 0]), (0, 0, 0));
        assert_eq!(quantize_256([255, 255, 255]), (255, 255, 255));
    }

    #[test]
    fn resizing_reallocates_and_forces_a_repaint() {
        let mut screen = Screen::new(8, 4, ColorMode::Truecolor);
        screen.compose(&pixels(8, 4, [1, 2, 3]));
        screen.flush(&mut Vec::new()).unwrap();
        screen.resize(20, 6);
        assert_eq!(screen.dims(), (20, 6));
        assert_eq!(screen.back.len(), 120);
        screen.compose(&pixels(20, 6, [1, 2, 3]));
        let mut out = Vec::new();
        screen.flush(&mut out).unwrap();
        assert!(!out.is_empty(), "a resize must repaint from scratch");
        screen.resize(0, 0);
        assert_eq!(screen.dims(), (1, 1));
    }

    #[test]
    fn plain_output_covers_every_row() {
        let mut screen = Screen::new(6, 3, ColorMode::Ascii);
        screen.compose(&pixels(6, 3, [200, 200, 200]));
        let mut out = Vec::new();
        screen.write_plain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn ascii_mode_puts_no_escape_sequences_on_the_wire() {
        // Regression: every row of plain output opened and closed with
        // `\x1b[39m\x1b[49m` — four SGR sequences a row — in the one mode that
        // exists because the terminal cannot be sent colour. On a `TERM=dumb`
        // terminal those arrive as visible garbage.
        let mut screen = Screen::new(12, 3, ColorMode::Ascii);
        let mut px = pixels(12, 3, [0, 0, 0]);
        px[5] = [255, 255, 255]; // something for the ramp to bite on
        screen.compose(&px);
        screen.overlay(0, 0, "THR", (255, 255, 255));

        let mut plain = Vec::new();
        screen.write_plain(&mut plain).unwrap();
        assert!(
            !plain.contains(&0x1b),
            "plain output carried an escape: {:?}",
            String::from_utf8_lossy(&plain)
        );
        assert!(plain.is_ascii(), "plain output left ASCII");

        // The interactive path may still position the cursor — it has no other
        // way to paint a grid — but it has no business setting colours either.
        screen.force_redraw();
        screen.compose(&px);
        let mut frame = Vec::new();
        screen.flush(&mut frame).unwrap();
        let text = String::from_utf8(frame).unwrap();
        for sequence in text.split('\u{1b}').skip(1) {
            let body = sequence.strip_prefix('[').expect("a CSI sequence");
            let end = body
                .find(|c: char| !c.is_ascii_digit() && c != ';')
                .expect("a final byte");
            assert_eq!(
                &body[end..end + 1],
                "H",
                "only cursor moves belong on an ascii terminal: {sequence:?}"
            );
        }
    }

    #[test]
    fn colour_modes_that_have_colour_still_send_it() {
        // The guard above is on the mode, not on the cell, so the modes that
        // do carry colour must be untouched by it.
        for mode in [ColorMode::Truecolor, ColorMode::Ansi256] {
            let mut screen = Screen::new(8, 2, mode);
            screen.compose(&pixels(8, 2, [10, 20, 30]));
            let mut out = Vec::new();
            screen.write_plain(&mut out).unwrap();
            assert!(out.contains(&0x1b), "{mode:?} stopped emitting colour");
        }
    }

    #[test]
    fn plain_output_only_re_sends_a_colour_when_it_changes() {
        // Regression: every cell carried its own pair of escape codes, so a
        // frame cost about 40 bytes a cell no matter what was on it — around
        // half a megabyte for a 200x60 terminal, most of it identical black.
        let mut screen = Screen::new(40, 4, ColorMode::Truecolor);
        screen.compose(&pixels(40, 4, [10, 20, 30]));
        let mut out = Vec::new();
        screen.write_plain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        // One pair of codes per row is all a uniform frame needs.
        assert_eq!(
            text.matches("38;2;10;20;30").count(),
            4,
            "foreground repeated"
        );
        assert_eq!(
            text.matches("48;2;10;20;30").count(),
            4,
            "background repeated"
        );
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn every_plain_row_stands_on_its_own() {
        // Rows reset at the end, so the run-length state must not carry over:
        // a row that opens with the colour the previous one ended on still has
        // to say so, or it inherits the reset instead.
        let mut screen = Screen::new(2, 2, ColorMode::Truecolor);
        let mut px = pixels(2, 2, [7, 8, 9]);
        px[0] = [1, 2, 3]; // only the very first subpixel differs
        screen.compose(&px);
        let mut out = Vec::new();
        screen.write_plain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        for (i, line) in text.lines().enumerate() {
            assert!(
                line.starts_with('\u{1b}'),
                "row {i} opened without setting a colour: {line:?}"
            );
        }
    }
}
