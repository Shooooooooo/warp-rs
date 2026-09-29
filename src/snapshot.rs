//! Writing frames out as PNG: one as a still, or a run of them as an animation
//! that loops.

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

/// Colours an animation's palette is cut to from the frames themselves. The
/// other two of the 256 a PNG palette holds are transparency and black.
const CUT_COLOURS: usize = 254;
/// The two palette entries that are not cut: nothing sent, and black.
const TRANSPARENT: u8 = 0;
const BLACK: u8 = 1;

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

/// A run of frames being recorded, to be written out as an animated PNG.
pub struct Animation {
    width: usize,
    height: usize,
    frames: Vec<Vec<[u8; 3]>>,
}

impl Animation {
    /// Nothing yet, of a `width × height` canvas.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            frames: Vec::new(),
        }
    }

    /// Add a frame, row-major and `width × height`.
    pub fn push(&mut self, pixels: &[[u8; 3]]) {
        assert_eq!(
            pixels.len(),
            self.width * self.height,
            "pixel buffer does not match its dimensions"
        );
        self.frames.push(pixels.to_vec());
    }

    /// Write it out magnified by `scale`, looping forever at `fps`. Frame
    /// `poster` is also written as the still a viewer shows when it cannot
    /// animate, which is a separate image the animation does not play.
    ///
    /// It is drawn in a palette cut from the frames themselves, and each frame
    /// after the first sends only the pixels whose palette entry changed,
    /// inside the smallest rectangle that holds them.
    pub fn write(&self, path: &Path, scale: usize, fps: u16, poster: usize) -> io::Result<()> {
        let Some(still) = self.frames.get(poster) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the poster is frame {poster} of an animation {} long",
                    self.frames.len()
                ),
            ));
        };
        let (w, h) = (self.width, self.height);
        let scale = scale.max(1);
        let palette = Palette::cut(&self.frames);

        let file = BufWriter::new(File::create(path)?);
        let encode = || -> Result<(), png::EncodingError> {
            let mut encoder = png::Encoder::new(file, (w * scale) as u32, (h * scale) as u32);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(palette.colours.concat());
            encoder.set_trns(vec![0u8]);
            // Indexed pixels are not samples, so there is nothing for a filter
            // to predict, and the strongest deflate is worth its time on a file
            // that is written once and fetched by everyone who reads the page.
            encoder.set_filter(png::Filter::NoFilter);
            encoder.set_deflate_compression(png::DeflateCompression::Level(9));
            encoder.set_animated(self.frames.len() as u32, 0)?;
            encoder.set_sep_def_img(true)?;
            encoder.set_frame_delay(1, fps.max(1))?;
            encoder.set_dispose_op(png::DisposeOp::None)?;
            encoder.set_blend_op(png::BlendOp::Over)?;
            let mut writer = encoder.write_header()?;

            let still: Vec<u8> = still.iter().map(|&p| palette.index_of(p)).collect();
            writer.write_image_data(&magnify(&still, w, (0, 0, w, h), scale))?;

            let mut shown = vec![TRANSPARENT; w * h];
            for frame in &self.frames {
                let mut sent = vec![TRANSPARENT; w * h];
                let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
                for (i, (&now, was)) in frame.iter().zip(shown.iter_mut()).enumerate() {
                    let index = palette.index_of(now);
                    if index == *was {
                        continue;
                    }
                    *was = index;
                    sent[i] = index;
                    let (x, y) = (i % w, i / w);
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
                // Nothing moved at all: a frame still has to be sent, so it is
                // one transparent pixel.
                let (x, y, rw, rh) = if x0 > x1 {
                    (0, 0, 1, 1)
                } else {
                    (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
                };
                writer.set_frame_position(0, 0)?;
                writer.set_frame_dimension((rw * scale) as u32, (rh * scale) as u32)?;
                writer.set_frame_position((x * scale) as u32, (y * scale) as u32)?;
                writer.write_image_data(&magnify(&sent, w, (x, y, rw, rh), scale))?;
            }
            writer.finish()
        };
        encode().map_err(io::Error::other)
    }
}

/// The `(x, y, width, height)` region of a row of palette indices `stride`
/// wide, magnified by `scale` on both axes.
fn magnify(
    indices: &[u8],
    stride: usize,
    (x, y, w, h): (usize, usize, usize, usize),
    scale: usize,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(w * h * scale * scale);
    for row in y * scale..(y + h) * scale {
        let from = (row / scale) * stride;
        for col in x * scale..(x + w) * scale {
            data.push(indices[from + col / scale]);
        }
    }
    data
}

/// The colours an animation is drawn in, and which of them each colour in its
/// frames is drawn as.
struct Palette {
    /// Transparent, then black, then the colours cut from the frames.
    colours: Vec<[u8; 3]>,
    /// Every colour other than black the frames hold, packed and sorted, and
    /// the entry each one is drawn in.
    seen: Vec<u32>,
    drawn: Vec<u8>,
}

