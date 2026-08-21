# CLAUDE.md

Notes for AI assistants working in this repository. [`README.md`](README.md) is
the user-facing document, and it is deliberately thin: what the program is, how
to run it, and what the flags do. It no longer lists the keys or the tmux
wiring, so it is not a second copy of anything here to be kept in step. This one
is about the inside: how the code is arranged, what it depends on that is not
obvious, and how to write changes that read like what is already here.

## What this is

`warp-rs` is a terminal starfield renderer: a starship flown at warp, drawn
with the upper half block `▀` in a character grid. Stars live in a world-space
volume the ship is somewhere *inside*, are projected each frame through
whichever camera is flying, and are drawn as the segment each one swept over
the last few seconds of flight — accumulated into a floating-point buffer and
tonemapped at the end, so the hyperspace smear falls out of the motion rather
than being drawn as an effect.

The distances are real, which is most of what the sky's behaviour follows from.
A star is placed by its own photometry — how bright it is against how bright it
looks — so the nearest are a few light years off and the brightest giants are
thousands. At impulse that makes the sky *still*: 0.9 c against a star four
light years away is a hundredth of a subpixel a second, and a full-impulse frame
differs from a parked one only by the twinkle. At warp, five and a half light
years a second, the near sky tears past a far one that barely moves, which is
the depth cue the old three-hundred-unit-deep volume could not have had.

One crate, `warp-rs`, with a binary named `warp`. `src/lib.rs` carries
everything; `src/main.rs` is thirty-odd lines of entry point. That split is
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
touch the manifest, and so is exactly how far the enforcement goes. The
committed `Cargo.lock` and the `--locked` every cargo invocation in CI passes
catch a manifest edit whose lock was *not* regenerated: re-enabling crossterm's
`derive-more` adds packages, so it fails with *cannot update the lock file*
rather than landing green. What they do not catch is a feature that pulls no
new package, or a `cargo add` whose regenerated lock is committed alongside it
— which is what that command produces by default. So `--locked` guards against
a forgotten regeneration, and the rule itself is guarded by whoever reads the
diff.

The other consequence of pinning the tree is that it will never drift into a
fix either, so nothing here would ever report an advisory against one of the 41
crates. That is what the `audit` job is for; it runs on a schedule rather than
on pull requests, because it takes minutes to build the auditor and says
nothing about a diff.

`tests/golden.rs` is the rule taken to its conclusion: it spells SHA-256 out in
a page of shifts and adds rather than adding a dependency for one test, and
`the_digest_agrees_with_the_published_answers` checks it against the canonical
vectors and the block-boundary lengths before anything else trusts a word of
it.

## Commands

```sh
cargo build --locked                    # default features; what people install
cargo test                              # 344 unit + 17 elsewhere, about 25s
cargo test --locked --all-features      # 348 unit — adds the snapshot-gated ones
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
cargo run --release --example bench 200 60 8 side truecolor
```

`bench` prints sim / draw / write milliseconds per case against the 60 fps
budget. Its arguments are `[cols] [rows] [magnitude] [view] [color]` — the
third one is how faint a sky rather than how many stars, since that is what the
program takes now — and the colour mode is a *column* rather than an assumption
for the reason
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
anyone thought to check it. Reshoot them whenever the sky changes: they are the
first thing anybody sees of this program and there is no test that will tell you
they have gone stale.

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

The `headless` CI job hashes these two now, so a reshoot that changed the sky
turns it red rather than going unnoticed — which is what happened for the whole
life of the renderer before anybody compared. That is a check on staleness and
not on taste: a moved hash is a reshoot to be looked at and committed, and
whether the picture is *good* is still a judgement nothing can make for you.
What the flags guarantee is only that the drive is lit and fully spooled: 600
warmup
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

**Ten flights, and the case list lives in four places** — `CASES` in
`tests/golden.rs`, the comment block in `frames.sha256`, the `headless` CI job,
and `.gitignore`. It is *checked* in all four now:
`the_case_list_says_the_same_thing_in_all_four_places` reads the workflow and
the ignore list and asks that each act on every flight, in both directions, so a
new one cannot be added to `CASES` alone and a deleted one cannot be left
behind. It matches on the redirect that writes a file and the line that ignores
it rather than on the name appearing anywhere, which is the difference between a
check and a formality: both files carry a paragraph naming all ten, and deleting
`/drift.txt` from `.gitignore` passed a plain substring search. The prose is
still prose and still yours to keep level. They share
`--headless --frames 120 --seed 1 --size 120x36` and differ in what they make
the renderer do: three `--demo` runs in truecolor, ascii and 256, one
`--engage --throttle 1.0`, one of those from `--view side`, the same again with
the camera swung off the beam by `--orbit 55,35,20`, one more from *behind*
the ship at `--orbit -75,6,20`, one more `--demo` at `--fps 10`, that last one
again from `--view side --orbit -60,0,0`, and one from dead *ahead* of the ship
at `--orbit 90,0,0`, which needs `--fps 10` of its own for the reason below.

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

The last three are not decoration, and what they are *for* has changed under
them. With only the `--demo` pair, the reference covered two seconds of flight
that never leaves sublight — `--demo` spends its opening six seconds easing the
throttle up — so it peaked at a quarter of light speed with the drive cold. A
deliberate change to the ramp along a warp streak did not move the hashes at
all, because a sublight streak is shorter than a subpixel and takes the branch
in `draw_streak` that never reads it. The streak ramp, the glare, the flash, the
Doppler shift and the entire view from outside were all outside the reference.

Each of the later cases went in to close a hole of that kind, and most of the
holes were in the star band's own machinery: the fast paths that reduced to
identities when the camera was abeam, the sign of the depth travel, the ramp the
near wall needed, the fold that carried a trail across the edge. **All of that
is gone**, so those reasons no longer describe anything. Read the spread as a
grid instead — ten flights over one sky, at both speeds, from six camera
angles, in three colour modes, with and without a hand on the stick — and
`tests/golden/frames.sha256` carries the same grid in full.

Two of the old reasons survive intact and are worth keeping in mind. `side.txt`
is still the shot with **no vanishing point at all**: `Eye::to_camera` at
`Orbit::LEVEL` is an exact swizzle, so the hull's axis lies flat in the image
plane and the engine lance's clamp cannot move that frame. And `drift.txt` is
still parked aft of the beam so the camera **crosses** it mid-flight, at frame
72 with the drive lit since frame 60, which traverses the band the drive swaps
sides over as a ramp rather than sitting beside it. `orbit.txt` sits at a fixed
55 degrees and `astern.txt` at a fixed -75; neither ever crosses.

**That angle is derived from `autopilot::CAMERA_TURN`, not chosen**, and it has
to be re-derived every time the camera's speed moves: it is how far the camera
walks in the seconds before the crossing should land, and the crossing has to
land *after* the drive lights at frame 60 or the whole framing says nothing. It
was `-20,0,0` while a turn took 137 seconds; speeding that up to 43 would have
put the same angle's crossing at frame 24, with the drive still cold.

One split got *stronger* and then went away altogether, and the second half is
the newer half. Five `--demo` flights used to steer against five `--engage` ones
holding every rate at an exact zero, and the ship carries an attitude now, which
the sky is projected through, so a turn moves the sky from **both** cameras
where the band took no steering argument at all — `drift.txt` is the flight that
watches that from outside. Then the autopilot stopped touching the stick, and
all ten hold `LEVEL_AXES` to the bit. What that leaves is a control rather than
a contrast, which is stronger for saying a steering change was the identity it
looked like and useless for saying where one landed. See **By stick** below,
which is the shape it collapsed into.

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
./target/release/warp $common --engage --throttle 1.0 --fps 10 --view side --orbit 90,0,0 --color truecolor > ahead.txt
sha256sum truecolor.txt ascii.txt ansi256.txt warp.txt side.txt orbit.txt astern.txt steer.txt drift.txt ahead.txt \
    > tests/golden/frames.sha256
