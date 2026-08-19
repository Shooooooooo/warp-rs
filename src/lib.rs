//! warp-rs — fly a starship through the universe at warp, in your terminal.
//!
//! The `warp` binary is a shell over this library: [`cli::Args`] is what the
//! command line parses into and [`app::run`] flies it. Underneath sit the
//! pieces it is assembled from — [`ship`] for the flight model, [`universe`]
//! for the sky and [`camera`] for the lens it is seen through, [`canvas`] and
//! [`render`] for turning that into light, [`term`] for getting the light onto
//! a terminal, and [`hud`] for the glass in front of it. They are public so a
//! flight can be driven from a test, a benchmark, or another program without
//! going through the binary.
//!
//! [`track`] is where the ship has *been*, which the sky needs because a
//! streak is an exposure: the track a star swept while the shutter was open,
//! drawn from the poses the ship actually held rather than rewound along the
//! one it is holding now.
//!
//! [`view`] says which camera is flying, and *only* that: there is one sky, and
//! both cameras are places to stand in it rather than skies of their own. The
//! second one watches from outside and brings the rest of the renderer with it
//! — [`lens`] for the way a lit warp drive bends the sky, [`bend`] for drawing
//! a streak it has bent, [`models`] for the ships it can now see, and [`menu`]
//! for choosing between them.

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
