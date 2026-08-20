//! What happens to the terminal, asked of a real one.
//!
//! `run_interactive` and [`warp_rs::term::RawGuard`] were named in no test in
//! the tree, and between them they hold the panic hook, the signal flag, the
//! event drain, the resize path and the mouse capture. That is the code whose
//! failure mode is the worst this program has — a user dropped back into a
//! shell in raw mode, on the alternate screen, with no cursor and no prompt —
//! and none of it can be reached from a unit test, because all of it is about
//! owning a terminal and a unit test has not got one.
//!
//! So this one gets one. `script` puts the binary on a pseudo-terminal and
//! records everything it writes, which is enough to read back the two things
//! that matter: that a flight ends by putting the terminal back, and that it
//! does so on the ways out that run no code of ours.
//!
//! Linux only, and for a plainer reason than `tests/golden.rs`'s: `script`'s
//! own command line is spelled differently on the BSDs, and the Windows leg of
//! the matrix has no such program at all. What is under test is the same
//! everywhere; the harness is not.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// The tail of what [`warp_rs::term::restore`] writes, in the order it writes
/// it: show the cursor, put autowrap back, leave the alternate screen.
///
/// Checked as the *tail* rather than as a substring, because what is being
/// asked is whether the terminal was handed back at the end — a restore
/// sequence in the middle of the stream would be a flight that undressed and
/// carried on.
const RESTORE_TAIL: &[&str] = &["\x1b[?25h", "\x1b[?7h", "\x1b[?1049l"];

