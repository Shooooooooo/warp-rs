# Improvements

A survey of the codebase, kept as a checklist. Items are grouped by kind and
roughly ordered by value within each group; numbers are stable, so a ticked box
stays where it is rather than being deleted.

Figures quoted here were measured on one machine (`--headless`, release build)
rather than estimated, and where a claim was checked and turned out not to hold
it is recorded as such.

Baseline for every timing below: a 240x68 terminal (a 240x136 subpixel canvas)
with the automatic star count of 1632. A frame cost **7.7 ms** at warp when this
was written. The same frame with a single star still cost **6.5 ms**, so roughly
85% of it was per-pixel work that does not depend on the starfield at all.

**Done: 8 of the 22 numbered items — every confirmed bug is now fixed.** The
feature list at the end is a separate wish list, not a backlog.

## Confirmed bugs

- [x] **1. The hint line overwrites the throttle bar between 63 and 89
  columns.** `draw_throttle` occupies columns 2-26 of row `rows - 2`;
  `draw_hints` right-aligned 61 characters onto that same row and checked only
  that they fit the width, not that they cleared the throttle. At the default 80
  columns the last three bar cells and the whole percentage readout were lost:

  ```
  THR ███░░░░░░░░░░SPACE warp  ↑↓ throttle  ←→IK steer  P pause  R reset  Q quit
  ```

  The first width at which the two did not collide was 90.
  **Fixed:** the hints have their own row, and terminals under 63 columns get a
  short hint instead of none.

- [x] **2. `--stars` and `--size` are unbounded, and the abort wrecks the
  terminal.** `--stars 500000000` asks for 20 GB and `--size 60000x60000` asks
  for 86 GB; both die with `memory allocation of N bytes failed` and `SIGABRT`.
  In `run_interactive` the `RawGuard` is installed before `Flight::new`
  allocates, and `SIGABRT` runs neither `Drop` nor the panic hook, so the user is
  left in raw mode on the alternate screen with a hidden cursor. Range-check both
  arguments the way `--fps` already is, and allocate before entering raw mode.
  **Fixed:** `--stars` caps at 1,000,000 (40 MB of pool, fifty times the
  automatic maximum) and `--size` at 10,000 per axis and 2,000,000 cells
  (about 110 MB across the four buffers). A terminal that reports something
  preposterous is clamped rather than believed, since an ioctl answer arrives
  unvetted. The `Flight` is now built before the terminal is taken over.

- [x] **3. Accumulated time is `f32`, so a long-running screensaver freezes.**
  `Flight::time` accumulates `dt` in `f32`. Stepping at 1/60 s it stops advancing
  entirely at **t = 524288 s (145 hours)**, taking the twinkle and the shake with
  it. The phase degrades much earlier: `(time * 31.0).sin()` at one hour is
  evaluating sine at 111600, where the `f32` ulp is 0.008. The autopilot's
  `start.elapsed().as_secs_f32()` has the same problem. Use `f64`, or wrap `time`
  modulo a period — it only ever feeds `sin`/`cos`.
  **Fixed:** `Flight::time` and the autopilot's clock are `f64`. The four shake
  terms are evaluated in `f64`, where argument reduction stays exact; the
  per-star twinkle phase is folded into one turn once per frame so the hot path
  stays `f32`. No wrap discontinuity, and no measurable cost.

- [x] **4. `RawGuard::new` can leave raw mode enabled.** `enable_raw_mode()?`
  succeeds, then any `?` on the following lines returns without a guard to drop.
  **Fixed:** the guard is constructed immediately after raw mode is enabled.

- [x] **5. A doc comment describes behaviour that does not exist.**
  `DepthRule::Anywhere` says it is "used when refilling the sides mid-turn", but
  `update` always respawns with `FarPlane`; `Anywhere` is reached only from `new`
  and `resize_pool`.
  **Fixed:** the comment now says what the variant is for.

- [x] **5a. Four resize faults.** None had an entry of their own because they
  only surfaced once the resize path was walked deliberately. `Canvas::resize`
  grew the buffer with `Vec::resize`, which keeps the old contents under a row
  stride that has just changed. `--size` was discarded by the first resize event,
  so the flag meant "until the window moves". Every resize event forced a clear
  and a full repaint, including the ones that settle on the size already in use.
  Autowrap was left on, so a terminal shrinking between our idea of its width and
  the next flush sheared the frame diagonally instead of clipping.
  **Fixed:** all four, with a pty test driving the interactive loop through
  1x1, 400x120 and both sides of the panel's breakpoints.

## Performance

- [x] **6. The tonemap is the single most expensive pass, and it reallocates
  every frame.** `Canvas::resolve` called `exp()` and `powf()` per channel per
  subpixel — 98k of each per frame, measured at **2.14 ms/frame, 28% of the
  total**. It also `collect()`ed a fresh `Vec` each time, in spite of the field
  it landed in being documented as "reused across frames".
  **Fixed:** a 1024-entry table over a sqrt-compressed domain plus a reused
  output buffer. Worst error one 8-bit level, monotonicity preserved, and the
  whole frame goes from **7.75 ms to 4.37 ms (44% faster)**.