# then put the comment block at the top of that file back
```

There is a step before that one now, and it is cheap enough to be routine: run
the same ten with `--fade 0` appended and check them against the digests you are
about to replace. A shot opens out of black, so every hash here moves for that
alone — and `--fade 0` is the renderer with the fade taken back out, to the bit.
If those ten do not reproduce exactly, the change reached something other than
what it was aimed at and there is no point regenerating anything yet.

Diff the old hashes against the new ones before committing and say which moved.
The split is usually the sharpest thing you have, and it comes in eight shapes
now: six about *what* a flight is doing, and two the fade added about *when* in
one a change bites and whether the tree can still be asked for a control. Count
them when you edit the list: the number in this sentence has been wrong twice,
which is what a hand-maintained count of the paragraphs below it will do.

**By view.** A change aimed at the hull moves `side.txt`, `orbit.txt` and
`astern.txt` and must leave all four cockpit flights alone — `warp.txt` is one
of them, since it never asks for `--view side`, and rebuilding the enterprise is
the worked example. One aimed at the tunnel glare moves `warp.txt` and only
`warp.txt`, since the two 60 fps `--demo` flights never leave sublight and the
outside view goes through `add_glow_oval` instead.

**By module.** A change inside `lens`, `bend` or `Canvas::draw_path` moves the
side-view flights and nothing else — not because of what geometry they have but
because of which modules they reach. Nothing in the cockpit goes near the three;
the tunnel is streaks and two glows. So the useful half of that shape is
`warp.txt` staying byte-identical, which is what says a change to how a span is
measured stayed inside the view it was aimed at. Taking `hypot` off the
bent-streak path is the worked example.

**By camera angle.** `side.txt` is abeam, where `Eye::to_camera` is an exact
swizzle and the hull's axis lies flat in the image plane, so anything that only
happens *off* the beam leaves it alone. Stopping the engine lance at its
vanishing point moved `orbit.txt` and no other, and so did putting the drive
behind the hull from ahead.

**By stick — which is a control now rather than a split, and that is a loss as
well as a gain.** Nothing in the reference steers. `render_headless` gates the
autopilot on `args.unattended()`, the autopilot flies straight, and no flag on
the command line puts a stick over — so all ten flights hold every steering rate
at an exact zero and `LEVEL_AXES` to the bit from first frame to last. A change
to how the sky is steered therefore moves *nothing*, and anything that does move
for one has leaked.

The gain is that a deletion from the steering path being the identity it looks
like now shows in ten byte-identical hashes rather than in six. The worked
example predates the collapse and is worth reading anyway: taking the lean out
of the cockpit picture deleted arithmetic from `Camera::project`, which every
star and every hull vertex in the program goes through, and seven byte-identical
hashes were what said the deletion really was the identity it looked like.

The loss is that this file can no longer say where a steering change landed, and
it reaches further than steering. A streak is the track a star swept while the
shutter was open, so one the ship flew straight through is a segment and one it
turned through is a curve — and **no flight here draws the second kind**. It
used to: `steer.txt` and `drift.txt` bent an exposure because their drive lights
at frame 60 of a twelve-second run with the weave still going, and drawing the
exposure along the flown track is what moved exactly those two. That contrast is
gone with the weave. `Universe::stations`, the multi-leg walk through
`Canvas::draw_path`, `Track::turn_over` and the near-plane cut on a tail a turn
has swung are pinned by property tests in `universe.rs`, `track.rs` and
`canvas.rs`, and by `a_turn_at_warp_can_be_flown_from_the_library_alone` — and
by nothing in the reference. **A change that repainted every turn ever flown
would move not one hash.** That is a hole with a floor under it rather than an
oversight, and closing it honestly means a flight that steers, which means a way
to steer one from a command line. `tests/golden.rs` asserts the premise rather
than leaving it as prose, so an autopilot that picks the stick back up fails
with a sentence instead of five unexplained hashes.

**By which way the nose points.** The sign of
`Orbit::nose_in_camera`'s depth component, `-cos(elevation)·sin(azimuth)`,
decides whether the exposure recedes from the eye or runs at it, so a change to
the near-plane cut moves `ahead.txt` and `orbit.txt` and must leave `side.txt`
alone — abeam that component is an exact zero, and the tail keeps the head's
depth to the bit. `astern.txt` is the other side of the same coin and must also
stay put. Careful with the two forward flights, though: `orbit.txt` is on the
forward side but only reaches a two-second exposure, so a change that only bites
past four light years of reach moves `ahead.txt` alone.

**And by camera drift.** A change to where the camera outside is pointed by
nobody moves `drift.txt` and only `drift.txt`. It is the one flight whose camera
moves at all: the other three side-view runs hold `orbit_target - orbit` at an
exact zero, so `Flight::advance`'s two eases add nothing to them, and the four
cockpit flights never read the orbit or the zoom in the first place.

**And by whether the drive is lit at all.** A change gated on `Ship::warp_intensity`
moves the seven flights that light one and leaves the three 60 fps `--demo` runs
byte for byte, whose ramp is an exact zero from the first frame to the last.
Making the Doppler shift a brightness as well as a colour is the worked example,
and so is the trap that came with it.

**The trap: a commit can take the last control away, and then the split has to
be measured on a build of its own.** That change landed beside two others — the
zero point coming down a fifth of a magnitude, and `reach_sq` being respelled —
and *both* of those move all ten, so with the three of them in the tree together
there was no byte-identical hash left in this file to say the shift was
warp-gated at all. The answer is not to skip the question. Build each half on
its own, shoot the ten, and record which moved: the shift alone left
`truecolor.txt`, `ascii.txt` and `ansi256.txt` untouched, the zero point alone
moved ten, the respelling alone moved nine. That went into the commit message
and into `frames.sha256`'s comment block, and it is the only reason any of it is
known. A first attempt at it measured "the shift" on a tree that still carried
the respelling and reported a leak that was not there — decompose to one change
per build or the answer is noise.

**And by *when* in a flight, which is the newest shape and the only one that
splits a file rather than the set.** All ten open out of black now, so a change
to the fade moves every hash — and moves nothing past frame 26 at 60 fps or
frame 5 at `--fps 10`, which is the fade less its fall, since a shot opens on
the trough rather than above it. Comparing the tails is a sharper statement than
"all ten moved" and it costs one `cmp` a case.

**And the other half of that: `--fade 0` is a control the tree can still be
asked for.** It restores the renderer to the bit — the gain is a literal one and
the view swaps at the press — so the ten commands with `--fade 0` appended have
to reproduce the digests the change replaced, exactly, in every case. That was
run before anything was regenerated and it is the strongest single check in this
file's history: it says the change reached nothing but the shutter, where a
moved hash only says something moved. It is also the one all-ten mover on the
list that never had to give a control up, which is what the trap above is about.
Any future change with a runtime switch owes the same check.

A hash moving outside the shape its change predicts has leaked.

**A hash that fails to move where its change predicted has not been vindicated
either** — it has found a hole, and the hole is worth more than the green tick.
The reference is one ship deep: not one of the five flights with a hull in it
passes `--ship`, so all five fly the enterprise. Clearing the hull band over the
span it is read over rather than written to changed real frames for four of the
ships then in the hangar — three of five camera angles for two of them — and
moved not one hash here, because the enterprise's outline does not happen to
expose it. When that happens, say so, and put the guard at the level the change
actually lives at: that one is a property test in `canvas.rs`, not an eleventh
flight.

The other answer is a new flight, and the test of which one you want is whether
the hole is a *variant* or a *region*. One ship out of the hangar is a variant,
and another flight would have pinned one more of them while leaving the rest
unlooked at; a property test over `models()` covers all of them at once. But
when the flights covered no camera aft of the beam, what was missing was half
the range of a control — the only half where the sky has a vanishing point at
all, and so the only half a trail could be stretched through one. A trail
running past the point its track vanishes at moved no hash for exactly that
reason, and it got both: a test at the level the fault lived at, and
`astern.txt` so the region stopped being unwatched. The matching clause went
into the case list's own coverage test beside the others, asked of the parsed
`--orbit` rather than of the angle written above it — and it is still there,
still earning its place, even though the fault it was written for cannot happen
any more: a star's tail is now projected from where the ship stood rather than
extrapolated toward a vanishing point, so there is nothing left to overshoot.

**A region can be covered on paper and unwatched in fact, which is the sharpest
version of this trap and the newest.** `ahead.txt` closes one. The exposure's
tail runs at the lens rather than away from it across the whole forward half of
the azimuth, and `orbit.txt` sits on that half — so by the astern clause's own
reasoning the region was covered. It was not: 120 frames at 60 fps is two
seconds, the drive covers a few light years in that, and the nearest a star can
be is four, so the near plane was never reached and the branch that deals with
reaching it was never taken. A bug that turned every star inside the exposure
into a bright point instead of a streak moved not one of the nine hashes. The
lesson is that an angle is only half of a camera case: **what a flight is
*doing* at that angle, for long enough, is the other half**, and the coverage
clause has to ask the second question as well. That one flies each case's own
sky and compares `Universe::exposure` against `universe::NEAREST_STAR`, because
the exposure is state and there is nothing else to read it off.

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

**The RNG draw order.** `Universe::spawn` draws the class, then the three that
scatter its absolute magnitude, then how deep into its own sphere it sits, then
the two that point it, then its twinkle — and it will redraw the first four if
the star comes out too faint to be as far off as the nearest one really is.
`--seed` reproducibility is a property of that sequence, so hoisting a draw out
of a struct literal or reordering two that "obviously do not interact" gives a
different sky. The same goes for anything that changes how many values are drawn
per star, the rejection loop included.

**The easing form.** Every ease in `src/ship.rs` is `1 - exp(-k·dt)` or
`exp(-k·dt)`, which is frame-rate independent. Rewriting one as `k * dt` is the
classic simplification and it silently makes the flight model depend on step
size. `src/autopilot.rs` was the tree's one counterexample and is the worked
example of what that costs: its throttle came down by a fixed step per *frame*
with no `dt` at all, so the eight-second drop-out shed 1.92 of throttle at
60 fps and 0.32 at `--fps 10`, where it never reached its floor; and the weave
was an impulse a frame against a decay a second, so the angle it swept was
*proportional to the frame rate* over a fiftyfold range. It is no longer a
counterexample at all: the throttle is a closed form of the clock, the weave is
gone, `Autopilot::update` takes no `dt` and could not read one, so frame-rate
independence there is structural rather than earned. The identity that governs
anything impulse-driven is kept in that module's own doc with no live caller
left, and is worth knowing before anything acquires one: a per-frame impulse `a`
against a decay `exp(-k·dt)` sweeps exactly `a/k` per frame whatever `dt` is, so
an unscaled impulse sweeps an angle proportional to the frame rate and one
proportional to `dt` sweeps a fixed angle per second. The stick still works that
way — it is impulse-driven because a terminal reports presses and not releases —
and `handle_key` is where it is now applied.
Perf edits are documented with the measurement that justified them —
`draw_streak` takes one reciprocal per streak rather than a divide per sample,
worth about six percent of drawing time at twenty thousand stars — so do not
undo one without a number saying why. The others on that list are the writer
spelling a colour into one stack sequence instead of seven capacity-checked
pushes (−30% of the write column), `canvas::length_of` in place of `hypot`
(−6% of an exterior frame), `Lens::inv_axes` (−1.8%), `draw_path` reusing the
span it already measured (−2.5%), and the twinkle `sin` skipped at warp, where
its amount is a hard zero (−2%). Together they took the expensive frame — the
outside view at twenty thousand stars on 200×60 — from 21.7 ms to 19.2.

Five more went in later and all five are **bit-exact**, which is the property to
protect when touching any of them: the ten reference flights did not move for
any, and that is what says each one really is the identity it looks like rather
than very nearly one. `Canvas::clip` takes a segment with both ends already on
the canvas without solving for it, and drops one with both ends past the same
edge without solving either; `draw_leg` returns the span it measured so
`draw_path` stops taking the same square root three times over; the divide and
the two conversions at the top of `draw_leg` moved *below* the clip, where a
rejected leg no longer pays for a `per_sample` nothing reads; `splat_inside`
binds the buffer once instead of indexing through `self` four times, since a
store through `self.buf`'s pointer is one the optimiser must assume could land
on that pointer and its length; and `bend::subdivide` skips the length and the
two curvatures when the arc budget is already one, which is every leg of every
turning exposure and which the optimiser cannot fold away because the budget is
a runtime value. Measured back to back, minimum of five sweeps: the outside view
at warp with the stick buried went 55.7 ms to 48.2, the cockpit at 72 000 stars
7.19% and the same in 256 colour 7.74%, the outside view straight 5.10%, and the
default sky at 200x60 3.59%. The 80x24 row did not move, because at 0.30 ms it
is under the granularity the bench reports.

**Those figures are from before the sky went world-space and none of them is
the number to reproduce now.** The same case is a different frame: the pool is
what a limiting magnitude asks for rather than a count, and a streak is a few
subpixels rather than one across the frame, so the outside view at warp runs
12.2 ms at 72 000 stars where it ran 21.1 at 20 000. Every optimisation above is
still in place and still worth its comment; what has moved is the baseline they
add up to. Re-measure before believing a regression, and re-measure the two
trees back to back.

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
| taking `splat_inside`'s four taps through one `&mut self.buf[base..=base + right + below]`, to pay one bounds check instead of four | **worse** — the turning frame went up 1.3% and the outside view 2.4%, and callgrind says why: the four checks came out at 424M instructions and the inclusive-range slice put 614M back, for a net 191M *added*. Binding the buffer as a plain slice is the half of that idea that works and it is in the tree; slicing it down to the window is the half that does not. |
| `#[inline]` on `Sink::move_to`, `set_color` and `glyph`, on the reading that the enum's `Commands` arm keeps the ANSI path out of line | nothing at all — 0.400 ms of write column either side, to the millisecond's third decimal. The hint is advisory and fat LTO had already made its decision. Splitting the enum into two types is a different proposal and is untested; it would have to be measured against `Screen::flush` rather than this bench, which times `present_plain`. |

Cachegrind is the other thing to know before reaching for a layout change, and
this paragraph has been remeasured on the world-space sky rather than inherited
from the one before it. **The conclusion survives and one of its numbers did
not.** On the turning cockpit frame at 200x60 and 72 000 stars the D1 miss rate
is **3.4% overall and 4.6% on reads**, against the 1.4% recorded when the pool
was thirty-six times smaller — but last-level misses are 71 482 for a whole run,
a rate that rounds to zero, so every one of those D1 misses is an L2 hit that an
out-of-order core largely covers. The working set still fits. This is still
instruction-bound, and shrinking `Star` or restriping the canvas is still
solving a problem it does not have: `splat_inside` alone retires 41% of the
turning frame's instructions and `draw_leg` another 22%, which is where an
optimisation has to go to be worth anything.

What has been measured since is the wall clock, and it went the good
way: the expensive frame — the outside view at warp — runs 12.2 ms against 21.1
before, with 72 000 stars against 20 000, because a streak that is a few
subpixels long costs a few samples where one across the frame costs a hundred.
The default cockpit frame at 200x60 went from 1.1 ms to 2.7.

**What a turn costs is a column of its own, and it is the one figure here worth
knowing before touching the exposure.** An exposure the ship flew straight
through is two points and the arithmetic it always was, so the seven rows above
measure a renderer with the curve switched off. `examples/bench.rs` has three
rows with the stick buried, which is the most curve it can be asked for and now
the only place in the tree a curve is measured at all — the autopilot flies
straight, so a flight left alone never asks for one. At 200x60 on this machine,
taking the minimum of five sweeps because the bench reports a bare mean of one
run: the default sky draws in 1.04 ms straight and 3.02 turning, comfortably
inside the frame budget either way; `--magnitude 8` draws in 6.77 and 40.13, and
the outside view in 9.33 and 45.62.

