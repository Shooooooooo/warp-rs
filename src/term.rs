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
/// own default rather than a colour the frame set.
#[cfg(test)]
type CellColors = (Option<(u8, u8, u8)>, Option<(u8, u8, u8)>);

/// How an overlaid glyph treats the frame behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backdrop {
    /// Write the ink and nothing else, leaving the cell's background exactly
    /// as it was composed. For instrument readouts, which are marks on the
    /// canopy rather than a plate bolted over it.
    Glass,
    /// Like [`Backdrop::Glass`], except that the ink may only ever add light:
    /// the glyph comes out as the brighter of itself and the pixel it
    /// replaces. For a mark that belongs in the scene, where writing a dim
    /// colour over a bright pixel reads as a hole rather than as an
    /// instrument.
    Lighten,
    /// Cover what is behind it outright, spaces included. For a dialogue,
    /// which is in front of the scene rather than painted on the glass — and
    /// which has to stay readable over whatever the sky happens to be doing.
    Panel,
}

/// How far the sky is taken down under a panel. Not to black: a dialogue that
/// blacks out a starfield reads as a hole cut in the frame, and the ship is
/// worth still seeing behind the list of ships.
const PANEL_DIM: f32 = 0.22;

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
    /// What it is to show next.
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

    /// Stamp instrument text over the composed frame without disturbing it:
    /// the glyph is the only thing written, so the field goes on showing
    /// through the panel. Anything off the edge is dropped.
    pub fn overlay(&mut self, col: usize, row: usize, text: &str, fg: (u8, u8, u8)) {
        self.stamp(col, row, text, fg, Backdrop::Glass);
    }

    /// Stamp a mark that must never darken the frame behind it — for glyphs
    /// that belong in the scene rather than on the glass.
    pub fn overlay_mark(&mut self, col: usize, row: usize, text: &str, fg: (u8, u8, u8)) {
        self.stamp(col, row, text, fg, Backdrop::Lighten);
    }

    /// Stamp a run of a dialogue box: the frame behind it is taken down and
    /// covered, spaces included, so the box is opaque rather than a stencil.
    pub fn overlay_panel(&mut self, col: usize, row: usize, text: &str, fg: (u8, u8, u8)) {
        self.stamp(col, row, text, fg, Backdrop::Panel);
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
            // A space carries no ink, so there is nothing to write and no
            // reason to spend the cell's upper pixel on it: the gaps between
            // the words keep both halves of the frame, where every other cell
            // of the panel keeps only the lower one. Except under a dialogue,
            // where a space is part of the box.
            if ch == ' ' && how != Backdrop::Panel {
                continue;
            }
            let cell = &mut self.back[row * self.cols + c];
            let under = cell.ch;
            cell.ch = ch;
            match how {
                Backdrop::Glass => {
                    // The ink and nothing else: whatever `compose` put behind
                    // the glyph stays exactly as it drew it. The panel used to
                    // take that pixel down to a quarter first, which bought
                    // legibility when a streak was blazing directly behind a
                    // readout and paid for it with a dark box fenced around
                    // every word — and against a sky that is mostly black, a
                    // box that was mostly a black one. Left alone, the text
                    // reads light against the dark and as a silhouette against
                    // the bright, and the field runs on underneath it unbroken.
                    //
                    // It also makes a stamp idempotent, which the shadow was
                    // not: two overlays landing on one cell dimmed it twice.
                    cell.fg = mark;
                }
                Backdrop::Panel => {
                    // Both halves of the cell go down, not just the background:
                    // the foreground *is* the top subpixel of the frame, so
                    // dimming only the background would leave a bright streak
                    // showing through the top half of every character.
                    let dim = |c: (u8, u8, u8)| {
                        let take = |v: u8| (v as f32 * PANEL_DIM) as u8;
                        (take(c.0), take(c.1), take(c.2))
                    };
                    cell.bg = cell.bg.map(dim);
                    if ch == ' ' {
                        // A gap in the box is backdrop and nothing else. In
                        // colour the half block stays, so the sky is still
                        // faintly there behind the dialogue rather than a hole
                        // being cut in the frame. With no colour to take down
                        // there is nothing to dim, so it has to be cleared
                        // instead or the brightness ramp shows through the box.
                        cell.ch = if mark.is_some() { under } else { ' ' };
                        cell.fg = cell.fg.map(dim);
                    } else {
                        cell.fg = mark;
                    }
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
        // Where the terminal's cursor sits, when that is known.
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

/// Cut a line to `max` *characters*, which is what a cell holds. Counting
/// bytes would slice a box rule or a degree sign in half and put the tail of
/// it on the wire.
///
/// Here rather than in the panel or the picker because both draw text into a
/// grid and both had a copy — the picker's this one, the panel's the same
/// thing behind a `chars().count()` guard that bought nothing, since taking
/// `max` characters from a shorter string already yields all of it.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
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

/// The six levels of the xterm-256 colour cube.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The distance between a cube level and a value, without a signed detour.
/// A free function rather than a closure because the table below is built in a
/// `const` context, where a closure cannot be called.
const fn gap(level: u8, value: usize) -> u8 {
    if level as usize > value {
        level - value as u8
    } else {
        value as u8 - level
    }
}

/// For every 8-bit value, the cube level nearest it.
///
/// This replaces a linear scan over six entries that depended on a single byte,
/// run twice per cell per frame. Composing a frame at 300x90 cost 2.35 ms in
/// this mode against 1.10 ms in truecolor, and the scan was most of the
/// difference — about 7% of a 60 fps budget, in the mode `ColorMode::detect`
/// hands to any terminal with a `TERM` entry and no `COLORTERM`, which is most
/// of them. Built at compile time, so it costs nothing at startup either.
///
/// Ties break downward, the way `min_by_key` broke them by keeping the first
/// minimum. 115, 155, 195 and 235 all sit exactly between two levels, and a
/// test pins every value against the scan this stands in for.
const NEAREST_CUBE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut value = 0usize;
    while value < 256 {
        let mut best = CUBE[0];
        let mut nearest = gap(CUBE[0], value);
        let mut i = 1;
        while i < CUBE.len() {
            let distance = gap(CUBE[i], value);
            // Strictly nearer, so an equidistant value keeps the lower level.
            if distance < nearest {
                nearest = distance;
                best = CUBE[i];
            }
            i += 1;
        }
        table[value] = best;
        value += 1;
    }
    table
};

/// Snap a colour to the nearest xterm-256 palette entry, then hand back that
/// entry's RGB. Emitting the RGB rather than the index means one code path in
/// the writer; the point is that the *values* are restricted to the palette,
/// so a 256-colour terminal renders them exactly.
fn quantize_256(rgb: [u8; 3]) -> (u8, u8, u8) {
    let cube = [
        NEAREST_CUBE[rgb[0] as usize],
        NEAREST_CUBE[rgb[1] as usize],
        NEAREST_CUBE[rgb[2] as usize],
    ];

    // The 24-step grey ramp is finer than the cube's grey diagonal, so
    // near-grey colours come out visibly better for letting it compete.
    //
    // Which step, in integers. This was a float divide, a `round` and a
    // `clamp` — a round trip through the FPU for what is a table lookup's worth
    // of arithmetic, taken twice per cell per frame. The `+ 5` is the rounding
    // the `round` was doing; below 8 both forms floor to zero, so the
    // saturating subtraction loses nothing. Worth about 0.4 ms a frame at
    // 300x90, and a test walks all 256 inputs against the form it replaced —
    // this decides an output colour, so "equivalent" has to mean equal.
    let avg = (rgb[0] as u32 + rgb[1] as u32 + rgb[2] as u32) / 3;
    let step = ((avg.saturating_sub(8) + 5) / 10).min(23);
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

/// Ask the terminal to report mouse *buttons* — which is where the wheel
/// arrives — and to report them in SGR, which is the encoding that survives a
/// window wider than 223 columns.
///
/// Written out rather than taken from crossterm's `EnableMouseCapture`, which
/// is a blanket that also turns on `?1002h` and `?1003h`: drag reporting, and
/// then reporting of *every* pointer movement over the window. Nothing in this
/// program is aimed at, so those two buy nothing and cost a stream of events on
/// every twitch of a mouse — arriving into the one loop that deliberately
/// spends its slack blocking on the queue rather than sleeping through it,
/// which is the last place worth handing a torrent of nothing to read. Asking
/// for less is the whole of the difference.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
/// And giving it back. A superset of what [`MOUSE_ON`] asks for, deliberately:
/// see [`restore`].
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// Puts the terminal into raw, full-screen mode and — crucially — puts it back
/// on the way out, including when the way out is a panic.
pub struct RawGuard;

impl RawGuard {
    /// Take the terminal over. `mouse` asks it for button reports as well, for
    /// the wheel; a mode with no controls in it has no use for them and should
    /// not be taking the pointer off the user for the duration.
    pub fn new(mouse: bool) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // Own the undoing before anything else can fail. Constructing the guard
        // last meant a `?` on any of the lines below returned with raw mode
        // still on and nothing left to switch it off.
        let guard = Self;

        let mut out = io::stdout();
        out.queue(terminal::EnterAlternateScreen)?;
        // No autowrap. The grid is painted with explicit cursor moves and never
        // relies on it, and leaving it on means a terminal that shrinks between
        // the width last measured and the next flush shears the frame
        // diagonally instead of harmlessly clipping. It also keeps the
        // bottom-right cell — which every full repaint writes — from scrolling
        // the alternate screen.
        out.queue(terminal::DisableLineWrap)?;
        out.queue(cursor::Hide)?;
        if mouse {
            out.write_all(MOUSE_ON.as_bytes())?;
        }
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
///
/// Unconditional throughout, mouse included, and that is not an oversight: this
/// is what the panic hook calls, and a panic hook has no way of knowing which
/// modes were asked for. So it gives back every mouse mode rather than the two
/// that may have been taken — turning off a mode that was never on is nothing,
/// where leaving one on is a terminal that goes on reporting clicks at whatever
/// runs next. It already resets colours it may never have set on the same
/// reasoning.
pub fn restore() {
    let mut out = io::stdout();
    let _ = out.write_all(MOUSE_OFF.as_bytes());
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

    /// What `Sink` puts on the wire instead. Named to pair with the helper
    /// above: the whole point of these tests is that the two agree.
    fn sink_ansi(write: impl FnOnce(&mut Sink<Vec<u8>>)) -> String {
        let mut buf = Vec::new();
        write(&mut Sink::<Vec<u8>>::Ansi(&mut buf));
        String::from_utf8(buf).expect("escape sequences are utf-8")
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        let text = "\u{27E8} WARP \u{27E9}";
        assert_eq!(truncate(text, 3).chars().count(), 3);
        assert_eq!(truncate(text, 999), text);
    }

    #[test]
    fn the_escapes_written_by_hand_are_the_ones_crossterm_writes() {
        // The fast path spells its own sequences rather than formatting
        // crossterm's, which is most of why it is fast. It is only allowed to
        // do that while the two agree to the byte.
        for (col, row) in [(0, 0), (7, 3), (79, 23), (199, 59), (1234, 999)] {
            assert_eq!(
                sink_ansi(|s| s.move_to(col, row).unwrap()),
                crossterm_ansi(cursor::MoveTo(col as u16, row as u16)),
                "MoveTo({col}, {row})"
            );
        }
        for rgb in [(0, 0, 0), (255, 255, 255), (1, 10, 100), (58, 92, 118)] {
            assert_eq!(
                sink_ansi(|s| s.set_color(Some(rgb), Ground::Foreground).unwrap()),
                crossterm_ansi(SetForegroundColor(to_color(Some(rgb)))),
                "foreground {rgb:?}"
            );
            assert_eq!(
                sink_ansi(|s| s.set_color(Some(rgb), Ground::Background).unwrap()),
                crossterm_ansi(SetBackgroundColor(to_color(Some(rgb)))),
                "background {rgb:?}"
            );
        }
        assert_eq!(
            sink_ansi(|s| s.set_color(None, Ground::Foreground).unwrap()),
            crossterm_ansi(SetForegroundColor(Color::Reset))
        );
        assert_eq!(
            sink_ansi(|s| s.set_color(None, Ground::Background).unwrap()),
            crossterm_ansi(SetBackgroundColor(Color::Reset))
        );
        for ch in [' ', HALF_BLOCK, '@', '\n', '\u{250C}'] {
            assert_eq!(
                sink_ansi(|s| s.glyph(ch).unwrap()),
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

        // ...until what the terminal is believed to be showing is invalidated.
        screen.force_redraw();
        screen.compose(&px);
        let mut third = Vec::new();
        screen.flush(&mut third).unwrap();
        assert!(!third.is_empty());
    }

    #[test]
    fn overlay_text_lands_in_the_cells_it_was_given() {
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
    fn a_readout_writes_ink_and_leaves_the_sky_alone() {
        // The panel is glass rather than a plate bolted to it: a cell's
        // background is the lower subpixel of the frame, and a readout has no
        // business moving it in either direction. It used to arrive at a
        // quarter brightness, which fenced every word in a dark box. The whole
        // range is walked because a divide by four is invisible at the dark
        // end of it and unmissable at the bright one.
        for level in [0u8, 1, 3, 17, 128, 200, 255] {
            let mut screen = Screen::new(8, 2, ColorMode::Truecolor);
            screen.compose(&pixels(8, 2, [level, level, level]));
            screen.overlay(0, 0, "X", (255, 255, 255));
            let (fg, bg) = screen.cell_colors(0, 0);
            assert_eq!(fg, Some((255, 255, 255)), "the ink went missing");
            assert_eq!(
                bg,
                Some((level, level, level)),
                "the panel moved the frame behind it at {level}"
            );
        }
    }

    #[test]
    fn stamping_a_readout_twice_lands_where_stamping_it_once_did() {
        // The shadow was applied per stamp, so two readouts meeting on one
        // cell dimmed it twice and left a darker notch where they overlapped.
        // Transparent, a stamp has nothing to accumulate.
        let lit = [180u8, 195, 220];
        let mut once = Screen::new(8, 2, ColorMode::Truecolor);
        once.compose(&pixels(8, 2, lit));
        once.overlay(1, 0, "X", (255, 255, 255));

        let mut twice = Screen::new(8, 2, ColorMode::Truecolor);
        twice.compose(&pixels(8, 2, lit));
        twice.overlay(1, 0, "X", (255, 255, 255));
        twice.overlay(1, 0, "X", (255, 255, 255));

        assert_eq!(once.cell_colors(1, 0), twice.cell_colors(1, 0));
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
    fn a_panel_covers_the_gaps_a_readout_lets_through() {
        // An instrument skips its spaces so the starfield glows between the
        // words. A dialogue must not: the gaps in a box are part of the box,
        // and a picker with stars shining through it reads as a fault.
        let lit = [240u8, 245, 255];
        let mut screen = Screen::new(12, 2, ColorMode::Truecolor);
        screen.compose(&pixels(12, 2, lit));
        screen.overlay(0, 0, "A B", (200, 100, 50));
        screen.overlay_panel(0, 1, "A B", (200, 100, 50));

        // The readout left the gap exactly as the sky drew it.
        let (fg, bg) = screen.cell_colors(1, 0);
        assert_eq!((fg, bg), (Some((240, 245, 255)), Some((240, 245, 255))));
        // The panel took it down, both halves of the cell.
        let (fg, bg) = screen.cell_colors(1, 1);
        let (fg, bg) = (fg.unwrap(), bg.unwrap());
        assert!(
            fg.0 < 80 && bg.0 < 80,
            "the sky showed through: {fg:?} {bg:?}"
        );
        // And what it wrote is still legible in its own ink.
        assert_eq!(screen.cell_colors(0, 1).0, Some((200, 100, 50)));
        assert_eq!(screen.back[screen.cols].ch, 'A');
    }

    #[test]
    fn a_panel_in_ascii_mode_clears_what_it_covers() {
        // With no colour there is nothing to take down, so a gap has to be
        // blanked instead — otherwise the brightness ramp goes on drawing
        // stars inside the box.
        let mut screen = Screen::new(8, 1, ColorMode::Ascii);
        screen.compose(&pixels(8, 1, [255, 255, 255]));
        assert_ne!(screen.row_text(0).trim(), "", "the sky should be drawn");
        screen.overlay_panel(0, 0, "A B", (200, 100, 50));
        assert_eq!(&screen.row_text(0)[..3], "A B", "the gap kept its star");
    }

    #[test]
    fn a_panel_leaves_the_other_two_backdrops_alone() {
        // The new arm must not disturb the two the instrument panel relies on.
        let lit = [200u8, 210, 230];
        let mut screen = Screen::new(8, 3, ColorMode::Truecolor);
        screen.compose(&pixels(8, 3, lit));
        screen.overlay(0, 0, "X", (255, 255, 255));
        screen.overlay_mark(0, 1, "X", (58, 92, 118));
        screen.overlay_panel(0, 2, "X", (255, 255, 255));

        assert_eq!(
            screen.cell_colors(0, 0),
            (Some((255, 255, 255)), Some((200, 210, 230))),
            "glass still writes its ink and leaves the frame as it found it"
        );
        assert_eq!(
            screen.cell_colors(0, 1),
            (Some((200, 210, 230)), Some((200, 210, 230))),
            "a mark still never darkens anything, foreground included"
        );
        assert!(screen.cell_colors(0, 2).1.unwrap().0 < 60, "the panel dims");
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
    fn the_cube_table_is_exactly_the_search_it_replaces() {
        // The table stands in for a scan over six levels, and `min_by_key` kept
        // the *first* minimum — so a value sitting exactly between two levels
        // has to keep the lower one. Breaking that the other way would move
        // every colour on a boundary, which is a whole band of the sky.
        for value in 0..=255u8 {
            let scanned = CUBE
                .iter()
                .copied()
                .min_by_key(|level| (*level as i32 - value as i32).abs())
                .expect("the cube is not empty");
            assert_eq!(
                NEAREST_CUBE[value as usize], scanned,
                "the table disagrees with the scan at {value}"
            );
        }
        // The four midpoints, named, because they are the ones that would move.
        for (value, want) in [(115u8, 95u8), (155, 135), (195, 175), (235, 215)] {
            assert_eq!(
                NEAREST_CUBE[value as usize], want,
                "the tie at {value} broke upward"
            );
        }
    }

    #[test]
    fn the_grey_step_is_exactly_the_float_form_it_replaces() {
        // The integer form stands in for
        //
        //     ((avg as i32 - 8) as f32 / 10.0).round().clamp(0.0, 23.0) as i32
        //
        // and it picks an output colour, so agreeing closely is not enough. The
        // input is a mean of three bytes, so there are only 256 of them and the
        // check can simply be exhaustive.
        for avg in 0..256u32 {
            let float = ((avg as i32 - 8) as f32 / 10.0).round().clamp(0.0, 23.0) as u32;
            let integer = ((avg.saturating_sub(8) + 5) / 10).min(23);
            assert_eq!(
                integer, float,
                "the two forms disagree at an average of {avg}"
            );
        }
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
