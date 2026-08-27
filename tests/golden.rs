//! The reference frames, checked from `cargo test`.
#![cfg(target_os = "linux")]

use clap::Parser;
use warp_rs::app;
use warp_rs::cli::Args;

/// SHA-256, spelled out.
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

        // The tail, plus the terminator and the bit count.
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
    // against the published digest of that many zero bytes.
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
    // say so.
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
    // streak did not move them at all, because two seconds of `--demo` never
    // leaves sublight and a sublight streak is shorter than a subpixel, so it
    // takes the branch that never reads the constant.
    let engaged = CASES.iter().any(|(_, case)| case.contains(&"--engage"));
    let outside = CASES.iter().any(|(_, case)| case.contains(&"side"));
    assert!(engaged, "no reference flight ever lights the drive");
    assert!(outside, "no reference flight ever leaves the cockpit");

    // And some flight gets *behind* the ship, which is a third of the same kind
    // and was found the same way.
    let astern = CASES
        .iter()
        .any(|(_, case)| reference_args(case).orbit.nose_in_camera()[2] > 0.0);
    assert!(astern, "no reference flight ever gets behind the ship");

    // And *no* flight has a hand on the stick, which is this clause inverted —
    // it asked for the opposite until the autopilot stopped steering, and the
    // inversion is worth more than the assertion it replaces.
    for (name, case) in CASES {
        let args = reference_args(case);
        let mut ship = warp_rs::ship::Ship::new();
        let level = warp_rs::ship::Ship::new().axes;
        let mut autopilot = warp_rs::autopilot::Autopilot::default();
        let dt = 1.0 / args.fps as f64;
        for frame in 0..args.frames {
            if args.demo.is_some() {
                autopilot.update(&mut ship, frame as f64 * dt);
            }
            ship.update(dt as f32);
            assert_eq!(
                (ship.yaw_rate, ship.pitch_rate, ship.roll_rate),
                (0.0, 0.0, 0.0),
                "{name} steers at frame {frame}, so the ten hashes are no \
                 longer a control for the steering path"
            );
        }
        assert_eq!(
            ship.axes, level,
            "{name} does not finish pointed where it started"
        );
    }

    // And some flight has the autopilot on the *camera*, which is the fifth of
    // the same kind and arrived with the ability.
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
    // the set.
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
    // asking about.
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

    // And *no* flight bends an exposure, which is the other half of the clause
    // above and is the real cost of this reference having no stick in it.
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
            autopilot.update(&mut ship, frame as f64 * dt as f64);
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
        !bent,
        "a reference flight draws a curved exposure again, so the note above \
         about the track being pinned nowhere here is out of date"
    );

    // And every flight here spends most of itself with the shutter fully open,
    // which is a premise rather than a property of the renderer and is exactly
    // why it is worth asserting.
    for (name, case) in CASES {
        let args = reference_args(case);
        let seconds = args.frames as f32 / args.fps as f32;
        assert!(
            seconds > args.fade * 2.0,
            "{name} is {seconds} seconds long against a fade of {}, so less \
             than half of what it pins is a settled frame",
            args.fade
        );
    }

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
