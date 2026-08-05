# Improvements

A survey of the codebase as it stands. Items are grouped by kind and roughly
ordered by value within each group. Numbers quoted here were measured on this
machine (`--headless`, release build) rather than estimated; where a claim was
checked and turned out not to hold, it is recorded as such.

Baseline for every timing below: a 240x68 terminal (a 240x136 subpixel canvas)
with the automatic star count of 1632. A frame costs **7.7 ms** at warp. The
same frame with a single star still costs **6.5 ms**, so roughly 85% of the
frame is per-pixel work that does not depend on the starfield at all.

## Confirmed bugs

**1. The hint line overwrites the throttle bar between 63 and 89 columns.**
`draw_throttle` occupies columns 2-26 of row `rows - 2`; `draw_hints`
right-aligns 61 characters onto that same row and only checks that they fit the
width, not that they clear the throttle. At the default 80 columns the last
three bar cells and the whole percentage readout are lost:

```
THR ███░░░░░░░░░░SPACE warp  ↑↓ throttle  ←→IK steer  P pause  R reset  Q quit
```

The first width at which the two do not collide is 90.
*Fixed: the hints now have their own row.*

**2. `--stars` and `--size` are unbounded, and the abort wrecks the terminal.**
`--stars 500000000` asks for 20 GB and `--size 60000x60000` asks for 86 GB;
both die with `memory allocation of N bytes failed` and `SIGABRT`. In
`run_interactive` the `RawGuard` is installed before `Flight::new` allocates,
and `SIGABRT` runs neither `Drop` nor the panic hook, so the user is left in raw
mode on the alternate screen with a hidden cursor. Range-check both arguments
the way `--fps` already is, and allocate before entering raw mode.

**3. Accumulated time is `f32`, so a long-running screensaver freezes.**
`Flight::time` accumulates `dt` in `f32`. Stepping at 1/60 s it stops advancing
entirely at **t = 524288 s (145 hours)**, taking the twinkle and the shake with
it. The phase degrades much earlier: `(time * 31.0).sin()` at one hour is
evaluating sine at 111600, where the `f32` ulp is 0.008. The autopilot's
`start.elapsed().as_secs_f32()` has the same problem. Use `f64`, or wrap `time`
modulo a period — it only ever feeds `sin`/`cos`.

**4. `RawGuard::new` can leave raw mode enabled.** `enable_raw_mode()?`
succeeds, then any `?` on the following lines returns without a guard to drop.
Construct `Self` first, then do the rest.

**5. A doc comment describes behaviour that does not exist.**
`DepthRule::Anywhere` says it is "used when refilling the sides mid-turn", but
`update` always respawns with `FarPlane`; `Anywhere` is reached only from `new`
and `resize_pool`.

## Performance

**6. The tonemap is the single most expensive pass, and it reallocates every
frame.** `Canvas::resolve` calls `exp()` and `powf()` per channel per subpixel —
98k of each per frame, measured at **2.14 ms/frame, 28% of the total**. It also
`collect()`s a fresh `Vec` each time, in spite of the field it lands in being
documented as "reused across frames". A 1024-entry lookup table over a
sqrt-compressed domain plus a reused output buffer measured **5.1x faster**
(2.14 ms -> 0.42 ms) with a maximum error of one 8-bit level.
*Fixed.*

**7. `apply_vignette` costs 0.76 ms/frame** computing a `sqrt` per subpixel for
a value that depends only on distance from the centre. Precompute it and
invalidate on resize.

**8. `write_plain` is 10x more verbose than it needs to be.** It emits
`SetForegroundColor` and `SetBackgroundColor` unconditionally for every cell:
41 bytes/cell, **669 KB per frame**. `flush` already tracks the last colour
written; applying the same tracking gives **67 KB/frame**, exactly 10x smaller.
This is the path the README's determinism check pipes into `sha256sum`.

**9. `pick_class` re-sums the seven class weights on every spawn**, thousands of
times per second. Hoist the total into a `const`, or use a cumulative table.

Checked and **not** worth doing: hoisting `bank.sin_cos()` out of
`Camera::project`. It looks like three redundant calls per star per step, but
the optimiser already handles it and the measured difference is nil.

## Portability

**10. Windows falls back to ASCII.** `ColorMode::detect` returns `Ascii` when
`TERM` is unset, which is the normal state under Windows Terminal — a truecolor
terminal taking the degraded path. Check `WT_SESSION`, or ask crossterm.

**11. `▀` is East Asian Ambiguous**, as are `█ │ ┌ ‖ — ° ↑ ←`. Under a CJK
locale configured with ambiguous-width=double, the half block the whole
renderer is built on draws double-wide and every row breaks. `hud::truncate`
counts characters rather than display columns, so it cannot defend against this
either. At minimum this deserves a note in the README.

**12. No `is_terminal()` guard** — running without `--headless` into a pipe
sprays escape codes. There is also no `SIGTERM`/`SIGHUP` handler, so a signal
kill skips `Drop` and leaves the terminal dirty, the same failure class as (2).

## Project infrastructure

**13. There was no CI.** For a crate whose entire value is cross-terminal,
cross-platform behaviour, this was the largest single gap — it is what would
have caught (10).
*Fixed: `.github/workflows/ci.yml` runs the tests and clippy on Linux, macOS
and Windows, and re-runs the README's determinism check.*

**14. `Cargo.toml` declares `license = "MIT"` but there is no LICENSE file.**
Also absent: `rust-version` (MSRV), `keywords`, `categories`, `readme`.

**15. The tree is not rustfmt-clean** — 47 diff hunks under defaults, 28 under
`use_small_heuristics = "Max"`. The dense style is deliberate and readable; no
config reproduces it exactly, so enforcing `fmt --check` in CI means either
committing to a one-off reformat or leaving the job out. It is currently left
out.

**16. Binary-only crate.** No lib target means no `tests/`, no doc tests and no
`criterion` benches; everything has to live in `#[cfg(test)]` inside `src/`.
A thin `main.rs` over a `lib.rs` unlocks all three — and items 6 to 9 are
exactly the kind of thing that wants a benchmark guarding it.

**17. `ColorMode::detect` is untested** because it reads process-global
environment variables. Taking a lookup closure would make it testable, which
matters given (10).

## Rendering polish

**18. Streak length is pinned to the 120 Hz sim step, not to the frame.**
`star.prev` is overwritten on every sub-step, so the smear always represents
1/120 s of travel whether the display is running at 30 fps or 120. At low frame
rates the motion between frames is not covered and the field strobes. Snapshot
`prev` at the start of `advance` instead.

**19. Brightness jumps about 20% at the point-to-streak threshold.** Below
0.75 px a star gets one splat at full intensity; just above it gets two samples
totalling roughly 1.2x. Normalising by sample count would smooth the pop.

**20. Streaks lose light at the canvas edge.** `clip` clamps to `w-1`/`h-1`, so
a bilinear splat there spills half its weight into the discarded `w`-th tap.
Clipping to `w`/`h` and letting `splat` drop the out-of-range tap keeps edge
brightness uniform.

**21. Mutually exclusive flags are not declared.** `--headless` silently ignores
`--screensaver`, and `--frames` is meaningless without `--headless`. clap's
`conflicts_with` and `requires` would say so instead of quietly doing something
else.

## Features worth considering

Adaptive quality, tuning the star count to hold the target frame rate — the
machinery already exists in `resize_pool`. Knobs for field of view, star
density and palette. A `--record` mode emitting asciinema or GIF. Roll on
`Q`/`E`. Mouse steering. Nebulae or a planet flyby for visual variety.
A `--no-hud` flag.
