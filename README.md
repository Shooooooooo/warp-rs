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

| `--magnitude M` | How faint a star the sky holds. Default 6.0; each magnitude is about four times as many. Low enough is an empty sky. |
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

## The sky

Stars are where their own photometry puts them. A star is drawn from the
spectral census, given an absolute magnitude, and placed at the distance that
makes it look as bright as it does — so the near ones are faint red dwarfs a
few light years off and the bright ones are giants a thousand light years away,
which is the arrangement the real sky is in.

Two things follow, and they are the reason for it. At impulse the sky does not
move: 0.9 c against a nearest star four light years off is a hundredth of a
subpixel a second, so the stars sit still and only the twinkle moves. And at
warp — 2000 c is five and a half light years a second — the near sky tears past
a far one that barely shifts, which is depth rather than speed and is the thing
a fixed backdrop cannot show.

`--magnitude` is the knob, and it is the one astronomers use: a sky is
described by how faint a star you can pick out of it, not by how many there
are. `+` and `-` move it half a magnitude at a time. How many land on screen is
then the field of view's business, so a wider terminal shows more sky rather
than the same sky stretched.

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
