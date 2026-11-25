// OWNER: scaffold
//! Warden core library. Module ownership is defined in SPEC.md ("Repository layout").
//! Only scaffold/integration edit this file.

pub mod error;
pub mod events;
pub mod types;

pub mod checks; // analysis
pub mod config; // analysis
pub mod diff; // analysis
pub mod guardrails; // analysis

pub mod db; // poster
pub mod poster; // poster

pub mod forge; // forge
pub mod poller;
pub mod secrets; // forge // forge

pub mod ci;
pub mod worktree; // ci // ci

pub mod engine;
pub mod mcp; // engine // engine

pub mod discovery;
pub mod pipeline; // integration
pub mod stats; // integration

pub use error::{Result, WardenError};
pub use events::EventBus;
pub use types::*;
