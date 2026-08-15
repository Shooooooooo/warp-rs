# CLAUDE.md

Notes for AI assistants working in this repository. [`README.md`](README.md) is
the user-facing document, and it is deliberately thin: what the program is, how
to run it, and what the flags do. It no longer lists the keys or the tmux
wiring, so it is not a second copy of anything here to be kept in step. This one
is about the inside: how the code is arranged, what it depends on that is not
obvious, and how to write changes that read like what is already here.

## What this is

`warp-rs` is a terminal starfield renderer: a starship flown at warp, drawn
with the upper half block `▀` in a character grid. Stars live in a real 3D
volume, are projected each frame, and are drawn as the segment between where
they were last frame and where they are now — accumulated into a
floating-point buffer and tonemapped at the end, so the hyperspace smear falls
out of the motion rather than being drawn as an effect.

One crate, `warp-rs`, with a binary named `warp`. `src/lib.rs` carries
everything; `src/main.rs` is sixteen lines of entry point. That split is
load-bearing, and what holds it is `tests/flight.rs`, which drives a whole
flight through the public surface without touching the binary — so the library
cannot quietly grow a dependency on `main.rs` having done something first. Note
what that does *not* say: nothing counts the lines in `main.rs`, and nothing
fails if it grows. The test pins the direction of the dependency, not the size
of the shell.

Four dependencies, on purpose: `clap`, `crossterm`, `rand`, and `png` behind
the optional `snapshot` feature. **Do not add a dependency** without saying why
the tree cannot do it — no `nalgebra` for the three-by-three matrices, no
`anyhow` for the `io::Result` that is already enough. The rule reaches inside
the four as well: `crossterm` and `rand` are taken with `default-features =
false` and only the features something here calls, and `Cargo.toml` says which
and why beside each. Dropping `derive-more` alone takes a whole proc-macro
stack — `convert_case`, `unicode-segmentation`, `rustc_version`, `semver` and a
second `syn` — out of the tree.

There is a fifth, `signal-hook`, and the shape of the argument that let it in
is the one to copy rather than the conclusion. It is Unix-only, and it names a
crate `crossterm`'s `events` feature was already compiling for SIGWINCH — so
`cargo tree --locked -e normal` counts **41 crates before it and 41 after**,
and the lock file's whole diff was one line. What it buys is the only exit that
runs no code: a signal ends the process without `Drop`, so `RawGuard` never
restores. Weigh a dependency by what it adds to the *tree* and what nothing
else can do, not by the length of the list in `Cargo.toml`.

That the rule is real rather than aspirational is worth knowing when you next
touch the manifest: it is enforced, by the committed `Cargo.lock` and the
`--locked` every cargo invocation in CI passes. A features slip or a stray
`cargo add` fails with *cannot update the lock file* rather than landing green.

`tests/golden.rs` is the rule taken to its conclusion: it spells SHA-256 out in
a page of shifts and adds rather than adding a dependency for one test, and
`the_digest_agrees_with_the_published_answers` checks it against the canonical
vectors and the block-boundary lengths before anything else trusts a word of
it.

## Commands

```sh
cargo build --locked                    # default features; what people install
cargo test                              # 300 unit + 8 flight + 3 golden, ~20s
cargo test --locked --all-features      # 301 unit — adds the snapshot-gated one
cargo fmt --all --check                 # CI runs this first
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo package --locked --list           # CI runs this too; `exclude` is by hand
cargo package --locked
```

Clippy warnings are **errors** in CI. So is misformatting. Run both before
committing; they are the two cheapest ways to turn a branch red.

```sh
cargo run --release                     # fly it
cargo run --release -- --demo           # autopilot, 45s, no keyboard needed
cargo run --release --example bench     # where a frame's 16.7 ms goes
cargo run --release --example bench 200 60 20000 side truecolor
```

`bench` prints sim / draw / write milliseconds per case against the 60 fps
budget. Its arguments are `[cols] [rows] [stars] [view] [color]`, and the
colour mode is a *column* rather than an assumption for the reason
`tests/flight.rs` pins its own: a sweep that rides the flag's default measures
something else the day that default moves, and the figures below are compared
across exactly such changes. It is worth real time — the same case came out at
6.87, 7.31 and 6.66 ms of drawing in ascii, 256 and truecolor. The pin used to
be against detection reading `TERM`; that reason went with detection and the
weaker-sounding one that replaced it is the load-bearing half. The expensive frame in the program is the
outside view at warp — every streak near the ship is chopped into arcs and
drawn twice, once per lens image — and the default sweep includes it
deliberately.

Looking at a frame as an image rather than as a wall of escape codes:

```sh
cargo run --release --features snapshot -- \
    --snapshot warp.png --engage --throttle 1.0 --warmup 600 --scale 2
```

The PNG is the **starfield only**. The instrument panel and the ship picker
live in the character grid, not in the pixel buffer, so they are not in it.

The two images on the README's front page are that command with a seed on it,
and they are written down here because they were not: both recipes lived only
in the commit messages that shot them, which is how the hero came to advertise
twice the sky a default run draws for the whole life of the renderer before
anyone thought to check it.

```sh
common="--engage --throttle 1.0 --warmup 600 --scale 2"
warp --snapshot docs/warp.png   $common --seed 6
warp --snapshot docs/astern.png $common --view side --orbit 245,30,0 --seed 8
```

**Neither passes `--size`**, and that is the whole of what keeps the pair tidy.
`run_snapshot` falls back to 240x68 and a cell is two subpixels tall, so both
come out 480x272 and stack with their edges in line. The hero used to ask for
`--size 220x60` and came out 440x240, which put a forty-pixel step down the
right of the page for no reason anyone had written down. A reshoot that reaches
for `--size` puts it back.

Nothing pins these bytes — `tests/golden/frames.sha256` is the *text* frames and
knows nothing about the PNGs — so a reshoot is checked by looking at it. What
the flags guarantee is only that the drive is lit and fully spooled: 600 warmup
frames at the default `--fps 60` is ten simulated seconds, and `run_snapshot`
prints the velocity it finished at, which at `--throttle 1.0` is 2000 c.

## The golden frames — read this before touching the renderer

`--headless` renders on a fixed timestep with no terminal control, so a fixed
seed produces byte-identical output. `tests/golden/frames.sha256` pins those
bytes, and they are checked two ways from that one file: `cargo test --test
golden` reproduces them in process through `app::render_headless`, and the
`headless` CI job produces the same seven files from a release binary and runs
`sha256sum -c`. Between them the two also prove they agree — a library that
renders one thing and a binary another would be a bug of its own.

The in-process half is the reason `render_headless` is public and separate from
`run_headless`. It costs about four seconds — most of `cargo test`'s wall clock
after the unit tests — and is **Linux-gated**, for the reason below.

**Nine flights, and the case list lives in three places** — the comment block
in `frames.sha256`, `CASES` in `tests/golden.rs`, and the `headless` CI job.
Adding one means adding it to all three, and `.gitignore` needs the file's name
too, which is a list that has fallen behind that recipe twice. They share
`--headless --frames 120 --seed 1 --size 120x36` and differ in what they make
the renderer do: three `--demo` runs in truecolor, ascii and 256, one
`--engage --throttle 1.0`, one of those from `--view side`, the same again with
the camera swung off the beam by `--orbit 55,35,20`, one more from *behind*
the ship at `--orbit -75,6,20`, one more `--demo` at `--fps 10`, and that last
one again from `--view side --orbit -60,0,0`.

`ansi256.txt` is the odd one and the only case here recorded to be read against
another rather than against itself. It is `truecolor.txt`'s flight in the other
colour mode, so the two ask for the same sky and differ only in how a cell's
colour is spelled — meaning a change that moves one and not the other has landed
in the writer rather than in the renderer. It went in with the fix that made
that mode send `38;5;N` instead of palette values wrapped in a 24-bit sequence
the terminals it exists for cannot read, and the mode had no reference at all
until then while being the one detection handed to any terminal with a `TERM`
entry and no `COLORTERM`. Detection is gone and nothing is handed that mode now
— which makes this the *only* guard on `quantize_256` and the palette-index
path, since every other flight here is truecolor or ascii throughout.

The last three are not decoration. With only the `--demo` pair, the reference
covered two seconds of flight that never leaves sublight — `--demo` spends its
opening six seconds easing the throttle up — so it peaked at a quarter of light
speed with the drive cold. A deliberate change to `TAIL_BRIGHTNESS` did not move
the hashes at all, because a sublight streak is shorter than a subpixel and
takes the branch in `draw_streak` that never reads it. The streak ramp, the
glare, the flash, the Doppler shift and the entire view from outside — band,
lens, arcs and hulls — were all outside the reference.

The orbit case is there for the same kind of reason one step further in. The
view from outside is written to reduce *exactly* to the old arithmetic when the
camera is abeam, which is where `side.txt` has it — so a change that repainted
every angle except that one would have left the reference untouched.
`--orbit 55,35,20` has all three angles off zero at once, which is the only
configuration that turns the bubble's outline off the horizontal and puts the
star band's depth travel, its vertical fold and its wall recycle on the path
as well.

`astern.txt` is the newest and it closes a hole the other six could not see
past: every one of them watches from abeam or from in front. The camera goes
all the way round, and the point the ship's own *track* vanishes at is on the
screen from behind and from nowhere else — so half the range of a control had no
reference at all, and it was the half where a trail stretched in screen space
can be drawn clean through a place the track only ever approaches.
`--orbit -75,6,20` has all three angles off zero, the nose receding, and that
point landing 24 subpixels from the ship and 20 from the nearest edge of a
120x72 canvas; both were checked by shooting the frame, since a point inside the
bubble or half off the top asks nothing.

`steer.txt` closed the widest hole any of the seven before it left: **nothing
here had ever put a hand on the stick**. The four `--engage` runs carry no
autopilot, and the three `--demo` runs stopped four seconds short of the weave —
that cycle spent its opening six seconds on the throttle alone, and 120 frames
at 60 fps is two of them. So the cockpit's whole steering path was unpinned:
yaw, pitch, and the `swung_out` branch that carries a star off one edge and back
in the other. The case is `--demo` again at `--fps 10`, which in headless *is*
the timestep — the same 120 frames become twelve seconds of flight instead of
two. Note that its old distinction has since gone: the weave used to live in the
warp leg alone, so this was the only flight that turned at all; it runs the
whole cycle now, so all four `--demo` flights steer and the split is autopilot
against no autopilot. It earned its keep three times over on the way, moving
alone for both of the lean's faults.

`drift.txt` is the newest, and it is `steer.txt`'s hole seen from outside. The
autopilot flies the *camera* as well as the ship, and not one of the eight
before it could say a word about that: the four `--engage` runs carry no
autopilot, and the four `--demo` runs watch from the cockpit, which reads
neither the orbit nor the zoom. It shares every flag with `steer.txt` but the
view, which makes the pair one flight watched from inside and from outside —
`ansi256.txt`'s relationship to `truecolor.txt`, applied to a camera instead of
to a colour mode. It is parked at `--orbit -60,0,0` rather than on the beam for
two reasons, and both are about what a *moving* camera can ask: the swing is
added to where the flag put it, so a flag reading zero could not tell an
autopilot that honoured it from one that ignored it; and starting aft means the
camera **crosses** the beam mid-flight, at frame 72 with the drive lit since
frame 60, so the ship's own vanishing point stops existing partway through and
the band the drive swaps sides over is crossed as a ramp rather than sat beside.
`orbit.txt` sits at a fixed 55 degrees and `astern.txt` at a fixed -75; neither
ever crosses.

**That angle is derived from `autopilot::CAMERA_TURN`, not chosen**, and it has
to be re-derived every time the camera's speed moves: it is how far the camera
walks in the seconds before the crossing should land, and the crossing has to
land *after* the drive lights at frame 60 or the whole framing says nothing. It
was `-20,0,0` while a turn took 137 seconds; speeding that up to 43 would have
put the same angle's crossing at frame 24, with the drive still cold.

**Any change to renderer arithmetic changes those hashes and turns the test
red.** That is the point of them: an edit meant to touch one thing that touched
the whole sky has to say so. When the change was intended, regenerate
deliberately, with the diff in hand:

