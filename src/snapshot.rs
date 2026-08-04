//! Writing a frame out as a PNG.
//!
//! Terminal output is hard to inspect in a diff or a bug report, and it cannot
//! be looked at at all from a machine without a terminal. This dumps the
//! resolved pixel buffer straight to an image so the renderer can be checked
//! on its own terms. Enabled with `--features snapshot`.

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

/// Write `pixels` (row-major, `width × height`) as an RGB PNG, magnified by
/// `scale` so a terminal-sized frame is actually visible at 1:1.
pub fn write_png(
    path: &Path,
    pixels: &[[u8; 3]],
    width: usize,
    height: usize,
    scale: usize,
) -> io::Result<()> {
    assert_eq!(pixels.len(), width * height, "pixel buffer does not match its dimensions");
    let scale = scale.max(1);
    let (out_w, out_h) = (width * scale, height * scale);

    let mut data = Vec::with_capacity(out_w * out_h * 3);
    for y in 0..out_h {
        let row = (y / scale) * width;
        for x in 0..out_w {
            data.extend_from_slice(&pixels[row + x / scale]);
        }
    }

    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, out_w as u32, out_h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&data))
        .map_err(io::Error::other)
}