impl Palette {
    /// Cut one from everything the frames hold. Black is kept out of the cut
    /// and given an entry of its own, since it is most of every frame and a
    /// black that came out a level off would tint the whole picture.
    fn cut(frames: &[Vec<[u8; 3]>]) -> Self {
        let mut packed: Vec<u32> = frames
            .iter()
            .flatten()
            .filter(|&&p| p != [0; 3])
            .map(|&p| pack(p))
            .collect();
        packed.sort_unstable();
        let mut counted: Vec<(u32, u64)> = Vec::new();
        for colour in packed {
            match counted.last_mut() {
                Some((last, n)) if *last == colour => *n += 1,
                _ => counted.push((colour, 1)),
            }
        }
        let seen: Vec<u32> = counted.iter().map(|&(c, _)| c).collect();

        let mut colours = vec![[0; 3], [0; 3]];
        colours.extend(median_cut(counted, CUT_COLOURS));
        let drawn = seen.iter().map(|&c| nearest(&colours, unpack(c))).collect();
        Self {
            colours,
            seen,
            drawn,
        }
    }

    /// The entry a colour is drawn in.
    fn index_of(&self, colour: [u8; 3]) -> u8 {
        if colour == [0; 3] {
            return BLACK;
        }
        match self.seen.binary_search(&pack(colour)) {
            Ok(i) => self.drawn[i],
            Err(_) => nearest(&self.colours, colour),
        }
    }
}

fn pack([r, g, b]: [u8; 3]) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