```sh
cargo build --release
common="--headless --frames 120 --seed 1 --size 120x36"
./target/release/warp $common --demo --color truecolor > truecolor.txt
./target/release/warp $common --demo --color ascii     > ascii.txt
./target/release/warp $common --demo --color 256       > ansi256.txt
./target/release/warp $common --engage --throttle 1.0 --color truecolor > warp.txt
./target/release/warp $common --engage --throttle 1.0 --view side --color truecolor > side.txt
./target/release/warp $common --engage --throttle 1.0 --view side --orbit 55,35,20 --color truecolor > orbit.txt
./target/release/warp $common --engage --throttle 1.0 --view side --orbit -75,6,20 --color truecolor > astern.txt
./target/release/warp $common --demo --fps 10 --color truecolor > steer.txt
./target/release/warp $common --demo --fps 10 --view side --orbit -60,0,0 --color truecolor > drift.txt
sha256sum truecolor.txt ascii.txt ansi256.txt warp.txt side.txt orbit.txt astern.txt steer.txt drift.txt \
    > tests/golden/frames.sha256
# then put the comment block at the top of that file back
```

Diff the old hashes against the new ones before committing and say which moved.
The split is usually the sharpest thing you have: a change aimed at the hull
moves `side.txt`, `orbit.txt` and `astern.txt` and must leave all four cockpit
flights alone — `warp.txt` is one of them, since it never asks for `--view
side`, and rebuilding the enterprise is the worked example — and one aimed at
the tunnel glare moves `warp.txt` and only `warp.txt`, since
the two `--demo` flights never leave sublight and the outside view goes through
`add_glow_oval` instead. There is a third shape, and it is the sharpest of the
lot: a change to something the outside view only does *off* the beam leaves
`side.txt` alone, because the whole of that geometry reduces exactly to the old
arithmetic where `side.txt` has the camera. Which of the other two move then says
what the change turns on. Stopping the engine lance at its vanishing point moved
`orbit.txt` and no other, and so did putting the drive behind the hull from
ahead; anything gated on the star band's depth travel moves `orbit.txt` **and**
`astern.txt` together, since those are the two flights with a `travel[2]` at all
and they take opposite signs of it — the near wall's ramp and the recycle rule
are the worked example. A hash moving outside the shape its change predicts has
leaked, and `side.txt` moving at all means something meant to be switched off was
reduced to an identity instead.

There is a fourth, and it wears the first one's clothes: a change to the
arithmetic inside `lens`, `exterior` or `Canvas::draw_path` also moves
`side.txt` and `orbit.txt` and nothing else — not because of what geometry those
flights have, but because of which modules they reach. Nothing in the cockpit
goes anywhere near the three; the tunnel is streaks and two glows. So the useful
half of that shape is `warp.txt` staying byte-identical, which is what says a
change to how a span is measured stayed inside the view it was aimed at. Taking
`hypot` off the bent-streak path is the worked example, and
`tests/golden/frames.sha256` carries it in full.

And a fifth, which is the cockpit's version of the third: a change to how the
sky is *steered* moves the four `--demo` hashes and leaves the four `--engage`
ones bit for bit, since `render_headless` gates the autopilot on
`args.demo.is_some()` and those four hold every steering rate at an exact zero
from the first frame to the last. It used to be sharper than that — `steer.txt`
alone, because the weave lived in the warp leg and the three 60 fps `--demo`
runs stopped before it started — and the two worked examples are from then: the
same fault arriving and leaving, steering in the frame the lean turned the
picture to and then taking the lean out of the picture altogether. Each moved
that one hash and left the other seven bit for bit. The second is the more
useful of the two to read: it deleted arithmetic from `Camera::project`, which
every star and every hull vertex in the program goes through, so seven
byte-identical hashes are what says the deletion really was the identity it
looked like. The weave runs the whole cycle now, so all four carry it.

And a sixth, which is the sharpest of the lot and the one that replaced what the
fifth used to be: a change to **where the camera outside is pointed by nobody**
moves `drift.txt` and only `drift.txt`. It is the one flight here whose camera
moves at all — the other three side-view runs hold `orbit_target - orbit` at an
exact zero, so `Flight::advance`'s two eases still add nothing to them, and the
four cockpit flights never read the orbit or the zoom in the first place.

**A hash that fails to move where its change predicted has not been vindicated
either** — it has found a hole, and the hole is worth more than the green tick.
The reference is one ship deep: not one of the three flights with a hull in it
passes `--ship`, so all three fly the enterprise. Clearing the hull band over the span it is
read over rather than written to changed real frames for four of the ships then
in the hangar — three of five camera angles for two of them — and moved not one
hash here, because the enterprise's outline does not happen to expose it. When
that happens, say so, and put the guard at the level the change actually lives
at: that one is a property test in `canvas.rs`, not an eighth flight.

The other answer is a new flight, and the test of which one you want is whether
the hole is a *variant* or a *region*. One ship out of the hangar is a variant,
and another flight would have pinned one more of them while leaving the rest
unlooked at; a property test over `models()` covers all of them at once. But
when the flights covered no camera aft of the beam, what was missing was half the range
of a control — the only half where the sky has a vanishing point at all, and so
the only half a trail can be stretched through one. A trail running past the
point its track vanishes at moved no hash for exactly that reason, and it got
both: `a_trail_never_runs_past_the_point_the_track_vanishes_at` at the level the
fault lives at, and `astern.txt` so the region stops being unwatched. The
matching clause went into the case list's own coverage test beside the two
already there, asked of the parsed `--orbit` rather than of the angle written
above it.

Say in the commit message what moved and why. Regenerating without explanation
throws away the only thing that file is for.

The hashes hold across build profiles and rustc versions but **not across
platforms** — `sin`, `cos`, `exp` and `powf` come from the system maths library
and recycling the star pool turns a one-ulp difference into a different sky. So
they are checked on `ubuntu-latest` only.

The consequence for how you edit: **do not "clean up" duplicated float
arithmetic on the pinned paths.** `Renderer::exterior_camera` spells out the
same shake calculation as `Renderer::camera` rather than sharing a helper, and
says so in its own doc comment — extracting it is exactly the sort of obviously
equivalent edit that moves a float by an ulp and repaints the whole sky. The
ship picker is drawn from `Flight::draw` rather than inside `Renderer` for the
same reason.

Two more things the hashes are sensitive to:

**The RNG draw order.** `StarField::spawn` draws z, then sx, then sy, then the
magnitude sample, then the class, then the phase. `--seed` reproducibility is a
property of that sequence, so hoisting a draw out of a struct literal or
reordering two that "obviously do not interact" gives a different sky. The same
goes for anything that changes how many values are drawn per star.

**The easing form.** Every ease in `src/ship.rs` is `1 - exp(-k·dt)` or
`exp(-k·dt)`, which is frame-rate independent. Rewriting one as `k * dt` is the
classic simplification and it silently makes the flight model depend on step
size. `src/autopilot.rs` was the tree's one counterexample and is the worked
example of what that costs: its throttle came down by a fixed step per *frame*
with no `dt` at all, so the eight-second drop-out shed 1.92 of throttle at
60 fps and 0.32 at `--fps 10`, where it never reached its floor; and the weave
was an impulse a frame against a decay a second, so the angle it swept was
*proportional to the frame rate* over a fiftyfold range. Both are closed forms
of the clock now. The one control that cannot be — the stick, which is
impulse-driven because a terminal reports presses and not releases — is scaled
by `dt`, and the identity that makes that exact is worth knowing: a per-frame
impulse `a` against a decay `exp(-k·dt)` sweeps exactly `a/k` per frame whatever
`dt` is, so an impulse proportional to `dt` sweeps a fixed angle per second.
Perf edits are documented with the measurement that justified them —
`draw_streak` takes one reciprocal per streak rather than a divide per sample,
worth about six percent of drawing time at twenty thousand stars — so do not
undo one without a number saying why. The others on that list are the writer
spelling a colour into one stack sequence instead of seven capacity-checked
pushes (−30% of the write column), `canvas::length_of` in place of `hypot`
(−6% of an exterior frame), `Lens::inv_axes` (−1.8%), `draw_path` reusing the
span it already measured (−2.5%), and the twinkle `sin` skipped at warp, where
its amount is a hard zero (−2%). Together they take the expensive frame — the
outside view at twenty thousand stars on 200×60 — from 21.7 ms to 19.2, which
is the number to reproduce before believing a regression here.

Measure the two trees back to back rather than against a figure written down
earlier. This container drifts about ten percent between sessions, which is
wider than most of the entries above.

### What was measured and is not worth trying again

This tree is compiled with fat LTO and `codegen-units = 1`, and LLVM is
consequently doing far more than a reading of the source suggests. **The trap is
optimising by eye.** `Camera::project` used to call `self.bank.sin_cos()` on
every star, which looked like sixty thousand libm calls a frame and was not one:
`bank` was fixed for the frame, `sin` is `readnone`, and LICM hoisted it —
every `sinf` in the profile was accounted for by the twinkle alone, and taking
the
lean out of the projection for quite unrelated reasons cost the sweep nothing
measurable. Measure first, and measure instructions rather than wall clock when
the change is small; callgrind resolves a percent that a loaded machine's clock
cannot.

The following were each prototyped and measured, and none of them earned its
place:

| tried | measured |
| --- | --- |
| a table of pre-spelled decimals in the writer | **worse** — 0.46 ms to 0.64. One copy of a runtime-length slice costs more than the one to three pushes it replaces; the win was in copying the whole sequence at once, not in precomputing digits. |
| skipping the magnification on the lens samples that discard it | nothing. The dead field is already dropped. |
| `[f32; 4]` canvas pixels, for aligned SIMD splats | a wash — dense frames −1.3%, sparse frames +7%, because every whole-canvas pass moves a third more memory. |
| `get_unchecked_mut` in `splat_inside`, 28% of a cockpit frame's instructions | 2.5%. Not a price worth the first `unsafe` in the tree, but worth knowing the size of before anyone argues for it. |
| row-walking `apply_vignette` with `chunks_exact_mut` | nothing; the indexed form already compiles to the same thing. |
| dropping the `palette_256` → `palette_rgb` round trip in `quantize_256` | nothing; the constant divisors are already folded. |

Cachegrind is the other thing to know before reaching for a layout change: the
D1 miss rate is 1.4% and last-level misses are in the tens of thousands for a
whole run. The working set fits. This is instruction-bound, and shrinking
`Star` or restriping the canvas is solving a problem it does not have.

## Layout

```
src/lib.rs        module list and the orientation doc comment
src/main.rs       parse, fly, report — nothing else
src/app.rs        the three loops (interactive, headless, snapshot) and Flight
src/autopilot.rs  the hand on the stick and the camera for --demo/--screensaver
src/cli.rs        every flag, and the bounds each one is held to
src/view.rs       ViewMode, the outside camera: its orbit, its range, the zoom
src/ship.rs       flight model: throttle, warp, steering, transients
src/starfield.rs  the cockpit's sky — a cone opening forward, plus Camera
src/exterior.rs   the side view's sky — a band the ship flies through
src/lens.rs       the warp bubble: a point-mass lens in an elliptical metric
src/models.rs     the hulls, and how to draw one
src/menu.rs       the ship picker
src/canvas.rs     f32 RGB accumulation buffer, rasterisers, tonemap
src/render.rs     assembling a frame: sky, then what is lit, then the glass
src/hud.rs        the instrument panel
src/term.rs       Screen (double-buffered cells), ColorMode, RawGuard
src/snapshot.rs   PNG writer, behind `--features snapshot`

tests/flight.rs   a whole flight through the public surface, and nothing else
tests/golden.rs   the reference frames, reproduced in process; its own SHA-256
tests/golden/     frames.sha256 — the pinned bytes, and how to remake them
examples/bench.rs where a frame's 16.7 ms goes
docs/             the README's screenshots; excluded from the published crate
```

Everything is `pub` so a flight can be driven from a test, a benchmark, or
another program without going through the binary. `Flight`'s own fields are
private; new state that another module needs comes with an accessor.

### A frame, in order

`Flight::advance(dt)` then `Flight::draw(..)` then present.

`advance` accumulates wall-clock `dt` and steps the simulation at a **fixed
`SIM_STEP` of 1/120 s**, so the flight model behaves the same whether the
terminal keeps up or not. Only the sky for the current `ViewMode` is stepped.

There are **two** clamps on `dt` and they are not the same one. `advance`
holds its own argument to `MAX_STEP_DT` (1.0 s) and turns a non-finite step
into zero — it is `pub`, so that guard sits with the loop it protects rather
than at any one caller. The interactive loop separately holds a measured frame
to `MAX_FRAME_DT` (0.25 s), which is tighter because a frame on a real
terminal is never a quarter of a second, where `advance` has to leave headroom
for headless and snapshot stepping at `1.0 / --fps` with `--fps` floored at 1.

`draw` runs, per view:

