# warp-rs

Fly a starship through the universe at warp, in your terminal.

<!--
The images are linked absolutely rather than as `docs/warp.png`, and want to
stay that way. `Cargo.toml` excludes `docs/` from the published crate, and
docs.rs renders this file out of that package — so a relative path resolves to
nothing there and every screenshot comes out a broken icon, the one at the top
of the page included. The repository is public, so these load for everyone.
-->

![The view at warp factor 9.8](https://raw.githubusercontent.com/Shooooooooo/warp-rs/main/docs/warp.png)

![Chasing the ship at warp, from astern and above](https://raw.githubusercontent.com/Shooooooooo/warp-rs/main/docs/astern.png)

## Running it

```sh
cargo run --release            # fly it
cargo run --release -- --demo  # autopilot, 45 seconds, no keyboard needed
```

## Options

| Flag | |
| --- | --- |
| `--demo [SECS]` | Fly on autopilot, then exit. Defaults to 45 seconds. |
| `--screensaver` | Fly on autopilot forever; any key quits. For tmux's `lock-command`. |

The autopilot flies the camera as well as the ship, so `--screensaver --view
side` walks the shot round the hull rather than parking it, and no two runs up
to warp are flown quite alike. `--orbit` still says where the shot starts; the
camera wanders out from there.

| `--stars N` | Star count. Default 256; `0` for an empty sky. |
| `--fps N` | Frame rate cap, while nothing is being typed. Default 60. |
| `--color truecolor\|256\|ascii` | Colour depth. 24-bit by default. |
| `--engage` | Start with the drive already lit. |
| `--view cockpit\|side` | Which camera to start behind. `C` cycles them. |
| `--orbit AZ,EL[,ROLL]` | Where to park the outside camera, in degrees. |
| `--ship NAME` | Which ship to fly, and only visible from outside: `enterprise` (the default) or `normandy`. |
| `--throttle 0..1` | Starting throttle. |
| `--exposure N` | Tonemap exposure. Higher is brighter. |
| `--aa N` | Samples per subpixel, per axis, for the hull's outline. Default 3; `1` is a hard edge. |
| `--seed N` | Fix the sky. Omit for a different one each run. |
| `--size COLSxROWS` | Override the terminal size. |
| `--headless --frames N` | Print frames to stdout instead of taking over the terminal. |

## Terminals

The renderer targets 24-bit colour and draws with the upper half block, `▀`, so
each cell is two roughly square pixels. It opens in 24-bit everywhere and does
not inspect `COLORTERM` or `TERM` to decide: the narrower modes are asked for
rather than fallen into. `--color 256` snaps the frame to the xterm palette for
a terminal that cannot read a 24-bit sequence, and `--color ascii` drops colour
altogether for a brightness ramp. `--color ascii` sends no escape codes beyond
the cursor moves the grid is painted with, so piped rather than displayed it is
plain text and nothing else.

## License

MIT. See [LICENSE](LICENSE).
