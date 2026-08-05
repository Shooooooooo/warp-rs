//! warp-rs — fly a starship through the universe at warp, in your terminal.
//!
//! The `warp` binary is a shell over this library: [`cli::Args`] is what the
//! command line parses into and [`app::run`] flies it. Underneath sit the
//! pieces it is assembled from — [`ship`] for the flight model, [`starfield`]
//! for the universe, [`canvas`] and [`render`] for turning that into light,
//! [`term`] for getting the light onto a terminal, and [`hud`] for the glass
//! in front of it. They are public so a flight can be driven from a test, a
//! benchmark, or another program without going through the binary.
//!
//! [`view`] says which camera is flying. The second one watches from outside,
//! and brings its own half of the renderer with it: [`exterior`] for the band
//! of sky it flies alongside rather than into, [`lens`] for the way a lit warp
//! drive bends that sky, [`models`] for the ships it can now see, and [`menu`]
//! for choosing between them.

pub mod app;
pub mod autopilot;
pub mod canvas;
pub mod cli;
pub mod exterior;
pub mod hud;
pub mod lens;
pub mod menu;
pub mod models;
pub mod render;
pub mod ship;
pub mod starfield;
pub mod term;
pub mod view;

#[cfg(feature = "snapshot")]
pub mod snapshot;
