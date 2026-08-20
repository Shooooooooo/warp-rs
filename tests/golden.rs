//! The reference frames, checked from `cargo test`.
//!
//! `tests/golden/frames.sha256` pins the bytes `--headless` produces, which is
//! the only thing in the tree with an opinion about whether a change to the
//! renderer changed more than it meant to. It used to be read by nothing but a
//! CI job running `sha256sum` against a release binary, so the answer arrived
//! minutes after a push and a green `cargo test` was no evidence at all. This
//! reproduces the same bytes in process, against the same committed file.
//!
//! Linux only, for the reason written at the top of that file: `sin`, `cos`,
//! `exp` and `powf` come from the system maths library, and recycling the star
//! pool turns a one-ulp difference into a different sky.
#![cfg(target_os = "linux")]

use clap::Parser;
use warp_rs::app;
use warp_rs::cli::Args;

/// SHA-256, spelled out.
///
/// Written here rather than pulled in. The crate has four dependencies and a
/// standing rule against a fifth without saying why the tree cannot do the job
/// itself, and this is a page of shifts and adds with a published
/// specification — which `the_digest_agrees_with_the_published_answers` checks
/// against the two canonical vectors before anything else trusts a word of it.
mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    /// The digest of `data`, lowercase hex — the spelling `sha256sum` prints,
    /// so what comes out can be compared with the committed file directly.
    pub fn hex(data: &[u8]) -> String {
        let mut h = H0;
        let whole = data.len() / 64;
        for block in data[..whole * 64].chunks_exact(64) {
            compress(&mut h, block);
        }

        // The tail, plus the terminator and the bit count. Two blocks are
        // always enough: the length needs nine bytes of room, so it either
        // fits behind the remainder or spills into one more block.
        let rest = &data[whole * 64..];
        let mut tail = [0u8; 128];
        tail[..rest.len()].copy_from_slice(rest);
        tail[rest.len()] = 0x80;
        let used = if rest.len() + 9 <= 64 { 64 } else { 128 };
        let bits = data.len() as u64 * 8;
        tail[used - 8..used].copy_from_slice(&bits.to_be_bytes());
        for block in tail[..used].chunks_exact(64) {
            compress(&mut h, block);
        }

        h.iter().map(|word| format!("{word:08x}")).collect()
    }

    fn compress(h: &mut [u32; 8], block: &[u8]) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let (x, y) = (w[i - 15], w[i - 2]);
            let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut s] = *h;
        for (k, word) in K.iter().zip(&w) {
            let sig1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let t1 = s
                .wrapping_add(sig1)
                .wrapping_add(choose)
                .wrapping_add(*k)
                .wrapping_add(*word);
            let sig0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let major = (a & b) ^ (a & c) ^ (b & c);
            let t2 = sig0.wrapping_add(major);

            s = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, add) in h.iter_mut().zip([a, b, c, d, e, f, g, s]) {
            *slot = slot.wrapping_add(add);
        }
    }
}

/// Flags every reference flight shares.
const COMMON: [&str; 7] = [
    "--headless",
    "--frames",
    "120",
    "--seed",
    "1",
    "--size",
    "120x36",
];