fn unpack(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// The opaque entry closest to `colour`, the first of any that tie.
fn nearest(colours: &[[u8; 3]], colour: [u8; 3]) -> u8 {
    let mut best = (u32::MAX, BLACK);
    for (i, entry) in colours.iter().enumerate().skip(usize::from(BLACK)) {
        let d: u32 = (0..3)
            .map(|k| u32::from(colour[k].abs_diff(entry[k])).pow(2))
            .sum();
        if d < best.0 {
            best = (d, i as u8);
        }
    }
    best.1
}

/// Split `counted` — distinct packed colours and how many of each — into at
/// most `cells` groups, always splitting the one spread widest along its
/// widest axis at its median, and hand back each group's mean.
///
/// A colour's say in where a split falls is the square root of its count, so a
/// few thousand pixels of one faint glow do not buy a hundred entries away
/// from the handful of bright ones a streak is drawn in. With no more distinct
/// colours than cells, every colour gets a cell of its own and comes back
/// exactly.
fn median_cut(mut counted: Vec<(u32, u64)>, cells: usize) -> Vec<[u8; 3]> {
    if counted.is_empty() {
        return Vec::new();
    }
    let mut groups = vec![Group::of(&counted, 0, counted.len())];
    while groups.len() < cells {
        let mut widest: Option<usize> = None;
        for (i, g) in groups.iter().enumerate() {
            if g.spread > 0.0 && widest.is_none_or(|w| g.spread > groups[w].spread) {
                widest = Some(i);
            }
        }
        let Some(i) = widest else {
            break;
        };
        let Group { lo, hi, axis, .. } = groups[i];
        let run = &mut counted[lo..hi];
        run.sort_unstable_by_key(|&(c, _)| (unpack(c)[axis], c));
        let half = run.iter().map(|&(_, n)| (n as f64).sqrt()).sum::<f64>() / 2.0;
        let mut so_far = 0.0;
        let mut mid = lo + 1;
        for (j, &(_, n)) in run.iter().enumerate() {
            so_far += (n as f64).sqrt();
            if so_far >= half {
                mid = (lo + j + 1).clamp(lo + 1, hi - 1);
                break;
            }
        }
        groups[i] = Group::of(&counted, lo, mid);
        groups.push(Group::of(&counted, mid, hi));
    }
    groups
        .iter()
        .map(|g| {
            let run = &counted[g.lo..g.hi];
            let total: u64 = run.iter().map(|&(_, n)| n).sum();
            let mut mean = [0u8; 3];
            for (k, m) in mean.iter_mut().enumerate() {
                let sum: u64 = run.iter().map(|&(c, n)| u64::from(unpack(c)[k]) * n).sum();
                *m = ((sum + total / 2) / total) as u8;
            }
            mean
        })
        .collect()
}

/// A run of [`median_cut`]'s colours, how widely it is spread, and along which
/// axis.
#[derive(Clone, Copy)]
struct Group {
    lo: usize,
    hi: usize,
    spread: f64,
    axis: usize,
}

impl Group {
    fn of(counted: &[(u32, u64)], lo: usize, hi: usize) -> Self {
        let mut group = Self {
            lo,
            hi,
            spread: 0.0,
            axis: 0,
        };
        if hi - lo < 2 {
            return group;
        }
        let run = &counted[lo..hi];
        let weight: f64 = run.iter().map(|&(_, n)| (n as f64).sqrt()).sum();
        for axis in 0..3 {
            let value = |c: u32| f64::from(unpack(c)[axis]);
            let mean = run
                .iter()
                .map(|&(c, n)| (n as f64).sqrt() * value(c))
                .sum::<f64>()
                / weight;
            let spread: f64 = run
                .iter()
                .map(|&(c, n)| (n as f64).sqrt() * (value(c) - mean).powi(2))
                .sum();
            if spread > group.spread {
                (group.spread, group.axis) = (spread, axis);
            }
        }
        group
    }
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

    /// What a viewer shows of an animated PNG: the still it carries for
    /// anything that cannot animate, then every frame of the animation laid
    /// over the one before, the way a browser does it.
    struct Playback {
        width: usize,
        height: usize,
        plays: u32,
        poster: Vec<[u8; 3]>,
        frames: Vec<Vec<[u8; 3]>>,
        controls: Vec<png::FrameControl>,
    }

    fn play(path: &std::path::Path) -> Playback {
        let file = std::io::BufReader::new(File::open(path).expect("the animation should open"));
        let mut reader = png::Decoder::new(file)
            .read_info()
            .expect("the animation should have a header");
        let info = reader.info();
        let (width, height) = (info.width as usize, info.height as usize);
        assert_eq!(info.color_type, png::ColorType::Indexed, "not indexed");
        let palette: Vec<[u8; 3]> = info
            .palette
            .as_ref()
            .expect("an indexed image carries a palette")
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        let opaque = |i: u8| {
            info.trns
                .as_ref()
                .and_then(|t| t.get(usize::from(i)))
                .is_none_or(|&alpha| alpha == 255)
        };
        let opaque: Vec<bool> = (0..=255).map(opaque).collect();
        let control = info.animation_control.expect("not animated");

        let mut buf = vec![0; reader.output_buffer_size().expect("a bounded image")];
        reader
            .next_frame(&mut buf)
            .expect("the still should decode");
        let poster: Vec<[u8; 3]> = buf[..width * height]
            .iter()
            .map(|&i| {
                assert!(opaque[usize::from(i)], "the still has a hole in it");
                palette[usize::from(i)]
            })
            .collect();

        let mut canvas = vec![[0u8; 3]; width * height];
        let (mut frames, mut controls) = (Vec::new(), Vec::new());
        for _ in 0..control.num_frames {
            let out = reader.next_frame(&mut buf).expect("a frame should decode");
            let fc = reader
                .info()
                .frame_control
                .expect("every frame of an animation has a control of its own");
            assert_eq!((fc.width, fc.height), (out.width, out.height));
            assert!(
                matches!(fc.dispose_op, png::DisposeOp::None),
                "a frame should leave what it drew for the next"
            );
            for y in 0..fc.height as usize {
                for x in 0..fc.width as usize {
                    let i = buf[y * fc.width as usize + x];
                    if opaque[usize::from(i)] || matches!(fc.blend_op, png::BlendOp::Source) {
                        let at = (fc.y_offset as usize + y) * width + fc.x_offset as usize + x;
                        canvas[at] = palette[usize::from(i)];
                    }
                }
            }
            frames.push(canvas.clone());
            controls.push(fc);
        }
        Playback {
            width,
            height,
            plays: control.num_plays,
            poster,
            frames,
            controls,
        }
    }

    /// `pixels` as `write` should have drawn them at `scale`.
    fn magnified(pixels: &[[u8; 3]], w: usize, scale: usize) -> Vec<[u8; 3]> {
        let h = pixels.len() / w;
        (0..h * scale)
            .flat_map(|y| (0..w * scale).map(move |x| (x, y)))
            .map(|(x, y)| pixels[(y / scale) * w + x / scale])
            .collect()
    }

    /// A block of three colours wandering across black, stopping for a frame
    /// on the way, past a star that never moves and that the rectangles sent
    /// for the block reach over.
    fn wandering(w: usize, h: usize) -> Vec<Vec<[u8; 3]>> {
        let ink = [[200, 40, 10], [10, 220, 90], [60, 60, 250]];
        let at = [(0, 0), (1, 0), (1, 0), (3, 1), (2, 2), (4, 2)];
        at.iter()
            .map(|&(bx, by)| {
                let mut frame = vec![[0u8; 3]; w * h];
                frame[w + 2] = [250, 250, 250];
                for (k, colour) in ink.iter().enumerate() {
                    frame[(by + k / 2) * w + bx + k % 2] = *colour;
                }
                frame
            })
            .collect()
    }

    #[test]
    fn an_animation_plays_back_as_the_frames_it_was_handed() {
        let (w, h, scale, fps) = (6usize, 4usize, 2usize, 25u16);
        let frames = wandering(w, h);
        let mut film = Animation::new(w, h);
        for frame in &frames {
            film.push(frame);
        }
        let path = scratch("animation");
        film.write(&path, scale, fps, 3)
            .expect("writing should succeed");
        let seen = play(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!((seen.width, seen.height), (w * scale, h * scale));
        assert_eq!(seen.plays, 0, "the animation should loop forever");
        assert_eq!(seen.frames.len(), frames.len(), "a frame went missing");
        // Fewer colours than a palette holds, so nothing is approximated and
        // every frame has to come back exactly.
        for (i, (got, want)) in seen.frames.iter().zip(&frames).enumerate() {
            assert_eq!(
                *got,
                magnified(want, w, scale),
                "frame {i} did not play back as it was handed over"
            );
        }
        assert_eq!(
            seen.poster,
            magnified(&frames[3], w, scale),
            "the still is not the frame it was asked to be"
        );
        for fc in &seen.controls {
            assert_eq!(
                (fc.delay_num, fc.delay_den),
                (1, fps),
                "a frame is not held for one tick of the frame rate"
            );
        }
    }

    #[test]
    fn a_frame_sends_only_the_rectangle_that_changed() {
        // The whole of what keeps a flight small enough to put on a web page:
        // most of a starfield is black from one frame to the next.
        let (w, h, scale) = (6usize, 4usize, 2usize);
        let frames = wandering(w, h);
        let mut film = Animation::new(w, h);
        for frame in &frames {
            film.push(frame);
        }
        let path = scratch("animation-regions");
        film.write(&path, scale, 30, 0)
            .expect("writing should succeed");
        let seen = play(&path);
        let _ = std::fs::remove_file(&path);

        let region = |fc: &png::FrameControl| {
            let s = scale as u32;
            (
                fc.x_offset / s,
                fc.y_offset / s,
                fc.width / s,
                fc.height / s,
            )
        };
        assert_eq!(
            region(&seen.controls[0]),
            (0, 0, w as u32, h as u32),
            "the first frame has nothing under it and has to be whole"
        );
        // The block steps one to the right: the old left column goes black and
        // a new column lights beside it.
        assert_eq!(region(&seen.controls[1]), (0, 0, 3, 2));
        // It stands still, and a frame with nothing in it is one pixel.
        assert_eq!(region(&seen.controls[2]), (0, 0, 1, 1));
        // Down and right, clear of where it was: both rectangles, and nothing
        // outside them.
        assert_eq!(region(&seen.controls[3]), (1, 0, 4, 3));
    }

    #[test]
    fn more_colours_than_a_palette_holds_still_come_out_close() {
        // A plane of four thousand colours on a black border, cut to the 254 a
        // palette has room for once black and transparency have theirs.
        let (w, h) = (66usize, 66usize);
        let frame: Vec<[u8; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    [0, 0, 0]
                } else {
                    [(x * 4 - 4) as u8, (y * 4 - 4) as u8, 128]
                }
            })
            .collect();
        let mut film = Animation::new(w, h);
        film.push(&frame);
        film.push(&frame);
        let path = scratch("animation-colours");
        film.write(&path, 1, 30, 0).expect("writing should succeed");
        let seen = play(&path);
        let _ = std::fs::remove_file(&path);

        let worst = seen.frames[1]
            .iter()
            .zip(&frame)
            .map(|(got, want)| (0..3).map(|k| got[k].abs_diff(want[k])).max().unwrap_or(0))
            .max()
            .unwrap_or(0);
        // 254 cells over a square of colour 256 levels on a side are about
        // sixteen levels across, so that is the most a sane cut should cost.
        assert!(
            worst <= 16,
            "a colour came back {worst} levels off, which is more than cutting \
             the palette should cost"
        );
        for (got, want) in seen.frames[1].iter().zip(&frame) {
            if *want == [0, 0, 0] {
                assert_eq!(*got, [0, 0, 0], "black came back tinted");
            }
        }
        assert_eq!(
            seen.controls[1].width * seen.controls[1].height,
            1,
            "a frame the same as the last one still sent something"
        );
    }

    #[test]
    fn a_poster_past_the_end_is_refused_by_name() {
        let mut film = Animation::new(2, 2);
        film.push(&[[1, 2, 3]; 4]);
        let path = scratch("animation-no-poster");
        let err = film
            .write(&path, 1, 30, 1)
            .expect_err("frame 1 of one frame should not be a poster");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("frame 1"),
            "the refusal does not say which frame: {err}"
        );
        assert!(!path.exists(), "a refused animation still left a file");
    }
}
