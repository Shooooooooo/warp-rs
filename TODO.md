# TODO

Findings from a review of the tree at 9f35e34. Every measurement below was taken
on that commit; the numbers are reproducible with the commands quoted.

Ordered roughly by what is worth doing first.

## Bugs

### The control hints overwrite the throttle bar

`hud::draw_hints` right-aligns a 61-character string on row `rows - 2`, which is
the same row `hud::draw_throttle` uses for columns 2..27. They collide for any
terminal 63 to 99 columns wide — including the 80x24 that most people are
actually looking at:

```
  THR ███░░░░░░░░SPACE warp  ↑↓ throttle  ←→IK steer  P pause  R reset,+Q quit
```

The bar is cut off mid-fill and the percentage readout is gone. `draw_hints`
already tests whether the hint fits the screen; it needs to test whether it fits
the space the throttle leaves, or the two need to be laid out together.

Every HUD test renders at 100 or 120 columns, which is why nothing caught it. A
case at 80 columns should come with the fix.

### The starfield washes out in a turn

`StarField::update` recycles a star that has left sideways with
`DepthRule::FarPlane`, the same rule it uses for one that flew past the canopy.
But `depth` in `streaks` reaches zero exactly at the far plane, by design, so
those stars come back invisible and stay that way for a while. Bright stars
(intensity > 0.15) after five seconds of flight, straight versus hard yaw:

    straight = 390    after a hard turn = 84    (22%)

Four fifths of the sky fades out when you hold a turn.

The intended fix is already half-written: `DepthRule::Anywhere` exists and its
doc comment says it is "used when refilling the sides mid-turn" — but `update`
never passes it. A star leaving the side should come back in at the opposite
edge at a comparable depth, not at the far plane.

### `Renderer::pixels` is reallocated every frame

The field is documented as a "scratch buffer for resolved pixels, reused across
frames", but `render` does `self.pixels = self.canvas.resolve(...)` and
`Canvas::resolve` `.collect()`s a fresh `Vec`. At 400x100 that is a 240 KB
allocation sixty times a second. A `resolve_into(&mut self.pixels)` makes the
comment true.

## Robustness

- **No signal handling.** `RawGuard`'s `Drop` restores the terminal, but
  SIGTERM and SIGHUP skip destructors, so the terminal is left in raw mode
  inside the alternate screen. This matters most for `--screensaver` under
  tmux — the flagship use case, and one where the process may well be killed
  rather than asked to quit. crossterm already pulls in `signal-hook`.
- **`ColorMode::detect` ignores `NO_COLOR`** and never checks whether stdout is
  a terminal, so piping without `--headless` emits full escape sequences.
- **The `--demo` deadline is wall-clock** and keeps running while paused, so
  pausing a 45-second demo can end it while it is frozen.
- **Resizing discards manual star-count tuning:** `Flight::resize` re-derives
  the automatic count, silently undoing the pilot's `+`/`-` presses.
- **`--screensaver` is ignored under `--headless`** (`run_headless` only engages
  the autopilot for `--demo`), and clap has no `requires`/`conflicts_with`
  wiring for `--frames`, `--warmup` or `--scale`.

## Performance

At 400x100 — a plausible full-screen terminal on a 4K display — with 4000 stars
and the drive lit:

| | ms/frame |
| --- | --- |
| Render (`advance` + `draw`) | 8.8 |
| Output writer | 5.9 |
| **Total** | **14.7, so a 67 fps ceiling** |

The stars are not what costs. With `--stars 1` a frame still takes 12 ms: the
streaks are only about 2.3 ms of the total and everything else is fixed
per-pixel and per-cell work. Of that, the tonemap is around 2 ms (three `exp`
and three `powf` per pixel — 480k transcendental calls per frame), the vignette
0.13 ms, and roughly 4 ms goes into `compose` building 40k `Cell` structs.

So a large terminal barely clears 60 fps, and none of the work that dominates is
parallelised. Worth trying, in order:

- A quantised lookup table for the tonemap, instead of `exp`/`powf` per channel.
- `rayon::par_chunks_mut` across `resolve` and `compose`; both are per-pixel and
  embarrassingly parallel.
- Skip the pipeline entirely while paused — the main loop currently re-renders
  an identical frame at full rate.
- Reproduce with:
  `cargo run --release -- --headless --frames 300 --seed 1 --size 400x100 --engage --throttle 1.0 >/dev/null`

## Project infrastructure

- **No CI.** The README documents `cargo test` and
  `cargo clippy --all-targets --all-features -- -D warnings`, but nothing
  enforces either. No cross-platform check, so Windows and macOS support is
  plausible (nothing here is unix-only, crossterm is portable) but unverified.
- **No LICENSE file**, though `Cargo.toml` declares MIT. GitHub will not detect
  a licence without one.
- **Not rustfmt-clean** — 47 diff hunks. The hand-formatting reads better than
  rustfmt's default here, so the move is a committed `rustfmt.toml`:
  `use_small_heuristics = "Max"` alone cuts it to 28. Settle on a config, do one
  reformat pass, then `cargo fmt --check` in CI keeps the style from drifting.
- **No `lib.rs`.** Everything is a unit test inside the binary, which rules out
  integration tests and criterion benchmarks.
- **No golden-frame test**, even though the README describes exactly that by
  hand:
  `cargo run --release -- --headless --frames 120 --seed 1 --size 120x36 --demo | sha256sum`.
  Committing the hash as a test is what the deterministic headless mode was
  built for, and it would have caught the HUD collision above.
- `Cargo.toml` has no `rust-version` (MSRV), `keywords` or `categories`.

## Ideas

- **Roll control** on `Q`/`E`. The camera already has a `bank` field that does
  nothing but lag behind the yaw rate.
- **`--hud off`** for a screensaver with nothing on the glass.
- **Adaptive quality:** pick the star count from measured frame time rather than
  from canvas area, which would fix the 4K-terminal case above on its own.
- **Kitty or sixel graphics** where the terminal supports it. Half-blocks are a
  workaround for a constraint not every modern terminal still has.
- **Something to pass:** a planet, a nebula plane, a distant galaxy — anything
  that breaks up the tunnel on a long flight.
- **A theme file** for the spectral palette and the tunnel colour.