/// The reference flights, by the file each one is recorded under.
///
/// The last three are here because the first two turned out to cover very
/// little.
/// `--demo` spends its opening six seconds easing the throttle up and 120
/// frames at 60 fps is two of them, so the pinned sky was sublight throughout,
/// peaking at a quarter of light speed with the drive cold — leaving the streak
/// ramp, the glare, the flash, the Doppler shift and the whole view from
/// outside unpinned. A change to any of those could repaint the sky unremarked.
///
/// `ansi256.txt` is the same flight as `truecolor.txt` in the other colour
/// mode, which is the sharpest form that comparison takes: the two differ in
/// how a cell's colour is spelled and in nothing else, so a change that moves
/// one and not the other has landed in the writer rather than in the sky. It
/// arrived because the mode had no reference at all, back when it was the one
/// most terminals were handed by detection; it matters more now that detection
/// is gone and nothing is handed it. Reachable only by name means reachable
/// only by someone who needed it, and it means this file is the only thing
/// watching `quantize_256` and the palette-index path under it — every other
/// flight here is truecolor or ascii and would not notice them repainted.
///
/// `astern.txt` arrived the way `ansi256.txt` did — with the fix for a fault
/// the others could not see. Every one of them puts the camera abeam or forward
/// of it, and the point the *ship's own track* vanishes at is on the screen only
/// from behind: half the range of a control that goes all the way round, and the
/// only half where a trail can be stretched past somewhere it will never reach.
/// Recorded at `--orbit -75,6,20`, which has all three angles off zero and puts
/// that point on the canvas clear of the ship — the same two conditions
/// `models::forward_quarter()` picks its angles for, and for the same reason.
///
/// `steer.txt` is the newest, and it closes the widest hole any of them has
/// left: not one of the seven before it ever put a hand on the stick. The four
/// `--engage` flights carry no autopilot at all, and the three `--demo` ones
/// stop four seconds short of the weave, since the opening six seconds of that
/// cycle are the throttle easing up and 120 frames at 60 fps is two of them. So
/// the whole cockpit steering path — the yaw, the pitch, the lean they come
/// with, and the branch that swings a star out of the frame and back in the
/// other side — was outside the reference, and a change to any of it could
/// repaint every turn ever flown with all seven hashes standing still. It is
/// `--demo` again at `--fps 10`, which in headless *is* the timestep, so the
/// same 120 frames are twelve seconds of flight rather than two and the last
/// six of them have the stick moving.
///
/// `drift.txt` is `steer.txt`'s hole seen from outside.
/// The autopilot flies the camera as well as the ship now — a screensaver on
/// the outside view used to be very nearly a still life — and not one of the
/// eight flights before it could have said so: the four `--engage` ones carry
/// no autopilot, and the four `--demo` ones watch from the cockpit, which reads
/// neither the orbit nor the zoom. So the camera could have been swung anywhere
/// at all with every hash standing. It shares every flag with `steer.txt` but
/// the view, which makes the pair the same twelve seconds of flight watched
/// from inside and from outside: a change that moves both has landed in the
/// autopilot, and one that moves only one has landed in a view.
///
/// It is parked at `--orbit -60,0,0` rather than left on the beam, for two
/// reasons that are both about what a moving camera can ask and a parked one
/// cannot. The autopilot's swing is *added* to where the flag put it, so a
/// bubble that opened where `--orbit` asked and then flew off on its own would
/// look identical to one that ignored the flag if the flag said zero. And
/// starting aft of the beam means the camera **crosses** it, at frame 72, with
/// the drive lit since frame 60 — so the ship's own vanishing point stops
/// existing partway through, and the band the drive swaps sides over is
/// traversed as a ramp rather than sat on one side of. `orbit.txt` sits at a
/// fixed 55 degrees and `astern.txt` at a fixed -75; neither ever crosses.
/// Elevation and roll are parked at exactly zero, so anything off level in
/// these frames is the autopilot's own doing.
///
/// That angle is *derived from* `autopilot::CAMERA_TURN` rather than picked,
/// and has to be re-derived whenever that moves: it is how far the camera walks
/// in the seconds before the crossing should land, and the crossing has to land
/// after the drive lights at frame 60 or the sentence above stops being true of
/// anything. It was -20 while a turn took 137 seconds. At 43 that same angle
/// would have crossed at frame 24, with the drive still cold.
///
/// `ahead.txt` is the newest, and the hole it closes is the subtlest so far,
/// because on paper it was already covered. The exposure's tail is the star
/// projected from where the ship *was*, `nose · reach` further along the track,
/// and whether that runs away from the eye or straight at it is the sign of
/// `Orbit::nose_in_camera`'s depth component — negative across the whole
/// forward half of the azimuth. `orbit.txt` sits on that half. What it does not
/// do is stay there long enough for the sign to matter: 120 frames at 60 fps is
/// two seconds, in which the drive covers a few light years, and the nearest a
/// star can be is four. So the near plane was never reached, the branch that
/// deals with reaching it was never taken, and a bug that turned every star
/// inside the exposure into a bright point instead of a streak — a fifth of the
/// pool, and every one of the near stars that streak furthest — moved not one
/// of the nine hashes.
///
/// It is `--engage --throttle 1.0` at `--fps 10`, so the same 120 frames are
/// twelve seconds and the exposure settles at its full sixteen light years,
/// and `--orbit 90,0,0` puts the nose straight down the barrel where the depth
/// component is exactly -1 and the cut bites hardest. That the angle is the
/// extreme rather than a corner is deliberate and is the one difference from
/// how `astern.txt` was chosen: what wants pinning here is arithmetic that
/// switches on with a *sign*, so the shot is taken where the sign is least
/// ambiguous.
///
/// The same list appears at the top of `tests/golden/frames.sha256` and in the
/// `headless` job of `.github/workflows/ci.yml`; adding a flight means adding
/// it to all three, and to `.gitignore`.
const CASES: [(&str, &[&str]); 10] = [
    ("truecolor.txt", &["--demo", "--color", "truecolor"]),
    ("ascii.txt", &["--demo", "--color", "ascii"]),
    ("ansi256.txt", &["--demo", "--color", "256"]),
    (
        "warp.txt",
        &["--engage", "--throttle", "1.0", "--color", "truecolor"],
    ),
    (
        "side.txt",
        &[
            "--engage",
            "--throttle",
            "1.0",
            "--view",
            "side",
            "--color",
            "truecolor",
        ],
    ),
    (
        "orbit.txt",
        &[
            "--engage",
            "--throttle",
            "1.0",
            "--view",
            "side",
            "--orbit",
            "55,35,20",
            "--color",
            "truecolor",
        ],
    ),
    (
        "astern.txt",
        &[
            "--engage",
            "--throttle",
            "1.0",
            "--view",
            "side",
            "--orbit",
            "-75,6,20",
            "--color",
            "truecolor",
        ],
    ),
    (
        "steer.txt",
        &["--demo", "--fps", "10", "--color", "truecolor"],
    ),
    (
        "drift.txt",
        &[
            "--demo",
            "--fps",
            "10",
            "--view",
            "side",
            "--orbit",
            "-60,0,0",
            "--color",
            "truecolor",
        ],
    ),
    (
        "ahead.txt",
        &[
            "--engage",
            "--throttle",
            "1.0",
            "--fps",
            "10",
            "--view",
            "side",
            "--orbit",
            "90,0,0",
            "--color",
            "truecolor",
        ],
    ),
];