1. `canvas.clear()` — the f32 buffer, at `cols × 2·rows` subpixels.
2. Streaks: `field.streaks(..)` → `canvas.draw_streak`. Cockpit only.
   Side view goes through `ExteriorField::draw`, which bends the streaks the
   lens actually reaches — chopping each into arcs and drawing both images —
   and leaves the rest, which at sublight is all of them, on the ordinary
   `draw_streak` path. That fast path is why lighting the drive is what makes
   an exterior frame expensive.
3. The lit things — `add_glow`: cockpit gets the tunnel glare down the throat
   (a core inside a wider halo, both ramping with the *cube* of the warp ramp,
   and both wide and weak rather than narrow and fierce so the streaks stay
   legible through them); side view gets the wash inside the lens shadow —
   `add_glow_oval`, about the *bubble's* centre, to the shadow's own two axes
   and turned by `Lens::turn()`, all three of which move with the camera — so
   the swept-clear region reads as a bubble rather than as a hole punched in
   the sky.
4. The hull, side view only: `models::draw` — every plate handed to
   `canvas.fill_hull` in *one* call, still in far-to-near order, with the drive
   on whichever side of them it belongs on, which is each bell's glow and the
   exhaust it throws. It takes a `view::Eye` — the camera's basis and its
   standoff together, built once in `render_exterior` from the orbit and the
   zoom, alongside the half-length the lens is sized from. It also takes `time`,
   and only the flame's gutter reads it.
5. `apply_vignette`, then `add_flash` on top of it so a drive catching whites
   out the frame edges included.
6. `canvas.resolve_into(&tonemap, &mut pixels)` — HDR to 8-bit RGB.
7. `screen.compose(&pixels)` — two pixel rows fold into one cell.
8. `hud::draw(&mut screen, ..)`, then `menu::draw` if the picker is up.

Then `Screen::flush` (interactive: only cells that changed, and colour codes
only when they differ from the last cell written) or `write_plain` (headless:
a self-contained block of text).

**The panel and the picker are written into cells, after `compose`.** They are
not in `renderer.pixels()`. That is why the snapshot PNG has no instrument
panel.

### The three loops

| | `run_interactive` | `run_headless` | `run_snapshot` |
| --- | --- | --- | --- |
| clock | real, `start.elapsed()` | synthetic, `frame * dt` | synthetic |
| size | `resolved_size(args)` | `resolved_size(args)` | `args.size` or `(240, 68)` |
| terminal | `RawGuard`, alt screen | none | none |
| `--demo` | deadline **and** autopilot | autopilot only | autopilot only |

The autopilot flies the camera as well as the ship in all three — `Flight::fly_itself`
lays a swing of its own on top of whatever `--orbit` parked the shot at.
| output | `Screen::flush` | `write_plain` | `write_png` of `pixels()` |

`--demo 5 --headless` does not stop after five seconds; headless always runs
exactly `--frames` frames. The deadline is checked only in the interactive loop.

**`--fps` means two different things.** Interactively it is only a frame budget,
and only while nothing is being typed — see the drain loop below, which spends
it waiting and abandons what is left of it the moment a key has moved the
flight. The panel shows a smoothed measurement, which is why it reads high for a
moment when the stick is worked. In headless and snapshot it *is* the simulation
timestep — `dt = 1.0 / args.fps` — so changing it changes the flight, and
changing how it is used moves the golden frames.

Three more asymmetries worth knowing before you touch the loops: `P` gates only
`advance`, so a paused `--demo` still runs its autopilot, still repaints, and
still exits at its deadline. **One thing about that had to be qualified**, and
the qualification is not a hedge: the step handed to `fly_itself` is zero while
paused. The throttle and the camera are closed forms of the clock and so are
unaffected either way, but the stick is an impulse against a damper and the
damper is in `advance` — so a pause that stopped the damping and not the
impulse ratcheted the rate with nothing bleeding it off. Eleven seconds of `P`
pinned the yaw at `MAX_YAW_RATE`, held the hull at its full lean, drove
`models::drive_behind_hull` to a hard one, and snapped the ship into a turn on
the way out. In screensaver mode `handle_key` is never reached
at all — any non-release key breaks the loop, so there are no controls, not even
pause. And `R` restores what the *command line* asked for rather than any fixed
number: `args.throttle` rather than `Ship::new()`'s 0.18, and `args.orbit`
rather than `Orbit::LEVEL`. The zoom has no flag, so it goes back to
`ZOOM_DEFAULT`. All three are snapped rather than eased — `R` is the key for
when the view has got away from you, and watching it saunter back is not what
is wanted — and the snap is load-bearing beyond taste: the orbit ease is
asymptotic, so only an exact reset gets the camera back to the bitwise-level
shot the fast paths below are written for.

The interactive loop handles **input at the end of the frame**, in an
event-drain loop that blocks on `event::poll` rather than sleeping — so a key
is acted on the moment it arrives, which at `--fps 5` is the difference between
a screensaver that dismisses when touched and one that finishes its nap first.
`dt` is measured frame-start to frame-start and therefore includes that wait;
**there is no `sleep` anywhere in the loop, and adding one would put a frame
budget back on top of every keypress.**

Acting on the key was only ever half of answering it, and the drain used to go
back to `poll` afterwards and block out the rest of the budget anyway — so the
stick was answered at once and the picture was not. It now carries an `acted`
flag and breaks once something has moved the flight, which is a whole budget off
the wait: measured through a pty with the key written at the start of a frame's
window, 14.95 ms to 0.62 at `--fps 60`, 31.08 to 0.53 at 30, and 65.02 to 0.53
at 15. **The queue is emptied before the wait is cut short** — that is the
`!event::poll(Duration::ZERO)?` beside the flag, and it is load-bearing rather
than tidiness: without it a burst of wheel notches is one frame each rather than
one frame, and a resize the terminal settles out of buys itself a repaint.

**So the frame cap holds while nothing is being typed and does not while
something is**, which is the trade and is worth knowing before it is read as a
bug. Idle is untouched — 4.1% of a core either way, the same bytes — and a key
held down is free, 4.5% against 4.3%, because autorepeat tops out around thirty
a second and thirty extra frames of a sub-millisecond render is nothing. Only a
rate no hand reaches shows at all: a stuck key or a pasted burst at 500 Hz takes
5.3% to 18.7%, and that is self-limiting at the render's own speed. A floor
under how soon an input-driven frame may start was written and thrown away —
gating the break is not enough while the wait above it is still the whole
budget, and shortening the wait as well turned the drain into a spin that took
the held-key case to nearly three times its resting cost while handing the
latency back. If that ceiling ever wants bounding it
wants its own attempt and its own measurement, not that shape.

### The two skies

`starfield::StarField` is a cone opening forward from the canopy: right for the
cockpit, wrong to look at side-on, because there is nothing behind the ship.
`exterior::ExteriorField` is the same idea rebuilt as a band the ship flies
*through*. It is built lazily — a cockpit-only run pays nothing for it, not
even a draw from its generator — and gets its own RNG seeded with
`seed ^ EXTERIOR_SEED` so building it never disturbs the cockpit field's
stream, which is the one the reference frames were recorded from.

The two are steered by different things, and the asymmetry is the design.
`StarField::update` takes the *ship's* three rates, because the cockpit is
bolted to the hull. `ExteriorField::update` takes no steering at all and takes
the *camera's* `Orbit` instead: out there the camera rides with the ship, so a
turn of the hull moves nothing an eye can see, while swinging the eye has to
take the sky with it or the control is a barrel roll wearing new keys —
`swinging_the_camera_sweeps_the_sky_past_it` is the test that says so, and it
picks the case that makes it sharp, lifting the camera at zero azimuth, where
the *flow direction* does not change at all.

**`Ship::bank` reaches nothing the cockpit draws, and putting it back is a
two-commit mistake somebody has already made twice.** The lean into a turn is a
display affectation: it chases the yaw rate, centres itself, and is kept apart
from `Ship::roll`, which is the attitude. There is no way to put a yaw-coupled
angle into the sky's orientation without one of two faults, and both shipped
before this paragraph was written.

Rotate the *picture* by the lean — after the divide, in `Camera::project` —
and steer the sky underneath it, and the flow comes out across the frame at
exactly the bank angle: twenty degrees at the stops. A sky streaming diagonally
while the stick asks for a flat turn is not read as a lean but as a *climb*,
because a sky sliding downward is what going up looks like, and the lean's sign
follows the yaw's, so both A and D lifted the nose.

Steer in the frame the lean has turned the picture to instead, and a held turn
streams flat — but the step becomes the steering with a roll of however much
the lean moved in front of it, and rotations do not commute. Working the stick
round a closed loop leaves a residue about the camera's own `x`, going as the
*square* of the yaw rate and so the same way round whichever way it went: about
nine degrees a second of nose-down that nothing brings back. Parked, working
left and right was enough to fly the sky off the top of the frame. That is
honest rigid-body kinematics, which is the point — a cosmetic lean has no
business accumulating into the flight's attitude.

So the sky is steered about the camera's own axes and the picture is not rolled
at all. `Camera` has no bank; `Camera::project` is a divide and a translation.
`a_flat_turn_streams_the_sky_flat_across_the_frame` and
`working_the_stick_leaves_the_sky_where_it_found_it` hold the two halves, the
second exactly rather than within a tolerance: a yaw turns `x` and `z` and
leaves `y` alone to the bit, so a probe star on `y = 0` projects to precisely
`cy` however the stick is worked.

There is a third position and it is worth knowing before anyone re-derives the
second. The angle shown at projection does not have to *be* `bank`: a
high-passed one washes out to zero in a held turn, which streams flat, while the
path stays unconjugated, which closes exactly. That keeps a visible roll into
and out of a turn at the cost of a tilt during the transient. It was not taken:
the lean is a fact about the hull, and `models::attitude` poses it by
`roll + bank`, which is the one place a lean can be seen without also being
flown.

Note what the removal settled on the way past. `spawn` lays its rectangle out in
unrotated camera space while `escaped` tested the *rotated* projection against
the same rectangle, so past about eleven degrees of lean on an 80x24 terminal
the screen corners fell outside the seeded region and stopped being refilled
from the far plane. The two are in the same space again and the question does
not arise.

The band therefore has five fast paths that are switched **off** rather than
reduced to identities, and all five are exact at `Orbit::LEVEL`. The pool is
turned only when `orbit != self.orbit` — compared, not composed with an
identity, because the angles are unchanged bit for bit when nothing moved them.
Depth travel runs only when `travel[2] != 0.0`, and the vertical fold only when
depth or height moved, because `fold` is *not* an exact identity for a value
already inside its band and the level shot is exactly where `y` never moves. The
fourth is the recycle described two paragraphs down, gated on `turns_z`. The
fifth is the near wall's brightness ramp, gated on `travel[2]` being non-zero —
its width is `NEAR_FADE_STEPS · |travel[2] · step|`, which is a hard zero abeam
and at rest, so the branch in `lit` is never taken there. Widening any of the
five gates moves `side.txt`.
Off the beam three things the band was written never to face come alive. Stars
cross the near and far walls and have to be respawned, and a respawn is handed
the trail it *would* have had, one step back along the track — without that, at
full warp two to four percent of the pool draws a bare point every frame, which
is the sky flickering between streaks and dots that the sideways fold exists to
avoid, arriving by the other door. The Doppler is measured against the
cached `nose` rather than against camera `+x`, which is only the direction of
travel while the camera is abeam; measured against the frame, a chase view would
redden the sky ahead and blue the wake. Abeam the nose is `(1, 0, 0)`, so that
dot product is `pos[0]` to the bit.

And **a star's range changes while it flies, so a step has two of them and the
fold has to name which.** Carrying a folded trail across means turning a
world-space jump into a screen-space one, which is a divide by the range — and
the range it wants is the one `prev` was projected through, not the one the star
ends the step at. `z_prev` is captured beside that projection for exactly this,
and moving it below the depth travel puts the fault straight back. Abeam the two
are the same bit for bit, so nothing showed there for as long as the camera was
pinned; off the beam the trail lands out by `shift · focal · (1/z_new − 1/z_old)`
along one camera axis, `streaks` multiplies it by six at full warp, and the sky
grows a lattice of long faint streaks lying across the flow.
`a_trail_carried_over_the_fold_is_scaled_by_the_range_it_was_drawn_at` is the
guard, and it is a property over the whole pool rather than a reference frame —
`orbit.txt` had the fault recorded *in* it, which is what a pinned frame does
with anything nobody has looked at.