Those are a re-measurement of the three that stood here before — 1.38 and 4.13,
8.18 and 53.04, 11.96 and 63.20 — and the point of writing both down is that
nothing about the renderer moved between them. This container drifts about ten
percent between sessions and rather more between machines, so the ratios are the
part to carry forward and the absolute numbers are the part to re-measure. The
ratios did not move: about three to one on the default sky, five to one at
`--magnitude 8`, and a little under five to one from outside.

**And the ratios are what moved when the five bit-exact edits above went in**,
which is worth knowing because it is the first time anything here has bitten the
turn harder than the straight flight. Measured on the same machine, minimum of
five, the whole-frame column: 4.03 to 3.85 on the default sky turning, 48.07 to
46.00 at `--magnitude 8`, and 55.72 to **48.20** from outside — so the outside
view's turn came down from a little under five times its straight cost to about
four and a half, and almost all of that is one edit. `bend::subdivide` was
taking a square root and two `Lens::curvature` calls per leg to decide a number
that a budget of one had already decided, and a turning exposure is
twenty-three legs against a `MAX_ARCS` of twenty-four, so the whole of that work
was dead in exactly the case that could least afford it: 7.5% of the most
expensive frame the program draws, on its own.

**Why the turn costs what it does was measured rather than reasoned about, and
one plausible story is wrong.** It is not the `steps.clamp(1, ..)` floor putting
a sample under each of twenty-three legs where a straight star pays one: a leg
the canvas rejects returns before `steps` is ever computed. Callgrind on the two
frames says it plainly — `splat_inside` retires 8.04 times the instructions
turning that it does straight, and `splat_inside` is one call per subpixel of
arc. The smear is *rotational*, so a star sweeps the same few frame-widths of
arc whether it is ten light years out or a thousand, and the frame keeps
whatever crosses it. The cost is the arc length sampled, which is what this file
has said all along; what the profile adds is that it is the sample count and not
the leg count, so the per-leg overheads are worth taking only because they are
free, and the sample budget in `Canvas::draw_leg` is still the only lever that
would bound the thing.

That last pair is a fivefold cost and it was accepted rather than capped, on two
grounds. It is self-limiting — the steering rates decay in under a second and
the exposure forgets in three, so a hard turn costs a few seconds and cannot be
held. And the light is spread along the arc it was smeared over, so what a
hammered turn does is wash the sky out rather than blind it, which is both the
honest picture and the reason nothing is saturating while it happens.

**The escape hatch this paragraph used to name does not work, and that was
measured rather than argued.** It said the shape to reach for is fewer poses for
the *far* stars, on the reasoning that the count follows the worst parallax in
the sky — the nearest star there is — and most of the pool needs a fraction of
it. The pool statistics agree: at `--magnitude 8` the median star is 133 ly out
against an exposure reaching 16.4, so only 2 297 of 72 363 stars need more than
half the legs the nearest one does. It was built anyway. `Universe::stations`
already spaces one station list by equal *turn*, so a star can walk a stride
through it for nothing, and the whole thing is a per-star integer. It bought
**13%** — 46.57 ms to 40.59 in the cockpit, 54.40 to 47.19 from outside, with
every straight-flight case unchanged to the hundredth of a millisecond.

Two reasons, and both are worth knowing before anyone reaches for it again. The
pose count is not set by parallax at the stop, it is set by `MAX_STATIONS`: the
attitude alone asks for about 24 legs there against a ceiling of 23, so the
whole pool is already pinned at the ceiling and there is no spread to exploit.
And a stride that still tracks the arc rasterises the same arc — `draw_leg`
takes one sample per subpixel of span, so halving the legs while following the
same curve halves nothing. Forcing a straight chord beyond 50 ly, which is far
outside the sagitta budget and changes the picture, gets 24.01 ms, and that win
comes from drawing a *shorter polyline* rather than from fewer poses.

So the turn's cost is the arc length sampled. If it ever wants bounding the
lever is the sample budget in `Canvas::draw_leg`, not the pose count in
`Universe::stations` and not `SAGITTA`.

## Layout

