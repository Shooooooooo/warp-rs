//! warp-rs — fly a starship through the universe at warp, in your terminal.
//!
//! The `warp` binary is a shell over this library: [`cli::Args`] is what the
//! command line parses into and [`app::run`] flies it. Underneath sit [`ship`]
//! for the flight model, [`universe`] for the sky, [`camera`] for the lens it
//! is seen through, [`track`] for where the ship has been, [`canvas`] and
//! [`render`] for turning that into light, [`term`] for getting it onto a
//! terminal, and [`hud`] for the glass in front of it.
//!
//! [`view`] says which of the two cameras is flying. The one outside brings
//! [`lens`] and [`bend`] with it, for the way a lit warp drive bends the sky,
//! and [`models`] and [`menu`] for the ships it can see.
//!
//! Everything is public so a flight can be driven from a test, a benchmark or
//! another program without going through the binary.

pub mod app;
pub mod autopilot;
pub mod bend;
pub mod camera;
pub mod canvas;
pub mod cli;
pub mod hud;
pub mod lens;
pub mod menu;
pub mod models;
pub mod render;
pub mod ship;
pub mod term;
pub mod track;
pub mod universe;
pub mod view;

#[cfg(feature = "snapshot")]
pub mod snapshot;