And **a trail is stretched in screen space, so it has to be held back from the
point its own track vanishes at.** This is `models::draw_trail`'s problem
arriving from the other end, and it wears the same fix. `streaks` multiplies the
one step a star swept by six at full warp — but a straight ray running away from
the eye does not project onto a screen-space ray. It projects onto the segment
between the star and the point its direction vanishes at, *approaching* that
point and never arriving, so the multiply drew trail past a place it had already
gone: 77 of 800 came out the far side, the worst half again as far as the point,
and every star at the one depth where the stretch comes to exactly the whole
distance laid its trailing end precisely on it. `trail_head` answers both with
one division and no depth — `k` steps cover `k·u / (1 + (k−1)·u)` of the way,
which is under one for every `u` under one.

**It is switched off in three cases and every one of them is exact.**
`cam.vanishing_point(self.nose)` answers `None` abeam, where the nose lies flat
in the image plane; `None` from anywhere forward of the beam, where the track
runs toward the eye and its projection diverges rather than converging; and the
gate is `stretch > 1.0`, which is a hard identity at sublight. That is why not
one reference hash moved for it — and why the reference grew `astern.txt`, since
the case those three exclusions leave is exactly what nothing was watching.

**The fold is the right boundary for a translation and no boundary at all for a
scale, and a change of range is a scale.** A star's place in the frame is
`focal · pos / z`, so a step that moves its range does not slide it across the
band — it magnifies it about the middle of the frame, by `z_before / z_after`.
Wrapping says nothing about that, and the two directions fail differently:
contracting, the band widens away from a pool shrinking inside it and nothing
refills the frame edges; expanding, a star is put back on the very edge it was
just carried over and the next step carries it over again, so the pool piles
into the off-screen margin and the middle drains. Two seconds of holding `A` or
`D` emptied the top and bottom of the frame and left a bar of stars across the
middle — 45 to 1 across the frame against an abeam baseline of 1.3, or three
quarters of the pool off screen on the other side of the beam.

**What answers that is the profile itself, and not a correction applied to it.**
The band used to hold a flat count per unit range and pay for it every step, by
relocating stars: `1 − λ²` of each shell onto the rim a contraction vacated, and
whatever an expansion carried out scattered back over the whole band. Both were
exactly the right density and both were plainly visible, because neither asked
whether anyone was looking at the star it moved. Astern at full warp, twenty of
a 256-star pool a step landed back on screen out of nowhere; from ahead of the
beam, fourteen a step were taken *off* the screen from wherever they happened to
be. That cannot be faded away — at a tenth of the pool a step there is no fade
both long enough to read as one and short enough to finish — and it cannot be
placed better either, because under a magnification the deficit a step leaves is
*uniform over the screen*, so a uniform refill is the only correct one.

So the flat profile went instead. Advection through a frustum takes a count per
unit range `n(z + d)` to `n(z + d)·(z/(z + d))²`, and the only profile a step
returns unchanged is **`n ∝ z²`** — an even sky, seen through a cone. The rule
that gets there is one line and it is `crate::starfield`'s: **a star that leaves
comes back in through a boundary the flow is entering by, and nowhere else.**
Expanding, the exits are the near wall and the rim and the only way in is the far
wall, where `depth` has already faded a star to nothing. Contracting, the only
exit is the far wall and the way in is the rim plus the near cap — and those
balance exactly, the rim wanting `Z_FAR² − Z_NEAR²` a step against the cap's
`Z_NEAR²` and the far wall supplying `Z_FAR²`, which is why `spawn` picks between
them and picks the range with one draw across `Z_FAR²` and no counter at all.
Nothing is conjured; both ends of every move are off the screen or already dark.
`the_band_settles_at_the_one_profile_a_flow_leaves_alone` pins the profile,
`a_range_change_takes_exactly_the_stars_the_frame_no_longer_holds` pins the rate
and the shape on a single shell, `nothing_lit_is_ever_taken_out_of_the_middle_of_the_frame`
is the regression test, and `the_sky_is_as_even_off_the_beam_as_it_is_abeam` is
the acceptance test over a flight.

Two things about that rule are easy to get subtly wrong and were:

**The rim a contraction vacates is not centred where the band is.** A step
shrinks the band about the middle of the frame *and* slides it sideways, and the
gap the two leave together is an even frame about the place the flow has moved
the middle to. The offset is `travel[0] / travel[2]` scaled by the band's shape
and does not fall off with range, so at 45 degrees off the beam it is wider than
the rim itself and the fill misses the gap entirely: 2.2 to 1 across the frame
against 1.3 abeam, with only 0.57 of the pool on screen. The band wraps, so the
fix is to place on the rim about the origin and then fold the drift in.

**A star is left outside the band for the one frame that carries it off the
screen.** One step of magnification can be worth more than the band's 1.15
margin — `z_prev / z` reaches 1.5 at full warp — so taking a star the moment it
crosses makes it vanish a couple of cells short of the edge rather than sweeping
through it, which is the reported fault in miniature. It is held *still*
sideways for that frame, not merely unfolded: the sideways travel can otherwise
pull it back inside the band it just left, so it lingers at the edge for step
after step, which took the share of the pool on screen from 0.76 to 0.60. Held
still, the recycle's test is exact — the band at the range it came from is the
band it went out through — and every star costs one frame and no more. The
consequence for tests is that "every star is inside the band" is *not* an
invariant of this module and must not be reasserted;
`the_band_holds_together_from_every_angle` holds the sharper form, which is that
a star outside the band is outside the picture.

**What none of this reaches, and it is written down because it looks like the
same thing.** A swing scales the frame too — a rigid turn of the sky is
area-preserving on the sphere, so it scales screen area by the *cube* of its
range ratio, with the extra factor landing on whichever axis the camera turned
across. That scale is neither uniform over the frame nor equal on the two axes,
so the deficit it leaves is spread over the leading half rather than standing on
the rim. What survives is a left-to-right gradient of about 3.5 to 1 *while a
key is actually down*, gone within a step of letting go, with the frame still
even top to bottom — which is why the guard holds the bands tightly and the
frame loosely there. Putting it right means placing by the deficit profile,
which is its own change and its own measurement.

**The near wall gets the ramp the far wall has always had, and it needed it
more.** `depth` reaches zero at `Z_FAR` with zero slope so stars fade up out of
nothing; at `Z_NEAR` it reached *one*, and `Z_NEAR` is a flat plane in `z` while
the band is a fixed rectangle on the screen — so a star arriving there could be
anywhere in the frame, including dead centre where the flow is slowest and the
eye follows it best. Nothing had ever crossed it, because abeam `travel[2]` is a
hard zero. Astern at full warp it was taking four stars a step at a mean
brightness of 0.36 against a sky averaging half that, and twenty-six a second at
sublight: this is the reported bug. `lit` now folds a ramp in before the
exponent, `NEAR_FADE_STEPS` steps deep and scaled by the depth travel so it is a
fixed number of *frames* at any speed and goes to zero smoothly as the camera
returns to the beam — a fixed width behind a flag would dim a tenth of the pool
in the single frame the camera stopped being abeam. It is affordable because of
the profile above: at `z²` the range it dims holds five percent of the pool
rather than a third, and the measured cost to the sky's mean brightness is five
percent.

The cockpit reaches the same profile by the same rule — `starfield::DepthRule`
sends everything that leaves to the far plane, and the steady state of that *is*
`z²`, which is most of why the cockpit reads as distance rather than as a
curtain. The two are now one idea with two parameterisations, because out here
the flow's direction is a camera control and the entering boundary has to be
asked for rather than assumed. **The constants stay separate all the same** —
see below.

The two also carry **separate copies of the same-named constants** — `Z_NEAR`,
`Z_FAR`, `SPAWN_MARGIN`, `DEPTH_FALLOFF` all exist in both modules with
different values (0.9/260 in `starfield`, 18/320 in `exterior`). They are not
duplication to be merged; they describe different volumes.

`MAGNITUDE_FLOOR` belongs on that list and is the one to watch, because it is
the one where the argument is not self-evident: it stands at 0.14 in *both*
modules, and `exterior` already imports `CLASSES` and `shift_color` from
`starfield`, so there is an obvious-looking third import to be made and nothing
in the code to argue against it. The argument is the same one as for the other
four. It is a knob on how lopsided one volume's brightness distribution is, and
the two volumes are different; that they were tuned to the same number is where
they are, not what they are. `SPAWN_MARGIN` and `DEPTH_FALLOFF` were the same
number once too.

One thing *is* genuinely shared: `Camera`, and with it `Camera::project`, which
clips against `starfield::Z_NEAR` in **both** views — including for the hull.
`exterior::Z_NEAR` is the near wall of the band, where a star is allowed to be,
not the plane the projection gives up at. So `starfield::Z_NEAR` looks like a
cockpit-only number and is not one.

Coordinate frame, shared by the flight model and the hulls: **`+z` out the
nose, `+x` to starboard, `+y` down** — right-handed. Camera sizes are in canvas
subpixels, never in cells, and `Camera::focal` is derived from *height* alone,
so horizontal field of view widens with the terminal. That is why the spawn
bound is a rectangle rather than a disc: a circular bound spends most of the
star budget on corners a wide terminal never shows.

### Where the camera outside is

Two types in `src/view.rs` carry the whole of it. `Orbit` is three angles about
the ship — `azimuth` round it, `elevation` over it, `roll` about the view axis
— and `Eye` is `Orbit::basis()` plus the standoff `ship_distance(zoom)`. They
live there rather than beside the hull because they are what the *sky* is
streamed against as well as what the hull is posed by; putting them in
`models.rs` would make `exterior.rs` reach through the ship models to find out
which way it is being looked at.

**All three angles are the camera's, and all three go all the way round.** None
of them stops. The elevation used to clamp at the quarter turn on the argument
that a turn of azimuth already reaches everything past it — true about the
reachable *set*, and irrelevant to a hand holding a key, which is what a control
is for. `Orbit::held` folds each onto a single turn and sends a non-finite one
home to `LEVEL` outright rather than through `clamp`, which passes a NaN
straight out the other side. Nothing else is clamped and nothing needs to be:
`basis` is orthonormal and right-handed at every angle on every axis, because
the screen-right axis does not depend on the elevation at all, so there is no
pole to go singular at.

The fold has a guard in front of it — `turn_of` leaves an angle already inside
a single turn *exactly* alone — and that guard is not an optimisation to be
tidied away. `wrap_signed` goes through `rem_euclid` and back, which does not
return an in-range angle bit for bit, and this is applied on every press and
every eased step, so without it a notch and its opposite would not cancel. What
turns on that cancelling is `ExteriorField`, which lays the whole star band out
afresh whenever the orbit differs from the one it was last laid against: an
angle that came back a hair off would rebuild the rotation path every frame of
a flight nobody is touching the camera on. The band guards its own vertical
fold for exactly the same reason and it is the same trap.

`Flight` keeps an eased value and a target for both the orbit and the zoom, and
both are held on the **target**, not on the eased value — clamp the eased one
and the target winds up past the end, and the first notch back does nothing at
all. The orbit's ease chases through `wrap_signed`, so an axis that wraps takes
the short way round rather than unwinding 350° to reach a target 10° away. The
step is *additive* for the orbit and *geometric* for the zoom, and the reason is
the same in both directions: an angle has no far end to be shoved about and
every part of a turn is worth the same, while a zoom notch big enough to be
worth pressing at the far end would shove the near view through its stop. The
additive form is also exact:
`a_camera_notch_and_its_opposite_land_back_where_they_started` asserts equality
where the zoom's version of it has to settle for very nearly.

**`Orbit::LEVEL` is exact everywhere, and that is what the reference frames
rest on.** Every sine in the basis is an exact zero and every cosine an exact
one, so `Eye::to_camera` at `LEVEL` is `(x, y, z) → (z, y, distance − x)` to
the bit — the hand-written quarter turn it replaced.
`the_level_orbit_is_the_quarter_turn_it_replaced` flies that over the real
vertex data of every hull at every zoom, and
`the_shot_still_opens_exactly_where_it_always_did` in `app.rs` is the acceptance
test for the whole camera: swing it about with the stick, press `R`, and the
frame is the one a flight that never touched it drew, subpixel for subpixel.

## Invariants a careless edit breaks

**Allocation happens before the terminal is taken over.** A failed allocation
aborts the process outright — no unwind, no `Drop`, no panic hook — so anything
installed first would never be undone and the user is left in raw mode on the
alternate screen with no cursor and no prompt. `Flight::new` is constructed
before `RawGuard::new`; keep it that way. The same reasoning is why every
number `cli.rs` accepts is bounded at parse time — `MAX_CELLS`, `MAX_DIM` and
`MAX_STARS` for the ones that allocate, `MAX_COUNT` for `--frames` and
`--warmup`, which are spent rather than allocated, `MAX_SCALE` for `--scale`
and `canvas::MAX_HULL_SAMPLES` for `--aa`, both of which enter *squared* — and
why an ioctl answer is clamped rather than believed. `--orbit` is the one
exception and says why in its own parser: an angle has no end to run away past,
so a preposterous one costs a picture rather than an allocation, and
`Orbit::held` folds it with exactly the fold a keypress gets.

