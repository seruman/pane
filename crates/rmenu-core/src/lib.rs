//! rmenu's core: everything but drawing. A port of mmenu's `MenuCore` plus its `MenuModel`,
//! reshaped for a Rune host.
//!
//! - [`item`]: rows, the action context passed to commands and printed by `--json`.
//! - [`record`]: piped input and command output into rows (lines, NUL, JSONL).
//! - [`config`]: the `--config` JSON: sources, bindings, push levels, preview.
//! - [`command`]: argv commands in owned process groups, with limits and cancellation.
//! - [`selection`]: the highlight and marks, by identity.
//! - [`search`]: fzf-ranked matching over a cheap-to-clone index.
//! - [`model`]: the menu as a reducer. User input and finished background work are
//!   [`model::Event`]s; what the host must start, cancel, copy or print are [`model::Effect`]s.
//!   The model never spawns a thread or touches a descriptor, so its tests run without either.

pub mod command;
pub mod config;
pub mod item;
mod items;
pub mod model;
mod preview;
pub mod record;
pub mod search;
mod selection;
