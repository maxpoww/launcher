//! Concrete telemetry collectors, one module per layer.
//!
//! Layer 1 (Hyprland) is implemented; the remaining layers (selection,
//! app-bridges, behaviour, hardware) land here next, each as a [`Collector`]
//! following the same connect → stream → reconnect shape.
//!
//! [`Collector`]: crate::collector::Collector

pub mod bridge;
pub mod git;
pub mod hyprland;
pub mod media;
pub mod system;