`cli::MAX_STARS` is the one on that list that is *not* only a parse-time bound,
and it is public for that reason. A pool is the one thing there that can still
be resized after the command line has been read, so the `+` key clamps to the
same constant rather than to a number of its own. It used to have one, and it
sat *under* what `--stars` already allowed. There was a third door once — an
automatic count in `app.rs` that sized the pool from the canvas whenever
`--stars` was 0 — and it is gone; the flag is a literal count now, `0` included,
and `cli::DEFAULT_STARS` is what it is when nobody says.

**`RawGuard::new` builds the guard value immediately after `enable_raw_mode`,
before any other fallible call**, so an early `?` still restores. It installs a
panic hook that restores first, so a panic mid-render leaves a readable
terminal. Never `process::exit` inside the interactive loop — `Drop` would not
run. Returning an error is fine: `main` prints after the guard has restored.
And `let _guard = ...` must keep a real binding name; `let _ =` drops it
immediately.

It also catches **`SIGTERM` and `SIGHUP`**, which are the ways out that run no
code at all — a signal's default disposition ends the process where it stands,
so `Drop` never fires and a `lock-command` whose pane went away leaves the user
in raw mode on the alternate screen. The handler **sets an `AtomicBool` and
does nothing else**, and `run_interactive` reads it at the top of each frame:
`restore` writes to `io::stdout`, which takes a lock, and a signal landing while
the frame being flushed holds it would deadlock the process in the one place
that must not happen. Nothing else may be added to that handler on those terms.
`SIGINT` is deliberately *not* caught — raw mode turns off the terminal's own
signal generation, so `Ctrl-C` arrives as a key and `handle_key` answers it, and
a second door on one control would disagree with the picker about who owns the
keyboard. `event::poll` returning `Interrupted` breaks the drain rather than
propagating, for the same reason: a signal cutting the wait short is that flag's
business, not an error to report.

It also turns **autowrap off**, which is not cosmetic. The grid is painted with
explicit cursor moves and never leans on wrapping, so with it left on a terminal
that shrinks between the width last measured and the next flush shears the frame
diagonally instead of harmlessly clipping — and the bottom-right cell, which
every full repaint writes, scrolls the alternate screen.

It takes the mouse *conditionally* and gives it back *unconditionally*, and the
asymmetry is deliberate rather than an oversight. `new` asks for it only when
there is something for it to do — a screensaver has no controls, so it does not
take the terminal's pointer off the user for the duration — while `restore` is
what the panic hook calls, and a panic hook has no way of knowing what was
installed. It also asks for less than crossterm's `EnableMouseCapture`: that
one turns on any-motion reporting as well, and nothing here is aimed at, so
`term::MOUSE_ON` is `?1000h` and `?1006h` and nothing else. `MOUSE_OFF` is the
full set, because turning off a mode that was never on costs nothing and
leaving one on hands the next program a terminal that reports clicks at it.

**`f64` where a screensaver would otherwise break it.** `Flight::time`,
`Autopilot::update`'s `elapsed`, the twinkle phase folded once per frame, the
four shake terms and the engine flame's gutter are `f64` because a screensaver
is left running for days and an `f32` accumulator stops advancing after about
six — freezing the twinkle, the shake and the flame with it.
`Flight::accumulator` stays `f32` deliberately: it is bounded by one sim step
and never drifts. Do not "unify" these.

Everything the autopilot reads off that clock is on the same list, and one of
them needs more than `f64` to survive: the camera's azimuth is a *ramp* rather
than a wave, so `autopilot::ramp` folds the clock onto one period before working
the angle out. Unfolded it would be half a million radians by the time a
screensaver had been up a season, and at that magnitude an `f32` rounds
consecutive frames to the same angle — a camera meant to be gliding would
stutter instead. `autopilot::wave` shares the fold, where it is only tidiness,
because two spellings of one idea is how the two come apart.

The camera's own state is `f32` on the same test, not by omission. The zoom and
its target are bounded at both ends and are not accumulators. The orbit *is* the
accumulator the argument is about — two of its three angles go round for as long
as a key is leaned on — and what settles it is `Orbit::held`, which folds both
the eased value and the target back onto a single turn every step, so neither
can grow. `holding_a_camera_key_never_stops` leans on the keys for four thousand
frames to say so.

**Light adds; hulls write.** Everything in `Canvas` accumulates, because
everything in it is light and a hundred streaks crossing a subpixel ought to
pile up. `fill_hull` is the one exception — a hull is not light, so it covers
what is behind it. Values run past 1.0 and are pulled back once, at the end, by
the tonemap; that is what makes overlapping streaks bloom instead of clip. It
is also what makes the order of the drive and the plates the whole of the
occlusion between them: an opaque write over a glow erases it, so the side the
drive goes down on is the only depth test there is.

A subpixel the hull only partly stands on is written in proportion —
`buf·(1 − cov) + colour·cov` — and that is the one place a hull and the sky
behind it are ever mixed. It is not a hole in the rule: coverage is geometry
rather than transparency, and it is counted in whole samples and divided, so a
fully covered subpixel comes out at exactly one and takes exactly all of what
was under it. It is also why every plate goes over in **one call**. Coverage
composes only once per sample; blend them one at a time and each edge two plates
share *inside* the hull is blended twice, leaving `(1 − a)(1 − b)` of the sky in
a line down the middle of the ship. There is no per-plate fix — the composition
is what is wrong. `two_plates_that_share_an_edge_paint_the_rectangle_they_make`
in `canvas.rs` holds it bit for bit on two synthetic quads, and
`the_sky_never_shows_through_the_seams_of_a_hull` in `models.rs` holds it on the
real fleet by drawing both ways round and counting the difference.

**The band is cleared over the span the resolve loop *reads*, which is wider
than the span the outline is written to, and the two are not interchangeable.**
Writes land in `[u_lo, u_hi]`, the hull's box in sample columns; the loop that
turns samples into subpixels reads whole subpixels, so it reaches every sample
of the columns those two fall in — `u_lo % n` below and `n - 1 - u_hi % n`
above. `Canvas::clear` does not touch the band, and `u_lo` and `u_hi` come from
the whole hull rather than from a row, so anything left uncleared there is the
*previous frame's*, not the previous row's. Clearing only the written span put
a fringe of the last frame's hull down both sides of this one's, and made a
frame depend on the one before it. The guard is
`a_hull_is_drawn_the_same_whatever_the_band_last_held`, and it is a property
test rather than a reference frame for the reason in the golden-frames section
above.

`--aa` sets the grid, `HULL_SAMPLES` is its default, and **at one sample the
whole thing reduces to the rasteriser it replaced, byte for byte** — that is why
the sample grid is entered by scaling the coordinates (`x·n + (n−1)/2`) rather
than by offsetting them, since at `n = 1` that is a multiply by one and an add
of zero. `one_sample_a_subpixel_is_the_rasteriser_this_replaced` checks it
against the old body, kept in the test module as its oracle. Do not delete that
oracle to tidy up; it is the only thing the reduction is measured against.

**Which side of the plates the drive goes down on is a question, not a rule, and
`models::drive_behind_hull` is where it is asked.** Bells fire along the hull's
own `-z`, so one answer covers every bell on every ship: once the exhaust is
leaving the ship away from the eye, the hull is between the two and the drive
belongs underneath. That is the whole of the fix for the drive shining through
the ship from ahead — the enterprise's nacelle bells used to burn as two blue
lamps in the middle of a saucer standing squarely in front of them, because
`draw_engines` ran after `fill_hull` unconditionally and everything in it adds.

The measure is the depth the ship's axis gains over a unit of its length, taken
of the *posed* axis so the lean the hull is holding is in it, and it comes out a
hard zero abeam — `place` puts the standoff on both sides of the subtraction and
it cancels bit for bit, which is what left `side.txt` untouched.

Square to the track and behind it the plume genuinely is the nearer of the two,
and saying so costs something. The Normandy puts its one bell a hair aft of the
hull, so its plume streams into clear sky and the order could not matter less;
the enterprise's impulse bell is mid-ship, and its exhaust runs
*between* the nacelles — 0.19 hull units inboard of the nearer flank, 0.009
under their lower edge, in a fan 0.11 wide. So it misses them in the round and
lies straight across them from the beam, where hull `x` is nearly pure camera
depth. Drawn *under* the plates it
would be chopped by a silhouette it never touches. Over them it shines
through as the wash a hot plume genuinely puts on structure it plays over, which
is still the cheaper mistake and is still the one made, on that side of the beam.
`the_drive_still_washes_the_hull_it_plays_over` is what holds that half, and
`the_drive_does_not_shine_through_the_hull` the other; both measure only the
subpixels the hull covers *whole*, because a silhouette subpixel is part sky
whichever way round the two were drawn and would pass either test.

**The swap is a short ramp and not a switch, and the ramp is load-bearing.** At
the beam the bells sit *on* the silhouette's edge, half in and half out, and
neither order is the right one — so the drive is drawn twice there, sharing its
light between the two sides, each pass skipped when its share is nothing. A hard
swap was tried first and measured: crossing the beam moved a subpixel by 137 of
255 and shifted thirty of them at once, on a ship nobody had touched, because
the autopilot's weave carries the hull's lean across square every ten seconds.
With the ramp the crossing is indistinguishable from the frame either side of
it. `OCCLUSION_BAND` is what sets the width, and both ends of it are held: any
narrower and the step comes back, any wider and the drive goes on shining
through a hull plainly in front of it. Everything the drive draws is linear in
its intensity, so the two passes lay down the one pass they were split from
wherever no plate covers either.

**The streak falloff is physics for a star and a bug for the drive, which is
what `Canvas::streak_spread` is for.** `draw_streak` divides a streak's
per-sample light by its length, so a fast smear spreads instead of burning a
line. That is right when the length *is* the motion, which is every star in the
sky. It is wrong for the engine trail: a lit warp drive throws its lance at the
frame edge, so the length is the terminal's and left alone the drive would burn
dimmer the wider the window — the same flight looking different on two
machines, which is the one thing the whole test suite exists to stop.
`draw_trail` multiplies the factor back out, so what `TRAIL_INTENSITY` names is
the brightness at the nozzle. Anything else that picks its own streak length
rather than being handed one has the same problem and the same answer. Note that
this is measured on the *clipped* segment, by design and in both directions: a
caller dividing it out wants the number that is going to be applied, so a test
comparing a clipped lance against an unclipped one has to divide it out too or
it is measuring the falloff rather than whatever it meant to.

**That lance is stretched in screen space, and the frame edge is not the only
end it has.** A straight ray running away from the eye projects onto a point
that *approaches* the vanishing point of its own direction and never arrives, so
past that point there is no exhaust left to draw. The stretch used to go there
anyway — from anywhere forward of the beam every lance ran clean through and out
the far side, where a symmetric pair of bells swap over and cross — and
`Camera::vanishing_point` is what holds it back now. The useful identity is that
the fraction of the way to that point a tip covers is *exactly* one minus the
ratio of the two depths, so the clamp and the amount the fan narrows by come out
of one division and neither needs a depth. Reach for a hull-unit lance length
instead and two tests say why not: the length is the frame's on purpose.

**The lance is aimed at that point exactly, and what makes that drawable is
`Canvas::draw_fading_streak`.** It used to stop 8% short, and that margin was
never about the geometry. `draw_streak` ramps down to `TAIL_BRIGHTNESS` rather
than to nothing, and every bell on a ship shares one vanishing point — the
plumes run down the same hull axis and differ only by the bell's own reach,
which a point at infinity cannot see — so a lance ending *on* it put a third of
full brightness from every lane of every bell on one subpixel: a bead hanging in
the sky precisely where the exhaust was meant to have gone. The fading variant
takes the floor away, so the sample landing on the point carries a ramp of
exactly zero and the margin has nothing left to buy. Measured star-free at
`--orbit 75,12,0`, peak light within a dozen subpixels of the point:
0.83 stopping short with the floor, 1.02 running the whole way with it, 0.45 as
it is now — further and dimmer at once, against a plume peaking at 2.65.

