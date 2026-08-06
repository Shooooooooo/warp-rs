# CLAUDE.md

Notes for AI assistants working in this repository. [`README.md`](README.md) is
the user-facing document — what the program does, which keys fly it, how to
wire it into tmux. This one is about the inside: how the code is arranged, what
it depends on that is not obvious, and how to write changes that read like what
is already here.

## What this is

`warp-rs` is a terminal starfield renderer: a starship flown at warp, drawn
with the upper half block `▀` in a character grid. Stars live in a real 3D
volume, are projected each frame, and are drawn as the segment between where
they were last frame and where they are now — accumulated into a
floating-point buffer and tonemapped at the end, so the hyperspace smear falls
out of the motion rather than being drawn as an effect.

One crate, `warp-rs`, with a binary named `warp`. `src/lib.rs` carries
everything; `src/main.rs` is sixteen lines of entry point. That split is
load-bearing and there is a test that fails if it stops being true
(`tests/flight.rs`).

Four dependencies, on purpose: `clap`, `crossterm`, `rand`, and `png` behind
the optional `snapshot` feature. **Do not add a dependency** without saying why
the tree cannot do it — no `nalgebra` for the three-by-three matrices, no
`anyhow` for the `io::Result` that is already enough.

## Commands

```sh
cargo build --locked                    # default features; what people install
cargo test                              # 225 unit + 7 flight + 3 golden, ~25s
cargo test --locked --all-features      # 226 unit — adds the snapshot-gated one
cargo fmt --all --check                 # CI runs this first
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo package --locked                  # CI runs this too; `exclude` is by hand
```

Clippy warnings are **errors** in CI. So is misformatting. Run both before
committing; they are the two cheapest ways to turn a branch red.

```sh
cargo run --release                     # fly it
cargo run --release -- --demo           # autopilot, 45s, no keyboard needed
cargo run --release --example bench     # where a frame's 16.7 ms goes
cargo run --release --example bench 200 60 20000 side
```

`bench` prints sim / draw / write milliseconds per case against the 60 fps
budget. The expensive frame in the program is the outside view at warp — every
streak near the ship is chopped into arcs and drawn twice, once per lens image
— and the sweep includes it deliberately.

Looking at a frame as an image rather than as a wall of escape codes:

```sh
cargo run --release --features snapshot -- \
    --snapshot warp.png --engage --throttle 1.0 --warmup 600 --scale 2
```

The PNG is the **starfield only**. The instrument panel and the ship picker
live in the character grid, not in the pixel buffer, so they are not in it.

## The golden frames — read this before touching the renderer

`--headless` renders on a fixed timestep with no terminal control, so a fixed
seed produces byte-identical output. `tests/golden/frames.sha256` pins those
bytes, and they are checked two ways from that one file: `cargo test --test
golden` reproduces them in process through `app::render_headless`, and the
`headless` CI job produces the same four files from a release binary and runs
`sha256sum -c`. Between them the two also prove they agree — a library that
renders one thing and a binary another would be a bug of its own.

The in-process half is the reason `render_headless` is public and separate from
`run_headless`. It costs about three seconds and is **Linux-gated**, for the
reason below.

**Four flights, and the case list lives in three places** — the comment block in
`frames.sha256`, `CASES` in `tests/golden.rs`, and the `headless` CI job. Adding
one means adding it to all three. They share `--headless --frames 120 --seed 1
--size 120x36` and differ in what they make the renderer do: two `--demo` runs
in truecolor and ascii, one `--engage --throttle 1.0`, and one of those from
`--view side`.

The last two are not decoration. With only the `--demo` pair, the reference
covered two seconds of flight that never leaves sublight — `--demo` spends its
opening six seconds easing the throttle up — so it peaked at a quarter of light
speed with the drive cold. A deliberate change to `TAIL_BRIGHTNESS` did not move
the hashes at all, because a sublight streak is shorter than a subpixel and
takes the branch in `draw_streak` that never reads it. The streak ramp, the
glare, the flash, the Doppler shift and the entire view from outside — band,
lens, arcs and hulls — were all outside the reference.

