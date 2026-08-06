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
/// The last two are here because the first two turned out to cover very little.
/// `--demo` spends its opening six seconds easing the throttle up and 120 frames
/// at 60 fps is two of them, so the pinned sky was sublight throughout, peaking
/// at a quarter of light speed with the drive cold — leaving the streak ramp,
/// the glare, the flash, the Doppler shift and the whole view from outside
/// unpinned. A change to any of those could repaint the sky unremarked.
///
/// The same list appears at the top of `tests/golden/frames.sha256` and in the
/// `headless` job of `.github/workflows/ci.yml`; adding a flight means adding it
/// to all three.
const CASES: [(&str, &[&str]); 4] = [
    ("truecolor.txt", &["--demo", "--color", "truecolor"]),
    ("ascii.txt", &["--demo", "--color", "ascii"]),
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
    // And the block boundaries either side of where the padding has to spill.
    for len in [55usize, 56, 63, 64, 65, 119, 120] {
        assert_eq!(
            sha256::hex(&vec![0u8; len]).len(),
            64,
            "a {len}-byte message did not produce a digest"
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
    // discovered the hard way: a deliberate change to `TAIL_BRIGHTNESS` — the
    // ramp along a warp streak — did not move them at all, because two seconds
    // of `--demo` never leaves sublight and a sublight streak is shorter than a
    // subpixel, so it takes the branch that never reads the constant.
    //
    // Stated as the property the case list has to keep: some flight in it lights
    // the drive, some flight in it gets outside the ship.
    let engaged = CASES.iter().any(|(_, case)| case.contains(&"--engage"));
    let outside = CASES.iter().any(|(_, case)| case.contains(&"side"));
    assert!(engaged, "no reference flight ever lights the drive");
    assert!(outside, "no reference flight ever leaves the cockpit");

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
