//! Shared, compositor-independent logic for waverunner: configuration
//! types, the application (.desktop) index, and fuzzy search.
//!
//! Nothing in this crate may depend on Wayland, wgpu, or any daemon
//! runtime detail — it must stay unit-testable on a headless machine.

pub mod config;
pub mod index;
pub mod search;

pub use config::Config;
pub use index::{AppEntry, DesktopIndex};
pub use search::Searcher;