**Its ramp is measured on the whole streak, and `draw_streak`'s on what survived
clipping.** That is the one difference between them and it is the reason they
are two entry points rather than a flag. Abeam the lance is stretched to the
frame's diagonal and leaves by the edge, so its tail is off-screen; ramp it over
the clipped remainder and it fades to nothing at the edge of the *picture*
instead of at the end of the plume, and a drive whose reach is the frame's stops
short of the frame on every terminal.
`a_lit_warp_drive_trails_off_the_edge_of_the_frame` catches exactly that, and
`a_fading_streak_is_ramped_by_its_own_length_and_not_by_the_window` in
`canvas.rs` is the sharper statement of it. Where the window cuts a plume is not
a fact about the plume.

The abeam shot is the case with no vanishing point at all — `Eye::to_camera` at
`Orbit::LEVEL` is exactly `(x, y, z) → (z, y, distance − x)`, so the hull's axis
lies flat in the image plane and its depth term is exactly zero — which is why
`side.txt` did not move when the lance was first clamped and `orbit.txt` did.
The tail is the other shape: a ramp is not a question about the camera angle, so
giving the lance one moved both of the flights that light a drive.

**Every span on the canvas is measured by `canvas::length_of`, and the sharing
is the point rather than a convenience.** It is `(dx² + dy²).sqrt()`, and it
replaced `f32::hypot` — libm's overflow-safe, correctly-rounded routine, which
was buying a guarantee the inputs cannot need, since every span here is a
difference of two canvas coordinates. It was four percent of every instruction
an exterior frame retired, four calls per segment on the bent path. It lives in
`canvas` rather than in `lens` because **`draw_path` reuses the span it measured
as the length it samples along**, and that shortcut is only sound while both are
spelled by the same function. Tidying `lens.rs` back to `hypot`, or giving
`draw_path` its own inline form, breaks a bit-exactness rather than a style.

**`Canvas::splat_inside` does no bounds checking and will panic.** It is the
innermost loop in the program and it trusts its caller — `draw_streak` and
`draw_path` clamp every interpolated sample to `max_x`/`max_y` before handing
it over, and those clamps are not leftovers from the clipping above them.
Clipping puts the *endpoints* on the canvas; a sample interpolated between two
of them can still land a hair outside. A new primitive written against
`splat_inside` for speed must clamp the same way. Its arithmetic and tap
ordering are also deliberately identical to `splat`'s, so the two produce the
same frame rather than a similar one.

**A sublight frame must be the frame the lens code is not there for.**
`Lens::OFF` is an exact identity rather than a very close one, `Lens::bends`
routes a streak the bubble does not reach straight to `draw_streak`, and
`draw_path` over two points lays down the bytes `draw_streak` would. There are
tests comparing the buffers, not saying they look alike. A star crossing the
ten-Einstein-radii boundary swaps paths mid-flight, so an ulp of disagreement
twinkles a ring into the sky and makes engaging the drive re-render the field
instead of bending it.

**A glyph laid over the frame picks one of three backdrops, and they are not
interchangeable.** `Screen::overlay` is transparent: it writes its ink and
leaves the cell's background exactly as `compose` drew it, and skips its own
spaces so the gaps between the words keep *both* halves of the frame rather
than only the lower one. It used to drop a quarter-brightness shadow behind
every glyph, which bought legibility when a streak was blazing directly behind
a readout and paid for it with a dark box fenced around the whole panel —
mostly a black box, against a sky that is mostly black. `overlay_mark` will not
darken the *foreground* either, taking the brighter of the ink and the pixel it
replaces: the reticle sits inside the tunnel glare, and writing its dim rule
colour as given punched four dark notches into the brightest part of the view.
`overlay_panel` covers outright, spaces included, because a dialogue is in
front of the scene rather than painted on the glass — and its dimming is
applied per stamp, not per frame, so two panel overlays on one cell dim it
twice. The other two are idempotent, which the shadow was not.

**A stamp takes one colour for a whole run**, so anything drawn in more than one
colour on a line is more than one call, at its own column. The panel's nav rows
have always done that — `hud::draw_nav_panel` stamps its rule, its label and its
value separately — and the picker had not: it fused a `│` onto each end of the
row's text and handed the pair over in one call, so the sides of the box came
out in whatever the text beside them was written in, and it changed colour four
times down an edge whose corners were cyan the whole time. `menu::draw`'s
`framed` is the fix, and it is where the two rules meet: three runs laid *end to
end*, because a fourth colour would be free but a cell covered twice would be
dimmed twice. That is also why the spaces inside the frame characters travel
with the frame — under a dialogue a space takes no colour at all, so which run
carries one settles nothing except how often it is dimmed.

**There is no depth buffer and none is needed.** Four things stand in for one,
and all four have to keep holding:

- `exterior::Z_NEAR` (18.0) is beyond the ship, so no star can come between the
  camera and the hull. There is a `const _: () = assert!(...)` in
  `src/exterior.rs` that fails the build if that stops being true. It is
  measured against `view::MAX_SHIP_DISTANCE` — the *furthest* the zoom parks
  the ship — because the standoff is no longer one number. Its opposite number
  lives in `src/models.rs` and guards the other end: at
  `view::MIN_SHIP_DISTANCE` the hull must still clear `starfield::Z_NEAR`, or
  `plates` starts dropping faces whole and the ship comes apart a plate at a
  time while going on looking like a ship. Widening `ZOOM_MIN` or `ZOOM_MAX` in
  `src/view.rs` is what those two are there to stop.
- Faces are wound anticlockwise seen from outside, so the **sign of a plate's
  projected area** is a complete answer to which way it points. This is only
  true because every hull is a closed solid with no zero-thickness plates — a
  fin is a thin box, not a quad, because a quad has no outside and would wink
  out every time the ship rolled it edge-on.
- What survives is painted far to near, and handed over in one call, because
  that order now has to be settled *between the samples* of a subpixel rather
  than between whole ones.
- The drive is drawn on the side of the plates its exhaust is pointed toward.
  It is the one light in the frame that can be either side of a hull — the sky
  is always beyond and the vignette and the flash are over everything — so it
  is the one thing the three rules above cannot place, and `drive_behind_hull`
  is the whole of the answer.

**`q` flies something, it does not quit.** It rolls the ship inside and the
camera outside, and it has never been the way out since it went on the stick.
The only quits are `Esc`, `Ctrl-C` and `Ctrl-D` — and in screensaver mode, any
key at all. While the picker is up it owns the keyboard including `Esc`;
`Ctrl`-modified keys bypass it so `Ctrl-C` still works. In `handle_key`, the
`Char('c' | 'd') if ctrl` arm sits **above** `Char('c' | 'C') => cycle_view`;
reorder them and `Ctrl-C` cycles the camera.

**Key releases are discarded.** They arrive only from kitty-protocol terminals,
and acting on them counts a single press twice.

**`--size` is a fixed size, not a starting point.** `Flight::resize` returns
`false` immediately when it is set. Without that the flag held only until the
first resize event. A resize no longer retunes the star pool at all — it moves
the frustum the stars are laid out in, which is the two `retarget` calls, and
leaves how many of them there are alone. It used to re-derive the count from the
new canvas whenever `--stars` was 0, so dragging a window edge respawned the sky
a few hundred stars at a time and the panel's own count walked about while
nothing had asked it to. A count a window can overrule is not one.

**The sky is a fixed size too**, `cli::DEFAULT_STARS`, 256 on every terminal.
That replaced a density — stars per subpixel, 0.02 where it finished — which is
worth knowing about because the numbers it gave are what this one has to be read
against: 19 stars at 40x12, 76 at 80x24, 172 at 120x36, 480 at 200x60, 1080 at
300x90. What the density bought was constant apparent density; what it cost was
the same flight looking different on two machines, which is the one thing the
rest of this tree is written to stop. It also pulled the wrong way while doing
it — every star is drawn as the segment it swept, so a wider window got more
stars *and* longer streaks at once, and the depth parallax that is most of what
the view is for washed out precisely where there was most room for it. The
constant was thinned three times chasing that and then partly walked back —
0.05, 0.02, 0.01, 0.005, 0.02 — which is four moves of a knob answering the
wrong question. `--stars 0` used to be the sentinel that asked for it and is a
literal now: an empty sky, which the renderer draws quite happily and which is
the only way to see the tunnel, the bubble and the hull with nothing streaming
past them.

**The `+`/`-` keys are held to the same ceiling `--stars` is**, `cli::MAX_STARS`,
so they cannot walk the pool past what the command line would have accepted.
They used to clamp to a separate 20 000 that sat *under* what `--stars` allowed,
which meant `--stars 100000` and a single `+` shrank the pool by four fifths.
Their *floor* is `POOL_FLOOR`, and the argument for it lost half its subject when
the density went: 64 was chosen against an automatic minimum of 300, and a floor
up there sat over the count on any window the density opened thin, so `-` *added*
stars and landed both keys on the same number. Every window opens on the same
count now. What is left is 8 rather than 1, because `+` multiplies by 1.25 and
truncates, so below four stars the key gives the same number back and swallows
the press — `the_star_keys_move_the_pool_the_way_they_point` holds that, and
also that `-` leaned on *reaches* the floor and stops there, which is the only
thing left saying a pool cannot be walked to nothing.

**The one pool the floor may not lift is the empty one.** `--stars 0` is a sky
somebody asked for, so `Flight::resize_pool` returns early rather than letting
`-` put eight stars into it — and the early return is the whole of the fix
rather than a smaller number passed down, because `StarField::resize_pool` and
`ExteriorField::resize_pool` each hold their argument at `count.max(1)` and that
guard is theirs to keep. `+` is deliberately not caught by it: zero is a fixed
point of a multiply, so a key that is not lifted to the floor swallows the press
for the rest of the flight. That asymmetry is also what the two `is_empty` doc
comments now say — a sky can be *started* empty and cannot be *emptied*.

**The same six keys fly the ship inside and the camera outside, and the zoom is
switched off inside.** `WASD` and `QE` are the stick from the pilot's seat and
the camera's orbit from the beam. That is not two features sharing a keyboard by
accident: out there the camera rides with the ship, so a turn of the *hull*
moves nothing an eye can see — the stars stream on as they were and the ship
leans a few degrees — while what can usefully move in a view whose whole subject
is the ship is the eye looking at it. The zoom runs the other way for the mirror
reason: from inside there is no ship to be made bigger. A control that swallows
input and gives nothing back is worse than one plainly not connected, so each
key is wired exactly where it has something to show.

`handle_key` says this three times, as `steers`, `flies_the_camera` and `zooms`,
and they are deliberately the same shape. The two stick blocks sit one *below*
the other rather than nested: a match arm whose guard fails falls through to the
next arm that matches, so each key is written once per view and neither spelling
has to know about the other. Roll went with the pair, which cost the one thing
this view could do that the cockpit cannot — a barrel roll flown and watched
from the beam — and bought a stick that means one thing in each view instead of
two things in one of them.

Three places have to move together if you change any of it: `handle_key`, the
four hint arrays in `hud.rs` (a face per colour mode times a face per view —
and since the README stopped listing the keys, the only place the controls are
written down at all), and the tests that pin the split.
`the_stick_flies_the_camera_outside_and_the_ship_inside` says the ship's rates
never move when a camera key is pressed and that every one of those keys moves
*something*; `the_camera_is_not_connected_in_the_cockpit` and
`the_zoom_is_not_connected_in_the_cockpit` are the other halves, and they also
say the camera and the zoom are *state* rather than a mode — a trip through the
cockpit leaves both where they were.

**The zoom is a dolly, not a lens.** `Renderer::exterior_camera` keeps
`cam.focal` at `h * SIDE_FOCAL` whatever the zoom is doing; what moves is the
standoff carried on `view::Eye` and applied by `Eye::to_camera`. That is not a
stylistic choice: `ExteriorField` caches `cam.focal` and lays its whole band out
against it, so a zoom that touched the focal length would need `retarget` on
every notch — and `retarget` drops every trail, which is a scratch across the
frame. The dolly leaves the sky untouched, and
`the_zoom_moves_the_ship_and_leaves_the_sky_alone` in `app.rs` fails the moment
anything makes `exterior_camera` read the zoom. An *orbit* is the opposite case
and deliberately so: it is meant to take the sky with it, so it is handed to
`ExteriorField::update` separately rather than reaching the camera.

**The wheel is the zoom and nothing else.** `handle_mouse` returns no `Action`,
because a pointer wandering across the window must never be the thing that ends
a flight, and it ignores buttons and motion outright — nothing in this program
is aimed at. While the picker is up it moves the highlight instead, matching the
arrows the dialogue already owns; the picker forces the outside view, so the
alternative would be scrolling the list and zooming the ship behind it at once.
`the_brackets_and_the_wheel_ask_for_the_same_zoom` is what holds the two wirings
together, and it checks the direction as well as the value.

