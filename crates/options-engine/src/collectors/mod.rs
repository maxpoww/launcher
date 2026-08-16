//! Concrete telemetry collectors, one module per layer.
//!
//! Layer 1 (Hyprland) is implemented; the remaining layers (selection,
//! app-bridges, behaviour, hardware) land here next, each as a [`Collector`]
//! following the same connect → stream → reconnect shape.
//!
//! [`Collector`]: crate::collector::Collector

pub mod audio;
pub mod bridge;
pub mod deploy;
pub mod git;
pub mod hyprland;
pub mod media;
pub mod notifications;
pub mod selection;
pub mod system;
