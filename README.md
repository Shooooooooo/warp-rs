# warp-rs

Fly a starship through the universe at warp, in your terminal.

![The view at warp factor 9.8](docs/warp.png)

Stars live in a real 3D volume ahead of the ship and stream past the canopy.
Each one is drawn as the segment between where it was last frame and where it
is now, accumulated into a floating-point buffer and tonemapped at the end — so
overlapping streaks bloom instead of clipping, and the classic hyperspace smear
falls out of the motion rather than being drawn as a special effect.

## Running it

```sh
cargo run --release
```

Or, without a keyboard, on autopilot:

```sh
cargo run --release -- --demo
```

## Flying

| Key | |
| --- | --- |
| `↑` `↓` / `W` `S` | Throttle |
| `←` `→` / `A` `D` | Yaw |
| `I` `K` | Pitch |
| `Space` | Engage or disengage the warp drive |
| `+` `-` | More or fewer stars |
| `P` | Pause |
| `R` | Reset |
| `Q` / `Esc` / `Ctrl-C` | Quit |

Throttle and steering are impulse-driven — a press nudges the ship and it eases
back on its own — because terminals report key presses but not releases. Hold a
key and auto-repeat does the rest.

## What you are looking at

**Impulse** — sublight. Stars are points, faintly twinkling, and the instrument
panel reads a fraction of *c*.

![Cruising at sublight](docs/impulse.png)

**Spooling up** — past the light barrier the streaks lengthen, the field starts
to bend outward, and a glare opens up down the throat of the tunnel.

![Accelerating through the light barrier](docs/spool.png)

Engaging the drive kicks the view and whites out the frame; dropping out bleeds
speed off much harder than a normal throttle-down. The panel reports velocity in
multiples of *c*, warp factor on the TNG scale (`v = w^(10/3)·c`), distance
travelled, and heading.

Stars are coloured by spectral class, weighted toward the hot end of the main
sequence — which is what actually fills a real sky, since apparent brightness
favours the bright ones. As speed climbs they Doppler-shift: blue toward the
vanishing point, red out at the edges where they are falling behind.

Flight time is compressed so the odometer moves: one second at the stick is one
day underway, which is about 5 light years per second at full warp.

## Options

| Flag | |
| --- | --- |
| `--demo [SECS]` | Fly on autopilot, then exit. Defaults to 45 seconds. |
| `--stars N` | Star count. `0` (default) suits it to the terminal. |
| `--fps N` | Frame rate cap. Default 60. |
| `--color auto\|truecolor\|256\|ascii` | Colour depth. Auto-detected by default. |
| `--engage` | Start with the drive already lit. |
| `--throttle 0..1` | Starting throttle. |
| `--exposure N` | Tonemap exposure. Higher is brighter. |
| `--seed N` | Fix the sky. Omit for a different one each run. |
| `--size COLSxROWS` | Override the terminal size. |
| `--headless --frames N` | Print frames to stdout instead of taking over the terminal. |

## Terminals

The renderer targets 24-bit colour and draws with the upper half block, `▀`:
foreground paints the top pixel, background the bottom, so each cell is two
roughly square pixels and the field stays circular instead of being squashed
by cells that are twice as tall as they are wide.

Where truecolor is not available it degrades — to the xterm 256-colour palette,
and past that to a plain ASCII brightness ramp. `--color` forces the choice if
detection guesses wrong.

Only cells that changed are re-emitted each frame, and colour codes are only
re-sent when they differ from the last cell written.

## Development

```sh
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

`--headless` renders with a fixed timestep and no terminal control, so with a
fixed `--seed` the same flight produces byte-identical output — useful for
checking that a change to the renderer changed only what you meant it to:

```sh
cargo run --release -- --headless --frames 120 --seed 1 --size 120x36 --demo | sha256sum
```

The `snapshot` feature writes a frame out as a PNG, which is a great deal easier
to look at than a wall of escape codes:

```sh
cargo run --release --features snapshot -- \
    --snapshot warp.png --engage --throttle 1.0 --warmup 600 --scale 2
```

The images in this README were produced that way. Note that the snapshot is the
starfield only — the instrument panel lives in the character grid, not in the
pixel buffer.
