//! Subcommand modules for the Richter CLI.
//!
//! Each module implements a top-level subcommand. The `run` function
//! in each module accepts the parsed arguments and executes the command
//! against the daemon or local state.

pub mod agents;
pub mod audit;
pub mod claim;
pub mod config;
pub mod doctor;
pub mod events;
pub mod explain;
pub mod install;
pub mod mobile;
pub mod output;
pub mod repos;
pub mod run;
pub mod runs;
pub mod setup;
pub mod simulate;
pub mod status;
pub mod worktree;