- [ ] **7. `apply_vignette` costs 0.76 ms/frame** computing a `sqrt` per subpixel
  for a value that depends only on distance from the centre. Precompute it and
  invalidate on resize.

- [ ] **8. `write_plain` is 10x more verbose than it needs to be.** It emits
  `SetForegroundColor` and `SetBackgroundColor` unconditionally for every cell:
  41 bytes/cell, **669 KB per frame**. `flush` already tracks the last colour
  written; applying the same tracking gives **67 KB/frame**, exactly 10x smaller.
  This is the path the README's determinism check pipes into `sha256sum`.

- [ ] **9. `pick_class` re-sums the seven class weights on every spawn**,
  thousands of times per second. Hoist the total into a `const`, or use a
  cumulative table.

Checked and **not** worth doing, so not listed as an item: hoisting
`bank.sin_cos()` out of `Camera::project`. It looks like three redundant calls
per star per step, but the optimiser already handles it and the measured
difference is nil.

## Portability

- [ ] **10. Windows falls back to ASCII.** `ColorMode::detect` returns `Ascii`
  when `TERM` is unset, which is the normal state under Windows Terminal — a
  truecolor terminal taking the degraded path. Check `WT_SESSION`, or ask
  crossterm.

- [ ] **11. `▀` is East Asian Ambiguous**, as are `█ │ ┌ ‖ — ° ↑ ←`. Under a CJK
  locale configured with ambiguous-width=double, the half block the whole
  renderer is built on draws double-wide and every row breaks. `hud::truncate`
  counts characters rather than display columns, so it cannot defend against this
  either. At minimum this deserves a note in the README.

- [ ] **12. No `is_terminal()` guard** — running without `--headless` into a pipe
  sprays escape codes. There is also no `SIGTERM`/`SIGHUP` handler, so a signal
  kill skips `Drop` and leaves the terminal dirty. That is the last remaining way
  to exit without restoring the terminal, now that (2) no longer aborts.

## Project infrastructure

- [x] **13. There was no CI.** For a crate whose entire value is cross-terminal,
  cross-platform behaviour this was the largest single gap — it is what would
  have caught (10).
  **Fixed:** `.github/workflows/ci.yml` builds and tests on Linux, macOS and
  Windows with and without the `snapshot` feature, runs clippy with warnings
  denied, and re-checks the reproducibility property the README documents.

- [ ] **14. `Cargo.toml` declares `license = "MIT"` but there is no LICENSE
  file.** Also absent: `rust-version` (MSRV), `keywords`, `categories`, `readme`.

- [ ] **15. The tree is not rustfmt-clean** — 47 diff hunks under defaults, 28
  under `use_small_heuristics = "Max"`. The dense style is deliberate and
  readable; no config reproduces it exactly, so enforcing `fmt --check` in CI
  means either committing to a one-off reformat or leaving the job out. It is
  currently left out.

- [ ] **16. Binary-only crate.** No lib target means no `tests/`, no doc tests
  and no `criterion` benches; everything has to live in `#[cfg(test)]` inside
  `src/`. A thin `main.rs` over a `lib.rs` unlocks all three — and items 6 to 9
  are exactly the kind of thing that wants a benchmark guarding it.

- [ ] **17. `ColorMode::detect` is untested** because it reads process-global
  environment variables. Taking a lookup closure would make it testable, which
  matters given (10).

## Rendering polish

- [ ] **18. Streak length is pinned to the 120 Hz sim step, not to the frame.**
  `star.prev` is overwritten on every sub-step, so the smear always represents
  1/120 s of travel whether the display is running at 30 fps or 120. At low frame
  rates the motion between frames is not covered and the field strobes. Snapshot
  `prev` at the start of `advance` instead.

- [ ] **19. Brightness jumps about 20% at the point-to-streak threshold.** Below
  0.75 px a star gets one splat at full intensity; just above it gets two samples
  totalling roughly 1.2x. Normalising by sample count would smooth the pop.

- [ ] **20. Streaks lose light at the canvas edge.** `clip` clamps to `w-1`/`h-1`,
  so a bilinear splat there spills half its weight into the discarded `w`-th tap.
  Clipping to `w`/`h` and letting `splat` drop the out-of-range tap keeps edge
  brightness uniform.

- [ ] **21. Mutually exclusive flags are not declared.** `--headless` silently
  ignores `--screensaver`, and `--frames` is meaningless without `--headless`.
  clap's `conflicts_with` and `requires` would say so instead of quietly doing
  something else.

## Features worth considering

- [ ] Adaptive quality: tune the star count to hold the target frame rate. The
  machinery already exists in `resize_pool`.
- [ ] Knobs for field of view, star density and palette.
- [ ] A `--record` mode emitting asciinema or GIF.
- [ ] Roll on `Q`/`E`.
- [ ] Mouse steering.
- [ ] Nebulae or a planet flyby, for visual variety.
- [ ] A `--no-hud` flag.
