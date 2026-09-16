//! Concrete telemetry collectors, one module per layer.
//!
//! Layer 1 (Hyprland) is implemented; the remaining layers (selection,
//! app-bridges, behaviour, hardware) land here next, each as a [`Collector`]
//! following the same connect → stream → reconnect shape.
//!
//! [`Collector`]: crate::collector::Collector

pub mod audio;
pub mod bluetooth;
pub mod bridge;
pub mod daylight;
pub mod deploy;
pub mod downloads;
pub mod git;
pub(crate) mod gpu;
pub mod hyprland;
pub mod media;
pub mod notifications;
pub mod selection;
pub mod system;