/// Run `warp` on a pseudo-terminal, returning everything it painted.
///
/// `script -qec` is the whole of the pty: it forks the command onto one, so the
/// program under test sees a terminal on stdout and takes raw mode against it,
/// exactly as it would from a shell.
fn on_a_pty(args: &[&str], hold_for: Duration) -> (String, Option<i32>) {
    let binary = env!("CARGO_BIN_EXE_warp");
    let log = std::env::temp_dir().join(format!(
        "warp-pty-{}-{}.log",
        std::process::id(),
        args.join("-").replace(['/', ' ', '-'], "_")
    ));
    let _ = std::fs::remove_file(&log);
    let command = format!("{binary} {}", args.join(" "));

    // Stdin piped and the handle held for the duration, which is not
    // housekeeping: `script` forwards its own stdin to the pty and gives up
    // when that reaches end of file. Under `cargo test` stdin is closed, so
    // without this the host quits the instant it starts and the flight ends
    // before it has flown — a test that passes on a program that never took
    // the terminal over at all.
    let mut child = Command::new("script")
        .args(["-qec", &command])
        .arg(&log)
        .stdin(std::process::Stdio::piped())
        // Silenced, because `script` echoes the whole session to its own
        // stdout as well as recording it: inherited, that is twenty-eight
        // megabytes of escape sequences into the test log for one flight.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("`script` is needed to give the flight a terminal to fly in");
    let keyboard = child.stdin.take();

    // Wait for it either to finish on its own or to have flown long enough to
    // be worth interrupting. Polled rather than slept through, so the ordinary
    // case costs what the flight costs.
    let deadline = Instant::now() + hold_for;
    let mut status = None;
    while Instant::now() < deadline {
        if let Some(done) = child.try_wait().expect("the pty host should be waitable") {
            status = Some(done);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let code = match status {
        Some(done) => done.code(),
        None => {
            // Still flying, which is what a caller asking for a long hold
            // wants: it is about to signal the flight itself.
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    };
    drop(keyboard);
    let painted = std::fs::read(&log).map(|b| String::from_utf8_lossy(&b).into_owned());
    let _ = std::fs::remove_file(&log);
    (painted.unwrap_or_default(), code)
}

/// Whether a stream ends by putting the terminal back.
fn hands_the_terminal_back(painted: &str) -> bool {
    // `script` appends its own "Script done" line, so the restore sequence is
    // the tail of what the *program* wrote rather than of the file.
    let Some(cut) = painted.rfind("\x1b[?1049l") else {
        return false;
    };
    let end = &painted[..cut + "\x1b[?1049l".len()];
    RESTORE_TAIL.iter().all(|part| end.contains(part))
}

#[test]
fn a_flight_hands_the_terminal_back_when_it_is_finished() {
    let (painted, code) = on_a_pty(
        &["--demo", "1", "--seed", "1", "--size", "60x20"],
        Duration::from_secs(30),
    );
    assert_eq!(code, Some(0), "a demo flight did not end cleanly");
    assert!(
        painted.len() > 10_000,
        "the flight painted {} bytes, which is not a flight",
        painted.len()
    );
    assert!(
        hands_the_terminal_back(&painted),
        "a demo flight ended without putting the terminal back"
    );
}

#[test]
fn a_signal_hands_the_terminal_back_too() {
    // The case `signal-hook` was taken on as a dependency for, and the only one
    // of the ways out that runs no code of ours: a signal's default disposition
    // ends the process where it stands, so `Drop` never fires and `RawGuard`
    // never restores. What answers it is a handler that sets an `AtomicBool`
    // and a loop that reads the flag at the top of each frame — two pieces that
    // nothing checked either half of.
    let binary = env!("CARGO_BIN_EXE_warp");
    let log = std::env::temp_dir().join(format!("warp-pty-signal-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let command = format!("{binary} --demo 30 --seed 1 --size 60x20 --fps 30");
    let mut host = Command::new("script")
        .args(["-qec", &command])
        .arg(&log)
        .stdin(std::process::Stdio::piped())
        // Silenced, because `script` echoes the whole session to its own
        // stdout as well as recording it: inherited, that is twenty-eight
        // megabytes of escape sequences into the test log for one flight.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("`script` is needed to give the flight a terminal to fly in");
    let keyboard = host.stdin.take();

    // Let it get properly underway before interrupting it: a flight signalled
    // before it has painted anything would pass this test without ever having
    // taken the terminal over.
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) < 20_000 {
        assert!(Instant::now() < deadline, "the flight never got underway");
        std::thread::sleep(Duration::from_millis(20));
    }

    let pid =
        warp_pid_under(host.id(), binary).expect("the flight should be running to be signalled");
    let killed = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("`kill` should be available");
    assert!(killed.success(), "could not signal the flight");

    let done = wait_for(&mut host, Duration::from_secs(30))
        .expect("a signalled flight should end rather than hang");
    drop(keyboard);
    let painted = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);

    assert_eq!(
        done.code(),
        Some(0),
        "a signalled flight left by a door other than the ordinary one"
    );
    assert!(
        hands_the_terminal_back(&painted),
        "SIGTERM ended the flight with the terminal still taken over"
    );
}

#[test]
fn a_flight_says_so_when_there_is_no_terminal_to_fly_in() {
    // The other half of the same subject, and the one a person meets first:
    // without a terminal this used to be `No such device or address (os error
    // 6)` — `RawGuard::new`'s ioctl, travelling out through `main` — on a
    // command line that answers every other absurd argument with a sentence
    // naming the limit. No pty here on purpose.
    let out = Command::new(env!("CARGO_BIN_EXE_warp"))
        .args(["--demo", "1", "--size", "60x20"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the binary should run");
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "it flew without a terminal");
    assert!(
        said.contains("--headless"),
        "the refusal does not name the flag that would have worked: {said}"
    );

    // And the two modes that legitimately have no terminal are untouched.
    let headless = Command::new(env!("CARGO_BIN_EXE_warp"))
        .args([
            "--headless",
            "--frames",
            "1",
            "--size",
            "60x20",
            "--color",
            "ascii",
        ])
        .output()
        .expect("the binary should run");
    assert!(
        headless.status.success() && !headless.stdout.is_empty(),
        "`--headless` was refused a terminal it never wanted"
    );
}

/// The process id of the `warp` this test started: matched on its executable
/// rather than its name, *and* on being a descendant of the pty host we
/// spawned.
///
/// Both halves are load-bearing and the second one took a failure to learn. By
/// the executable alone this picked whichever matching process `/proc` listed
/// first, which on a machine with another flight open — a stray from an earlier
/// run, a developer watching one in the next window — is not the one under
/// test. It signalled a bystander, the flight under test carried on, and the
/// test failed thirty seconds later saying the flight had hung.
fn warp_pid_under(host: u32, binary: &str) -> Option<u32> {
    let target = Path::new(binary).canonicalize().ok()?;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if std::fs::read_link(entry.path().join("exe")).ok() != Some(target.clone()) {
            continue;
        }
        if descends_from(pid, host) {
            return Some(pid);
        }
    }
    None
}

/// Whether `pid` is `ancestor` or a child of one, walking the parent chain.
///
/// Bounded rather than looped until it reaches `init`: a chain that somehow
/// cycles is a hung test rather than a failing one, and the depth between a
/// test and the flight it started is two.
fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut walk = pid;
    for _ in 0..16 {
        if walk == ancestor {
            return true;
        }
        let Some(parent) = parent_of(walk) else {
            return false;
        };
        if parent == 0 {
            return false;
        }
        walk = parent;
    }
    false
}

fn parent_of(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|value| value.trim().parse().ok())
}

/// Wait for a child, giving up after `limit` rather than hanging a CI job.
fn wait_for(child: &mut std::process::Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Ok(Some(done)) = child.try_wait() {
            return Some(done);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    None
}
