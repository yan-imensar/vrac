//! Keyboard-first terminal client for Vrac.
//!
//! This crate owns transient interaction and rendering state. Persistent data
//! and every business mutation remain delegated to `vrac-engine`; local
//! workspace lifecycle and synchronization are delegated to `vrac-workspace`.

mod commands;
mod config;
mod editing;
mod editor;
mod input;
mod model;
mod navigation;
mod performance;
mod prompts;
mod refresh;
mod session;
mod setup;
mod ui;

#[doc(hidden)]
pub use performance::run_reference_scenario;
pub use session::{LaunchOptions, WorkspaceSelection, run};

const OUTLINE_INDENT: usize = 4;

#[cfg(test)]
mod tests;
