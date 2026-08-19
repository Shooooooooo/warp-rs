//! Drawing the sky through a lit warp bubble.
//!
//! [`crate::lens`] works out *where* the bubble sends a ray; this is what does
//! the drawing once it has. A streak the bubble does not reach goes straight to
//! [`Canvas::draw_streak`] and costs what it always cost, which is why a
//! sublight exterior frame is cheap and lighting the drive is what makes one
//! expensive: every streak the bubble touches is chopped into arcs and drawn
//! twice, once per image.
//!
//! This is all that is left of a module that used to hold a whole second sky —
//! a band of stars laid out in the side camera's own space, with a fold, a rim,
//! a near cap and an entering-surface sampler to patch the seams a camera swing
//! tore in it. [`crate::universe`] is why none of that is here any more: a sky
//! laid out in the world rather than in front of the eye has no seams for a
//! swing to tear, and the two cameras are two places to stand rather than two
//! skies.

use crate::camera::Streak;
use crate::canvas::Canvas;
use crate::lens::{Image, Lens};

/// Subpixels of arc per piece when a streak is chopped up to be bent. A curve
/// drawn as one straight segment between its bent ends cuts across the very
/// region doing the bending.
const ARC_STEP: f32 = 5.0;
/// Ceiling on that subdivision, so a streak stretching several screen widths at
/// full warp cannot turn into an unbounded amount of work.
const MAX_ARCS: usize = 24;
/// Below this the counter-image is not worth drawing. It is most of the sky —
/// the further a star is from the lens the fainter its counter-image — and what
/// is left of it lands in an invisible pile at the centre.
const FAINTEST_COUNTER_IMAGE: f32 = 0.03;

/// Scratch for bending a streak, reused across every star of every frame.
///
/// It has to live somewhere that outlives a frame, and it used to live on the
/// star field because that is what owned the draw. The sky is `&self` now — it
/// is the same sky for both cameras and has no business holding a buffer that
/// belongs to one of them — so this is the renderer's, alongside its pixels and
/// its hull band.
#[derive(Debug, Default)]
pub struct Bend {
    source: Vec<(f32, f32)>,
    bent: Vec<(f32, f32)>,
}

impl Bend {
    /// Draw a frame's worth of streaks, bending the ones the bubble reaches.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        lens: &Lens,
        streaks: impl Iterator<Item = Streak>,
    ) {
        for streak in streaks {
            if !lens.bends(streak.from, streak.to) {
                canvas.draw_streak(&streak);
                continue;
            }
            subdivide(&streak, lens, &mut self.source);
            for image in [Image::Primary, Image::Secondary] {
                // The head is where the star actually is, so its magnification
                // is the one that speaks for the whole streak — and its
                // position is what says whether there is anything to draw at
                // all. Asking the bubble *first* is most of what makes this
                // affordable: the counter-image of everything beyond about
                // two-thirds of an Einstein radius lands inside the shadow, so
                // without this check the great majority of the sky is
                // subdivided, mapped and then thrown away.
                let head = lens.map(streak.to, image);
                if head.gain < FAINTEST_COUNTER_IMAGE || lens.shadowed(head.at) {
                    continue;
                }
                let gain = head.gain;
                self.bent.clear();
                let mut swallowed = false;
                for p in &self.source {
                    let at = lens.map(*p, image).at;
                    // A counter-image that dips inside the bubble is dropped
                    // whole rather than being cut into the runs that survive.
                    // What that costs is a slightly soft inner edge to the
                    // ring; what it buys is that the disc where the ship sits
                    // is empty, which is the entire point of drawing this.
                    if lens.shadowed(at) {
                        swallowed = true;
                        break;
                    }
                    match self.bent.last() {
                        // Follow the sweep around the ring rather than cutting
                        // the chord across it.
                        Some(previous) => lens.arc_to(*previous, at, &mut self.bent),
                        None => self.bent.push(at),
                    }
                }
                if !swallowed {
                    canvas.draw_path(&self.bent, streak.color, streak.intensity * gain);
                }
            }
        }
    }
}

/// Chop a streak into pieces short enough that each can be bent as a point.
///
/// Driven by `length · curvature` rather than by length alone: out at the frame
/// edge the lens is a near-uniform displacement and one piece is plenty, which
/// is where most of the sky is.
fn subdivide(streak: &Streak, lens: &Lens, out: &mut Vec<(f32, f32)>) {
    out.clear();
    let (dx, dy) = (streak.to.0 - streak.from.0, streak.to.1 - streak.from.1);
    let length = crate::canvas::length_of(dx, dy);
    // The head, where the star actually is, speaks for the streak.
    let bend = lens.curvature(streak.to).max(lens.curvature(streak.from));
    let pieces = if length.is_finite() {
        ((length * bend / ARC_STEP).ceil() as usize).clamp(1, MAX_ARCS)
    } else {
        1
    };
    let inv = 1.0 / pieces as f32;
    for i in 0..=pieces {
        let t = i as f32 * inv;
        out.push((streak.from.0 + dx * t, streak.from.1 + dy * t));
    }
}
