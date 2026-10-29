//! The OPTIONS bar's one icon array.
//!
//! Everything the bar's boxes show as a picture shares it: the notification
//! cards' avatars and app icons, the clipboard's thumbnails, the playing box's
//! covers, the phone box's menu symbols. One layer per distinct picture, placed
//! as it arrives and written on its own ([`Renderer::update_icon_layer`]); the
//! array grows with the pictures and is never rewritten whole. It used to be:
//! the four sets were concatenated and every chain re-sent whenever any set
//! changed — with 37 avatars that was 13 MB of staging per resolved icon, and
//! a clipboard of pictures made every copied picture a many-MB upload
//! (2026-10-29). The sources stay where they came from (the pictures' files,
//! the icon resolver, the thumbnailer, the cover loader), so no chain is kept
//! in RAM for a re-upload that never comes: the bar's renderer is made once.
//!
//! Full, the oldest placement goes: its owner forgets the key and asks again
//! when it next wants the picture, as it does for a picture it never had.
//!
//! [`Renderer::update_icon_layer`]: crate::renderer::Renderer::update_icon_layer

use crate::App;

/// Layers the array may grow to (one is `ICON_CHAIN_BYTES`, 350 KB, on the
/// GPU; it grows by 16 at a time). The usual bar holds a few dozen.
pub(crate) const OPTIONS_ICON_CAP: usize = 320;

/// Who placed a layer, with the key it knows the picture by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A notification card's picture or app icon (`notif_icon_slot`).
    Notif(String),
    /// A clipboard thumbnail, by the hash of its source path.
    Clip(u64),
    /// A playing box cover, by its url.
    Art(String),
    /// A symbol of the phone box's menu, by its glyph key.
    Glyph(String),
}

#[derive(Default)]
pub(crate) struct OptionsIcons {
    /// Layer → who holds it.
    at: Vec<Owner>,
    /// Where the next eviction looks once the array is full.
    next: usize,
}

impl OptionsIcons {
    /// A layer for `owner`'s picture: the next free one, or — full — the
    /// oldest placement, whose owner is given back to be forgotten.
    fn place(&mut self, owner: Owner) -> (u32, Option<Owner>) {
        if self.at.len() < OPTIONS_ICON_CAP {
            self.at.push(owner);
            return ((self.at.len() - 1) as u32, None);
        }
        let slot = self.next % OPTIONS_ICON_CAP;
        self.next = self.next.wrapping_add(1);
        let old = std::mem::replace(&mut self.at[slot], owner);
        (slot as u32, Some(old))
    }

    /// Layers placed so far.
    pub(crate) fn len(&self) -> usize {
        self.at.len()
    }
}

impl App {
    /// Put `chain` in the bar's icon array for `owner`: its layer, or `None`
    /// before the bar's renderer exists (the owner asks again when it next
    /// wants the picture: the measure that comes with the renderer, the next
    /// draw of the box).
    pub(crate) fn options_icon_put(&mut self, owner: Owner, chain: &[u8]) -> Option<u32> {
        let renderer = self.options_renderer.as_mut()?;
        let (layer, evicted) = self.options_icons.place(owner);
        renderer.update_icon_layer(layer, chain);
        match evicted {
            Some(Owner::Notif(key)) => {
                self.notif_icon_slot.remove(&key);
            }
            Some(Owner::Clip(key)) => {
                self.clip.icon_slot.remove(&key);
            }
            Some(Owner::Art(url)) => {
                self.play_art_slot.remove(&url);
            }
            Some(Owner::Glyph(key)) => {
                self.phone_pill.glyph_layer.remove(&key);
            }
            None => {}
        }
        Some(layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_then_recycles_the_oldest() {
        let mut icons = OptionsIcons::default();
        for i in 0..OPTIONS_ICON_CAP {
            let (layer, old) = icons.place(Owner::Clip(i as u64));
            assert_eq!(layer as usize, i);
            assert!(old.is_none());
        }
        let (layer, old) = icons.place(Owner::Art("a".into()));
        assert_eq!(layer, 0);
        assert_eq!(old, Some(Owner::Clip(0)));
        let (layer, old) = icons.place(Owner::Notif("n".into()));
        assert_eq!(layer, 1);
        assert_eq!(old, Some(Owner::Clip(1)));
        assert_eq!(icons.len(), OPTIONS_ICON_CAP);
    }
}
