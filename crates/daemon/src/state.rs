//! UI state machine with three rest points:
//! `Hidden -> Dock (slim bar) -> Open (full popup)`.
//!
//! `toggle`/`show`/`hide` move between Hidden and Dock; scrolling up on
//! the dock (or `expand`) slides to Open, scrolling down (or `collapse`,
//! Escape, focus loss) slides back to the dock.
//!
//! The animated quantity is `extent`: how far the card has risen above
//! the bottom surface edge, in pixels (docked, just its top sliver;
//! open, its full height plus the bottom gap). Each transition animates
//! `extent` from wherever it currently is toward the target rest point,
//! so interrupting an animation never causes a visual jump. Pure logic,
//! no Wayland types.

use waverunner_core::config::AnimationConfig;
use waverunner_proto::Command;

use crate::animation::{lerp, Animator};

/// The rest point the UI is at (or animating toward).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Fully off-screen.
    Hidden,
    /// Slim dock bar at the bottom edge.
    Dock,
    /// Full popup at its rest position.
    Open,
}

/// The animation-driving state machine.
#[derive(Debug)]
pub struct UiState {
    target: Target,
    /// Current visible content height in pixels.
    extent: f32,
    /// Extent when the running transition started.
    start_extent: f32,
    animator: Option<Animator>,
    animation: AnimationConfig,
    dock_extent: f32,
    full_extent: f32,
}

impl UiState {
    /// Start hidden. `dock_extent`/`full_extent` are the dock-bar and
    /// full-popup heights in pixels.
    pub fn new(animation: AnimationConfig, dock_extent: f32, full_extent: f32) -> Self {
        Self {
            target: Target::Hidden,
            extent: 0.0,
            start_extent: 0.0,
            animator: None,
            animation,
            dock_extent: dock_extent.max(1.0),
            full_extent: full_extent.max(1.0),
        }
    }

    /// Rest point currently targeted.
    pub fn target(&self) -> Target {
        self.target
    }

    /// Current rise of the card above the bottom surface edge, in pixels.
    pub fn extent(&self) -> f32 {
        self.extent
    }

    /// Content height of a rest point, in pixels.
    pub fn extent_of(&self, target: Target) -> f32 {
        match target {
            Target::Hidden => 0.0,
            Target::Dock => self.dock_extent,
            Target::Open => self.full_extent,
        }
    }

    /// Overall opacity: fades in over the first dock-height of travel,
    /// fully opaque from dock level upward.
    pub fn alpha(&self) -> f32 {
        (self.extent / self.dock_extent).clamp(0.0, 1.0)
    }

    /// Whether the surface should accept keyboard input right now.
    /// The dock is pointer-only; only the open popup takes keys.
    pub fn wants_keyboard(&self) -> bool {
        self.target == Target::Open
    }

    /// Whether a redraw/frame-callback cycle is needed.
    pub fn is_animating(&self) -> bool {
        self.animator.is_some()
    }

    /// Apply an IPC command. Returns `true` if the visual state changed.
    pub fn apply(&mut self, command: Command) -> bool {
        match command {
            Command::Toggle => match self.target {
                Target::Hidden | Target::Dock => self.set_target(Target::Open),
                Target::Open => self.set_target(Target::Hidden),
            },
            Command::Show => match self.target {
                Target::Hidden => self.set_target(Target::Dock),
                Target::Dock | Target::Open => false,
            },
            Command::Hide => self.set_target(Target::Hidden),
            Command::Expand => match self.target {
                Target::Dock => self.set_target(Target::Open),
                Target::Hidden | Target::Open => false,
            },
            Command::Collapse => match self.target {
                Target::Open => self.set_target(Target::Dock),
                Target::Hidden | Target::Dock => false,
            },
        }
    }

    /// Retarget the animation. Growing transitions use the open curve,
    /// shrinking ones the (faster) close curve.
    fn set_target(&mut self, target: Target) -> bool {
        if target == self.target {
            return false;
        }
        let curve = if self.extent_of(target) > self.extent {
            &self.animation.open
        } else {
            &self.animation.close
        };
        self.animator = Some(Animator::new(curve));
        self.start_extent = self.extent;
        self.target = target;
        true
    }

