//! WHAT A KIND OF THING IS FOR: one action on the selection pill (in Open
//! with's place) and up to three on the menu, right after Open.

use super::*;

/// The actions a selection is for: the pill's one (its symbol, its words,
/// what it does) and the menu's.
pub(super) struct Smart {
    pub(super) bar: Option<(&'static str, &'static str, Action)>,
    pub(super) menu: Vec<Row>,
}

impl App {
    /// What `sel` (in window `win`) is for.
    pub(super) fn filer_smart(&self, _win: &Win, _sel: &[&Item]) -> Smart {
        Smart {
            bar: None,
            menu: Vec::new(),
        }
    }
}