fn reference_args(case: &[&str]) -> Args {
    let argv = ["warp"]
        .into_iter()
        .chain(COMMON)
        .chain(case.iter().copied());
    Args::try_parse_from(argv)
        .unwrap_or_else(|e| panic!("the reference flags should parse, for {case:?}: {e}"))
}

/// The committed hashes, as `(file name, digest)`. Comments and blank lines are
/// skipped; the rest is the two-space format `sha256sum` writes and reads.
fn committed() -> Vec<(String, String)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/frames.sha256");
    let text = std::fs::read_to_string(path).expect("the reference file should be committed");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once("  "))
        .map(|(digest, name)| (name.trim().to_owned(), digest.trim().to_owned()))
        .collect()
}

#[test]
fn the_case_list_says_the_same_thing_in_all_four_places() {
    // `CASES` above, the comment block at the top of `frames.sha256`, the
    // `headless` job's shell in `.github/workflows/ci.yml`, and `.gitignore`.
    // Two of the four were already held together — the hashes below check the
    // names — and the other two were held by a note asking the next editor to
    // remember. `.gitignore`'s own comment records that the note has been
    // forgotten twice, each time leaving the documented regeneration recipe
    // with an untracked file waiting to be swept into somebody's `git add .`.
    //
    // Asked of the *file names*, which is the part that has to agree; what each
    // flight is for is prose and belongs to whoever writes it. Guarded on the
    // files existing rather than assumed, because `exclude` in `Cargo.toml`
    // keeps `.github/` out of the published crate and this test travels with
    // the package.
    // Matched against what each file *does* with a flight rather than against
    // anywhere its name appears, which is the difference between a check and a
    // formality: both files carry a prose block naming all ten, so a plain
    // substring search finds a flight in the paragraph about it long after the
    // line that acts on it has gone. Deleting `/drift.txt` from `.gitignore`
    // passed that way.
    let root = env!("CARGO_MANIFEST_DIR");
    for (place, path, acts_on) in [
        (
            "the CI job",
            "/.github/workflows/ci.yml",
            // The `headless` job writes each flight with a redirect.
            (|line: &str, name: &str| line.contains(&format!("> {name}")))
                as fn(&str, &str) -> bool,
        ),
        (
            "the ignore list",
            "/.gitignore",
            // An ignore entry is the whole line, anchored to the root.
            |line: &str, name: &str| line.trim() == format!("/{name}"),
        ),
    ] {
        let Ok(text) = std::fs::read_to_string(format!("{root}{path}")) else {
            continue;
        };
        for (name, _) in CASES {
            assert!(
                text.lines().any(|line| acts_on(line, name)),
                "{place} does not act on {name}, which is one of the reference flights"
            );
        }
        // And nothing is named there that the list has since dropped, which is
        // the other direction and the one a deletion goes wrong in.
        for line in text.lines() {
            for word in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_')) {
                if word.ends_with(".txt") {
                    assert!(
                        CASES.iter().any(|(name, _)| *name == word),
                        "{place} names {word}, which is not a reference flight"
                    );
                }
            }
        }
    }
}

