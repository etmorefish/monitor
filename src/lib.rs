//! monitor core: Windows sampling, formatting, terminal UI and output modes.
//!
//! The library exists so the optional egui shell (`monitor-gui`) can reuse the
//! exact same sampler and data model as the TUI. Building with default features
//! keeps it dependency-free.

pub mod cli;
pub mod format;
pub mod metrics;
pub mod output;
pub mod sys;
pub mod term;
pub mod ui;

#[cfg(feature = "gui")]
pub mod gui;
