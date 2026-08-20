//! The `warp` binary. Everything it does lives in the library beside it; this
//! is only the entry point — parse the command line, fly, and report anything
//! that went wrong once the terminal has been handed back.

use clap::Parser;
use warp_rs::app;
use warp_rs::cli::Args;

fn main() {
    let args = Args::parse();
    if let Err(err) = app::run(&args) {
        // A closed pipe is somebody downstream having seen enough, not a fault
        // to report. `--headless` exists to be read by something else, so
        // `warp --headless --frames 500 | head` is the obvious thing to try
        // with it, and it used to answer `warp: Broken pipe (os error 32)` and
        // exit 1 — the one rough edge on a command line that otherwise refuses
        // every absurd argument with a message naming the limit.
        //
        // Rust ignores `SIGPIPE`, so the write comes back as an ordinary error
        // and reaches here rather than ending the process where it stood. That
        // is the whole reason this arm has to exist at all, and why it is here
        // rather than at the write: every writer in the tree propagates with
        // `?`, and this is where they all arrive.
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            return;
        }
        // The guard has already put the terminal back by the time this prints.
        eprintln!("warp: {err}");
        std::process::exit(1);
    }
}