#[test]
fn the_digest_agrees_with_the_published_answers() {
    // The hasher above is only worth as much as this: a wrong constant in it
    // would fail every comparison below and read exactly like a renderer that
    // had drifted, which is the most misleading way this file could break.
    // Both canonical vectors, and a message long enough to need a second block.
    assert_eq!(
        sha256::hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256::hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256::hex(&[b'a'; 1000]),
        "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
    // And the block boundaries either side of where the padding has to spill,
    // against the published digest of that many zero bytes. This used to
    // assert that the digest was sixty-four characters long, which `hex` gets
    // right by construction — it formats eight `u32`s — so the one case the
    // loop exists for, the length word landing in a second block, was checked
    // by an assertion that could not fail.
    for (len, want) in [
        (
            55,
            "02779466cdec163811d078815c633f21901413081449002f24aa3e80f0b88ef7",
        ),
        (
            56,
            "d4817aa5497628e7c77e6b606107042bbba3130888c5f47a375e6179be789fbb",
        ),
        (
            63,
            "c7723fa1e0127975e49e62e753db53924c1bd84b8ac1ac08df78d09270f3d971",
        ),
        (
            64,
            "f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b",
        ),
        (
            65,
            "98ce42deef51d40269d542f5314bef2c7468d401ad5d85168bfab4c0108f75f7",
        ),
        (
            119,
            "f616b0d54e78571a9611f343c9f8e022e859e920381ab0e4d3da01e193a7bd7e",
        ),
        (
            120,
            "6edd9f6f9cc92cded36e6c4a580933f9c9f1b90562b46903b806f21902a1a54f",
        ),
    ] {
        assert_eq!(
            sha256::hex(&vec![0u8; len]),
            want,
            "{len} zero bytes hashed to the wrong thing"
        );
    }
}

#[test]
fn the_frames_are_the_ones_committed_as_the_reference() {
    // Any change to renderer arithmetic moves these, and that is the point of
    // them: an edit meant to touch one thing that touched the whole sky has to
    // say so. When the change was intended, regenerate with the recipe at the
    // top of `tests/golden/frames.sha256` and say in the commit what moved.
    let committed = committed();
    assert_eq!(
        committed.len(),
        CASES.len(),
        "the committed hashes and the case list disagree: {committed:?}"
    );

    for (name, case) in CASES {
        let want = committed
            .iter()
            .find(|(file, _)| file == name)
            .map(|(_, digest)| digest.as_str())
            .unwrap_or_else(|| panic!("nothing committed for {name}"));

        let mut frames = Vec::new();
        app::render_headless(&reference_args(case), &mut frames)
            .expect("writing to a Vec cannot fail");
        assert!(!frames.is_empty(), "{name} rendered nothing");
        assert_eq!(
            sha256::hex(&frames),
            want,
            "the frames for {name} ({}) are not the ones committed in \
             tests/golden/frames.sha256",
            case.join(" ")
        );
    }
}

#[test]
fn the_reference_flights_between_them_reach_the_whole_renderer() {
    // The hashes are only worth what they cover, and what they covered was
    // discovered the hard way: a deliberate change to the ramp along a warp
    // streak did not move them at all, because two seconds
    // of `--demo` never leaves sublight and a sublight streak is shorter than a
    // subpixel, so it takes the branch that never reads the constant.
    //
    // Stated as the property the case list has to keep: some flight in it
    // lights the drive, some flight in it gets outside the ship.
    let engaged = CASES.iter().any(|(_, case)| case.contains(&"--engage"));
    let outside = CASES.iter().any(|(_, case)| case.contains(&"side"));
    assert!(engaged, "no reference flight ever lights the drive");
    assert!(outside, "no reference flight ever leaves the cockpit");

    // And some flight gets *behind* the ship, which is a third of the same
    // kind and was found the same way. The camera goes all the way round, and
    // the point the ship's own track vanishes at is on the screen from that
    // half of the range and no other — so with every flight abeam or forward of
    // it, a trail could be stretched clean through a point it never reaches and
    // nothing here would say so. Asked of the parsed orbit rather than of the
    // angle written above it, so it is the renderer's own arithmetic answering.
    let astern = CASES
        .iter()
        .any(|(_, case)| reference_args(case).orbit.nose_in_camera()[2] > 0.0);
    assert!(astern, "no reference flight ever gets behind the ship");

    // And some flight has a hand on the stick, which is the third of the same
    // kind and the one that went unnoticed longest. The four `--engage` flights
    // fly with nobody at the controls, and `--demo` spends its opening six
    // seconds on the throttle alone — so for seven flights the cockpit's whole
    // steering path went unpinned. Asked by running each case's own autopilot
    // over its own frame count and timestep, rather than by reading the flags,
    // because what makes a flight steer is whether it lasts long enough to
    // reach the weave. The ship is stepped once a frame where the renderer
    // steps it at `SIM_STEP`, which is not the same flight and does not need to
    // be: the question is zero against not zero.
    //
    // Asked of the yaw rate alone. It was `yaw_rate != 0.0 && bank != 0.0` for
    // one commit, which read as tighter and was not: the lean is a fact about
    // the hull now and reaches nothing the cockpit draws, so pinning the
    // reference's steering coverage to it would have been pinning it to
    // something no cockpit frame can see.
    let steered = CASES.iter().any(|(_, case)| {
        let args = reference_args(case);
        if args.demo.is_none() {
            return false;
        }
        let mut ship = warp_rs::ship::Ship::new();
        let mut autopilot = warp_rs::autopilot::Autopilot::default();
        let dt = 1.0 / args.fps as f64;
        (0..args.frames).any(|frame| {
            autopilot.update(&mut ship, frame as f64 * dt, dt as f32);
            ship.update(dt as f32);
            ship.yaw_rate != 0.0
        })
    });
    assert!(steered, "no reference flight ever touches the stick");

    // And some flight has the autopilot on the *camera*, which is the fifth of
    // the same kind and arrived with the ability. The four `--engage` flights
    // carry no autopilot, and the four `--demo` flights that watch from the
    // cockpit read neither the orbit nor the zoom — so an autopilot that swung
    // the camera anywhere at all could have left every one of those eight
    // hashes standing. Asked the same way as the stick above, by running the
    // real path over the case's own frame count and timestep rather than by
    // reading the flags, and of the parsed view rather than the string.
    let swung = CASES.iter().any(|(_, case)| {
        let args = reference_args(case);
        if args.demo.is_none() || args.view.resolve() != warp_rs::view::ViewMode::Side {
            return false;
        }
        let autopilot = warp_rs::autopilot::Autopilot::default();
        let dt = 1.0 / args.fps as f64;
        (0..args.frames)
            .any(|frame| autopilot.camera(frame as f64 * dt).0 != warp_rs::view::Orbit::LEVEL)
    });
    assert!(
        swung,
        "no reference flight ever swings the camera on its own"
    );

    // And some flight gets *ahead* of the ship with the exposure long enough to
    // matter, which is the sixth of the same kind and the sharpest lesson in
    // the set. The camera being forward of the ship is not the question — the
    // `astern` clause above has an exact counterpart in `orbit.txt`, whose nose
    // points at the lens. The question is whether any flight holds that angle
    // long enough for the exposure to reach *past* the nearest a star can be,
    // because that is what puts a trail's far end behind the near plane, and
    // for nine flights nothing did: the four `--engage` runs last two seconds,
    // where the drive has flown only a few light years, and the `--demo` pair
    // that lasts twelve never opens the throttle far enough. So a bug that lost
    // the trail of every star inside the exposure — a fifth of the pool, and
    // every one of the near ones that streak furthest — moved not one hash.
    //
    // Asked by flying each case's own sky at its own timestep, because the
    // exposure is *state* and there is nothing to read it off but the sky that
    // unrolled it.
    let ahead = CASES.iter().any(|(_, case)| {
        let args = reference_args(case);
        let nose_z = args.orbit.nose_in_camera()[2];
        if nose_z >= 0.0 {
            return false;
        }
        let mut ship = warp_rs::ship::Ship::new();
        let mut sky = warp_rs::universe::Universe::new(args.magnitude, args.seed.unwrap_or(0));
        if args.engage {
            ship.toggle_warp();
        }
        ship.throttle = args.throttle;
        let dt = 1.0 / args.fps as f32;
        for _ in 0..args.frames {
            ship.update(dt);
            sky.advance(
                ship.position,
                ship.axes,
                dt,
                ship.warp_intensity(),
                ship.velocity_ly_per_s(),
            );
        }
        -nose_z * sky.exposure() > warp_rs::universe::NEAREST_STAR
    });
    assert!(
        ahead,
        "no reference flight ever gets ahead of the ship with the exposure \
         reaching past the nearest star, so nothing pins the near-plane cut"
    );

    // And every colour mode is spelled by some flight, which is the one region
    // the prose at the top of `frames.sha256` names that nothing here was
    // asking about. It matters most for the mode nobody is handed: detection is
    // gone and `--color` defaults to truecolor, so `ansi256.txt` is the only
    // thing in the tree that draws `quantize_256` or the palette-index path
    // under it, and if it were ever dropped every other flight would go on
    // passing while that writer went unwatched. Asked of the parsed mode, so a
    // renamed value cannot satisfy it by spelling.
    for mode in [
        warp_rs::term::ColorMode::Truecolor,
        warp_rs::term::ColorMode::Ansi256,
        warp_rs::term::ColorMode::Ascii,
    ] {
        assert!(
            CASES
                .iter()
                .any(|(_, case)| reference_args(case).color.resolve() == mode),
            "no reference flight is recorded in {mode:?}, so nothing watches how it spells a cell"
        );
    }

    // Some flight has to *bend* an exposure, which is a different question from
    // whether some flight steers. An exposure is drawn along the track the ship
    // actually flew, so it is a curve only where it reaches back past a turn —
    // and a flight that steers hard at impulse, or lights the drive after it
    // has stopped steering, does not draw one. The two `--demo --fps 10` cases
    // qualify because their drive lights at frame 60 of a twelve-second flight
    // and the weave never stops.
    //
    // Asked by flying each case's own ship and autopilot rather than by reading
    // the flags, for the same reason the clause above is: it is a fact about
    // what a flight *did*, and `--demo` on the command line is not that fact.
    //
    // What it deliberately does not claim: no reference flight sweeps far
    // enough for the drawn curve to be more than a chord between the right two
    // points. The autopilot's weave turns about a twentieth of what a hand on
    // the stick does, which asks for one leg. The *shape* of a curve is pinned
    // by property tests in `universe.rs` and by nothing here, which is the
    // right level for it — it is a variant, not a region.
    let bent = CASES.iter().any(|(_, case)| {
        let args = reference_args(case);
        let Some(demo) = args.demo else {
            return false;
        };
        let mut ship = warp_rs::ship::Ship::new();
        let mut sky = warp_rs::universe::Universe::new(args.magnitude, args.seed.unwrap_or(0));
        let mut autopilot = warp_rs::autopilot::Autopilot::default();
        if args.engage {
            ship.toggle_warp();
        }
        ship.throttle = args.throttle;
        let dt = 1.0 / args.fps as f32;
        let _ = demo;
        let mut ever = false;
        for frame in 0..args.frames {
            autopilot.update(&mut ship, frame as f64 * dt as f64, dt);
            ship.update(dt);
            sky.advance(
                ship.position,
                ship.axes,
                dt,
                ship.warp_intensity(),
                ship.velocity_ly_per_s(),
            );
            ever |= sky.exposure() > sky.straight();
        }
        ever
    });
    assert!(
        bent,
        "no reference flight ever draws an exposure that reaches back past a \
         turn, so nothing pins the track the sky is drawn along"
    );

    // And the warp cases really are at warp rather than nominally engaged: a
    // flight that lit the drive and never spooled up would pin the same
    // sublight arithmetic under a different name.
    let mut ship = warp_rs::ship::Ship::new();
    ship.throttle = 1.0;
    ship.toggle_warp();
    for _ in 0..120 {
        ship.update(1.0 / 60.0);
    }
    assert!(
        ship.warp_intensity() > 0.9,
        "the reference warp flight only reaches {}",
        ship.warp_intensity()
    );
}
