//! This crate parses menu input, matches rows, runs commands, and updates menu state.
//!
//! The host sends user input and completed background work to the model as [`model::Event`]s.
//! The model returns [`model::Effect`]s for the host to execute.
//! The model does not spawn threads or access file descriptors.

pub mod command;
pub mod config;
pub mod item;
mod items;
pub mod model;
mod preview;
pub mod record;
pub mod search;
mod selection;