```
src/lib.rs        module list and the orientation doc comment
src/main.rs       parse, fly, report — nothing else
src/app.rs        the three loops (interactive, headless, snapshot) and Flight
src/autopilot.rs  the throttle, the drive and the camera for --demo/--screensaver
src/cli.rs        every flag, and the bounds each one is held to
src/view.rs       ViewMode, the outside camera: its orbit, its range, the zoom
src/ship.rs       flight model: throttle, warp, steering, transients
src/universe.rs   the sky: a world-space volume the ship is inside, and the
                  photometry that puts each star where it is
src/camera.rs     the projection both views and the hull share, and a Streak
src/lens.rs       the warp bubble: a point-mass lens in an elliptical metric
src/bend.rs       drawing a streak the bubble reaches: arcs and two images
src/models.rs     the hulls, and how to draw one
src/menu.rs       the ship picker
src/canvas.rs     f32 RGB accumulation buffer, rasterisers, tonemap
src/render.rs     assembling a frame: sky, then what is lit, then the glass
src/hud.rs        the instrument panel
src/term.rs       Screen (double-buffered cells), ColorMode, RawGuard
src/track.rs      where the ship has been: the flown track an exposure is
                  drawn along, and how far back it is straight
src/snapshot.rs   PNG writer, behind `--features snapshot`

tests/flight.rs   a whole flight through the public surface, and nothing else
tests/golden.rs   the reference frames, reproduced in process; its own SHA-256
tests/golden/     frames.sha256 — the pinned bytes, and how to remake them
tests/terminal.rs a real terminal, from `script`: that a flight hands it back,
                  on the deadline and on a signal, and says so when there is none
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
terminal keeps up or not. There is one sky and it is stepped whichever camera
is flying — there is no longer a second one to be kept warm or skipped, and
`Universe::advance` is one sample and one distance test per star, because the
*ship* does all the moving and [`track::Track`] is what remembers it having done
so. The one thing it carries between steps is the exposure's length —
see the long-exposure section below, where a length recomputed afresh each frame
is the bug it exists to have fixed — which is why it is handed the step, the
warp ramp and the speed rather than only the ship's position.

There are **two** clamps on `dt` and they are not the same one. `advance`
holds its own argument to `MAX_STEP_DT` (1.0 s) and turns a non-finite step
into zero — it is `pub`, so that guard sits with the loop it protects rather
than at any one caller. The interactive loop separately holds a measured frame
to `MAX_FRAME_DT` (0.25 s), which is tighter because a frame on a real
terminal is never a quarter of a second, where `advance` has to leave headroom
for headless and snapshot stepping at `1.0 / --fps` with `--fps` floored at 1.

`draw` runs, per view:

1. `canvas.clear()` — the f32 buffer, at `cols × 2·rows` subpixels.
2. Streaks: `sky.sweep(cam, &observer, time, draw)` hands each star's exposure
   to a closure as the track it swept — two points where the ship flew straight
   through the shutter's whole reach, and a path of up to twenty-four where it
   turned. Every one of them goes to `canvas.draw_path`, two points or
   twenty-four. It used to pick `draw_streak` for a pair on the grounds that
   the two share `draw_leg` and so lay down the same light — true of the light
   and false of the *pace*, which is the third component of a point and the one
   thing a `Streak` has nowhere to carry. `draw_streak` is still what the
   drive's own trail goes through, since a plume has a length rather than a
   track. The same sky, through a different `Observer`, and the side view
   hands the result to `bend::Bend::draw`, which bends the ones
   the lens actually reaches — chopping each into arcs and drawing both images
   — and leaves the rest, which at sublight is all of them, on the ordinary
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
6. `canvas.resolve_into(&tonemap, gain, &mut pixels)` — HDR to 8-bit RGB, and
   the one place the shutter is applied. `gain` is 1.0 for an ordinary frame
   and less while a shot is opening or a cut is dipping; see **The cut** below
   for why it rides this pass and nothing else.
7. `screen.compose(&pixels)` — two pixel rows fold into one cell.
8. `hud::draw(&mut screen, ..)` — unless the flight is flying itself, in which
   case the `Readout`'s `panel` is false and it returns having drawn nothing.
   Then `menu::draw` if the picker is up, which is *not* gated the same way:
   a panel is chrome that arrives unbidden and the picker is a dialogue
   somebody pressed `M` for.

Then `Screen::flush` (interactive: only cells that changed, and colour codes
only when they differ from the last cell written) or `write_plain` (headless:
a self-contained block of text).

**The panel and the picker are written into cells, after `compose`.** They are
not in `renderer.pixels()`. That is why the snapshot PNG has no instrument
panel — and why neither of them fades. The shutter is an exposure and the panel
is on the glass rather than in it, so a shot opens on instruments over a dark
sky, and the one place the keys are written down to somebody flying is never on
a clock. Taking them with it would need the gain threaded through every stamp in
`hud.rs` and `menu.rs`, where `--color ascii` discards colour outright so the
panel could not fade at all — a picture effect that exists in two colour modes
out of three is not one — or a second pass over the cell grid, on eight bits,
which would dim the picker twice because `overlay_panel` is deliberately not
idempotent.

### The three loops

| | `run_interactive` | `run_headless` | `run_snapshot` |
| --- | --- | --- | --- |
| clock | real, `start.elapsed()` | synthetic, `frame * dt` | synthetic |
| size | `resolved_size(args)` | `resolved_size(args)` | `args.size` or `(240, 68)` |
| terminal | `RawGuard`, alt screen | none | none |
| `--demo` | deadline **and** autopilot | autopilot only | autopilot only |
| panel | `!args.unattended()` | the same | the same, and invisible |
| output | `Screen::flush` | `write_plain` | `write_png` of `pixels()` |

The autopilot flies the camera and the throttle in all three — `Flight::fly_itself`
lays a swing of its own on top of whatever `--orbit` parked the shot at. It does
not fly the *ship's attitude*: a flight nobody is flying holds the heading it
was given, which is what `Args::unattended` gates along with the panel. Note the
snapshot column: the panel is stamped into cells and a PNG is written from
`pixels()`, so the gate cannot change a byte of one — it is spelled the same way
there so that one predicate answers the question in all three loops, which is
how the autopilot gate and the panel gate came to disagree about
`--snapshot --screensaver` in the first place.

`--demo 5 --headless` does not stop after five seconds; headless always runs
exactly `--frames` frames. The deadline is checked only in the interactive loop.

**`--fps` means two different things**, and `--help` says both now — leaving
the second out is how somebody sets `--fps 10` on a headless run to save time
and gets a twelve-second flight instead of a two-second one. Interactively it
is only a frame budget, and only while nothing is being typed — see the drain
loop below, which spends it waiting and abandons what is left of it the moment
a key has moved the flight. The panel shows a smoothed measurement, which is
why it reads high for a moment when the stick is worked. In headless and
snapshot it *is* the simulation timestep — `dt = 1.0 / args.fps` — so changing
it changes the flight, and changing how it is used moves the golden frames.

Three more asymmetries worth knowing before you touch the loops. **`P` stops the
flight, including the schedule it is flying to**, which took two corrections to
arrive at and neither was a hedge.

It gated `advance` alone, and the paragraph here said a paused `--demo` goes on
flying itself, repainting and exiting at its deadline. The first correction was
the *step*, and it has since become unreachable rather than merely fixed: the
stick was an impulse against a damper and the damper is in `advance`, so a pause
that stopped the damping and not the impulse ratcheted the rate with nothing
bleeding it off — eleven seconds of `P` pinned the yaw at `MAX_YAW_RATE`, held
the hull at its full lean, drove `models::drive_behind_hull` to a hard one, and
snapped the ship into a turn on the way out. A zero step was passed while paused
to stop it, and that argument went with the weave: the autopilot touches no
impulse now, so there is no step to zero and `fly_itself` takes none. The second
correction is the one still holding this up. It was the *clock*, left on wall time on the
argument that the throttle and the camera are closed forms of it and so are
unaffected either way. True of the throttle and false of the camera: the closed
form gives the orbit *target*, and a frame is built from `Flight::orbit`, which
eases toward it in `advance` — which the pause also stops. So the target walked
and the camera stood still, at `CAMERA_TURN`'s turn every 43 seconds, and the
ease closes 7.2% of the gap per sim step: ten seconds of `P` whipped the camera
most of the way round the hull inside a second of resuming, saturating at half a
turn after about twenty-one.

`fly_itself` is handed `Flight::time` now, which `advance` is the only thing
that moves, so there is no second accumulator to keep in step. What that costs
is the sentence about flying itself: a paused demo does not, and the throttle
holds where it was. The two things it goes on doing are the two that were worth
having — it repaints, and it exits at its deadline, which still reads the wall.

In screensaver mode `handle_key` is never reached at all — any non-release key
breaks the loop, so there are no controls, not even pause. That is half of why
neither unattended mode draws an instrument panel: a hint line naming keys would
be naming keys that quit. The other half is that the instruments are readings
for somebody at the controls, and `--demo` has nobody there either even though
its keyboard still works. And `R` restores
what the *command line* asked for rather than any fixed number: `args.throttle`
rather than `Ship::new()`'s 0.18, and `args.orbit` rather than `Orbit::LEVEL`.
The zoom has no flag, so it goes back to `ZOOM_DEFAULT`. All three are snapped
rather than eased — `R` is the key for when the view has got away from you, and
watching it saunter back is not what is wanted — and the snap is load-bearing
beyond taste: the orbit ease is asymptotic, so only an exact reset gets the
camera back to the bitwise-level shot the fast paths below are written for.

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

### The one sky

`universe::Universe` is a volume of space the ship is somewhere inside, laid out
in an **inertial world frame** and projected through whichever camera is flying.
There used to be two of these — a cone opening forward from the canopy and a
band the ship flew through side-on — and the whole of the difference between
them was that each was laid out *in front of its own camera*. That is what made
a camera swung ninety degrees round the ship show the sky it had already been
showing, and what the fold, the rim, the near cap and the entering-surface
sampler existed to patch up afterwards.

**Nothing about looking at the sky may move it.** That is the invariant the
rebuild bought and the one to protect: `a_camera_swing_moves_no_star` asserts it
bitwise over a full turn on all three axes, and
`the_sky_is_left_alone_by_everything_but_the_flight` says the same of the zoom,
the twinkle clock and a ship sitting still. Only `Universe::advance` may move a
star, and only because the ship went somewhere.

#### How a star gets where it is

`spawn` draws a class from the census in `CLASSES`, gives it an absolute
magnitude with `ABSOLUTE_SPREAD` of scatter, and places it **uniformly by
volume** inside the sphere it could still be seen from. Its distance therefore
falls out of its photometry — how bright it is against how bright it looks —
which is the real correlation and is why the sky reads as depth rather than as
confetti. At the default limit an M dwarf lands ten light years off, a G dwarf
seventy, an A star three hundred and an O giant thousands.

**The scatter is load-bearing.** With one absolute magnitude per class a star's
brightness would *determine* its distance, so at warp every equally bright star
would move at exactly the same rate and brightness would read as speed.

**`COUNT_SLOPE` is 0.6 and not the 0.5 the real sky measures**, and the reason
is the same one the old band eventually reached about its own profile: a rigid
translation through space preserves a uniform density and nothing else. The
measured sky is flatter because the Galaxy is a disc that thins out; sample that
and the sky would relax into `10^(0.6 m)` over the first minutes of a flight,
thickening as it went. Self-consistency wins here because the alternative is a
sky that changes while you fly it.

**`NEAREST_STAR` is a fact rather than a margin.** There is nothing between here
and Proxima at 4.2 light years, and the catalogue needs telling because it is
built from photometry rather than from a volume: the faint end of the scatter
has a visibility sphere under two light years across and is therefore *always*
very close. Those were the stars that tore past at impulse — thirty seconds of
it moving one twelve thousand subpixels — in a sky whose whole point is to hold
still down there. They are left out rather than moved, since a star that faint
is not visible from anywhere it could actually be. That tips the census toward
the luminous classes as the limit comes down, which is the observation and not a
side effect: a magnitude-two sky really is all giants.

#### What leaves, and what comes back

A star is in the catalogue while its own distance keeps it brighter than the
limit. `reach_sq` is that distance squared, folded once at spawn, and it is the
same number as `lumen / limiting intensity` — so the ratio `reach_sq / range_sq`
answers two questions at once: above one exactly while the star is still worth
drawing, and how far above says how much of the fade is left to climb.

When one crosses out it is put back on the **entering hemisphere** of its own
sphere, distributed by how much of the flow crosses there — density
proportional to the cosine of the angle from the direction it is coming from,
which is the square root of a uniform rather than a uniform.
`the_flow_brings_stars_in_from_the_side_it_is_coming_from` checks the mean
cosine is two thirds rather than the half a uniform sampler would give. Nothing
arrives in view: a star put back at its own reach is by construction at exactly
the limiting magnitude, where `FADE_MAGNITUDES` has it at nothing at all.

#### The streak is a long exposure

This is the part most likely to be misread as an effect. At honest distances one
sim step at full warp moves a ten-light-year star **0.19 subpixels**, and the
`1 + 5·warp²` multiplier the old sky used makes that 1.1 against the 119 the
renderer used to draw. Multiplying harder is not the answer; saying what the
streak *is* is.

It is the track the star actually flew over the last `TRAIL_SECONDS`, and it is
**remembered rather than computed** — which is the second correction to this
paragraph and the one that made it true. The first said the ship was at
`P − n·v·T`, so the tail is that star projected from there, and that is exact
for a straight run and a chord across a curve for anything else. `src/track.rs`
records where the ship has been and how it was pointed while it was there, and
`Universe::sweep` projects each star from the poses it actually held.

- **It is exact** for a straight track, where a linear extrapolation of one
  step is a chord across a curve — and for a turning one, which the
  extrapolation could not be at all.
- **It can fall behind the near plane from anywhere, the seat included.** This
  bullet has been wrong twice and in the same shape both times. It first said
  the tail cannot fall behind the plane, on the argument that going back in
  time moves a star *away from the nose* — true of one camera, written down as
  a fact about the arithmetic, and it cost a shipped bug the moment the view
  from outside got ahead of the ship. It then said the exception was the view
  from outside. It is not: a turn swings the camera, so a star ahead now can
  have been square abeam three seconds ago, and the cut fires in the cockpit
  too. There are two boundaries now — `TAIL_NEAR`, the plane, and `TAIL_COS`,
  a *cone* — and the cone is the one that matters, because what has to stay
  bounded is `focal · lateral / depth` and only a ratio bounds a ratio. Note
  that the clamp in `Universe::advance` is a third thing again — it is on the
  exposure's *length*, and has its own answer.
- **It deletes `prev`**, and with it the range a trail was drawn at, the fold's
  trail shift, the rule about handing a recycled star the trail it would have
  had, and `retarget` dropping every trail on a resize.

And it puts depth in the tunnel for the first time: 67 subpixels for a star at
ten light years, 9.6 at seventy, 2.1 at three hundred, 0.7 at a thousand. The
old sky drew every streak the same length because every star was the same
distance away.

**What the tail is computed *from* is state, and that took a shipped bug to
learn.** `TRAIL_SECONDS · warp · velocity` is right for a flight that has
settled and wrong for one that is changing, because on screen a trail may only
ever extend *outward*: it is laid down behind a star as the ship flies, so the
end away from the vanishing point is the only end that may move. The tail sits
at depth `z + reach` and the star closes at the ship's speed, so `z` falls by
`v·dt` a step and the tail moves inward exactly when `reach` grows faster than
that. Lighting the drive takes the velocity from 3.4 c to 2000 c, a factor of
588, so a `reach` recomputed each frame with no memory of the last one reached
backwards into the past faster than time was passing — 12x faster a quarter of a
second in, 6x at one second, honest only after about 2.7 — and for the whole of
every spool-up each streak grew from its middle outward in both directions at
once. Nothing in the picture was moving inward. The renderer was changing its
mind about how much history to draw.

`Universe::trail` is the fix and it is one clamp: the exposure may grow by at
most `v·dt` a step, which pins the tail's *world* point while the ship flies
away from it. Shrinking is deliberately not held back, since a shorter trail
moves its tail outward and a drive shutting down should visibly stop smearing.
`TRAIL_SECONDS` is therefore the length the exposure **settles** at rather than
the length it has, and the length is honest as well as monotone: three seconds
after the drive lights there are three seconds of track behind the ship rather
than a retroactive thirty. The two guards are
`a_trail_only_ever_grows_away_from_the_vanishing_point`, which is the symptom
stated as a property over a hundred thousand tail samples across an engage, and
`the_exposure_never_outruns_the_flight_that_earned_it`, which is the same
statement down where the arithmetic is. Both need the on-canvas filter
`the_sky_holds_still_at_impulse` needs, and for the same reason: a star square
abeam has a camera depth of nearly nothing, so `focal · lateral / (z + trail)`
is a division whose noise is a third of a subpixel.

**From outside, the tail can and does fall behind the near plane, and
`universe::tail_of` is the cut.** The nose in camera space is
`Orbit::nose_in_camera`, whose depth component is
`-cos(elevation)·sin(azimuth)` — negative across the whole forward half of the
azimuth, where the camera is ahead of the ship looking back down its own track.
There the exposure runs *at* the lens and through it, `project_beyond` refuses
the tail, and what used to catch that was an `unwrap_or(head)` written under the
belief that it could not happen. It happened to every star inside the exposure's
reach — 16.4 light years at full warp, a fifth of the pool, and every one of the
near stars that carry the longest trails — each collapsing to a point drawn at
*full* intensity, since `draw_streak`'s short branch skips the `spread`
division. A field of conspicuous bright dots exactly where the tunnel should be.

The cut is the one `models::draw_flame` already makes against the same plane and
in the same closed form: the track is a straight segment in camera space, so
where it crosses is one division, and what is left is the part of the exposure
that happened in front of the lens. `TAIL_NEAR` carries the margin for the same
reason `PLUME_NEAR` does — the solve cancels two numbers of about the exposure's
length down to a thousandth of a light year — and the depth is *set* rather than
recomputed from the parameter, so only the lateral offsets go through the
subtraction. It costs nothing measurable: the cut tail projects far off-canvas
and `Canvas::clip` bounds the sample count by the frame, so 200 forward-camera
frames at 200x60 and 72 000 stars run 4.98 ms each against 4.95 before.

**Rotational smear came back, and the measurement is why.** It went once, and
the number that sent it away was right about what it measured: a whole sim step
at the yaw stop moves a star at the frame edge 0.58 of a subpixel, inside the
branch `draw_streak` takes for anything under three quarters of one. What that
number was about was *one step*. The exposure is three seconds long — three
hundred and sixty of them — so the same smear is 209 subpixels on a canvas
forty-eight tall, four frame-heights of arc per star. The instruction to bring a
number before putting it back stands; this is the number.

It is not drawn as a smear, though, which is the part worth keeping straight.
Nothing multiplies a step by anything: `src/track.rs` remembers where the ship
was and how it was pointed, and the exposure is projected from those poses. The
rotation falls out because the camera is bolted to the hull, and it arrives
together with the curve the ship's own track puts in the near stars — one
mechanism, because the ship flies where it points and so the attitude's turn
*is* the track's curvature.

#### Brightness is a magnitude, and the canvas is linear

`intensity = 10^(−0.4·(m − ZERO_POINT))`, which is the one transform between a
logarithmic scale and a linear buffer and is the only thing here that has to be
exactly right. `ZERO_POINT` is **derived, and then deliberately moved a fifth of
a magnitude off the derivation** — which is the sentence to read before
restoring it. The derivation is still the anchor: the old renderer's typical
star laid down `luminosity · magnitude · depth^1.4 ≈ 0.155`, and the mean of
`10^(−0.4 m)` under the count law is `slope/(slope − 0.4) · 10^(−0.4 L)`, so the
two agree at `10^(0.4 z) = 20.6`, which is 3.28. It is 3.08. Move the count law
or the limit and this wants re-deriving — and a re-derivation has to carry the
offset forward or it silently undoes what the offset was for.

**What it was for is the shift below.** Re-deriving `ZERO_POINT` and pocketing
the fifth of a magnitude is the one edit here that would look like a correction
and be a regression.

**That the sky can be dimmed without moving is a property of how `photometry`
is spelled, and it was not one until it was measured.** A star's reach is
`lumen / faintest`, so the zero point cancels out of it *algebraically* — and
that is what the code said, as that quotient. In `f32` it does not: each side
went through its own `powf`, and `10^x` turns an ulp on the exponent into a part
in a million on the answer. Moving the zero point moved every reach by about
4e-7, and with it every `sqrt`, every radius and every star's place — 51% of a
default seeded sky. Harmless as it happened, and not harmless in principle: a
star landing that close to `NEAREST_STAR` flips the rejection loop in `spawn`,
takes four more draws out of the RNG, and redraws the whole seeded sky from that
index on, which no hash could tell from the brightness change that prompted it.
`reach_sq` is taken from the limit directly now and carries no zero point at
all, so a build either side of the constant puts all 4 565 stars of a default
sky in byte-identical places. Do not put the quotient back.

`--exposure` is the wrong knob for the same job and it is worth knowing why: it
is the *tonemap*, so it takes the tunnel glare, the hull, the plume and the
flash down with the sky. This dims the stars and leaves everything else exactly
where it was. Somebody who wants the old level back can have it exactly — the
curve is `1 − exp(−v·exposure)`, so `--exposure 2.28` cancels the 0.20 precisely
— at the price of brightening everything that is not a star along with it.

That replaced a `DEPTH_FALLOFF` of 1.4 and a cubed magnitude sample, both of
which existed to fake a lopsided brightness distribution and to stop honest
inverse-square making everything invisible. Neither problem survives a catalogue
in which every star is by construction brighter than the limit.

#### The Doppler shift is a brightness as well as a colour

`shift_color` carries a star's rest colour toward blue ahead and red astern, and
`shift_light` is the other half of the same shift: brighter ahead, fainter
astern, on the strength `eye.warp * 0.9`. Both take their two weights from
`shift_weights`, and the sharing is the design rather than a tidiness — with one
constant, `SHIFT_LIGHT`, applied to the *difference* of the two weights, a star
brightens exactly while it is being carried toward blue and dims exactly while
it is being carried toward red. The light turns over at precisely the angle the
colour stops leaning blue: 73.3 degrees off the nose, where `forward³` meets
`0.75·(1 − forward)`. A gain and a loss tuned apart buy a second knob and sell
that, putting the two turnovers twelve degrees from each other — a ring where a
star reads faintly red and is nonetheless brighter than at rest.

**From the seat nothing visible is receding**, and that is the fact that shapes
the curve. `Camera::focal` is `0.85` of the canvas height, so the cockpit's
frame reaches 48.8 degrees off the nose at the corner and `forward` never falls
below about 0.83. Everything in view is approaching; what the cockpit shows is a
*gradient of brightening*, steep down the throat and flat at the corners, and
the dimming half is a view-from-outside effect. That is also why the blue weight
is a **cube** and the red one linear: a curve that is gentle in `cos θ` spends
almost all of its range where nobody in the seat can see it.

At full warp, as a multiplier on the light a star laid down before any of this:
1.28 dead ahead, 1.03 at the corner of the cockpit's frame, 0.72 square abeam,
0.50 dead astern, a whole sky at 0.78 and the cockpit's own cone at 1.15. In
eight bits a median star reads 141 down the throat against 95 astern, from 128
before.

`side.txt` is where it reads cleanest: `Orbit::LEVEL` puts the nose at exactly
`(1, 0, 0)` in camera space, so it lies flat in the image plane and `forward`
tracks screen *x* with no depth mixed into it at all.

The gain multiplies `intensity` and never `lumen`, `reach_sq` or the limit, and
it is applied **after** the `intensity <= 0.0` guard in `sweep`. Both matter.
The fade is what makes a recycled star arrive at nothing, and the recycle puts
it on the *entering* hemisphere — dead ahead, where the gain is largest — so a
multiplier after the fade is what keeps nothing from popping into view. And
leaving the guard in front keeps the set of stars drawn a function of their own
photometry alone, which is what lets a test sweep one sky at two warps and pair
the results by index.

#### The transform chain

This is the subtlest part of the module and the place a bug will live.

```
hull space     h = Aᵀ · (w − P)      A = ship.axes, P = ship.position
cockpit        pos = h               the seat is bolted to the hull
outside        pos = Eye::basis · h  rotation only
```

**`P` is `f64` and that is not a preference.** At impulse the ship covers 2.1e-5
ly a sim step against stars a thousand light years out — a relative change of
2e-8 where `f32` resolves 1.2e-7 — so a position carried on the stars and
decremented in place would round the whole far sky to a standstill permanently
rather than merely slowly. Holding the stars still and moving the ship puts the
small number in the accumulator where it belongs.

**The outside camera's standoff is deliberately not applied to stars.**
`Eye::to_camera` adds nine to fifteen *hull* units of it; a starship is a few
hundred metres, so against a sky in light years that is a translation of about
1e-13 of a star's range. Applying it would be a unit error, not a refinement.
The hull still goes through `Eye::to_camera` with the standoff in it, because
for the hull it is the whole of the framing.

**The ship carries the attitude, so steering moves the sky from both cameras.**
That is a change from the band, which took no steering argument at all on the
argument that out there the camera rides with the ship. With one world-space sky
and a real attitude that option is gone, and the honest answer is the better
one: a chase camera locked to a turning hull does see the sky swing past.
`Ship::bank` still reaches nothing either camera draws — it is a display
affectation, posed only by `models::attitude` — and the two shipped-and-reverted
bugs about it were both about putting a yaw-coupled angle into a *projection*.
Nothing here does. See `Ship::steer`, which is that rotation moved off the sky
and onto the hull and otherwise untouched.

`Camera::project` clips at `camera::Z_NEAR`, which is a **hull** number: nine
tenths of a unit where the ship is about one. Stars are asked for
`project_beyond` with `STAR_NEAR` instead, because a star held to the hull's
plane would be thrown away at nine tenths of a light year for a reason that has
nothing to do with it. One projection, two near planes, and each caller says
which it means.

### Where the camera outside is

Two types in `src/view.rs` carry the whole of it. `Orbit` is three angles about
the ship — `azimuth` round it, `elevation` over it, `roll` about the view axis
— and `Eye` is `Orbit::basis()` plus the standoff `ship_distance(zoom)`. They
live there rather than beside the hull because they are what the *sky* is
streamed against as well as what the hull is posed by; putting them in
`models.rs` would make `render.rs` reach through the ship models to find out
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
turns on that cancelling used to be the star band, which laid itself out afresh
whenever the orbit differed from the one it was last laid against — so an angle
that came back a hair off rebuilt the rotation path every frame of a flight
nobody was touching the camera on. The sky no longer cares, since a swing moves
nothing, but the guard stays: `Orbit`'s equality is what `Flight::advance` eases
against, and an angle that would not cancel is a camera that never settles.

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
`MAX_MAGNITUDE` for the one that allocates, `MAX_COUNT` for `--frames` and
`--warmup`, which are spent rather than allocated, `MAX_SCALE` for `--scale`
and `canvas::MAX_HULL_SAMPLES` for `--aa`, both of which enter *squared* — and
why an ioctl answer is clamped rather than believed. `--orbit` is the one
exception and says why in its own parser: an angle has no end to run away past,
so a preposterous one costs a picture rather than an allocation, and
`Orbit::held` folds it with exactly the fold a keypress gets.

`cli::MAX_MAGNITUDE` is the one on that list that is *not* only a parse-time
bound, and it is public for that reason. A sky is the one thing there that can
still be asked for again after the command line has been read, so the `+` key
clamps to the same constant rather than to a number of its own — it used to have
one, and it sat *under* what `--stars` already allowed.

The bound is also one step further back than it looks. The pool grows as
`10^(0.6 m)`, so what is held is the *observer* and what that holds is the
allocation: 9.5 comes to 574 000 stars, which is under the million `--stars`
used to allow. There was a third door once — an automatic count in `app.rs` that
sized the pool from the canvas — and it is long gone.

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
`Flight::cut`, `Autopilot::update`'s `elapsed`, the twinkle phase folded once
per frame, the four shake terms and the engine flame's gutter are `f64` because
a screensaver is left running for days and an `f32` accumulator stops advancing
after about six — freezing the twinkle, the shake and the flame with it.
`Flight::cut` is on the list for a variant of the same reason rather than for
the same one: it does not accumulate, it *stamps* the clock that does, so an
`f32` copy of it would be unable to resolve half a second against a week of
uptime and every cut after the first day would land already over.
`Flight::accumulator` stays `f32` deliberately: it is bounded by one sim step
and never drifts, and so do the cut's own `from` and `fade`, which are bounded
at both ends. Do not "unify" these.

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
the autopilot wove in those days and the hull's lean crossed square every ten
seconds. Who crosses it has since changed and the ramp has not — a ship nobody
is flying holds its lean at an exact zero now, so what is left to cross are a
hand on the stick and the camera walking round the hull, which `drift.txt`
traverses at frame 72.
With the ramp the crossing is indistinguishable from the frame either side of
it. `OCCLUSION_BAND` is what sets the width, and both ends of it are held: any
narrower and the step comes back, any wider and the drive goes on shining
through a hull plainly in front of it. Everything the drive draws is linear in
its intensity, so the two passes lay down the one pass they were split from
wherever no plate covers either.

**The streak falloff is physics for a star and a bug for the drive, which is
what `Canvas::streak_spread` is for.** `draw_streak` divides a streak's
per-sample light by how fast the image was moving, so a fast smear spreads
instead of burning a line. That is right when the pace *is* the motion, which
is every star in the sky. It is wrong for the engine trail: a lit warp drive
throws its lance at the frame edge, so the length is the terminal's and left
alone the drive would burn dimmer the wider the window — the same flight
looking different on two machines, which is the one thing the whole test suite
exists to stop.
`draw_trail` multiplies the factor back out, so what `TRAIL_INTENSITY` names is
the brightness at the nozzle. Anything else that picks its own streak length
rather than being handed one has the same problem and the same answer.

**It is asked for a pace and not a length**, and that is the second of the two
corrections this has taken. A point of a streak carries how fast the star's
image was moving on the leg leaving it, measured as the length the whole
exposure would have covered at that pace — which for a streak flown straight
through is simply its own length, so the falloff is the number it always was and
not one reference flight moved for the change. For a streak the ship *turned*
through it is not: the tail swings toward the near-plane cone, where the
projection accelerates hyperbolically, so the image spends almost no time out
there and must leave almost nothing behind it. Dividing by the total length
instead charged the star's whole budget to that excursion — 4.6 times too dim on
the axis and about 3 at the frame edge, with the size of the error set by where
the cone happens to sit rather than by anything about the star. A pace travels
through `bend` unchanged for the same reason the moment it replaced could not:
the bubble's stretching is the magnification the bend already applies, and
reading it as the star having moved faster charges it twice.

It is measured on the **whole** segment, before any clipping, and it used to be
measured on what survived the window — which was the same fault one level down,
in the half of the tree that was supposed to be the sound one. A star's light is
laid along the track it flew, so the part of that track on screen carries the
part of the light that fell on it; measured on the remainder, the whole star's
brightness was poured into whatever fragment the frame kept, and an edge streak
burned up to three times brighter per subpixel on a narrower terminal.
`a_streak_is_ramped_by_its_own_length_and_not_by_the_window` is that stated as
two terminals drawing one streak and being compared where they overlap. The
cancellation the lance depends on is unaffected, since both ends of it moved
together.

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
that a streak runs out at nothing.** It used to stop 8% short, and that margin
was never about the geometry. `draw_streak` ramped down to a floor of a third
rather than to nothing, and every bell on a ship shares one vanishing point —
the plumes run down the same hull axis and differ only by the bell's own reach,
which a point at infinity cannot see — so a lance ending *on* it put a third of
full brightness from every lane of every bell on one subpixel: a bead hanging in
the sky precisely where the exhaust was meant to have gone. Taking the floor
away means the sample landing on the point carries a ramp of exactly zero, and
the margin has nothing left to buy. Measured star-free at `--orbit 75,12,0`,
peak light within a dozen subpixels of the point: 0.83 stopping short with the
floor, 1.02 running the whole way with it, 0.45 as it is now — further and
dimmer at once, against a plume peaking at 2.65.

**There is one ramp now, and it runs out at nothing.** A star's used to stop at
a floor of a third, on the argument that a tail "is simply where it was a frame
ago" and that light stopping dead there would read as a dash with no star on it.
A frame ago that is harmless, because nothing ever *leaves* a streak through its
tail. The premise went when a streak became an exposure seconds long: the tail
is where the shutter closes, everything ageing out of the picture leaves through
exactly that edge, and a floor made it leave at eight-bit level 178 against a
head of 237 with the next subpixel at zero. Hold a turn at low warp, where the
whole trail is the smear the turn left, and the sky holds its streaks for a
second after the stick comes back and is then wiped — 59 000 lit subpixels for a
second, then 9 000 over the next one and three quarters. That is the report this
came from, and the floor was the whole of it.

So the second entry point and its floor are both gone, and one `draw_streak`
is what is left. The argument that got the lance the fading ramp first is now
the argument for all of it: where a streak ends is where its light ran out.
Abeam the lance is stretched to the frame's diagonal and leaves by the edge, so
its tail is off-screen; ramp it over the clipped remainder and it fades to
nothing at the edge of the *picture* instead of at the end of the plume, and a
drive whose reach is the frame's stops short of the frame on every terminal.
`a_lit_warp_drive_trails_off_the_edge_of_the_frame` catches exactly that, and
`a_lance_is_ramped_by_its_own_length_and_not_by_the_window` in `canvas.rs` is
the sharper statement of it, beside the star's own
`a_streak_is_ramped_by_its_own_length_and_not_by_the_window`. Where the window
cuts a plume is not a fact about the plume — and, it turns out, where it cuts a
star is not a fact about the star either. `draw_path` had always said so;
`draw_streak` says so now, which is what lets one exposure swap between the two
mid-flight without the frame changing brightness underneath it.

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

**A sublight frame must be the frame the lens code is not there for, and a
straight flight the frame the track is not there for.** `Lens::OFF` is an exact
identity rather than a very close one, `Lens::bends` routes an exposure the
bubble does not reach straight to `draw_streak`, `draw_path` over two points
lays down the bytes `draw_streak` would — clipped or not, which it did not
manage until the two were made one body — and an exposure that does not reach
back past a turn takes the arithmetic it always took, spelled out rather than
derived as the one-pose case of the walk. There are tests comparing the buffers,
not saying they look alike.

Two boundaries get crossed mid-flight and each is why. A star crossing the
ten-Einstein-radii boundary swaps between the bent and unbent paths, so an ulp
of disagreement twinkles a ring into the sky and makes engaging the drive
re-render the field instead of bending it. And the *whole sky* swaps between
straight and curved the moment a hand touches the stick — every star at once,
which is why a disagreement there is a step in the frame's brightness rather
than a ring in it, and why the falloff had to be measured the same way on both
sides before any of this could land.

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

- No star can come between the camera and the hull, which used to need a
  `const _: () = assert!(...)` against the star band's near wall at eighteen
  units. `universe::NEAREST_STAR` is four *light years* against a hull that
  reaches seventeen units, so the two are now twelve orders of magnitude apart
  and there is no arithmetic left to check. Its opposite number is still real
  and still in `src/models.rs`: at `view::MIN_SHIP_DISTANCE` the hull must clear
  `camera::Z_NEAR`, or `plates` starts dropping faces whole and the ship comes
  apart a plate at a time while going on looking like a ship. Widening
  `ZOOM_MIN` or `ZOOM_MAX` in `src/view.rs` is what that one is there to stop.
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

**The cut is made in the dark, and the whole of it is four fields and one
multiply.** A change of camera dips the picture to black, changes over at the
bottom, and brings the new one up; a run opens at that same bottom and rises out
of it, which is why the two are one mechanism rather than two. `--fade` is the
length of the whole cut and `app::FADE_OUT_SHARE` is how much of it goes down.

Five things about it are load-bearing and none is obvious from the code alone.

*It is a closed form of `Flight::time`, not a decaying transient.* `Ship::flash`
is the tree's model for "a thing that happens and then stops", and it is the
wrong one here for a reason that is **not** frame-rate independence —
`exp(-k·dt)` has that too, which is why `ship.rs` eases that way. What a closed
form buys is what the autopilot's throttle bought when it stopped stepping:
independence that is structural because nothing is stepped, and an end that is
*reached* rather than approached. That second half is what everything else rests
on. `app::fade_gain` returns a literal 1.0 past the end, `v * 1.0` is the
identity in IEEE, and so a settled frame is byte for byte the frame the fade is
not there for. A gain a single ulp under one would repaint the whole sky for the
rest of a flight and no hash here could say which change did it.

*The gain rides `Canvas::resolve_into` and nothing else.* That is the one pass
that already visits every subpixel — `clear` fills, `add_flash` adds, and
`apply_vignette` multiplies by a radius — so the fade costs no pass of its own,
and it measured as costing nothing at all: 8.22 ms of drawing against 7.96, and
0.93 against 0.92 in the cockpit, minimum of five sweeps at 200x60. Scaling the
*linear* light rather than the eight bits at the far end is what makes it a stop
of exposure instead of a dissolve, since the tonemap is `1 - exp(-v·exposure)`
and a gain on `v` is algebraically a gain on the exposure: the faint stars go
out first and the bright ones hold on. There is deliberately **no**
`gain == 1.0` fast path — a branch would be a second spelling of arithmetic the
reference frames are pinned to, and the multiply is already the identity.

*`Flight::view` and `Flight::shown` are two different questions.* The first is
which camera the *flight* is on and it changes the instant `C` is pressed, so
the stick split in `handle_key`, the wheel in `handle_mouse` and the hint tiers
all change hands at once and nothing has to learn about a switch that has not
landed. The second is which camera *this frame* is built from, and it waits for
the bottom of the dip. `Readout` carries the first, because the hints name keys
and the keys have already moved.

*`set_view` is the only funnel and it takes `previous` from `shown`, not from
`view`.* The difference shows only when a second cut lands inside the first
one's fall, which auto-repeat on `C` reaches constantly: what has to fade out is
whatever is on screen, not a camera the flight committed to and never drew. The
`from` field is the other half of that — the curve parameter the fall starts
from — and without it a second press would slam the shutter back open and drop
it again. The early return is what keeps `M` pressed while already outside from
dipping the picture for a dialogue that is only opening, and it is why `R`, a
resize and a change of ship cut nothing: none of them changes the view.

*A cut made while the flight is stopped lands at once.* `P` stops `advance`,
which is the only thing that moves `time`, so a cut armed there never gets
anywhere — it sits at zero elapsed, where the shutter is fully open and the
picture is still the outgoing camera, and `C` looks like a key that does nothing
whatever. `Flight::draw` lands it, because that is the only function in the tree
handed both the flight and whether it is stopped, so one line covers `C` while
paused, `M` while paused, and a `P` taken mid-cut. The argument against the long
essay on `P` above is that a cut is not a schedule: it is a dissolve between two
moving pictures and a stopped flight has neither. The nearest precedent is `R`,
which snaps rather than eases for the same reason. And unlike the camera bug
that essay is built on, this owes nothing on the way out — `from` goes back to
one and the picture is settled, so unpausing resumes a flight rather than
replaying a stored dip.

Two consequences worth knowing and neither worth fixing. While a fade runs every
cell moves a level or two a frame, so `Screen::flush`'s diff stops skipping
anything and emits the whole grid for the length of the cut; it is bounded by
the cut's own length, and the frames at the trough are identically black, where
the diff emits nothing at all. And `--snapshot --warmup 0` now shoots the
trough, which is a black PNG — that is what `--fade 0` is for, and the flag's
help says so. Everything in the tree clears the fade by a wide margin: `--warmup`
defaults to 300 frames, CI's snapshot step uses 120, and both `docs/` recipes
use 600.

**`q` flies something, it does not quit.** It rolls the ship inside and the
camera outside, and it has never been the way out since it went on the stick.
The only quits are `Esc`, `Ctrl-C` and `Ctrl-D` — and in screensaver mode, any
key at all. While the picker is up it owns the keyboard including `Esc`;
`Ctrl`-modified keys bypass it so `Ctrl-C` still works. In `handle_key`, the
`Char('c' | 'd') if ctrl` arm sits **above** `Char('c' | 'C') => cycle_view`;
reorder them and `Ctrl-C` cycles the camera.

**Key releases are discarded.** They arrive only from kitty-protocol terminals,
and acting on them counts a single press twice.

**A resize goes through `cli::clamp_size`, and for a while only the *first* one
did.** `resolved_size` runs the opening ioctl answer through it on the argument
that a terminal claiming to be enormous should not be believed to the point of
exhausting memory; every later answer arrives as `Event::Resize` and went
straight into the canvas, which bounded nothing but zero. So a size the command
line refuses by name — `--size 5000x5000` answers "25000000 cells, past the
limit of 2000000" — was taken silently through the other door and allocated 1.1
GB, and `stty cols 30000 rows 30000` against a running flight asks for nine
hundred million cells. A failed allocation *aborts*: no unwind, no `Drop`, no
panic hook, so `RawGuard` never restores and the user is dropped back into a
shell in raw mode on the alternate screen — the exact outcome `Flight::new` is
built before `RawGuard::new` to prevent, reached through the far end of the same
program. The clamp lives in `Flight::resize` rather than at the call site for
the reason `advance` holds its own `dt`: it is `pub`, and a guard belongs with
the allocation it protects.

**`--size` is a fixed size, not a starting point, and that answers one question
rather than two.** `Flight::resize` returns `false` immediately when it is set —
which is right about the *canvas* and was wrong about the repaint, because the
terminal moved whatever the canvas did. The alternate screen discards the rows
past a new smaller height, `Screen::front` goes on holding the cells it drew
there, and `diff` re-emits none of them: a band of the frame stays blank until
each cell in it happens to take a different value, which on a starfield is
mostly never. The repaint follows the last size the *terminal* reported now and
the canvas follows `resize`, and the reason the gate exists survives intact — a
resize event that settles on the size already in use reports the same pair and
still repaints nothing. Without that the flag held only until the
first resize event. A resize no longer touches the sky at all — it resizes the
canvas and stops. That is a stronger statement than the one that stood here for
a while, which said it moved "the frustum the stars are laid out in, which is
the two `retarget` calls": there is no frustum and there are no `retarget`
calls, and there have been neither since the sky went world-space. The sky is
laid out in the world rather than in front of the camera, so a wider window is
more of the same sky rather than a differently-shaped pool, and the two calls
that used to stand in `Flight::resize` existed to paper over exactly that —
each of them dropping every trail in the field to hide the seam. Before *that*
it re-derived the count from the new canvas whenever `--stars` was 0, so
dragging a window edge respawned the sky a few hundred stars at a time and the
count the panel showed at the time walked about while nothing had asked it to.
A count a window can overrule is not one. The panel does not show one any more —
it reads out the magnitude that was asked for and lets the count follow — but
the fault was about the pool rather than about the readout, and it is still
there to be reintroduced.

**A sky is asked for by how faint a star it holds**, `--magnitude`, and the
count follows. That is the third answer to this question and the first one that
is about the sky rather than about the program. It was a *density* once — stars
per subpixel, 0.02 where it finished — which bought constant apparent density
and cost the one thing the rest of this tree is written to stop: the same flight
looking different on two machines. Its own answers say how wide that spread was:
19 stars at 40x12, 76 at 80x24, 172 at 120x36, 480 at 200x60, 1080 at 300x90. It
also pulled the wrong way while doing it, since a wider window got more stars
*and* longer streaks at once. Then it was a fixed count, `--stars 256`, which
held the line by giving up on saying anything about what a window shows.

A limiting magnitude says both at once: the same universe on every terminal, and
as much of it on screen as the window can see. Two things follow that are worth
knowing before the default is argued about again. The cockpit's field of view is
fixed by `Camera::focal` following the canvas height, so **the on-screen count
is the same on every terminal of the same aspect** — a bigger window is the same
sky better resolved rather than more of it. And `cli::DEFAULT_MAGNITUDE` is 6.0
rather than the 6.5 a dark sky really reaches, which is a half magnitude paid to
the grid rather than to taste: at 6.5 a 120x36 canvas is asked for about a
thousand stars across 8 640 subpixels, the faint end of the count law is most of
them, and the picture comes out an even wash rather than a sky. Both were shot
and looked at. If you move it, shoot it again.

**A star is placed inside its own visibility sphere, and that sphere is centred
on the ship rather than on the world's origin.** `Universe::spawn` centred it on
the origin, which is right exactly once — at construction, before the ship has
gone anywhere — and `set_limit` is the other caller, arriving from a key at any
point in a flight. Ten minutes of full warp put the ship three thousand light
years out, and the four and a half thousand stars a press of `+` added landed in
a clump directly astern: a mean direction from ship to fresh star of (0.000,
0.001, -0.989) against about nothing for a sky made fresh. Nobody reported a
clump, because none of them survived a step — every one was outside its own
`reach_sq`, so the next `advance` recycled it to exactly the limiting magnitude,
where it is at no brightness at all. The key moved the readout, quadrupled the
pool, and did not change the picture.

`stock` reads the origin once and hands it down, spelled the way the recycle
spells it. `Track::pose_at` answers `[0.0; 3]` on an empty ring, so construction
is bit-identical and no reference hash moved. What is worth carrying forward is
why nothing caught it: all three tests over this path — the two on `set_limit`
and `no_direction_holds_a_thinner_sky_than_any_other`, which is the acceptance
test for the whole world-space rebuild — fly a ship parked at the origin, which
is the one place the bug cannot happen.

**The `+`/`-` keys reach the two ends `--magnitude` reaches** — `cli::MIN_MAGNITUDE`
and `cli::MAX_MAGNITUDE` — so a sky the keys walk to can never be one the
command line would have refused. That invariant is older than the flag it now
guards: `+` used to carry a ceiling of 20 000 that sat *under* what `--stars`
allowed, so `--stars 100000` and a single press shrank the pool by four fifths.

The floor and the two special cases that went with it are gone, and the reason
is worth keeping because it is the same shape of argument. A multiply has fixed
points — zero and one — so the old keys needed a `POOL_FLOOR` of 8 to stop `+`
swallowing the press below four stars, and an early return in `resize_pool` to
stop `-` clamping a deliberately empty sky *up* to that floor. An addition on a
magnitude has neither, so a sky can be emptied and refilled by the same two keys
that set it, and `the_magnitude_keys_move_the_limit_the_way_they_point` says
both ends are reached rather than approached.

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
stylistic choice, though the reason has softened: the star band cached
`cam.focal` and laid its whole layout against it, so a zoom that touched the
focal length needed a `retarget` on every notch — and that dropped every trail,
which is a scratch across the frame. The sky is in the world now and would
survive it, but a zoom is still a dolly because that is the honest picture. The
dolly leaves the sky untouched, and
`the_zoom_moves_the_ship_and_leaves_the_sky_alone` in `app.rs` fails the moment
anything makes `exterior_camera` read the zoom. An *orbit* is the opposite case
and deliberately so: it is meant to take the sky with it, so it is handed to
`Universe` through the camera's own basis rather than through its focal length.

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
the ship flies where its nose points, the sky is streamed along that track by
the ship's own attitude, and there is no horizon for an angle to be measured
against. So `heading` and `pitch` are a *compass* — an
instrument reading, and the panel's business — not a bearing off some fixed
frame, and posing the hull from either tips it off the track and leaves it
there.

**They are read off the attitude rather than integrated beside it, and the
direction of that is the whole of it.** `pitch` was an integral of the pitch
rate with a clamp on top, under a comment saying the axis stops short of
straight up because there is no way back over the top. The axis does not stop;
only the number did — and a clamp discards the *input* rather than the rate, so
what it threw away never came back. Five seconds of `W` and five of `S` leave
the ship level, at a flown nose of -0.7 degrees, with the panel reading +76.5
and staying there. `heading` was the same shape without a clamp to make it
obvious: the yaw rate is about the ship's own vertical and rotations do not
commute, so a yaw flown while rolled moved the compass by the amount it would
have moved level.

Posing or projecting *from* a compass is the bug this tree has shipped twice,
because a reading is one number where an attitude is three axes and the missing
two get invented. Reading a compass *off* the attitude is the other direction
and cannot go that way: nothing in the frame depends on the answer. `roll` is
the one that still cannot be read back, since it turns the ship about the axis
it is flying along and leaves no trace in the nose — and it is what
`models::attitude` poses the hull by, where the other two are instruments and
nothing else. That was a real bug: a few seconds of `W` inside, then `C`, and the ship
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
`an_unknown_colour_mode_is_refused_and_the_message_says_what_there_is` in
`cli.rs`, and the default
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

**The panel has two layouts and a third state, and the third is decided by who
is flying rather than by the window.** `Readout::panel` is false for `--demo`
and `--screensaver`, and `hud::draw` returns before it measures anything — above
the size check deliberately, since the compact layout is the same panel on a
window with no room for it. Of the two layouts that remain: below `MIN_COLS` (46) or
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
/// There is nothing between here and Proxima at 4.2 light years, and the
/// catalogue needs telling because it is built from photometry rather than
/// from a volume: the faint end of the scatter has a visibility sphere under
/// two light years across and is therefore *always* very close. Those were the
/// stars that tore past at impulse in a sky whose whole point is to hold still
/// down there. They are left out rather than moved, since a star that faint is
/// not visible from anywhere it could actually be.
pub const NEAREST_STAR: f32 = 4.0;
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

A property that holds bitwise on the fast path is worth a word of its own,
because the whole trail suite went on passing unchanged when the exposure
started following the flown track — every one of those tests flies a ship nobody
steers, so all of them stayed on the arithmetic they were written for. That is a
hole rather than a reassurance, and the answer was siblings rather than surgery:
`a_streak_is_the_track_the_star_actually_flew` kept its assertions and became
the fast path's guard, and
`a_streak_is_the_track_the_star_actually_flew_when_the_ship_was_turning` is the
one that looks at the new arithmetic. When a change leaves a suite green, ask
what the suite was flying.

**And when a new test passes first time, mutate the thing it guards before
believing it.** That is the same question asked of a test rather than of a
suite, and it is worth the two minutes every time. Three of the guards written
for `bend.rs` passed immediately and one of them was worthless: a sheaf of
exposures running from six ring-radii straight through the bubble leaves the
shadow perfectly clear, and goes on doing so with *both* `shadowed` guards
deleted, because every head in it sits out where the counter-image's gain is
under `FAINTEST_COUNTER_IMAGE` and the second image is never drawn at all. It
was a test of the primary image wearing the counter-image's name. What fixed it
was measuring where the interesting band actually is — one to three and a half
ring-radii out, where a counter-image is both inside the shadow and bright
enough to draw — and putting the heads there.

Two habits fall out of that and both are cheap. Write down what a test was
measured to catch, in the test, because the next reader cannot re-derive it from
the assertions: `a_bent_exposure_leaves_the_disc_the_ship_sits_in_empty` says
that it fails on a broken `map` and on both guards going, and *not* on either
going alone, because at a head in that band each masks the other. And put a
non-vacuity assertion beside every comparison of two things that could both be
empty — two blank canvases agree beautifully, and so do two identical
subtractions of a sky that holds no stars.

The sky's own guards are the sharpest examples of that style in the tree and
are worth reading before adding to them. Three of them are what the rebuild is
warranted by: `a_camera_swing_moves_no_star` is bitwise over a full turn on all
three axes, `no_direction_holds_a_thinner_sky_than_any_other` counts the pool
inside a cone about two hundred directions and holds every one within five
standard deviations of the mean, and `the_sky_holds_still_at_impulse` bounds how
far a star may move in thirty seconds of full impulse. That last one has a
margin of 3.3x and **deliberately not more**: a threshold a hundred times clear
would go on passing a speed map that had regressed by an order of magnitude,
which is exactly the fault it exists to catch. The isotropy test asks the *pool*
rather than a rendered frame, and that is the point rather than convenience — a
frame at any sane star count is Poisson-noisy at better than two to one between
its fullest and emptiest corner, so a frame cannot tell an anisotropy from an
ordinary evening.

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
`Flight::draw` and a third `universe::Observer` constructor beside `cockpit` and
`outside` — which has to set `mount`, the camera's pose in the hull's own frame,
since that is what an exposure is re-mounted on when it walks back through the
poses the ship held (`Flight::advance` no longer branches on the view: there is one sky
and it is stepped the same way whoever is watching), the `ViewArg` in
`src/cli.rs`, and, if the controls
differ, a *pair* of hint arrays in `src/hud.rs`, since every face is spelled
twice over for the ASCII terminal. `hud::Readout` carries `view` for exactly
this, and `Glyphs::hints_for` is where a third answer goes. Then decide what the
stick does in it: `handle_key`'s `steers`, `flies_the_camera` and `zooms` are
all `== ViewMode::X` today, so a third view wants a third answer to each of the
three — and a key matching none of them falls through to nothing at all, which
is a control that has quietly gone missing rather than a compile error.

The cut costs a third view nothing, and that is worth knowing rather than
rediscovering: `Flight::set_view` is the only funnel, it compares views rather
than enumerating them, and `Flight::shown` picks between two names without
caring which. The arm in `Flight::draw` matches on `shown()` rather than on
`view`, so the new one only has to be added there once, like the other two.

**Changing the controls.** Five places now, and the fifth is the one a user
reads. `CONTROLS` in `src/cli.rs` is an `after_help` block naming every key,
split by view, and `the_help_names_every_key_that_does_something` holds it to
the keyboard — it drives each key as well as looking it up, so the list cannot
become a description of controls that no longer exist. It exists because the
hint line is chosen by width: the widest cockpit tier is 89 characters and so
wants 91 columns, and on the eighty-column terminal most people have it falls to
a tier naming four keys of a dozen, with `+` and `-` on no tier at any width and
nothing at all below `MIN_COLS`. Widening the tiers is the wrong answer twice
over — it costs columns the narrow tiers have not got, and five of the ten
reference flights render at 120 columns with a panel on them, so their hashes
would move to say it. It was all ten until the flights that fly themselves
stopped drawing one. Help text costs no columns and moves nothing.

Watch how that guard first passed, because the shape recurs: `help.contains("M")`
is true of a page that also spells `--magnitude <MAG>`, so it answered yes with
the picker's whole line deleted. It reads the key *column* of the block now.

Then the four hint arrays in `src/hud.rs` — a face per colour
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
camera. Those tiers are the only place the keys are written down to somebody
flying, so a control that fits none of them is one nothing tells the user about
outside `--help`. They are drawn only when somebody *is* flying: `--demo` and
`--screensaver` get no panel, which is not a gap, since a screensaver has no
keys but the one that quits.

**Adding a NAV readout.** The panel has room for one, and the room is borrowed
rather than earned. The bottom three rows are counted *up* from the bottom —
status at `rows - 3`, throttle at `rows - 2`, hints at `rows - 1` — while the
NAV panel is counted *down* from the top and closes at `nav_bottom_row`, which
is `2 + nav_rows(view)`. At `MIN_ROWS` (12), in the side view where the `SHIP`
row already makes five, the closing rule lands on row 7 and the banner on row 9.
A sixth row lands on 8 and still clears; a seventh collides.

That last sentence is the one that has not moved, and it is worth knowing why:
the budget was spent to the last row until the `WARP` row came out. It closed on
8 from outside with nothing between it and the banner, and it read `FACTOR 9.78`
while the banner said `WARP DRIVE ENGAGED — FACTOR 9.78` across the bottom of
the same frame. So the spare row is the redundancy refunded, and there is only
one of it.

**And a sixth row moves the reticle, which is the coupling to know about.**
`draw_reticle` refuses to draw when its top brackets would land inside the
panel, and it asks `nav_rows` rather than counting, because it goes down
*before* the panel and so cannot look — that order is not free to change, since
the reticle lightens what is behind it where the panel covers. A
`debug_assert_eq!` in `draw_nav_panel` holds the row list and `nav_rows`
together. This paragraph used to end "and no test guarding it", and what was
sitting in that gap was the brackets showing through the panel's own spaces at
every height from `MIN_ROWS` to 19 — 21 while the panel was a row taller — in
both faces, at every width. It is
`the_reticle_never_lands_in_the_instrument_panel` now — a sweep over heights,
which is what nothing had, beside the sweep over widths that
`the_hints_never_eat_the_throttle_readout` already was.

**That sweep also has to look below the header, and did not.** `Glyphs::ASCII`
spells the panel's opening and closing corners with the same `+`, so a search
for the closing rule that started at row 1 answered with the header — and the
ASCII half of a test whose comment says "in both faces" swept exactly one row.
Taking the reticle's standoff out and running that face alone passes searching
from row 1 and fails searching from row 2, which is how it was measured.

**And a shorter panel lets the reticle down sooner, which nothing was watching.**
The refusal is `cy - dy <= nav_bottom` against `cy = rows / 2`, so every NAV row
shed is worth two rows of terminal: dropping `WARP` took the cockpit's floor
from 22 rows to 20. Neither guard above notices — the sweep only asks that no
bracket land *inside* the panel, which a reticle that refused everywhere
satisfies perfectly, and its sibling looks at one hard-coded 120x36. So the
threshold is pinned at both heights either side of it now, by
`the_reticle_comes_back_as_soon_as_the_panel_leaves_it_room`.

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
place a person types the number. `--magnitude` needed the second of those too,
for the same reason and quite independently: its range runs down past zero, so
`--magnitude -2` is an empty sky and without the attribute it is an error.

**A note to the next editor goes in a `//` block, not a `///` one.** Clap's
derive publishes a `///` block as help text — a field's as that flag's entry,
this struct's as the whole program's long about — so a doc comment in `cli.rs`
is addressed to whoever is *running* the program, where the house style
everywhere else is an essay to whoever edits the tree next. The two had
collided on three flags: `--magnitude` and `--orbit` each explained
`allow_hyphen_values` to the world, and `--color` published a paragraph about
an `auto` mode that no longer exists, closing on a comparison to `--stars`,
which does not exist either — so the longest entry in `--help` advertised two
things the program refuses to do. `long_about = None` on the struct is the same
rule applied to the struct's own comment, which otherwise prints above the
usage line, and it is what lets it keep one for `cargo doc`.
`the_help_text_is_addressed_to_whoever_is_running_the_program` asks the rendered
help rather than the source, because what went wrong was not where the words
were written but where they came out.

