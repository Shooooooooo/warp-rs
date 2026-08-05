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
| `W` `S` / `I` `K` | Pitch — nose up and nose down |
| `A` `D` / `←` `→` | Yaw — nose left and nose right |
| `Q` `E` | Roll — port and starboard |
| `↑` `↓` | Throttle |
| `Space` | Engage or disengage the warp drive |
| `+` `-` | More or fewer stars |
| `P` | Pause |
| `R` | Reset |
| `Esc` / `Ctrl-C` / `Ctrl-D` | Quit |

Throttle and steering are impulse-driven — a press nudges the ship and it eases
back on its own — because terminals report key presses but not releases. Hold a
key and auto-repeat does the rest.

The three axes are the ship's, not the screen's, so they compose the way an
aircraft's do: roll ninety degrees and pitch has come round to where yaw was, so
`Q` and then `W` is a turn. Pitch stops short of straight up, because there is
no way back over the top — roll has no such stop, so `Q` or `E` held down is a
barrel roll.

A roll you are not currently flying is invisible against a starfield, since
space has no horizon to be level with, so the panel carries a `ROLL` readout.
It is the only thing that will tell you the ship is inverted once the sky has
stopped turning.

`Q` steers rather than quits, so a hand on the stick cannot end the flight by
accident. `Esc` or `Ctrl-C` gets you out.

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
travelled, heading, and roll.

Stars are coloured by spectral class, weighted toward the hot end of the main
sequence — which is what actually fills a real sky, since apparent brightness
favours the bright ones. As speed climbs they Doppler-shift: blue toward the
vanishing point, red out at the edges where they are falling behind.

Flight time is compressed so the odometer moves: one second at the stick is one
day underway, which is about 5 light years per second at full warp.

## As a tmux screensaver

`--screensaver` flies on autopilot indefinitely and quits on **any** key, which
is exactly the contract tmux's `lock-command` wants: tmux runs the command when
a client goes idle and unlocks when it exits.

Put the binary somewhere on `PATH`:

```sh
cargo install --path .
```

Then in `~/.tmux.conf`:

```tmux
set -g lock-after-time 300              # idle seconds before it kicks in
set -g lock-command "warp --screensaver"
```

Reload with `tmux source-file ~/.tmux.conf`. Leave a session alone for five
minutes and the stars come out; press anything and you are back where you were.

tmux binds no key to `lock-session` by default, so add one if you want to
trigger it on demand:

```tmux
bind L lock-session
```

Note that `lock-server` is on by default, which means all sessions lock together
on the server's idle time. `set -g lock-server off` makes it per-session.

This is a screensaver, not a lock: any key dismisses it and no password is
asked for. If you want the screen actually *locked*, chain a real locker —
`set -g lock-command "warp --screensaver; vlock"`.

### Without the lock mechanism

As a popup over whatever you are working on (tmux 3.2+), where `-E` closes it
when the command exits:

```tmux
bind W display-popup -E -w 90% -h 90% "warp --screensaver"
```

Or just as a window: `tmux new-window -n warp 'warp --screensaver'`.

## Options

| Flag | |
| --- | --- |
| `--demo [SECS]` | Fly on autopilot, then exit. Defaults to 45 seconds. |
| `--screensaver` | Fly on autopilot forever; any key quits. For tmux's `lock-command`. |
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

`--color ascii` is for a terminal that cannot be sent colour, so it is not sent
any: no escape codes beyond the cursor moves the grid is painted with, and an
instrument panel that swaps its box rules, block bars and degree signs for
characters a 1970s terminal would recognise. Piped rather than displayed, that
mode is plain text and nothing else.

Only cells that changed are re-emitted each frame, and colour codes are only
re-sent when they differ from the last cell written.

## Development

```sh
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

CI runs both on Linux, macOS and Windows, and re-checks the reproducibility
property below.

To see where a frame goes — simulating the flight, drawing it, and getting it
out — against the 16.7 ms that 60 fps allows:

```sh
cargo run --release --example bench
cargo run --release --example bench 200 60 20000    # a specific size and count
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

`src/lib.rs` carries the whole thing — flight model, starfield, renderer,
terminal — and `src/main.rs` is only the entry point, so the pieces can be
driven from `tests/` or from another program.

## License

MIT. See [LICENSE](LICENSE).
