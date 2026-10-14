//! ACECM native core: everything the shell does, as a library so the
//! app binary and the dev tools in src/bin share one implementation.

pub mod api;
pub mod auth;
pub mod config;
pub mod detect;
pub mod kspkg;
pub mod logs;
pub mod paths;
pub mod procs;
pub mod proxy;
pub mod sidecar;
pub mod web;
