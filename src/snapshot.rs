//! Writing a frame out as a PNG.

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
    assert_eq!(
        pixels.len(),
        width * height,
        "pixel buffer does not match its dimensions"
    );
    // `--scale` is bounded to at least one when it is parsed, so this is here
    // for the other caller: the module is `pub`, and a zero would otherwise
    // write a PNG with no pixels in it rather than say why.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Somewhere to write, named for the test that wants it so two running at
    /// once cannot collide. Removed on the way out; a leftover file in the
    /// temporary directory is not worth a panic, so the removal is not checked.
    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("warp-rs-snapshot-{name}.png"))
    }

    /// Read one back with the `png` crate that this feature already carries, so
    /// checking the writer costs no dependency at all. The whole rule about not
    /// adding one is about the *tree*, and under `--features snapshot` this is
    /// already in it.
    fn decode(path: &std::path::Path) -> (usize, usize, Vec<[u8; 3]>) {
        let file = std::io::BufReader::new(File::open(path).expect("the snapshot should open"));
        let decoder = png::Decoder::new(file);
        let mut reader = decoder
            .read_info()
            .expect("the snapshot should have a header");
        let mut buf = vec![0; reader.output_buffer_size().expect("a bounded image")];
        let info = reader
            .next_frame(&mut buf)
            .expect("the snapshot should decode");
        assert_eq!(info.color_type, png::ColorType::Rgb, "not an RGB image");
        assert_eq!(info.bit_depth, png::BitDepth::Eight, "not eight bits deep");
        let pixels = buf[..info.buffer_size()]
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        (info.width as usize, info.height as usize, pixels)
    }

    #[test]
    fn a_snapshot_is_the_pixels_it_was_handed() {
        // The only check on this module was a CI step asserting the file it
        // wrote was not empty, which a writer that dropped the magnification or
        // emitted the wrong dimensions would pass comfortably.
        let (w, h) = (5usize, 3usize);
        let pixels: Vec<[u8; 3]> = (0..w * h)
            .map(|i| [(i * 7) as u8, (i * 13 + 5) as u8, (i * 29 + 11) as u8])
            .collect();

        for scale in [1usize, 2, 4] {
            let path = scratch(&format!("scale-{scale}"));
            write_png(&path, &pixels, w, h, scale).expect("writing should succeed");
            let (out_w, out_h, got) = decode(&path);
            let _ = std::fs::remove_file(&path);

            assert_eq!(
                (out_w, out_h),
                (w * scale, h * scale),
                "a scale of {scale} did not magnify the image"
            );
            // Every source pixel comes back as an exact `scale × scale` block,
            // which is the whole of what magnifying means here: no smoothing,
            // no resampling, nothing borrowed from a neighbour.
            for y in 0..out_h {
                for x in 0..out_w {
                    assert_eq!(
                        got[y * out_w + x],
                        pixels[(y / scale) * w + x / scale],
                        "({x}, {y}) is not the source pixel it magnifies, at \
                         scale {scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_scale_of_nothing_still_writes_a_picture() {
        // `scale.max(1)` exists for callers other than the command line, which
        // bounds `--scale` when it parses it.
        let pixels = vec![[9u8, 8, 7], [6, 5, 4], [3, 2, 1], [0, 1, 2]];
        let path = scratch("zero-scale");
        write_png(&path, &pixels, 2, 2, 0).expect("writing should succeed");
        let (w, h, got) = decode(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            (w, h),
            (2, 2),
            "a scale of zero should behave as a scale of one"
        );
        assert_eq!(got, pixels, "the picture came back changed");
    }
}