**The warp bubble is measured in ships.** `Lens::for_warp` takes the hull's
on-screen half-length, not the canvas height, so the bubble scales with the
zoom instead of hanging in the frame at a fixed size. That constant used to be
`0.48` of the canvas with a comment saying it was twice what `SHIP_SCREEN_FRAC`
stood at then, `0.24` — a relationship nothing could check, and exactly the sort
that rots. It rotted: that constant is `0.125` now, and the sentence describing
the pair went on quoting the old figure long after nothing in the tree held it.
`the_bubble_is_the_same_number_of_ships_across_at_every_zoom` in `view.rs` is
what checks it now, through both real arithmetics rather than against a
constant.

**The bubble is an ellipse, and the whole module is written in its metric.**
`Lens::offset` reports how far a point is from the centre *in rings* — exactly
1.0 on the ring, whichever way round it is measured — and every question the
module used to answer with a distance and a radius it now answers with that and
a bare number: the shadow is `SHADOW_FRAC`, the reach is `REACH`, the ring is
one. Four consequences worth keeping.

`RING_MINOR` is `1.0 / RING_MAJOR` and must stay derived. The ring then encloses
`π·radius²` whatever the elongation is, so `bends` sweeps the same area it
always did and an exterior frame at warp costs what it always cost — measured,
not assumed: 22.1 ms of drawing before, 21.4 ms after, at 20 000 stars on
200×60. `the_bubble_sweeps_the_same_sky_clear_however_it_is_shaped` in `lens.rs`
is what holds it, integrating through `offset` rather than multiplying the two
constants together.

`inv_axes` is on that list too, and for the same reason: it is `1.0 / axes`
component-wise, it is set in all three constructors beside the `axes` it comes
from, and **nothing may set one without the other.** It is there because
`offsets` used to end on a divide per axis and is the floor every gate in the
module stands on — `bends` and `shadowed` run it over the whole pool, `map` and
`crosses_the_ring` over every sample of every bent streak — so two numbers fixed
for the entire frame were being divided by a dozen times per sample. Worth 1.8%
of drawing an exterior frame at twenty thousand stars, and it moves the last
bit, so it arrived with the reference frames regenerated.

Being an *ellipse* rather than an angle-dependent radius is why this got
cheaper. Membership of an ellipse is a closed form, so `bends`, `shadowed`,
`crosses_the_ring` and `curvature` have no square root in them at all; a ring
whose radius varied with the angle would have needed one in each, on the two
hottest gates in the program. If you reshape this again, reshape it by changing
what `offset_sq` scales by — which now means `axes` and `inv_axes` together,
since the second is the one the arithmetic actually reads.

The wake is the *centre*, not the outline. `for_warp` seats the bubble
`WAKE_SHIFT` of a semi-major axis astern of the ship it is handed, and the
outline stays a symmetric convex ellipse. Skewing the ring fore and aft is the
obvious way to draw a teardrop and it is the wrong one: each image then has to
be solved on its own ray with its own radius, which costs the `θ₊θ₋ = e²`
cancellation the counter-image depends on, and at 2.2 to one it pinches the
waist into a peanut — `1 + A·cos²φ` has a local *minimum* of width at the waist
for any `A` past 0.5. Both ends of the shift are guarded: a `const _: ()` in
`lens.rs` fails the build if the nose leaves the shadow, and
`every_hull_stays_inside_its_own_bubble` in `models.rs` flies every hull in the
hangar through every zoom, a full turn of roll and every camera angle in
`orbits()` to say the same thing about real geometry rather than about a ship
one unit long. That test carries **two** bounds and the difference is not slack:
square to the track the hull is inside the *shadow*, and swung round toward the
nose or the tail the honest bound is the *ring*. The reason is perspective
rather than the bubble — end-on the hull's length lies along the line of sight
and its near end magnifies while the bubble it sits in does not, and the drawn
outline is the silhouette of the spheroid rather than the perspective
silhouette, which would be a general conic with its own centre.

**The bubble is a solid, so the camera foreshortens it.** `for_warp` takes the
nose direction in camera space — `Orbit::nose_in_camera` — and works the outline
out with support functions: the long axis runs from `L` broadside down to `W`
end-on while the short one never moves, which is why a ship seen head-on sits in
a circle rather than in a line and why nothing has to bound how far the camera
may be swung. The `turn` it stores is which way that long axis lies, as
`(cos, sin)`, and it is exactly `(1.0, 0.0)` whenever the camera is abeam of the
track — at any elevation and any roll of its own. Broadside is *branched* rather
than computed for the same reason `offsets` branches on a zero sine:
`√(RING_MAJOR²)` would very probably survive IEEE arithmetic, and "very
probably" is not what the reference frames are pinned with. The branch in
`offsets` is also a measured five percent of drawing time in a warp frame at
twenty thousand stars — 21.3 ms against 22.3 — because it sits under the two
hottest gates in the program.

**The hull points along the track, and `models::attitude` is where that is
kept.** Out there the direction of travel is the one thing that cannot move:
the ship flies where its nose points, `ExteriorField` streams the sky along
that track and takes no steering argument at all, and there is no horizon for
an angle to be measured against. So `heading` and `pitch` are a *compass* — an
instrument reading, and the panel's business — not a bearing off some fixed
frame, and posing the hull from either tips it off the track and leaves it
there. That was a real bug: a few seconds of `W` inside, then `C`, and the ship
sat nose-high for the rest of the flight against stars streaming dead level.
Pitch and yaw are leans off the *rates* for that reason, so they say the pilot
is on the stick now and hand the ship back to its track when it is let go. Roll
is the exception and is taken as flown, because it turns the ship about the
very axis it is flying along and so moves the profile without moving the nose.

**Each colour mode is a different *spelling*, and the mode has to reach the
writer for that to be true.** `--color 256` sends `38;5;N` and truecolor sends
`38;2;r;g;b`; `quantize_256` splits into `palette_256`, which answers the entry
number the wire wants, and `palette_rgb`, which turns it back into a colour for
the callers that go on blending with it. It used to snap to the palette and
then send the *values* in a 24-bit sequence, which the terminals that mode
exists for cannot read. Two things follow. The index is worked out in
`Sink::set_color` rather than carried on the `Cell`, because a mark takes the
brighter of itself and the pixel beneath and a dialogue dims what is behind it,
so the colour has to survive `compose` — and because the writer runs once per
colour *change* where `compose` runs twice per cell. And it cannot assume what
reaches it is already snapped, because that dimming lands between entries.
`the_escapes_written_by_hand_are_the_ones_crossterm_writes` runs over both
modes: the hand-spelled bytes and crossterm's `AnsiValue` have to agree, or a
Windows console without virtual terminal processing draws a different picture
from the one every test here checks.

**Which spelling is used is a command-line answer and nothing else.** `--color`
defaults to `truecolor` and there is no `auto`. There was one, and
`ColorMode::detect` behind it read `COLORTERM` and then `TERM` — so a terminal
exporting no `COLORTERM` was handed `Ansi256` whatever it could really do,
which is most terminals, and the mode this whole canvas is designed for was the
one the program least often opened in. Keeping `auto` as a non-default value
was considered and rejected: with the default at truecolor it is a second
answer to a question already answered, and it is the answer that guesses.
`--color auto` is now a parse error naming the three that remain, which is
`the_narrow_colour_modes_are_asked_for_by_name` in `cli.rs`, and the default
itself is pinned in `defaults_are_sane` beside the frame rate and the star
count. **Nothing in the tree reads an environment variable** — those two
`env::var` calls were the only ones, so `grep -rn 'env::var' src/` finding
anything at all means a door has been reopened. Note what did *not* move: not
one golden hash, because every flight in `tests/golden.rs` passes an
explicit `--color` and none of them ever exercised the default.

**`--color ascii` emits no escape codes beyond cursor moves, and no byte
outside printable ASCII.** Not even a reset — on a `TERM=dumb` terminal even
`\x1b[39m` arrives as visible garbage, and there is a regression test for it.
CI greps for both properties. Note what that does *not* say: nothing puts a
terminal into this mode, `TERM=dumb` included. The flag is the whole of the
contract, and the paragraph above is why.

Three consequences for anything drawn in that mode. `stamp` discards the colour
outright, so `LABEL`, `VALUE`, `ACCENT` and the rest do nothing and the choice
of **glyph** is the only thing separating instrument from sky. Which is why the
ASCII face's odder picks — `|` for a throttle bar, `[` and `]` for the reticle
— are chosen against `term::ASCII_RAMP` as much as against the alphabet: a mark
drawn in a character the starfield also draws reads as a bright star. `#`, `*`
and `+` are all in the ramp — which is why none of them is a bar or a reticle,
and `the_ascii_shapes_stay_clear_of_the_brightness_ramp` is what holds the
face's marks out of it. Note which way round that test runs: it pins the
*shapes*, not the ramp, and the words are deliberately let alone — `NAV` beside
a star is legible, a bar of `#` against `#` is not. Second, the face's
*glyph* substitutes are one column wide so the panels lay out identically and
only the ink differs — the hint strings are the exception, since `UP/DN` is
genuinely wider than `↑↓`, so that row is right-aligned on a character count
rather than assumed to match. Third, vertical resolution halves: with no colour
to carry two pixels, `pixel_pair` averages both subpixels into one ramp glyph.

**The panel has two layouts, not one that shrinks.** Below `MIN_COLS` (46) or
`MIN_ROWS` (12), `draw_compact` runs instead and drops the reticle, the nav
panel and the hints rather than squeezing them.

### Which beam the shot opens on

**Starboard**, and the arithmetic is the thing to check rather than the prose:
`Eye::to_camera` at `Orbit::LEVEL` is `(x, y, z) → (z, y, distance − x)`, a
quarter turn about the ship's own down axis that puts the nose to screen right
and the starboard side toward the camera. The three dot products come from
`Orbit::basis`, which is orthonormal of determinant one at every angle — so face
winding survives it, where the mirror image looks almost the same and inverts
every facing test in `models::plates`. Two doc comments said "port" for a while
and neither changed what was drawn, so if you find prose and geometry
disagreeing again, `basis` and `to_camera` are the answer.

The camera can now be anywhere, so "which beam" is only where it *starts* — but
that spot is load-bearing far past taste, because it is the one place the whole
outside view reduces to arithmetic older than the orbit: an exact swizzle, an
exactly abeam sky travel of `(-1, 0, 0)`, an unforeshortened bubble outline, and
a hull axis flat in the image plane with no vanishing point at all. `side.txt`
is recorded there and `orbit.txt` is recorded away from it, which is what makes
the pair able to tell those two halves apart.

## Conventions

### Comments

This is the most distinctive thing about the codebase and the easiest to get
wrong. Comments here are **essays about why**, not labels on what. They are
written in full sentences, often several, frequently with an em-dash, and they
routinely explain the alternative that was rejected and what went wrong with
it. A comment that restates the code is worse than no comment.

```rust
/// Stars live in a *rectangular* screen-space frustum this much larger than
/// the visible area. A circular bound wastes most of the star budget on the
/// corners of a disc that never intersects a wide terminal; the margin that
/// remains is the run-up a star gets before it reaches the screen, so nothing
/// a turn brings into view has to appear in view.
const SPAWN_MARGIN: f32 = 1.3;
```

```rust
// Cleared, not just re-length-ed: the row stride follows the width,
// so light left over from the old layout would reappear somewhere
// it was never drawn. Every frame clears before it draws, but that
// is the renderer's habit, not something resize may lean on.
```

Every module opens with a `//!` block that says what the module is for and
names the one or two decisions carrying the most weight — `src/models.rs` and
`src/lens.rs` are the fullest examples. Tuned constants carry a `///` saying
what pulls them in each direction and, where it applies, what the value was
before and what that broke. Regression tests carry a comment naming the bug.

Third person, no hedging, no first person, no emoji, no `TODO`. Wrap at 80.

### Naming and idiom

Test functions are **sentences**: `the_seed_is_the_whole_of_the_state`,
`a_preposterous_terminal_is_clamped_rather_than_believed`,
`warp_opens_a_hole_in_the_sky_around_the_ship`. Assertion messages are
sentences too, and say what should have been true.

`f32` throughout the render and flight paths; `f64` only where accumulated time
demands it (see above). Errors are `io::Result` and propagate with `?`.

No `unwrap` outside tests, and at present there is not one anywhere in the
tree. The last was an infallible `min_by_key(..)` over the six levels of the
colour cube in `quantize_256`, and it went when that scan became the
`NEAREST_CUBE` lookup table.

