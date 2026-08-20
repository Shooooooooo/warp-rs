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
use crate::canvas::{Canvas, Trace};
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
    source: Vec<Trace>,
    bent: Vec<Trace>,
}

impl Bend {
    /// Draw one star's exposure, bending it if the bubble reaches it.
    ///
    /// It takes the path rather than a [`Streak`] because an exposure is one:
    /// when the ship turned while the shutter was open, the track the star
    /// swept is a curve and the sky hands it over as the poses it was open at.
    /// A path of two points is the straight case and goes to `draw_streak`
    /// exactly as it always did, which is what keeps a sublight frame — and
    /// every frame of a flight nobody steers — the frame it was.
    pub fn draw_one(
        &mut self,
        canvas: &mut Canvas,
        lens: &Lens,
        points: &[Trace],
        color: [f32; 3],
        intensity: f32,
    ) {
        let (Some(tail), Some(head)) = (points.first(), points.last()) else {
            return;
        };
        if !lens.bends(points) {
            if points.len() == 2 {
                canvas.draw_streak(&Streak {
                    from: (tail.0, tail.1),
                    to: (head.0, head.1),
                    color,
                    intensity,
                });
            } else {
                canvas.draw_path(points, color, intensity);
            }
            return;
        }
        {
            subdivide(points, lens, &mut self.source);
            for image in [Image::Primary, Image::Secondary] {
                // The head is where the star actually is, so its magnification
                // is the one that speaks for the whole streak — and its
                // position is what says whether there is anything to draw at
                // all. Asking the bubble *first* is most of what makes this
                // affordable: the counter-image of everything beyond about
                // two-thirds of an Einstein radius lands inside the shadow, so
                // without this check the great majority of the sky is
                // subdivided, mapped and then thrown away.
                let bent = lens.map((head.0, head.1), image);
                if bent.gain < FAINTEST_COUNTER_IMAGE || lens.shadowed(bent.at) {
                    continue;
                }
                let gain = bent.gain;
                self.bent.clear();
                let mut swallowed = false;
                for p in &self.source {
                    let at = lens.map((p.0, p.1), image).at;
                    // A counter-image that dips inside the bubble is dropped
                    // whole rather than being cut into the runs that survive.
                    // What that costs is a slightly soft inner edge to the
                    // ring; what it buys is that the disc where the ship sits
                    // is empty, which is the entire point of drawing this.
                    if lens.shadowed(at) {
                        swallowed = true;
                        break;
                    }
                    // The pace is *dropped*, and the zero is what says so.
                    // A pace is how fast the star's image was moving because
                    // the ship moved, and the bubble has just re-imaged the
                    // track it was moving along — stretching it by an amount
                    // that varies down its length and that the magnification
                    // below already answers for. Carrying the star's own pace
                    // across the bend charges the stretch to the star and
                    // repaints seventy percent of an exterior frame; measuring
                    // the bent path instead is what `draw_path` has always
                    // done, and it is right here for the same reason it was
                    // right before there were paces at all.
                    let at = (at.0, at.1, 0.0);
                    match self.bent.last() {
                        // Follow the sweep around the ring rather than cutting
                        // the chord across it.
                        Some(previous) => lens.arc_to(*previous, at, &mut self.bent),
                        None => self.bent.push(at),
                    }
                }
                if !swallowed {
                    canvas.draw_path(&self.bent, color, intensity * gain);
                }
            }
        }
    }
}

/// Chop an exposure into pieces short enough that each can be bent as a point.
///
/// Driven by `length · curvature` rather than by length alone: out at the frame
/// edge the lens is a near-uniform displacement and one piece is plenty, which
/// is where most of the sky is.
///
/// [`MAX_ARCS`] is shared out across the legs rather than applied to each of
/// them, and that is the difference between a ceiling and a ceiling per leg. An
/// exposure the ship turned through arrives already cut into as many as
/// twenty-three pieces, so a per-leg ceiling would let one star cost five
/// hundred and the constant would stop meaning what it says.
fn subdivide(points: &[Trace], lens: &Lens, out: &mut Vec<Trace>) {
    out.clear();
    let budget = (MAX_ARCS / points.len().saturating_sub(1).max(1)).max(1);
    for (leg, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = crate::canvas::length_of(dx, dy);
        // The far end of the leg, which for the first one is where the star
        // actually is, speaks for it.
        let bend = lens.curvature((b.0, b.1)).max(lens.curvature((a.0, a.1)));
        let pieces = if length.is_finite() {
            ((length * bend / ARC_STEP).ceil() as usize).clamp(1, budget)
        } else {
            1
        };
        let inv = 1.0 / pieces as f32;
        // Only the first leg lays its own near end down; every later one starts
        // where its predecessor stopped. At one leg that is the whole loop the
        // straight case always ran, to the bit.
        let first = usize::from(leg > 0);
        for i in first..=pieces {
            let t = i as f32 * inv;
            // Every piece keeps the pace of the leg it came from, so the
            // falloff cannot notice the chopping — which is the property this
            // primitive has always had to have.
            out.push((a.0 + dx * t, a.1 + dy * t, a.2));
        }
    }
}
