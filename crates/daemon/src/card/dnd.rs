//! What goes IN and OUT of the card: another app's drag dropped on it, and
//! an item of ours dragged out — both real Wayland drags.

use smithay_client_toolkit::data_device_manager::data_offer::DragOffer;
use smithay_client_toolkit::data_device_manager::WritePipe;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{info, warn};
use wayland_client::protocol::wl_data_device_manager::DndAction;

use super::model::*;
use super::Drag;
use crate::desktop::DragIcon;
use crate::App;

impl App {
    /// Take item `id` into a Wayland drag: any app it is let go on gets a
    /// copy of it.
    pub(super) fn card_lift(&mut self, id: u64, serial: u32) {
        self.card.press = None;
        let Some(item) = self.card.item(id).cloned() else {
            return;
        };
        let (Some(manager), Some(device), Some(layer)) = (
            self.data_device_manager.as_ref(),
            self.data_device.as_ref(),
            self.card_layer.as_ref(),
        ) else {
            warn!("card: no data device; items cannot be dragged out");
            return;
        };
        let source =
            manager.create_drag_and_drop_source(&self.qh, out_mimes(&item), DndAction::Copy);
        // A picture travels as itself; anything else under the bare pointer.
        let image = item
            .path
            .as_ref()
            .and_then(|p| self.card.chains.get(p))
            .and_then(|chain| self.drag_image(chain, (24.0, 24.0), self.icon_scale()));
        source.start_drag(
            device,
            layer.wl_surface(),
            image.as_ref().map(DragIcon::surface),
            serial,
        );
        if let Some(image) = image.as_ref() {
            image.surface().commit();
        }
        info!(
            "card: {:?} in hand",
            item.body.chars().take(60).collect::<String>()
        );
        self.card.drag = Some(Drag {
            id,
            source,
            _icon: image,
        });
        self.request_card_draw();
    }

    /// Whether `source` is the card's drag.
    pub(crate) fn is_card_drag_source(
        &self,
        source: &wayland_client::protocol::wl_data_source::WlDataSource,
    ) -> bool {
        self.card
            .drag
            .as_ref()
            .is_some_and(|d| d.source.inner() == source)
    }