**Refusing to fly is done by name too.** Interactive mode asks whether stdout is
a terminal before it takes one over, and says `no terminal on stdout; use
--headless to write frames somewhere else` when it has not got one. Without it
the answer was `RawGuard::new`'s ioctl travelling out through `main` as `No such
device or address (os error 6)` — the worst message this program prints, on a
command line that answers every other absurd argument with a sentence naming the
limit. The second failure was worse for half-working: crossterm takes raw mode
against the tty while frames go to stdout, so `warp > frames.txt` turned the
terminal's echo and line editing off, never entered the alternate screen, and
left a blank shell that looks hung. `IsTerminal` reads a fact off a file
descriptor rather than guessing a capability from the environment, which is the
distinction `ColorMode::detect` was deleted over, and `--headless` and
`--snapshot` never reach that function.

**Taking a flag away.** Do it by name rather than by silence. Clap's answer to a
flag it does not know is "unexpected argument", which is no use to a shell
history or a script — so `--stars` is still declared, `hide = true`, with a
`value_parser` that always fails and says what replaced it, and
`the_star_count_flag_says_what_replaced_it` holds the message to naming
`--magnitude`. `--color auto` is turned away the same way and for the same
reason: the value that used to work is the one worth naming.

**Changing the sky.** Beyond the hashes, four things are worth looking at
directly because no test says anything about them. Shoot a frame with
`--features snapshot` and look at it — the density that reads as a sky rather
than as static is a judgement, and `cli::DEFAULT_MAGNITUDE` was settled by
shooting 5.5, 6.0 and 6.5 and comparing. Re-derive `universe::ZERO_POINT` if the
count law or the default limit moves, and carry its fifth of a magnitude of
offset forward when you do — see above, where what that offset is for is written
down. And reshoot `docs/` — the README's two images are the first thing anybody
sees of this program and nothing will tell you they have gone stale.

Measure the shot rather than squinting at it, because an eye is a poor
photometer over a whole frame and was wrong here: `docs/astern.png` plainly
looked dimmer after the shift went in and its mean pixel had moved from 24.57 to
24.52. What had actually changed was the *distribution* — 2% fewer lit subpixels
and a middle ninth up from 45.5 to 49.0. Decode the PNG and take a mean, a lit
count and a half-frame ratio before believing what the picture seems to say.

The fourth is a *turn*, which nothing automatic reaches at all — not a reference
flight, not a snapshot recipe, and not `--demo`, whose weave used to sweep about
a twentieth of what a hand on the stick does and which flies straight now. So
this is the only route rather than merely the better one: fly one and watch it,
or drive `Flight::nudge_stick` from a scratch example and shoot a frame. What to look for is that a sustained bank draws
smooth arcs, and that the corners a *hard* turn leaves are where the turn began
rather than where the poses are: an abrupt start really does put a corner in
every trail, and telling that from too few poses took measuring the drawn
polyline against a finely sampled version of the same walk.

**Changing the renderer.** Expect the golden hashes to move, and regenerate
them in the same commit with the reason written down. Run
`cargo run --release --example bench` before and after if the change is in a
hot loop — `draw_streak`, `draw_leg`, `resolve_into`, `Universe::sweep`,
`Bend::draw_one`. Three of its rows hold the stick over, which is the only
thing in the sweep that asks the sky for a curve; the other seven measure the
renderer with the curve switched off, which is what most frames are.

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
valgrind --tool=callgrind --cache-sim=no /tmp/prof/release/examples/bench 200 60 8 side truecolor
callgrind_annotate --auto=no callgrind.out.*
```

