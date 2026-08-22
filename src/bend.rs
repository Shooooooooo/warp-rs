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
    /// It takes the path rather than a [`crate::camera::Streak`] because an
    /// exposure is one: when the ship turned while the shutter was open, the
    /// track the star swept is a curve and the sky hands it over as the poses
    /// it was open at. A path the bubble does not reach goes straight to
    /// [`Canvas::draw_path`] whatever its length, which is what keeps a
    /// sublight frame — and every frame of a flight nobody steers — the frame
    /// it was.
    pub fn draw_one(
        &mut self,
        canvas: &mut Canvas,
        lens: &Lens,
        points: &[Trace],
        color: [f32; 3],
        intensity: f32,
    ) {
        let Some(head) = points.last() else {
            return;
        };
        if !lens.bends(points) {
            // `draw_path` whatever the length, including two. It lays down the
            // bytes `draw_streak` would and it reads the pace off the point
            // that starts a leg, which a `Streak` has nowhere to carry — see
            // the same call in `render.rs`, where dropping it charged a cut
            // stump the whole exposure's pace.
            canvas.draw_path(points, color, intensity);
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
        // A budget of one is a `clamp(_, 1, 1)`, which is one whatever the arc
        // asks for — a saturating `as usize` puts a NaN at zero and an
        // infinity at the top, and both clamp to the same one. So when the
        // exposure has already been cut finely enough to spend the whole
        // allowance, the length and the two curvatures below decide nothing
        // and are not worth taking: a root and two calls into the lens, per
        // leg, per star. That is exactly the turning case — twenty-three legs
        // against a `MAX_ARCS` of twenty-four — and the optimiser cannot see
        // it, because `budget` is a runtime value it cannot fold against.
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let pieces = if budget == 1 {
            1
        } else {
            let length = crate::canvas::length_of(dx, dy);
            // The far end of the leg, which for the first one is where the star
            // actually is, speaks for it.
            let bend = lens.curvature((b.0, b.1)).max(lens.curvature((a.0, a.1)));
            if length.is_finite() {
                ((length * bend / ARC_STEP).ceil() as usize).clamp(1, budget)
            } else {
                1
            }
        };
        let inv = 1.0 / pieces as f32;
        // Only the first leg lays its own near end down; every later one starts
        // where its predecessor stopped. At one leg that is the whole loop the
        // straight case always ran, to the bit.
        let first = usize::from(leg > 0);
        for i in first..=pieces {
            let t = i as f32 * inv;
            // No pace, and the zero is what says so rather than an oversight.
            // It used to carry `a.2`, described as every piece keeping the pace
            // of the leg it came from — which the loop did not do: a leg's last
            // push lands on the vertex the *next* leg starts at, and
            // `draw_path` reads a pace off the point that starts a leg, so
            // every shared vertex handed the following leg the previous one's
            // number. Nothing was ever drawn from it — `Bend::draw_one` zeroes
            // the pace of every point it maps, for the reason written there —
            // so the value was wrong and dead at once, which is the worse of
            // the two states to leave a comment asserting a property over.
            //
            // Zero is also the right answer if this is ever drawn directly: a
            // bent path is measured by `draw_path`, because the bubble has
            // restretched it and the magnification already answers for that.
            out.push((a.0 + dx * t, a.1 + dy * t, 0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Streak;
    use crate::canvas::Canvas;
    use crate::view::Orbit;

    /// The nose abeam, which is where every lens test here is written from and
    /// the one angle the outline is not foreshortened at.
    fn abeam() -> [f32; 3] {
        Orbit::LEVEL.nose_in_camera()
    }

    /// A lit bubble in the middle of a canvas this size, sized off a ship of
    /// the half-length `render` hands over at the default zoom.
    fn lit(w: usize, h: usize) -> Lens {
        Lens::for_warp(
            (w as f32 * 0.5, h as f32 * 0.5),
            1.0,
            h as f32 * 0.1,
            abeam(),
        )
    }

    fn canvas() -> Canvas {
        Canvas::new(160, 80)
    }

    /// How much of the canvas has anything on it. Every comparison below is
    /// between two canvases, and two *empty* canvases agree beautifully — so
    /// each one is paired with a count saying the fixture drew something.
    fn lit_subpixels(canvas: &Canvas) -> usize {
        let (w, h) = canvas.dims();
        (0..h)
            .map(|y| (0..w).filter(|x| canvas.light_at(*x, y) > 0.0).count())
            .sum()
    }

    /// Every subpixel, so a comparison is of the whole picture rather than of
    /// wherever the writer happened to look.
    fn same_picture(a: &Canvas, b: &Canvas) -> Option<(usize, usize)> {
        let (w, h) = a.dims();
        for y in 0..h {
            for x in 0..w {
                if a.color_at(x, y) != b.color_at(x, y) {
                    return Some((x, y));
                }
            }
        }
        None
    }

    #[test]
    fn an_exposure_the_bubble_does_not_reach_is_the_streak_it_always_was() {
        // The invariant this module opens by stating, and nothing checked it.
        // A sublight frame has to be the frame the lens code is not there for,
        // which means `draw_one` on an unreached exposure must lay down the
        // bytes `Canvas::draw_streak` and `Canvas::draw_path` would — not
        // something very close to them, since a star crossing the ten-Einstein
        // -radii boundary mid-flight swaps between the two paths and an ulp of
        // disagreement twinkles a ring into the sky.
        //
        // Both entry points, because `draw_one` chooses between them on the
        // length of the path and that choice must decide nothing about the
        // picture.
        let colour = [0.8, 0.9, 1.0];
        let far_away = [
            // Switched off outright, which is the sublight frame.
            Lens::OFF,
            // And lit, but parked far enough off that the exposure is outside
            // `REACH` of it — which takes some doing, since the reach is ten
            // ring-radii and a bubble sized for a ship on this canvas covers
            // the whole of it several times over. That is the ordinary case
            // rather than a contrivance: at warp the great majority of the sky
            // is inside the reach and goes through `map`, and it is the stars
            // out at the frame's own edge that take this path.
            Lens::for_warp((4000.0, 4000.0), 1.0, 4.0, abeam()),
        ];

        for lens in far_away {
            // Two points: the straight case, and the pace is the segment's
            // own length because that is what makes it the straight case. A
            // pace is how fast the star's image was moving, expressed as the
            // length the whole exposure would have covered at that rate — so
            // for an exposure flown straight through it *is* the length, and
            // `draw_streak`, which has nowhere to carry a pace, assumes as
            // much. The fixture used to say 40 against a span of 43.08, which
            // asserted the equivalence over a stump the ship never flew.
            let span = crate::canvas::length_of(44.0 - 4.0, 22.0 - 6.0);
            let pair: [Trace; 2] = [(4.0, 6.0, span), (44.0, 22.0, span)];
            assert!(!lens.bends(&pair), "the test's exposure is inside the ring");

            let (mut bent, mut plain) = (canvas(), canvas());
            Bend::default().draw_one(&mut bent, &lens, &pair, colour, 0.7);
            plain.draw_streak(&Streak {
                from: (pair[0].0, pair[0].1),
                to: (pair[1].0, pair[1].1),
                color: colour,
                intensity: 0.7,
            });
            assert_eq!(
                same_picture(&bent, &plain),
                None,
                "an unreached two-point exposure is not the streak it was"
            );
            // Two empty canvases are also identical, and this comparison would
            // be delighted by them.
            assert!(
                lit_subpixels(&plain) > 20,
                "the two-point fixture drew almost nothing, so the comparison \
                 above says almost nothing"
            );

            // And a walk of poses: the turning case.
            let curve: Vec<Trace> = (0..9)
                .map(|i| {
                    let t = i as f32;
                    (4.0 + t * 4.0, 6.0 + t * t * 0.25, 40.0 - t)
                })
                .collect();
            assert!(!lens.bends(&curve), "the test's curve is inside the ring");

            let (mut bent, mut plain) = (canvas(), canvas());
            Bend::default().draw_one(&mut bent, &lens, &curve, colour, 0.7);
            plain.draw_path(&curve, colour, 0.7);
            assert_eq!(
                same_picture(&bent, &plain),
                None,
                "an unreached walk of poses is not the path it was"
            );
            assert!(
                lit_subpixels(&plain) > 20,
                "the walk fixture drew almost nothing, so the comparison above \
                 says almost nothing"
            );
        }
    }

    #[test]
    fn a_bent_exposure_costs_a_bounded_number_of_arcs() {
        // `MAX_ARCS` is shared out across the legs rather than applied to each
        // of them, and the difference is a ceiling against a ceiling per leg.
        // An exposure arrives already cut into as many as `MAX_STATIONS` poses,
        // so a per-leg ceiling would let one star cost several hundred samples
        // and the constant would stop meaning what it says.
        //
        // Asked of `subdivide` directly, because the share-out is arithmetic
        // rather than a picture and the difference between the two readings is
        // a factor of twenty at the top of the range.
        let lens = lit(160, 80);
        let (cx, cy) = (80.0f32, 40.0f32);
        let mut out = Vec::new();

        for poses in [2usize, 3, 5, 13, 24] {
            // A run straight through the middle of the bubble, which is where
            // the curvature is worth subdividing for.
            let points: Vec<Trace> = (0..poses)
                .map(|i| {
                    let t = i as f32 / (poses - 1) as f32;
                    (
                        cx - 60.0 + t * 120.0,
                        cy - 30.0 + t * 60.0,
                        120.0 * (1.0 - t),
                    )
                })
                .collect();
            assert!(
                lens.bends(&points),
                "the {poses}-pose exposure should reach the bubble"
            );

            subdivide(&points, &lens, &mut out);

            // One start point, then at most `budget` per leg — and the budget
            // is the share, so the whole walk stays inside `MAX_ARCS` until
            // there are more legs than arcs to go round, after which it is one
            // apiece and the pose count is the bound.
            let ceiling = 1 + MAX_ARCS.max(poses - 1);
            assert!(
                out.len() <= ceiling,
                "{poses} poses subdivided into {} samples, past the {ceiling} \
                 a shared budget allows — a per-leg ceiling would allow {}",
                out.len(),
                1 + (poses - 1) * MAX_ARCS
            );
            // And it is a subdivision rather than a decimation: every leg is
            // still represented, so the walk cannot lose a pose to the budget.
            assert!(
                out.len() >= poses,
                "{poses} poses came back as {} samples, so a leg went missing",
                out.len()
            );
        }
    }

    #[test]
    fn a_subdivided_leg_stays_on_the_leg_it_came_from() {
        // The pieces are interpolated along the straight leg between two poses,
        // so each has to lie on it — the bending happens afterwards, in `map`.
        // A piece that drifted off its leg would bend from the wrong place and
        // the error would look like curvature rather than like a bug.
        //
        // The pace is the other half, and this used to assert the wrong thing
        // about it while passing. It asked that every piece carry the pace of
        // *a* leg it lies on, which a shared vertex satisfies either way: a
        // vertex is on both legs, so the off-by-one that handed each leg its
        // predecessor's number was invisible here. It was dead as well as
        // wrong, since `Bend::draw_one` zeroes the pace of every point it maps.
        // So the pieces carry no pace now, the zero says so, and this asks for
        // exactly that — a claim that can be false.
        let lens = lit(160, 80);
        let points: [Trace; 4] = [
            (20.0, 20.0, 90.0),
            (60.0, 45.0, 60.0),
            (100.0, 30.0, 30.0),
            (140.0, 60.0, 10.0),
        ];
        let mut out = Vec::new();
        subdivide(&points, &lens, &mut out);

        for piece in &out {
            let on_a_leg = points.windows(2).any(|pair| {
                let (a, b) = (pair[0], pair[1]);
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                // Where along the leg the piece falls, and how far off it sits.
                let len_sq = dx * dx + dy * dy;
                let t = ((piece.0 - a.0) * dx + (piece.1 - a.1) * dy) / len_sq;
                if !(-1e-3..=1.0 + 1e-3).contains(&t) {
                    return false;
                }
                let (ox, oy) = (a.0 + dx * t - piece.0, a.1 + dy * t - piece.1);
                ox * ox + oy * oy < 1e-4
            });
            assert!(
                on_a_leg,
                "the piece {piece:?} is not on any leg of {points:?}"
            );
            assert_eq!(
                piece.2, 0.0,
                "the piece {piece:?} carried a pace off a leg it was cut from"
            );
        }
    }

    /// A point `rings` ring-radii out from the centre on the bearing `angle`,
    /// found by walking outward rather than computed, because the metric's two
    /// axes are the module's own business and a test that rebuilt them here
    /// would be asserting one spelling of them against another.
    fn at_rings(lens: &Lens, center: (f32, f32), angle: f32, rings: f32) -> (f32, f32) {
        let (dx, dy) = (angle.cos(), angle.sin());
        let mut r = 0.0f32;
        while r < 4000.0 {
            let p = (center.0 + dx * r, center.1 + dy * r);
            if lens.offset(p) >= rings {
                return p;
            }
            r += 0.05;
        }
        panic!("{rings} ring-radii is off the end of the world");
    }

    #[test]
    fn a_bent_exposure_leaves_the_disc_the_ship_sits_in_empty() {
        // The entire point of drawing this: the bubble sweeps a hole in the sky
        // around the ship, and the disc where the ship sits comes out empty.
        //
        // Where the exposures are put is the whole of whether this says
        // anything, and the first draft of it said nothing. A sheaf running
        // from six ring-radii through the centre and out the other side leaves
        // the shadow beautifully clear — and goes on doing so with *both*
        // `shadowed` guards deleted, because every head in it sits out where
        // the counter-image's gain is under `FAINTEST_COUNTER_IMAGE` and the
        // second image is never drawn at all. It was a test of the primary
        // image with the counter-image's name on it.
        //
        // Measured, the band that matters is a star between about one and three
        // and a half ring-radii out: there the counter-image is inside the
        // shadow and still bright enough to draw, which is exactly the case
        // both guards exist for. Every head below is put in it.
        //
        // What this reaches, stated plainly, because it took mutating the
        // module to find out and the next reader should not have to. It fails
        // if `map` stops bending, and it fails if *both* shadow guards go. It
        // does not fail if either one goes alone: at a head in this band the
        // head guard refuses the counter-image outright, and with that gone the
        // per-sample guard drops the same image on its first sample, so each
        // masks the other. That redundancy is real rather than an accident of
        // the fixture — the head is only a shortcut for the common case — and
        // a test that pinned one of them separately would be pinning which of
        // the two happens to fire, which is not a promise this module makes.
        let (w, h) = (160usize, 80usize);
        let lens = lit(w, h);
        let center = (w as f32 * 0.5, h as f32 * 0.5);
        let mut canvas = Canvas::new(w, h);
        let mut bend = Bend::default();

        let mut drawn = 0usize;
        for i in 0..48 {
            let angle = i as f32 * std::f32::consts::TAU / 48.0;
            // A head in the band, and a tail further in — so the walk crosses
            // the offset at which the counter-image enters the shadow, which is
            // what the per-sample guard is for, while the head itself is in the
            // band the head guard is for.
            let head = at_rings(&lens, center, angle, 1.4);
            let tail = at_rings(&lens, center, angle, 0.55);
            let points: Vec<Trace> = (0..5)
                .map(|s| {
                    let t = s as f32 / 4.0;
                    (
                        tail.0 + (head.0 - tail.0) * t,
                        tail.1 + (head.1 - tail.1) * t,
                        20.0,
                    )
                })
                .collect();
            assert!(
                lens.bends(&points),
                "the fixture at bearing {i} does not reach the bubble"
            );
            bend.draw_one(&mut canvas, &lens, &points, [1.0, 1.0, 1.0], 1.0);
            drawn += 1;
        }

        let mut outside = 0usize;
        for y in 0..h {
            for x in 0..w {
                let light = canvas.light_at(x, y);
                if lens.shadowed((x as f32, y as f32)) {
                    assert_eq!(
                        light, 0.0,
                        "light landed at ({x}, {y}), inside the shadow the ship \
                         sits in"
                    );
                } else if light > 0.0 {
                    outside += 1;
                }
            }
        }
        // A `draw_one` that drew nothing at all would sweep the shadow clear
        // very convincingly, so the sheaf has to have gone down somewhere.
        assert!(
            outside > 200 && drawn == 48,
            "only {outside} subpixels lit outside the shadow over {drawn} \
             exposures, so the sheaf was never drawn and the emptiness above \
             proves nothing"
        );
    }

    #[test]
    fn nothing_the_sky_can_hand_it_produces_a_nan() {
        // A star can be projected anywhere at all, including off to infinity a
        // hair past the near plane, and one NaN spreads across the canvas for
        // the rest of the frame. `lens.rs` makes this promise about its own
        // arithmetic; this is the same promise about the drawing on top of it.
        let (w, h) = (64usize, 32usize);
        let lens = lit(w, h);
        let wild = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            0.0,
            -1e30,
            1e30,
            w as f32 * 0.5,
        ];
        let mut canvas = Canvas::new(w, h);
        let mut bend = Bend::default();

        for a in wild {
            for b in wild {
                for lens in [lens, Lens::OFF] {
                    let pair: [Trace; 2] = [(a, b, 30.0), (b, a, 30.0)];
                    bend.draw_one(&mut canvas, &lens, &pair, [1.0; 3], 1.0);
                    let three: [Trace; 3] = [(a, b, 30.0), (16.0, 16.0, 20.0), (b, a, 10.0)];
                    bend.draw_one(&mut canvas, &lens, &three, [1.0; 3], 1.0);
                }
            }
        }
        // And an exposure of nothing at all, which `sweep` cannot produce but
        // the signature admits.
        bend.draw_one(&mut canvas, &lens, &[], [1.0; 3], 1.0);

        for y in 0..h {
            for x in 0..w {
                assert!(
                    canvas.light_at(x, y).is_finite(),
                    "a non-finite value reached ({x}, {y})"
                );
            }
        }
    }
}