**Any change to renderer arithmetic changes those hashes and turns the test
red.** That is the point of them: an edit meant to touch one thing that touched
the whole sky has to say so. When the change was intended, regenerate
deliberately, with the diff in hand:

```sh
cargo build --release
common="--headless --frames 120 --seed 1 --size 120x36"
./target/release/warp $common --demo --color truecolor > truecolor.txt
./target/release/warp $common --demo --color ascii     > ascii.txt
./target/release/warp $common --engage --throttle 1.0 --color truecolor > warp.txt
./target/release/warp $common --engage --throttle 1.0 --view side --color truecolor > side.txt
sha256sum truecolor.txt ascii.txt warp.txt side.txt > tests/golden/frames.sha256
# then put the comment block at the top of that file back
```

Say in the commit message what moved and why. Regenerating without explanation
throws away the only thing that file is for.

The hashes hold across build profiles and rustc versions but **not across
platforms** — `sin`, `cos`, `exp` and `powf` come from the system maths library
and recycling the star pool turns a one-ulp difference into a different sky. So
they are checked on `ubuntu-latest` only.

The consequence for how you edit: **do not "clean up" duplicated float
arithmetic on the pinned paths.** `Renderer::exterior_camera` spells out the
same shake calculation as `Renderer::camera` rather than sharing a helper, and
says so at `src/render.rs:99` — extracting it is exactly the sort of obviously
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
size. Perf edits are documented with the measurement that justified them —
`draw_streak` takes one reciprocal per streak rather than a divide per sample,
worth about six percent of drawing time at twenty thousand stars — so do not
undo one without a number saying why.

## Layout

