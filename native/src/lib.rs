//! ACECM native core: everything the shell does, as a library so the
//! app binary and the dev tools in src/bin share one implementation.

pub mod api;
pub mod assists;
pub mod auth;
pub mod carsmap;
pub mod config;
pub mod content;
pub mod detect;
pub mod gamesettings;
pub mod kspkg;
pub mod livery;
pub mod logs;
pub mod misc;
pub mod mods;
pub mod paths;
pub mod pb;
pub mod procs;
pub mod protos;
pub mod proxy;
pub mod sidecar;
pub mod thumbs;
pub mod trackimport;
pub mod viewer;
pub mod web;
