//! warp-rs — fly a starship through the universe at warp, in your terminal.
//!
//! The `warp` binary is a shell over this library: [`cli::Args`] is what the
//! command line parses into and [`app::run`] flies it. Underneath sit the
//! pieces it is assembled from — [`ship`] for the flight model, [`starfield`]
//! for the universe, [`canvas`] and [`render`] for turning that into light,
//! [`term`] for getting the light onto a terminal, and [`hud`] for the glass
//! in front of it. They are public so a flight can be driven from a test, a
//! benchmark, or another program without going through the binary.

pub mod app;
pub mod autopilot;
pub mod canvas;
pub mod cli;
pub mod exterior;
pub mod hud;
pub mod lens;
pub mod render;
pub mod ship;
pub mod starfield;
pub mod term;
pub mod view;

#[cfg(feature = "snapshot")]
pub mod snapshot;