    /// Advance the running animation by `dt` seconds.
    /// Returns `true` while more frames are needed.
    pub fn tick(&mut self, dt: f32) -> bool {
        let target_extent = self.extent_of(self.target);
        let Some(animator) = self.animator.as_mut() else {
            return false;
        };
        let progress = animator.step(dt);
        self.extent = lerp(self.start_extent, target_extent, progress);
        if animator.is_settled() {
            self.extent = target_extent;
            self.animator = None;
        }
        self.is_animating()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCK: f32 = 48.0;
    const FULL: f32 = 420.0;

    fn state() -> UiState {
        UiState::new(AnimationConfig::default(), DOCK, FULL)
    }

    fn settle(state: &mut UiState) {
        for _ in 0..2000 {
            if !state.tick(1.0 / 144.0) {
                return;
            }
        }
        panic!("state machine never settled");
    }

    #[test]
    fn toggle_shows_dock_then_hides() {
        let mut s = state();
        assert_eq!(s.target(), Target::Hidden);

        assert!(s.apply(Command::Toggle));
        assert_eq!(s.target(), Target::Dock);
        settle(&mut s);
        assert_eq!(s.extent(), DOCK);
        assert_eq!(s.alpha(), 1.0);

        assert!(s.apply(Command::Toggle));
        assert_eq!(s.target(), Target::Hidden);
        settle(&mut s);
        assert_eq!(s.extent(), 0.0);
        assert_eq!(s.alpha(), 0.0);
    }

    #[test]
    fn expand_slides_dock_to_open() {
        let mut s = state();
        s.apply(Command::Show);
        settle(&mut s);

        assert!(s.apply(Command::Expand));
        assert_eq!(s.target(), Target::Open);
        assert!(s.wants_keyboard());
        settle(&mut s);
        assert_eq!(s.extent(), FULL);
        assert_eq!(s.alpha(), 1.0);

        assert!(s.apply(Command::Collapse));
        settle(&mut s);
        assert_eq!(s.extent(), DOCK);
        assert!(!s.wants_keyboard());
    }

    #[test]
    fn expand_from_hidden_is_a_noop() {
        let mut s = state();
        assert!(!s.apply(Command::Expand));
        assert!(!s.apply(Command::Collapse));
        assert_eq!(s.target(), Target::Hidden);
    }

    #[test]
    fn toggle_when_open_hides_entirely() {
        let mut s = state();
        s.apply(Command::Show);
        settle(&mut s);
        s.apply(Command::Expand);
        settle(&mut s);

        assert!(s.apply(Command::Toggle));
        assert_eq!(s.target(), Target::Hidden);
        settle(&mut s);
        assert_eq!(s.extent(), 0.0);
    }

    #[test]
    fn interrupting_a_transition_does_not_jump() {
        let mut s = state();
        s.apply(Command::Show);
        settle(&mut s);
        s.apply(Command::Expand);
        for _ in 0..5 {
            s.tick(1.0 / 144.0);
        }
        let mid = s.extent();
        assert!(mid > DOCK && mid < FULL, "test needs a mid-flight state");

        // Change of plan: hide while expanding.
        s.apply(Command::Hide);
        s.tick(1.0 / 144.0);
        let after = s.extent();
        assert!(
            (after - mid).abs() < FULL * 0.05,
            "extent jumped on interrupt: {mid} -> {after}"
        );
        settle(&mut s);
        assert_eq!(s.extent(), 0.0);
    }

    #[test]
    fn alpha_fades_only_below_dock_level() {
        let mut s = state();
        s.apply(Command::Show);
        settle(&mut s);
        s.apply(Command::Expand);
        // While expanding above dock height, alpha must stay pinned at 1.
        for _ in 0..30 {
            s.tick(1.0 / 144.0);
            assert_eq!(s.alpha(), 1.0);
        }
    }
}