Buffers a frame needs are allocated once and reused — `Renderer::pixels`,
`Screen::scratch`, the exterior field's two arc scratches, and the canvas's
hull band, which is one output row of subpixels `HULL_SAMPLES` sample rows deep
and therefore grows with the width alone rather than with the area — and nothing
allocates per star. The hull path is the exception there:
`models::plates` builds its vertex, screen and plate vectors fresh each
side-view frame, and `models::draw` maps them into a `Vec` of `Facet` on top of
that, which is only cheap because a hull is a few dozen faces.

### Tests

Unit tests sit in a `#[cfg(test)] mod tests` at the bottom of the module they
test and may reach into private state. `tests/flight.rs` deliberately cannot:
it drives a flight through the same public surface any other program would have
to use, which is what keeps the library honest about being usable without the
binary.

Tests assert **properties**, not snapshots of numbers: that a seeded flight is
reproducible and two seeds differ, that the ship is the same fraction of the
frame at 40×12 and at 300×90, that every view is reachable on the `C` cycle,
that a frame never re-sends a colour it is already using. Several iterate over
`models::models()` or `ViewMode::ALL` so a new entry is covered automatically —
keep that property when you add to either.

`models.rs` has a third list of that kind, and it is two lists on purpose.
`orbits()` is a spread of camera angles chosen to cover the *basis* — both
poles and past them, head-on, dead astern, and corners with all three angles off
zero at once — and `forward_quarter()` is a second spread chosen so the ship's
own vanishing point lands on the canvas with the hull clear of it. Not one of
`orbits()`'s angles meets both halves of that, which is exactly how a lance
several frame widths too long went unnoticed: every sweep flew straight past
the one question. A sweep that can stop asking its question should say so —
`a_plume_stops_at_the_point_it_vanishes_at` counts the frames that qualified and
fails on a floor well under today's number, so a later nudge to the framing
cannot quietly take the coverage down to a corner of the range while the test
goes on passing.

### Commits

Imperative subject, sentence case, no scope prefix, no conventional-commits, no
trailing period. Real ones:

```
Make the hulls solid
Put the stick on the camera in the view from outside
Take the division out of the streak sampling loop
Reject nonsense flags, and stop shouting colours at a pipe
Wait for input instead of sleeping through it
Measure a hull's outline finer than a subpixel
Let the lance run to its vanishing point and fade out there
```

Bodies are the same essayistic prose as the comments, wrapped at ~72: what was
wrong, what was tried, what the trade was, and what did *not* need to change
and why. Multi-paragraph is normal. Work happens on a branch and lands through
a pull request.

## Common tasks

**Adding a ship.** Build it in `src/models.rs` from the section/shell
primitives rather than typing vertex and index lists — the primitives get the
winding right by construction, and one wrong index is a plate that quietly
disappears at some particular angle. Keep it closed, keep it inside the unit
box, give it a lowercase ASCII `name` and a one-line `blurb`. `--ship` and the
picker both read `models::models()`, and the tests iterate it, so nothing else
needs touching. The cockpit draws neither the hull nor its name — the panel's
`SHIP` row is gated on `ViewMode::Side` — so the golden hashes do not move.

That "nothing else needs touching" is very nearly true and was not: four tests
had a ship's *name* written out in quotes, and every one of them went stale the
day that ship left. They are all driven off `models()` now. Two more traps for
whoever next takes a ship *out*. Removing one can take a primitive with it —
deleting the needle left `Builder::hoop` with no caller, and a `#[cfg(test)]`
use does not silence `dead_code` in the lib build that `--all-targets` also
compiles, where clippy is `-D warnings`. And any test counting frames *in total*
over `models()` is a floor on the hangar's size as much as on whatever it meant
to measure: `a_plume_stops_at_the_point_it_vanishes_at` counted 154 over six
hulls and 56 over two, and it counts per ship now.

`shell` is `loft` at four sides and the count is per *solid*, so a blade or a
strut goes on costing four points while a tube or a disc pays for being round.
Keep it a multiple of four: `hx` and `hy` are half-extents at 4, 8 and 12
because a vertex lands on each axis exactly, and at ten the ring overshoots
`hy` by five percent. `Section::ring` hands the corners straight back at four
rather than recomputing them, and
`four_sided_a_loft_is_the_shell_it_replaced` holds that bit for bit — every
ship in the hangar is built through it, and an ulp there repaints two reference
frames. Face count is cheap but not free: the enterprise went from 100 faces to
242 for about 3.7% of the drawing time on the exterior frame where the hull is
the largest share, and nothing measurable on the one at twenty thousand stars.

An `Engine`'s `radius` is doing more work than it looks like. It sets the bell's
glow, and it also sets how long and how bright a trail that bell throws — a
bigger drive throws a longer flame, which is what hands the fleet its variety
without a per-ship table. `NOMINAL_BELL` is 0.12, and it is a fixed reference
rather than a mean of whatever happens to be in the hangar — the surviving bells
run 0.07 to 0.15, so it no longer sits in the middle of them, and recentring it
would rescale every plume's brightness and repaint `side.txt`, `orbit.txt` and
`astern.txt` to say nothing. A new ship far outside that range will trail unlike
anything else in the hangar, which may well be the point, but it is a decision
rather than a detail.

The picker mostly takes care of itself now. It reserves `CHROME_ROWS` (6) for
its frame, title, rule, blank and footer, gives what is left to the list, and
windows that on the cursor — so another ship costs nothing on a terminal with
the room and scrolls on one without. It used to lay the list out at full height
and clip, which at six ships already overflowed `MIN_ROWS` and dropped the
footer and the closing rule; that is what the box-always-closes test is for.

Note what that leaves at a hangar of two: `MIN_ROWS` has room for three, so
**no terminal the box will draw itself in can window the list at all**, and
`a_short_box_windows_the_list_and_says_that_it_did` asks `title()` directly
rather than going through `draw`. It went through `draw` once, and when the
hangar shrank under it the test did not turn red — both of its framings fell
below `MIN_ROWS`, took the one-line fallback, and passed on a picker that was
never drawn. A third ship does not fix that; a fourth would quietly make the
old shape satisfiable again.

Two things a new ship still moves. The box's *width* is derived from the longest
blurb, so a long one widens the dialogue on every terminal that can afford it —
keep blurbs to a line. And when the list is windowed the title carries an `n/N`
counter, which is the one degradation here that announces itself: shedding a
blurb can go quietly, hiding whole ships cannot.

**Adding a camera.** Add the variant to `ViewMode::ALL` in `src/view.rs` — the
cycle and `label()` are written so a third costs one line — then the arm in
`Flight::advance`/`draw`, the `ViewArg` in `src/cli.rs`, and, if the controls
differ, a *pair* of hint arrays in `src/hud.rs`, since every face is spelled
twice over for the ASCII terminal. `hud::Readout` carries `view` for exactly
this, and `Glyphs::hints_for` is where a third answer goes. Then decide what the
stick does in it: `handle_key`'s `steers`, `flies_the_camera` and `zooms` are
all `== ViewMode::X` today, so a third view wants a third answer to each of the
three — and a key matching none of them falls through to nothing at all, which
is a control that has quietly gone missing rather than a compile error.

**Changing the controls.** Four hint arrays in `src/hud.rs` — a face per colour
mode times a face per view — and one test holds all four to the same shape:
exactly three tiers, strictly decreasing in width so the first that fits is the
most detailed, and the shortest no wider than `MIN_COLS - 2`. It also pins what
each face has to *name*: `WASD steer` on every cockpit tier, `WASDQE cam` on
every outside one and `steer` on none of them, and the zoom out there and
nowhere else. The shortest tier is a tighter budget than it reads; the narrowest
cockpit tier uses 41 of its 44 columns and the narrowest side tier 40, so a new
control does not simply get appended to all three.

Two worked examples, and they are the two walls. `[] zoom` costs nine columns
with its separator, which is more than the narrowest side tier has spare, so it
went on the widest tier only — where `M ships`, `P pause` and `R reset` already
stop, on both faces of both views. And
naming the camera cost three columns over `QE roll`: `WASDQE cam` rather than
`WASDQE camera`, because the ASCII middle side tier is 56 characters and
`draw_hints` needs `chars + 2 <= cols`, so it fits a sixty-column window with
two columns to spare — and sixty is the width `tests/flight.rs` flies at, so a
longer word there would shed the tier and lose the *throttle* to gain the
camera. Those tiers are now the only place the keys are written down anywhere,
so a control that fits none of them is one nothing ever tells the user about.

**Adding a NAV readout.** The panel has one spare row and no test guarding it.
The bottom three rows are counted *up* from the bottom — status at `rows - 3`,
throttle at `rows - 2`, hints at `rows - 1` — while the NAV panel is counted
*down* from the top and closes at `2 + rows.len()`. At `MIN_ROWS` (12), in the
side view where the `SHIP` row already makes six, the closing rule lands on row
8 and the banner on row 9. A seventh row collides.

**Changing what a stick key does.** It is written twice, once per view, in
`handle_key`'s guard-gated arms — the cockpit block first, the camera block
below it, each key spelled once per view and falling through from one to the
other — and in three places besides: the hint tiers in `hud.rs` (both faces)
and the pair of tests in `app.rs` that pin the split,
`the_stick_flies_the_camera_outside_and_the_ship_inside` and
`the_camera_is_not_connected_in_the_cockpit`. Between them they say the ship's
rates do not move when a camera key is pressed, the camera does not move when a
ship key is, and every one of the six keys moves *something* in both views — a
key that swallows the press and gives nothing back is the thing this view used
to have.

**Adding a flag.** `src/cli.rs`, with a bound and a `value_parser`, and
`conflicts_with`/`requires` if it only means something in combination. Every
number there is bounded; unbounded ones have caused real bugs, and the
regression tests at the bottom of the file name them. Snapshot-only flags are
`#[cfg(feature = "snapshot")]`. Two things `--orbit` is the worked example of:
a flag whose value is not a number takes a parser that produces the domain type
rather than a string, so an unknown ship or a malformed angle is a message at
the command line rather than a silent fallback — and a flag whose range starts
with a minus sign needs `allow_hyphen_values`, or clap reads `--orbit -75,10`
as a flag it does not know. Degrees rather than radians, because this is the one
place a person types the number.

**Changing the renderer.** Expect the golden hashes to move, and regenerate
them in the same commit with the reason written down. Run
`cargo run --release --example bench` before and after if the change is in a
hot loop — `draw_streak`, `resolve_into`, `ExteriorField::draw`.

Two things about that instrument, because it is easy to trust further than it
goes. **It does not measure `Screen::flush` at all**: the write column times
`present_plain`, which is the headless writer and emits every cell, where the
interactive path diffs against the front buffer. A change aimed at the cell diff
is invisible here and wants a harness of its own. And it reports a bare mean of
one run per case, which cannot tell a 2% improvement from the scheduler — take
the minimum of five to seven sweeps before believing a small number.

For anything finer, callgrind counts instructions and does not care what else
the machine is doing. Release carries no debug info, so line attribution needs
the profile overridden on the invocation rather than in `Cargo.toml`:

```sh
CARGO_TARGET_DIR=/tmp/prof CARGO_PROFILE_RELEASE_DEBUG=true \
    cargo build --release --examples
valgrind --tool=callgrind --cache-sim=no /tmp/prof/release/examples/bench 200 60 5000 side truecolor
callgrind_annotate --auto=no callgrind.out.*
```

## CI

`.github/workflows/ci.yml`, four jobs:

- **test** — `cargo test --locked` then `cargo test --locked --all-features`, on
  Linux, macOS and Windows. The matrix is the point: the renderer's whole job is
  to behave the same everywhere. There is deliberately no `cargo build` step in
  front of them; `cargo test` compiles the bin targets too, so one bought
  nothing but a second full compile on each of three operating systems.
- **lint** — `cargo fmt --all --check` first (it needs no build), then clippy
  with `-D warnings`, then `cargo package --list` and `cargo package`. That last
  pair is there because `exclude` in `Cargo.toml` is hand-maintained and its
  failure mode is quiet: a crate that builds from the repository and not from
  the tarball people install. The `--list` runs first so a diff that drops a
  file says which one in the log rather than in a build error. Touching
  `exclude` means watching those steps.
- **msrv** — reads `rust-version` from `Cargo.toml` (currently **1.85**) and
  `cargo check`s against it. Bumping the floor means editing that field.
- **headless** — same seed twice gives the same bytes, different seeds give
  different ones, the bytes match `tests/golden/frames.sha256`, `--color ascii`
  is really ASCII, and a snapshot can still be written.
