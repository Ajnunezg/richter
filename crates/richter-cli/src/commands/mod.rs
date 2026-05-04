//! Subcommand modules for the Richter CLI.
//!
//! Each module implements a top-level subcommand. The `run` function
//! in each module accepts the parsed arguments and executes the command
//! against the daemon or local state.

pub mod agents;
pub mod claim;
pub mod doctor;
pub mod events;
pub mod install;
pub mod mobile;
pub mod repos;
pub mod run;
pub mod runs;
pub mod simulate;
pub mod status;
pub mod output;
pub mod worktree;
pub mod explain;
pub mod audit;
pub mod config;
pub mod setup;