## CI

`.github/workflows/ci.yml`, five jobs, under a workflow-level `permissions:
contents: read`. That one line is worth knowing about: GitHub hands a job the
repository's default token scope when a workflow says nothing, nothing here
writes, and it is the mitigation that does not depend on trusting an action —
`dtolnay/rust-toolchain@master` is a mutable branch reference rather than a
commit, and so are the `actions/*` majors. Pinning those to hashes is the other
half and is not done.

- **test** — `cargo test --locked` then `cargo test --locked --all-features`, on
  Linux, macOS and Windows. The matrix is the point: the renderer's whole job is
  to behave the same everywhere. There is deliberately no `cargo build` step in
  front of them; `cargo test` compiles the bin targets too, so one bought
  nothing but a second full compile on each of three operating systems.
- **lint** — `cargo fmt --all --check` first (it needs no build), then clippy
  with `-D warnings`, then `cargo package --list` and `cargo package`, then
  `cargo doc`. The package pair is there because `exclude` in `Cargo.toml` is
  hand-maintained and its failure mode is quiet: a crate that builds from the
  repository and not from the tarball people install. The `--list` runs first so
  a diff that drops a file says which one in the log rather than in a build
  error. Touching `exclude` means watching those steps.
- The `cargo doc` step takes `RUSTDOCFLAGS="-D warnings"` and is the only
  mechanical check anywhere on the prose, which is otherwise the one thing in
  this tree nothing guards. Sixteen warnings had accumulated under it before it
  existed — fifteen of them public documentation linking to private items, which
  renders as plain text rather than as a link, and one a reference to
  `models::attitude` that resolved to nothing at all. `Cargo.toml` asks docs.rs
  for `all-features` so the published pages are complete, and nothing in the
  tree renders those pages, so nothing noticed.