    /// Another app asked for the item in hand, as `mime`. Written off the
    /// loop (the other side reads when it pleases).
    pub(crate) fn card_send_drag(&mut self, mime: &str, pipe: WritePipe) {
        let item = self.card.drag.as_ref().and_then(|d| self.card.item(d.id));
        let Some(payload) = item.and_then(|it| payload(it, mime)) else {
            warn!("card: {mime} asked of a drag that never offered it");
            return;
        };
        let fd: std::os::fd::OwnedFd = pipe.into();
        let mime = mime.to_owned();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut file = std::fs::File::from(fd);
            let bytes = match payload {
                Payload::Bytes(bytes) => Ok(bytes),
                Payload::File(path) => std::fs::read(path),
            };
            match bytes {
                Ok(bytes) => {
                    if let Err(e) = file.write_all(&bytes) {
                        warn!("card: writing the dragged item's {mime} failed: {e}");
                    }
                }
                Err(e) => warn!("card: the dragged file cannot be read: {e}"),
            }
        });
    }

    /// The card's drag is over, dropped or not: the item never left.
    pub(crate) fn card_drag_end(&mut self, done: bool) {
        if self.card.drag.take().is_some() {
            info!("card: {}", if done { "dropped" } else { "drag cancelled" });
            self.request_card_draw();
        }
    }

    /// A drag came over the card. Another app's (or a desktop icon's) is
    /// taken if it carries anything the card keeps (files, a picture, text)
    /// — as a copy; the card's own is refused (an item does not land on
    /// itself).
    pub(crate) fn card_dnd_enter(&mut self, offer: DragOffer) {
        let mimes = offer.with_mime_types(|m| m.to_vec());
        let own = self.card.drag.is_some();
        let first = if mimes.iter().any(|m| m == URI_LIST) {
            Some(URI_LIST)
        } else {
            image_mime(&mimes)
                .map(|(m, _)| m)
                .or_else(|| text_mime(&mimes))
        };
        if !own {
            info!(
                "card: a drag came over offering {mimes:?}, actions {:?}",
                offer.source_actions
            );
        }
        let Some(first) = first.filter(|_| !own) else {
            offer.accept_mime_type(offer.serial, None);
            return;
        };
        offer.accept_mime_type(offer.serial, Some(first.to_owned()));
        offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
        self.card.dnd_over = true;
        self.card.dnd_mimes = mimes;
        self.card.dnd_hint = None;
        self.card_dnd_offer = Some(offer);
        self.request_card_draw();
    }

    /// Whether a drag is over the card (the data device's events are the
    /// card's then, not the desktop's).
    pub(crate) fn card_dnd_active(&self) -> bool {
        self.card_dnd_offer.is_some()
    }

    /// The drag left — unless it has just been DROPPED here: Hyprland
    /// sends `leave` right after `drop`, while what was dropped is still
    /// on its way through the pipe (see `desktop_dnd_leave`).
    pub(crate) fn card_dnd_leave(&mut self) {
        let dropped = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .is_some_and(|o| o.dropped);
        if dropped {
            return;
        }
        if self.card_dnd_offer.take().is_some() {
            self.card.dnd_over = false;
            self.request_card_draw();
        }
    }

    /// Let go on the card: read what it carries, the richest first — the
    /// list of files; a picture's pixels; the text.
    pub(crate) fn card_dnd_drop(&mut self) {
        if self.card_dnd_offer.is_none() {
            return;
        }
        self.card.dnd_over = false;
        self.request_card_draw();
        let first = if self.card.dnd_mimes.iter().any(|m| m == URI_LIST) {
            Some(URI_LIST)
        } else {
            image_mime(&self.card.dnd_mimes)
                .map(|(m, _)| m)
                .or_else(|| text_mime(&self.card.dnd_mimes))
        };
        match first {
            Some(mime) => self.card_dnd_read(mime),
            None => self.card_dnd_end(false),
        }
    }

    /// Ask the dragging app for `mime` and read it off the loop.
    fn card_dnd_read(&mut self, mime: &'static str) {
        // The live offer where there is one (the one kept since `enter` is
        // a snapshot).
        let live = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .filter(|o| o.dropped);
        let Some(offer) = live.or_else(|| self.card_dnd_offer.clone()) else {
            return;
        };
        let pipe = match offer.receive(mime.to_owned()) {
            Ok(pipe) => pipe,
            Err(e) => {
                warn!("card: cannot receive the drop as {mime}: {e}");
                self.card_dnd_end(false);
                return;
            }
        };
        // Through `OwnedFd`, never `into_raw_fd` (SCTK closes the pipe).
        let fd: std::os::fd::OwnedFd = pipe.into();
        let (tx, rx) = calloop::channel::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            if let Err(e) = std::fs::File::from(fd)
                .take(DROP_MAX)
                .read_to_end(&mut bytes)
            {
                warn!("card: reading the drop's {mime} failed: {e}");
            }
            let _ = tx.send(bytes);
        });
        if self
            .loop_handle
            .insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(bytes) = event {
                    app.card_dnd_received(mime, &bytes);
                }
            })
            .is_err()
        {
            warn!("card: cannot wait for the drop");
            self.card_dnd_end(false);
        }
    }

    /// One type of the drop arrived: keep it, or ask for the next.
    fn card_dnd_received(&mut self, mime: &'static str, bytes: &[u8]) {
        if self.card_dnd_offer.is_none() {
            warn!("card: the drop's {mime} arrived after its drag was gone");
            return;
        }
        info!("card: the drop's {mime} is {} bytes", bytes.len());
        if mime == URI_LIST {
            let list = String::from_utf8_lossy(bytes).into_owned();
            let paths = crate::desktop::uri_list_paths(&list);
            if !paths.is_empty() {
                self.card_add_paths(paths);
                self.card_dnd_end(true);
                return;
            }
            // No file of this machine in it: a picture or a link out of a
            // web page. Its pixels if they are offered, else its text —
            // else the addresses themselves.
            self.card.dnd_hint = remote_name(&list);
            if let Some((next, _)) = image_mime(&self.card.dnd_mimes) {
                self.card_dnd_read(next);
            } else if let Some(next) = text_mime(&self.card.dnd_mimes) {
                self.card_dnd_read(next);
            } else {
                self.card_add_text(&list);
                self.card_dnd_end(true);
            }
            return;
        }
        if let Some((_, ext)) = IMAGE_MIMES.into_iter().find(|(m, _)| *m == mime) {
            if bytes.is_empty() {
                // Offered and not delivered: the text, if there is one.
                if let Some(next) = text_mime(&self.card.dnd_mimes) {
                    self.card_dnd_read(next);
                    return;
                }
                self.card_dnd_end(false);
                return;
            }
            self.card_keep_picture(bytes, ext);
            self.card_dnd_end(true);
            return;
        }
        self.card_add_text(&String::from_utf8_lossy(bytes));
        self.card_dnd_end(true);
    }

    /// A picture that came as pixels: saved beside the list, as the card's
    /// own.
    fn card_keep_picture(&mut self, bytes: &[u8], ext: &str) {
        self.card_load();
        let dir = crate::persist::data_path(PICTURES_DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("card: cannot keep the picture ({e})");
            return;
        }
        let path = dir.join(format!("{}.{ext}", self.card.next_id.max(1)));
        if let Err(e) = std::fs::write(&path, bytes) {
            warn!("card: cannot keep the picture ({e})");
            return;
        }
        let name = self
            .card
            .dnd_hint
            .take()
            .unwrap_or_else(|| "picture".to_owned());
        let body = format!("{name} · {}", size_text(bytes.len() as u64));
        let aspect = aspect_of(&path);
        self.card_push(
            Kind::Image,
            body,
            Some(path.to_string_lossy().into_owned()),
            true,
            aspect,
        );
    }

    /// The drop is over: tell the dragging app (it stays mid-drag until
    /// it hears), taken or not.
    fn card_dnd_end(&mut self, taken: bool) {
        if let Some(offer) = self.card_dnd_offer.take() {
            if taken {
                offer.finish();
            } else {
                offer.destroy();
            }
        }
        self.card.dnd_over = false;
        self.card.dnd_mimes.clear();
        self.card.dnd_hint = None;
        self.request_card_draw();
    }
}
