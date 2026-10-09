//! What goes IN and OUT of the card: another app's drag dropped on it, and
//! an item of ours dragged out — both real Wayland drags.

use smithay_client_toolkit::data_device_manager::data_offer::DragOffer;
use smithay_client_toolkit::data_device_manager::WritePipe;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{info, warn};
use wayland_client::protocol::wl_data_device_manager::DndAction;

use super::model::*;
use super::view::{tile_height, tile_picture, GAP};
use super::Drag;
use crate::desktop::DragIcon;
use crate::App;

/// How long the dragging app may go without sending anything before the
/// drop is given up on. An app that takes the request and never writes (or
/// never closes its end) used to leave the reading thread waiting for ever,
/// and its own drag unfinished.
const DROP_PATIENCE: std::time::Duration = std::time::Duration::from_secs(8);

/// Read everything the other end of `fd` sends, up to `max` bytes — but
/// wait no longer than `patience` for any one piece of it. `None`: it went
/// quiet for that long (or the read failed) before it was done.
pub(super) fn read_patiently(
    fd: std::os::fd::OwnedFd,
    max: u64,
    patience: std::time::Duration,
) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let mut file = std::fs::File::from(fd);
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, for a descriptor this function owns.
        let ready = unsafe { libc::poll(&mut poll, 1, patience.as_millis() as libc::c_int) };
        if ready == 0 {
            return None;
        }
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        match file.read(&mut buf) {
            Ok(0) => return Some(out),
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.len() as u64 >= max {
                    out.truncate(max as usize);
                    return Some(out);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

impl App {
    /// Take item `id` into a Wayland drag: any app it is let go on gets a
    /// copy of it.
    /// The item as it stands in the list, as a drag image: seen all the way
    /// (Max, 2026-10-08: *"when i grab a item it becomes invisible. i want
    /// to see it all the time"*). `tile`: its box on the card's surface.
    fn card_drag_picture(&mut self, item: &Item, tile: super::Rect) -> Option<DragIcon> {
        let scale = self
            .card_fscale
            .as_ref()
            .map_or(1.0, |fs| fs.scale_or(1.0))
            .ceil()
            .clamp(1.0, 3.0);
        let paint = self.card_paint();
        let zoom = self.card.zoom();
        let renderer = self.card_renderer.as_mut()?;
        let lines = self.card.lines.get(&item.id).map_or(&[][..], Vec::as_slice);
        let thumb = item.path.as_ref().and_then(|p| self.card.chains.get(p));
        let side = crate::apps::ICON_SIZE as usize;
        let canvas = tile_picture(
            item,
            lines,
            tile.w,
            scale,
            zoom,
            &paint,
            thumb.map(|chain| &chain[..(side * side * 4).min(chain.len())]),
            side,
            &mut |canvas, line, px, family, ink, at| {
                let ink8 = ink.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
                let (ox, oy) = (at.0.round() as i32, at.1.round() as i32);
                renderer.text_to_pixels(line, px, family, ink8, &mut |x, y, c| {
                    // (Text colours are the screen's own encoding.)
                    let lin = |v: u8| crate::options::srgb_to_linear(v as f32 / 255.0);
                    let c = [lin(c[0]), lin(c[1]), lin(c[2]), c[3] as f32 / 255.0];
                    canvas.blend(ox + x, oy + y, c, 1.0);
                });
            },
        );
        let grip = match self.card.ptr {
            Some((x, y)) => (
                (x - tile.x).clamp(0.0, tile.w),
                (y - tile.y).clamp(0.0, tile.h),
            ),
            None => (tile.w / 2.0, tile.h / 2.0),
        };
        self.drag_picture(&canvas.bytes(), canvas.w, canvas.h, scale as i32, grip)
    }

    pub(super) fn card_lift(&mut self, id: u64, serial: u32) {
        self.card.press = None;
        // Where it is picked up: the middle of its own box.
        let tile = self.card.tiles.iter().find(|t| t.id == id).map(|t| t.rect);
        let from_y = tile.map_or(0.0, |r| r.y + r.h / 2.0);
        let Some(item) = self.card.item(id).cloned() else {
            return;
        };
        // It travels as a picture of itself, held where it was grabbed.
        let image = tile.and_then(|tile| self.card_drag_picture(&item, tile));
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
            from_y,
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
    /// — as a copy. One of the card's OWN items is taken too: it is being
    /// moved to another place in the list.
    pub(crate) fn card_dnd_enter(&mut self, offer: DragOffer) {
        let mimes = offer.with_mime_types(|m| m.to_vec());
        let own = self.card.drag.is_some();
        // Something brought to the card while Memory is up is for the
        // session: its page comes back to take it.
        // (A window with no session yet keeps Memory up: the drop starts
        // one, `card_push`.)
        if self.card.page == super::view::Page::Memory && self.card.open.is_some() {
            self.card_show(super::view::Page::Session);
        }
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
        let Some(first) = first else {
            offer.accept_mime_type(offer.serial, None);
            return;
        };
        offer.accept_mime_type(offer.serial, Some(first.to_owned()));
        offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
        self.card.dnd_over = true;
        self.card.drop_own = own;
        // (Where the pointer is comes with the first motion: the enter's
        // own position is the surface's middle on this compositor. An
        // item of ours starts where it was picked up.)
        self.card.drop_y = self.card.drag.as_ref().filter(|_| own).map(|d| d.from_y);
        if own {
            self.card_carried_steps(1.0);
        }
        self.card.dnd_mimes = mimes;
        self.card.dnd_hint = None;
        self.card_dnd_offer = Some(offer);
        self.request_card_draw();
    }

    /// An item of ours leaves the list as its drag comes over the card
    /// (`way` 1) and stands in it again when the drag goes off (`way` -1).
    /// The items under it keep standing exactly where they are at that
    /// moment and EASE from there, instead of jumping by its height first
    /// (Max, 2026-10-08: *"the one that is under it moves up instantly, it
    /// looks bad"*).
    fn card_carried_steps(&mut self, way: f32) {
        let Some(id) = self.card.drag.as_ref().map(|d| d.id) else {
            return;
        };
        let Some(at) = self.card.items.iter().position(|it| it.id == id) else {
            return;
        };
        let lines = self.card.lines.get(&id).map_or(1, Vec::len);
        let step = way * (tile_height(self.card.items[at].kind, lines, self.card.zoom()) + GAP);
        for it in &self.card.items[at + 1..] {
            *self.card.shifts.entry(it.id).or_insert(0.0) += step;
        }
    }

    /// The drag over the card moved: the list opens a place where it would
    /// land now.
    pub(crate) fn card_dnd_motion(&mut self, _x: f32, y: f32) {
        if self.card.dnd_over {
            self.card.drop_y = Some(y);
            self.request_card_draw();
        }
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
            if self.card.drop_own {
                self.card_carried_steps(-1.0);
            }
            self.card.dnd_over = false;
            self.card.drop_own = false;
            self.card.drop_y = None;
            self.request_card_draw();
        }
    }

    /// Let go on the card, at the place the list had opened. One of the
    /// card's own items: it moves there, and that is all. Anything else:
    /// read what it carries, the richest first — the list of files; a
    /// picture's pixels; the text — and put it there.
    pub(crate) fn card_dnd_drop(&mut self) {
        if self.card_dnd_offer.is_none() {
            return;
        }
        let index = self.card.opening().map(|(index, _)| index);
        let own = self.card.drop_own;
        self.card.dnd_over = false;
        self.card.drop_own = false;
        self.card.drop_y = None;
        // What is there now is where everything belongs: nothing eases
        // back from a place that has just been filled.
        self.card.shifts.clear();
        if own {
            if let (Some(id), Some(index)) = (self.card.drag.as_ref().map(|d| d.id), index) {
                if self.card.move_to(id, index) {
                    info!("card: an item moved to place {index}");
                    self.card_save();
                }
            }
            // → our source's `dnd_finished` → `card_drag_end`.
            self.card_dnd_end(true);
            return;
        }
        self.card.drop_index = index;
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
        let (tx, rx) = calloop::channel::channel::<Option<Vec<u8>>>();
        std::thread::spawn(move || {
            let read = read_patiently(fd, DROP_MAX, DROP_PATIENCE);
            if read.is_none() {
                warn!("card: the dragging app never sent the drop's {mime}; given up on");
            }
            let _ = tx.send(read);
        });
        if self
            .loop_handle
            .insert_source(rx, move |event, _, app: &mut App| match event {
                calloop::channel::Event::Msg(Some(bytes)) => app.card_dnd_received(mime, &bytes),
                // It never came: the drag is told it is over, taken or not.
                calloop::channel::Event::Msg(None) => app.card_dnd_end(false),
                calloop::channel::Event::Closed => {}
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
                let at = self.card.drop_index.take();
                self.card_add_paths_at(paths, at);
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
                let at = self.card.drop_index.take();
                self.card_add_text_at(&list, at);
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
        let at = self.card.drop_index.take();
        self.card_add_text_at(&String::from_utf8_lossy(bytes), at);
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
        let at = self.card.drop_index.take();
        self.card_push(
            Kind::Image,
            body,
            Some(path.to_string_lossy().into_owned()),
            true,
            aspect,
            at,
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
        self.card.drop_own = false;
        self.card.drop_y = None;
        self.card.drop_index = None;
        self.card.dnd_mimes.clear();
        self.card.dnd_hint = None;
        self.request_card_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::time::{Duration, Instant};

    fn pipe() -> (OwnedFd, std::fs::File) {
        let mut fds = [0; 2];
        // SAFETY: a plain pipe; both ends are owned from here on.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe {
            (
                OwnedFd::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
            )
        }
    }

    #[test]
    fn a_drop_is_read_whole_when_the_other_end_sends_and_closes() {
        let (read, mut write) = pipe();
        write.write_all(b"file:///tmp/a.txt\r\n").unwrap();
        drop(write);
        assert_eq!(
            read_patiently(read, 1024, Duration::from_secs(2)).as_deref(),
            Some(&b"file:///tmp/a.txt\r\n"[..])
        );
        // No more than it may be.
        let (read, mut write) = pipe();
        write.write_all(b"0123456789").unwrap();
        drop(write);
        assert_eq!(
            read_patiently(read, 4, Duration::from_secs(2)).as_deref(),
            Some(&b"0123"[..])
        );
    }

    #[test]
    fn an_app_that_never_sends_is_given_up_on() {
        // The other end stays open and silent: not for ever.
        let (read, _write) = pipe();
        let began = Instant::now();
        assert_eq!(read_patiently(read, 1024, Duration::from_millis(60)), None);
        assert!(began.elapsed() < Duration::from_secs(2));
    }
}