- **msrv** — reads `rust-version` from `Cargo.toml` (currently **1.85**) and
  `cargo check`s against it. Bumping the floor means editing that field.
- **audit** — `cargo audit`, on the schedule and on pushes to main rather than
  on every pull request: it takes minutes to build the auditor and says nothing
  about a diff. It exists because pinning the tree has a second consequence
  beside the one above — nothing will ever drift into a fix either, so an
  advisory against one of the 41 crates would sit there indefinitely under a
  green tick. The point is the notification; an update would fight the lock
  file on purpose.
- **headless** — same seed twice gives the same bytes, different seeds give
  different ones, the bytes match `tests/golden/frames.sha256`, `--color ascii`
  is really ASCII, a closed pipe is not an error, a flight without a terminal
  says which flag was wanted, the three documented benchmark recipes still run,
  a snapshot can still be written, and the two images on the README's front page
  are still the sky the renderer draws.
  The pipe check is here rather than in a unit test because what is
  under test is the process's own exit status against a pipe a shell built, and
  there is no honest way to ask that from inside the library: Rust ignores
  `SIGPIPE`, so a write to a reader that has gone away comes back as an error
  and travels out to `main` rather than ending the process where it stood. The
  terminal check is here for the same reason and against a real file
  descriptor. The benchmark recipes are here because documentation that can be
  run is worth keeping runnable and those three had stopped being: the third
  argument became a limiting magnitude when the sky did, and every recipe went
  on passing a star count and aborting.

  **The screenshot check is the one that closes a hole this file used to
  describe as unclosable.** It said of `docs/warp.png` and `docs/astern.png`
  that nothing pins these bytes, so a reshoot is checked by looking at it —
  which is how the hero came to advertise twice the sky a default run draws for
  the whole life of the renderer. They are exactly reproducible from the recipes
  written down here, on the same Linux-only caveat as the text frames and with
  the same `--locked` pinning the encoder, so they are simply hashed. A moved
  hash means the sky has changed and the front page has not: that is a reshoot
  to be looked at and committed, not a build to be fixed.
