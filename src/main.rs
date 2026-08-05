//! The `warp` binary. Everything it does lives in the library beside it; this
//! is only the entry point — parse the command line, fly, and report anything
//! that went wrong once the terminal has been handed back.

use clap::Parser;
use warp_rs::app;
use warp_rs::cli::Args;

fn main() {
    let args = Args::parse();
    if let Err(err) = app::run(&args) {
        // The guard has already put the terminal back by the time this prints.
        eprintln!("warp: {err}");
        std::process::exit(1);
    }
}