```
src/lib.rs        module list and the orientation doc comment
src/main.rs       parse, fly, report — nothing else
src/app.rs        the three loops (interactive, headless, snapshot) and Flight
src/autopilot.rs  the hand on the stick for --demo and --screensaver
src/cli.rs        every flag, and the bounds each one is held to
src/view.rs       ViewMode, the side camera's geometry, and the zoom's range
src/ship.rs       flight model: throttle, warp, steering, transients
src/starfield.rs  the cockpit's sky — a cone opening forward, plus Camera
src/exterior.rs   the side view's sky — a band the ship flies through
src/lens.rs       the warp bubble as a thin point-mass gravitational lens
src/models.rs     the six hulls, and how to draw one
src/menu.rs       the ship picker
src/canvas.rs     f32 RGB accumulation buffer, rasterisers, tonemap
src/render.rs     assembling a frame: sky, then what is lit, then the glass
src/hud.rs        the instrument panel
src/term.rs       Screen (double-buffered cells), ColorMode, RawGuard
src/snapshot.rs   PNG writer, behind `--features snapshot`
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
   (a tight core inside a wide halo, both ramping with the *cube* of the warp
   ramp); side view gets the wash inside the lens shadow, so the swept-clear
   disc reads as a bubble rather than as a hole punched in the sky.
4. The hull, side view only: `models::draw` — plates via `canvas.fill_convex`,
   then the drive on top of them, which is each bell's glow and the exhaust it
   throws. It takes the standoff as an argument, since the zoom moves it:
   `view::ship_distance(zoom)`, worked out once in `render_exterior` alongside
   the half-length the lens is sized from. It also takes `time`, and only the
   flame's gutter reads it.
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
| output | `Screen::flush` | `write_plain` | `write_png` of `pixels()` |

`--demo 5 --headless` does not stop after five seconds; headless always runs
exactly `--frames` frames. The deadline is checked only in the interactive loop.

**`--fps` means two different things.** Interactively it is only a frame budget,
and the panel shows a smoothed measurement. In headless and snapshot it *is* the
simulation timestep — `dt = 1.0 / args.fps` — so changing it changes the flight,
and changing how it is used moves the golden frames.

Three more asymmetries worth knowing before you touch the loops: `P` gates only
`advance`, so a paused `--demo` still runs its autopilot, still repaints, and
still exits at its deadline. In screensaver mode `handle_key` is never reached
at all — any non-release key breaks the loop, so there are no controls, not even
pause. And `R` restores `args.throttle`, not `Ship::new()`'s 0.18.

The interactive loop handles **input at the end of the frame**, in an
event-drain loop that spends the rest of the frame budget blocking on
`event::poll` rather than sleeping — so a key is acted on the moment it
arrives, which at `--fps 5` is the difference between a screensaver that
dismisses when touched and one that finishes its nap first. `dt` is measured
frame-start to frame-start and therefore includes that wait; **there is no
`sleep` anywhere in the loop, and adding one would break the frame cap.**

### The two skies

`starfield::StarField` is a cone opening forward from the canopy: right for the
cockpit, wrong to look at side-on, because there is nothing behind the ship.
`exterior::ExteriorField` is the same idea rebuilt as a band the ship flies
*through*. It is built lazily — a cockpit-only run pays nothing for it, not
even a draw from its generator — and gets its own RNG seeded with
`seed ^ EXTERIOR_SEED` so building it never disturbs the cockpit field's
stream, which is the one the reference frames were recorded from.

The two also carry **separate copies of the same-named constants** — `Z_NEAR`,
`Z_FAR`, `SPAWN_MARGIN`, `DEPTH_FALLOFF` all exist in both modules with
different values (0.9/260 in `starfield`, 18/320 in `exterior`). They are not
duplication to be merged; they describe different volumes.

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

## Invariants a careless edit breaks

**Allocation happens before the terminal is taken over.** A failed allocation
aborts the process outright — no unwind, no `Drop`, no panic hook — so anything
installed first would never be undone and the user is left in raw mode on the
alternate screen with no cursor and no prompt. `Flight::new` is constructed
before `RawGuard::new`; keep it that way. The same reasoning is why every
number `cli.rs` accepts is bounded at parse time (`MAX_CELLS`, `MAX_DIM`,
`MAX_STARS`), and why an ioctl answer is clamped rather than believed.

**`RawGuard::new` builds the guard value immediately after `enable_raw_mode`,
before any other fallible call**, so an early `?` still restores. It installs a
panic hook that restores first, so a panic mid-render leaves a readable
terminal. Never `process::exit` inside the interactive loop — `Drop` would not
run. Returning an error is fine: `main` prints after the guard has restored.
And `let _guard = ...` must keep a real binding name; `let _ =` drops it
immediately.

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

**Light adds; hulls write.** Everything in `Canvas` accumulates, because
everything in it is light and a hundred streaks crossing a subpixel ought to
pile up. `fill_convex` is the one exception — a hull is not light, so it covers
what is behind it. Values run past 1.0 and are pulled back once, at the end, by
the tonemap; that is what makes overlapping streaks bloom instead of clip. It
is also why the drive is drawn *after* the plates: an opaque write over a glow
erases it.

That rule is what settles where the exhaust goes, and it costs something to
keep. Five of the six ships put every bell a hair aft of the hull, so their
plumes stream into clear sky and the order could not matter less; the
enterprise's impulse bell is mid-ship, and its plume clears the nacelle tops by
0.165 hull units — a subpixel and a half at the reference framing, so a roll
walks it straight across them. Drawn *under* the plates it would be chopped by
a silhouette it is barely clear of. Over them it shines through as the wash a
hot plume genuinely puts on structure it plays over, which is the cheaper of
the two mistakes.

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

**There is no depth buffer and none is needed.** Three things stand in for one,
and all three have to keep holding:

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
- What survives is painted far to near.

**`q` steers, it does not quit.** The only quits are `Esc`, `Ctrl-C` and
`Ctrl-D` — and in screensaver mode, any key at all. While the picker is up it
owns the keyboard including `Esc`; `Ctrl`-modified keys bypass it so `Ctrl-C`
still works. In `handle_key`, the `Char('c' | 'd') if ctrl` arm sits **above**
`Char('c' | 'C') => cycle_view`; reorder them and `Ctrl-C` cycles the camera.

**Key releases are discarded.** They arrive only from kitty-protocol terminals,
and acting on them counts a single press twice.

**`--size` is a fixed size, not a starting point.** `Flight::resize` returns
`false` immediately when it is set. Without that the flag held only until the
first resize event. Relatedly, a resize only retunes the star pool when
`--stars` is 0 — an explicit count is not a suggestion — and the `+`/`-` keys
clamp to `AUTO_MAX_STARS` (20 000) whatever `--stars` said, so with
`--stars 100000` a single `+` shrinks the pool.

**Pitch and yaw are switched off outside the cockpit, and the zoom is switched
off inside it.** Not because either could not be wired up, but because out
there a turn moves nothing an eye can see and in here there is no ship to be
made bigger, and a control that swallows input and gives nothing back is worse
than one plainly not connected. `handle_key` says this twice, as `steers` and
`zooms`, and they are deliberately the same shape. If you change either, the
hint tiers in `hud.rs` have to follow — they are the only place the *running
program* writes the controls down — and so does `README.md`, whose `Flying`
table annotates pitch and yaw "Cockpit only" and the zoom "Outside only", and
says both again in prose further down.

**The zoom is a dolly, not a lens.** `Renderer::exterior_camera` keeps
`cam.focal` at `h * SIDE_FOCAL` whatever the zoom is doing; what moves is the
standoff in `models::to_camera`, which is now an argument. That is not a
stylistic choice: `ExteriorField` caches `cam.focal` and lays its whole band out
against it, so a zoom that touched the focal length would need `retarget` on
every notch — and `retarget` drops every trail, which is a scratch across the
frame. The dolly leaves the sky untouched, and
`the_zoom_moves_the_ship_and_leaves_the_sky_alone` in `app.rs` fails the moment
anything makes `exterior_camera` read the zoom.

**The warp bubble is measured in ships.** `Lens::for_warp` takes the hull's
on-screen half-length, not the canvas height, so the bubble scales with the
zoom instead of hanging in the frame at a fixed size. That constant used to be
`0.48` of the canvas with a comment saying it was twice `SHIP_SCREEN_FRAC`'s
`0.24` — a relationship nothing could check, and exactly the sort that rots.
`the_bubble_is_the_same_number_of_ships_across_at_every_zoom` in `view.rs` is
what checks it now, through both real arithmetics rather than against a
constant.

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

**`--color ascii` emits no escape codes beyond cursor moves, and no byte
outside printable ASCII.** Not even a reset — on a `TERM=dumb` terminal even
`\x1b[39m` arrives as visible garbage, and there is a regression test for it.
CI greps for both properties.

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

### Which beam the camera is on

**Starboard**, and the arithmetic is the thing to check rather than the prose:
`to_camera` is `(x, y, z) → (z, y, −x)`, a quarter turn about the ship's own
down axis that puts the nose to screen right and the starboard side toward the
camera. Determinant one, so face winding survives it — the mirror image
`(z, y, x)` looks almost the same and inverts every facing test. Two doc
comments said "port" for a while and neither changed what was drawn, so if you
find prose and geometry disagreeing again, `to_camera` is the answer.

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
`Screen::scratch`, the exterior field's two arc scratches — and nothing
allocates per star. The hull path is the exception there:
`models::plates` builds its vertex, screen and plate vectors fresh each
side-view frame, which is only cheap because a hull is a few dozen faces.

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

### Commits

Imperative subject, sentence case, no scope prefix, no conventional-commits, no
trailing period. Real ones:

```
Make the hulls solid
Switch off pitch and yaw in the view from outside
Take the division out of the streak sampling loop
Reject nonsense flags, and stop shouting colours at a pipe
Wait for input instead of sleeping through it
Put the camera and the hangar on the hint line
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

An `Engine`'s `radius` is doing more work than it looks like. It sets the bell's
glow, and it also sets how long and how bright a trail that bell throws — a
bigger drive throws a longer flame, which is what hands the fleet its variety
without a per-ship table. The fleet spans 0.07 to 0.17 and `NOMINAL_BELL` is the
middle of that; a new ship far outside the range will trail unlike anything else
in the hangar, which may well be the point, but it is a decision rather than a
detail.

The picker mostly takes care of itself now. It reserves `CHROME_ROWS` (6) for
its frame, title, rule, blank and footer, gives what is left to the list, and
windows that on the cursor — so a seventh ship costs nothing on a terminal with
the room and scrolls on one without. It used to lay the list out at full height
and clip, which at six ships already overflowed `MIN_ROWS` and dropped the
footer and the closing rule; that is what the box-always-closes test is for.

Two things a new ship still moves. The box's *width* is derived from the longest
blurb, so a long one widens the dialogue on every terminal that can afford it —
keep blurbs to a line. And when the list is windowed the title carries an `n/N`
counter, which is the one degradation here that announces itself: shedding a
blurb can go quietly, hiding whole ships cannot.

**Adding a camera.** Add the variant to `ViewMode::ALL` in `src/view.rs` — the
cycle and `label()` are written so a third costs one line — then the arm in
`Flight::advance`/`draw`, the `ViewArg` in `src/cli.rs`, and a hint tier in
`src/hud.rs` if the controls differ. `hud::Readout` carries `view` for exactly
this.

**Changing the controls.** Four hint arrays in `src/hud.rs` — a face per colour
mode times a face per view — and one test holds all four to the same shape:
exactly three tiers, strictly decreasing in width so the first that fits is the
most detailed, and the shortest no wider than `MIN_COLS - 2`. That last budget
is tighter than it reads; the narrowest cockpit tier already uses 41 of its 44
columns, so a new control does not simply get appended to all three. The zoom
is the worked example: `[] zoom` costs nine columns with its separator, the
narrowest side tier has seven spare, and putting it on the middle tier would
carry that one past sixty columns — which is the width `tests/flight.rs` flies
at, so those runs would shed the tier and lose the *throttle* to gain the zoom.
It went on the widest tier only, where `C view` and `M ships` already stop.
`README.md`'s `Flying` table is the other place the keys are written down.

**Adding a NAV readout.** The panel has one spare row and no test guarding it.
The bottom three rows are counted *up* from the bottom — status at `rows - 3`,
throttle at `rows - 2`, hints at `rows - 1` — while the NAV panel is counted
*down* from the top and closes at `2 + rows.len()`. At `MIN_ROWS` (12), in the
side view where the `SHIP` row already makes six, the closing rule lands on row
8 and the banner on row 9. A seventh row collides.

**Adding a flag.** `src/cli.rs`, with a bound and a `value_parser`, and
`conflicts_with`/`requires` if it only means something in combination. Every
number there is bounded; unbounded ones have caused real bugs, and the
regression tests at the bottom of the file name them. Snapshot-only flags are
`#[cfg(feature = "snapshot")]`.

**Changing the renderer.** Expect the golden hashes to move, and regenerate
them in the same commit with the reason written down. Run
`cargo run --release --example bench` before and after if the change is in a
hot loop — `draw_streak`, `resolve_into`, `Screen::flush`, `ExteriorField::draw`.

## CI

`.github/workflows/ci.yml`, four jobs:

- **test** — build and test on Linux, macOS and Windows, default features then
  `--all-features`. The matrix is the point: the renderer's whole job is to
  behave the same everywhere.
- **lint** — `cargo fmt --all --check` first (it needs no build), then clippy
  with `-D warnings`, then `cargo package`. That last one is there because
  `exclude` in `Cargo.toml` is hand-maintained and its failure mode is quiet: a
  crate that builds from the repository and not from the tarball people
  install. Touching `exclude` means watching that step.
- **msrv** — reads `rust-version` from `Cargo.toml` (currently **1.85**) and
  `cargo check`s against it. Bumping the floor means editing that field.
- **headless** — same seed twice gives the same bytes, different seeds give
  different ones, the bytes match `tests/golden/frames.sha256`, `--color ascii`
  is really ASCII, and a snapshot can still be written.
