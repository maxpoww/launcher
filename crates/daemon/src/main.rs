//! waverunner daemon: persistent Wayland layer-shell launcher.
//!
//! Single-threaded calloop event loop driving three sources:
//! the Wayland connection, the IPC control socket, and (from P4 on) a
//! channel from the background desktop-entry indexer.
//!
//! Rendering is frame-callback driven: while animating we request a
//! `wl_surface.frame` callback per drawn frame; once settled we stop, and
//! the process goes fully idle.

mod animation;
mod applier;
mod apps;
mod boxes;
mod clip_source;
mod clipboard;
mod content;
mod dict;
mod dragging;
mod files;
mod frame;
mod groups;
mod hypr;
mod install;
mod ipc;
mod jelly;
mod launch;
mod managed;
mod managed_webapps;
mod nix;
// Notification OPTION data plane: a D-Bus worker that mirrors the options-notify
// daemon's active list into the UI and sends back dismiss/act/reply. The render
// surface (the vertical-list box + entry/exit springs) consumes this next; the
// allow drops once `spawn`/`NotifHandle` are wired into the event loop.
#[allow(dead_code)]
mod notifications;
// The notification OPTION UI (bell → peek → history dropdown) on the topbar.
mod notif;
// Off-thread resolver: a notification's app icon → premultiplied mip chain.
mod notif_icons;
mod options;
mod order;
mod pager;
mod pages;
mod persist;
mod pins;
mod renderer;
mod screencopy;
mod state;
mod surface;
mod thumbs;
mod unfurl;
// FreeDesktop trash backend. Read (`list`/`file_path`) and trash (drop a file
// on the bin) are wired; `restore`/`erase`/`empty`/`is_empty` are complete and
// tested but not yet triggered from the UI — drop this allow once the
// empty/restore gestures land.
#[allow(dead_code)]
mod trash;
mod usage;
mod webapps;

use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use anyhow::Context;
use calloop::channel;
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::data_device_manager::data_device::{DataDevice, DataDeviceHandler};
use smithay_client_toolkit::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use smithay_client_toolkit::data_device_manager::data_source::DataSourceHandler;
use smithay_client_toolkit::data_device_manager::{DataDeviceManagerState, WritePipe};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager;
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shm::raw::RawPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_registry,
    delegate_seat, delegate_shm, registry_handlers,
};
use tracing::{debug, error, info, warn};
use waverunner_core::index::AppEntry;

use crate::install::{PendingInstall, INSTALL_REVEAL, PKG_QUERY_MIN};
use waverunner_core::{Config, Searcher};
use waverunner_proto::Command;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{
    wl_data_device, wl_data_source, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

use crate::content::Hit;
use crate::renderer::Renderer;
use crate::state::{Target, UiState};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "waverunner=info".into()),
        )
        .init();

    let config = Config::load().context("loading config.toml")?;
    info!(
        "config loaded: {}x{} popup",
        config.window.width, config.window.height
    );

    let conn = Connection::connect_to_env()
        .context("connecting to Wayland (is this a Wayland session?)")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("wayland registry init")?;
    let qh: QueueHandle<App> = event_queue.handle();

    let mut event_loop: EventLoop<App> =
        EventLoop::try_new().context("creating calloop event loop")?;
    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("registering wayland source: {e}"))?;

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("zwlr_layer_shell_v1 not available (compositor must support wlr-layer-shell)")?;

    // The surface is tall enough for the fully risen card, the gap
    // beneath it, the magnification headroom above, and the transparent
    // drag margin around it (room for a dragged icon to roam past the
    // card edges); the card slides within the extent range only.
    let full_extent = config.window.height + config.window.bottom_margin;
    let surface_height =
        full_extent + content::MAGNIFY_HEADROOM as u32 + content::DRAG_MARGIN_TOP as u32;
    let surface_width = config.window.width + 2 * content::DRAG_MARGIN_X as u32;
    let render_scale = config.window.render_scale.max(1);
    let layer = surface::create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        surface_width,
        surface_height,
        render_scale,
    );

    // The OPTIONS topbar: a second top-anchored strip, created only when
    // enabled. Its renderer is built on its first configure (like the dock's).
    let options_layer = config.options.enabled.then(|| {
        surface::create_top_surface(
            &compositor,
            &layer_shell,
            &qh,
            config.options.height,
            config.options.height + OPTIONS_OVERHANG + OPTIONS_DROPDOWN_H,
            config.options.render_scale.max(1),
        )
    });
    // wlr-screencopy + shm for the smart-gaps colour-match. Both optional:
    // without them (or without Hyprland IPC) the bar just never matches.
    let shm = Shm::bind(&globals, &qh).ok();
    let screencopy = options_layer
        .is_some()
        .then(|| {
            globals
                .bind::<ZwlrScreencopyManagerV1, App, _>(&qh, 1..=3, ())
                .map_err(|e| warn!("wlr-screencopy unavailable; bar colour-match off: {e}"))
                .ok()
        })
        .flatten();

    // App discovery runs on the one allowed background thread; it
    // rescans on request (dock reveals) and delivers results over this
    // channel. The initial scan is queued by spawn_indexer.
    //
    // Patch XDG_DATA_DIRS first so both the indexer and nix threads can
    // find icons under ~/.nix-profile/share (nix profile installs land
    // there; freedesktop_icons uses XDG_DATA_DIRS for all its lookups).
    apps::patch_xdg_data_dirs();
    // Materialize a launcher for every catalog webapp so the indexer finds
    // them; whether each shows on the grid or only in the Install section is
    // decided at runtime by managed-webapps membership.
    webapps::materialize_catalog();
    // Icon-plate mode is a process-wide switch read by the tile producers on
    // several worker threads; set it before any of them start.
    apps::set_icon_plate(config.theme.icon_plate);
    let (apps_tx, apps_rx) = channel::channel::<apps::LoadedApps>();
    let indexer = apps::spawn_indexer(config.theme.icon_theme.clone(), apps_tx);

    // The nix thread owns the nixpkgs package index (Install-section
    // search) and serializes `nix profile` mutations; results arrive
    // over this channel.
    let (nix_tx, nix_rx) = channel::channel::<nix::Event>();
    let nix = nix::spawn(nix_tx, config.theme.icon_theme.clone());

    // File thumbnails arrive from their own worker as they render.
    let (thumb_tx, thumb_rx) = channel::channel::<thumbs::Event>();
    let thumbs = thumbs::spawn(thumb_tx);

    // Clipboard paste reads answer over this channel (each paste reads
    // the selection pipe on a short-lived thread).
    let (paste_tx, paste_rx) = channel::channel::<String>();

    // Notification OPTION: the options-notify D-Bus client worker feeds the
    // topbar's bell + history. Only spawned when the topbar (its host) is on.
    let (notif_handle, notif_rx) = if config.options.enabled {
        let (tx, rx) = channel::channel::<notifications::NotifEvent>();
        (Some(notifications::spawn(tx)), Some(rx))
    } else {
        (None, None)
    };

    // Clipboard OPTION: a worker watches the Wayland clipboard and feeds the
    // browsable history; copy-back commands ride back over the handle. Only
    // spawned when the topbar (its host) is on.
    let (clip_handle, clip_rx) = if config.options.enabled {
        let (tx, rx) = channel::channel::<clipboard::ClipEvent>();
        (Some(clipboard::spawn(tx)), Some(rx))
    } else {
        (None, None)
    };
    // Clipboard OPTION thumbnailer: its own instance of the Files-section
    // thumbnail worker, for image/file clip previews in the history box.
    let (clip_thumbs, clip_thumb_rx) = if config.options.enabled {
        let (tx, rx) = channel::channel::<thumbs::Event>();
        (Some(thumbs::spawn(tx)), Some(rx))
    } else {
        (None, None)
    };

    // Clipboard link unfurl worker (opt-in via `[options] link_unfurl`): fetches
    // page metadata for copied links off the loop. Absent unless enabled.
    let (clip_unfurl, clip_unfurl_rx) = if config.options.enabled && config.options.link_unfurl {
        let (tx, rx) = channel::channel::<unfurl::Event>();
        (Some(unfurl::spawn(tx)), Some(rx))
    } else {
        (None, None)
    };

    // Offline dictionary loader for the clipboard "define a word" panel. The
    // worker is spawned lazily (first panel open); here we only make the channel
    // whose finished map the loop folds in via `on_dict_loaded`.
    let (dict_tx, dict_rx) = if config.options.enabled {
        let (tx, rx) = channel::channel::<dict::Event>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Notification OPTION icon resolver: turns a notification's app_icon /
    // desktop_entry into a card-tile mip chain off the compositor thread.
    let (notif_icons_handle, notif_icon_rx) = if config.options.enabled {
        let (tx, rx) = channel::channel::<notif_icons::Resolved>();
        (
            Some(notif_icons::spawn(config.theme.icon_theme.clone(), tx)),
            Some(rx),
        )
    } else {
        (None, None)
    };

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        conn: conn.clone(),
        qh: qh.clone(),
        loop_handle: event_loop.handle(),
        compositor,
        layer,
        renderer: None,
        options_layer,
        options_renderer: None,
        options_size: (0, 0),
        options_bar_matched: None,
        options_pill_color: None,
        options_match: None,
        capture: None,
        options_poll_pending: false,
        screencopy,
        shm,
        shm_pool: None,
        pointer_surface: options::PointerSurface::Dock,
        options_ptr: None,
        options_hover: None,
        options_clock: App::options_clock_init(),
        options_date: App::options_date_init(),
        options_title: None,
        options_active_addr: None,
        options_clock_w: 0.0,
        options_date_w: 0.0,
        options_title_w: 0.0,
        options_fullscreen: false,
        options_hidden: false,
        options_reveal_deadline: None,
        options_hide_deadline: None,
        options_ctrl: options::CtrlAnim::default(),
        options_clock_meta: options::ClockMeta::default(),
        notif: notif::NotifState::new(notif_handle),
        clip: clipboard::ClipState::new(clip_handle, clip_thumbs, clip_unfurl),
        dict_tx,
        notif_icons_handle,
        notif_icon_chains: Vec::new(),
        notif_icon_slot: HashMap::new(),
        notif_icon_pending: HashSet::new(),
        ui: UiState::new(
            config.animation.clone(),
            // Docked, the card is a floating bar: dock height plus the
            // bottom gap it now hovers above the screen edge.
            (config.window.input_bar_height + config.window.bottom_margin) as f32,
            // Fully open, the card has risen its own height plus the gap.
            full_extent as f32,
        ),
        config,
        icon_size: load_icon_size(),
        buffer_size: (0, 0),
        scale_factor: 1,
        last_frame: None,
        frame_pending: false,
        dirty: false,
        keyboard: None,
        pointer: None,
        cursor_shape: CursorShapeManager::bind(&globals, &qh).ok(),
        cursor_device: None,
        enter_serial: 0,
        cursor_now: None,
        agua_card: animation::Follower::new(content::AGUA_CARD_K, content::AGUA_CARD_C),
        agua_icons: animation::Follower::new(content::AGUA_ICONS_K, content::AGUA_ICONS_C),
        agua_content: animation::Follower::new(content::AGUA_CONTENT_K, content::AGUA_CONTENT_C),
        agua_breath: animation::Follower::new(content::AGUA_BREATH_K, content::AGUA_BREATH_C),
        jelly: jelly::JellyMembrane::new(),
        box_jelly: jelly::JellyMembrane::new(),
        pointer_inside_box: false,
        dock_wave_h: Vec::new(),
        dock_wave_v: Vec::new(),
        dock_crest_prev: Vec::new(),
        modifiers: Modifiers::default(),
        force_new_instance: false,
        data_device_manager: DataDeviceManagerState::bind(&globals, &qh).ok(),
        data_device: None,
        paste_tx,
        hide_deadline: None,
        rest_hide_pending: false,
        restore_window: None,
        pending_refocus: None,
        focus_launched: None,
        interactive: false,
        input_extent: None,
        entries: Vec::new(),
        kinds: Vec::new(),
        icon_layers: Vec::new(),
        base_len: 0,
        assets: HashMap::new(),
        thumbs,
        thumb_map: HashMap::new(),
        thumb_next: 0,
        thumb_pending: HashSet::new(),
        audio_paths: HashSet::new(),
        nix,
        pkg_hits: Vec::new(),
        pkg_hits_query: None,
        pkg_hit_icons: Vec::new(),
        pkg_hit_placeholders: Vec::new(),
        pkg_layer_base: 0,
        pkg_state: PkgIndexState::Loading,
        busy_ids: HashSet::new(),
        failed_ids: HashMap::new(),
        launching: HashSet::new(),
        managed: managed::ManagedDb::load(),
        managed_webapps: managed_webapps::ManagedWebapps::load(),
        webapp_recommended: webapps::recommended_slugs(),
        removable_ids: HashSet::new(),
        installed_app_ids: HashSet::new(),
        file_index: Vec::new(),
        files_dir: None,
        groups: groups::GroupDb::load(),
        app_group: None,
        dock_stack: None,
        dir_stack: None,
        box_from_dock: false,
        box_drag: None,
        box_drag_page_at: None,
        box_page: 0,
        box_pager: pager::Pager::default(),
        box_slide: Vec::new(),
        group_minis: Vec::new(),
        order: order::OrderDb::load(),
        pending_installs: Vec::new(),
        pending_webapps: Vec::new(),
        install_notify: Vec::new(),
        notify_dock_hide_at: None,
        managed_install_attrs: Vec::new(),
        known_app_ids: HashSet::new(),
        cli_ids: HashSet::new(),
        uninstalling: HashMap::new(),
        just_installed: None,
        dock_hover_since: None,
        trash_react: 0.0,
        trash_hover: 0.0,
        reorder_slot: None,
        grid_drag_page_at: None,
        apps_slide: Vec::new(),
        apps_slots: Vec::new(),
        apps_page_map: Vec::new(),
        apps_cap: 24,
        apps_span: 0,
        just_dropped: None,
        install_drag_reset: false,
        prev_resting_grid: true,
        dock_slide: Vec::new(),
        mag_sleep: None,
        mag_amount: 1.0,
        group_anim: 1.0,
        group_anim_target: 1.0,
        group_origin: None,
        closing_members: None,
        pending_icons: None,
        hover: None,
        pointer_pos: None,
        pointer_inside_card: false,
        scroll: ScrollState::default(),
        gesture: GestureState::default(),
        search: SearchState::default(),
        indexer,
        usage: usage::UsageDb::load(),
        pins: pins::PinDb::load(),
        dock_order: Vec::new(),
        running: HashMap::new(),
        dock_divider: None,
        last_rescan: Instant::now(),
        bounce: None,
        placeholders: Vec::new(),
        zone_free: false,
        exit: false,
    };

    // Reconcile the Wayland surface size and UiState extents with the
    // persisted icon_size (main() computed them assuming scale 1.0).
    if app.icon_size != 1 {
        app.apply_icon_size_change();
    }

    // Own the newest clip up front so it's pasteable from a fresh session (our
    // data-control source releases the selection when the previous daemon exits).
    app.serve_newest_clip();

    let socket_path = waverunner_proto::socket_path();
    let _socket_guard = ipc::listen(&event_loop.handle(), &socket_path)?;

    event_loop
        .handle()
        .insert_source(apps_rx, |event, _, app| {
            if let channel::Event::Msg(loaded) = event {
                app.on_apps_loaded(loaded);
            }
        })
        .map_err(|e| anyhow::anyhow!("registering apps channel: {e}"))?;

    event_loop
        .handle()
        .insert_source(nix_rx, |event, _, app| {
            if let channel::Event::Msg(event) = event {
                app.on_nix_event(event);
            }
        })
        .map_err(|e| anyhow::anyhow!("registering nix channel: {e}"))?;

    event_loop
        .handle()
        .insert_source(thumb_rx, |event, _, app| {
            if let channel::Event::Msg(event) = event {
                app.on_thumb(event);
            }
        })
        .map_err(|e| anyhow::anyhow!("registering thumbs channel: {e}"))?;

    event_loop
        .handle()
        .insert_source(paste_rx, |event, _, app| {
            if let channel::Event::Msg(text) = event {
                app.on_paste(&text);
            }
        })
        .map_err(|e| anyhow::anyhow!("registering paste channel: {e}"))?;

    if let Some(notif_rx) = notif_rx {
        event_loop
            .handle()
            .insert_source(notif_rx, |event, _, app| {
                if let channel::Event::Msg(ev) = event {
                    app.on_notif_event(ev);
                }
            })
            .map_err(|e| anyhow::anyhow!("registering notif channel: {e}"))?;
    }

    if let Some(clip_rx) = clip_rx {
        event_loop
            .handle()
            .insert_source(clip_rx, |event, _, app| {
                if let channel::Event::Msg(ev) = event {
                    app.on_clip_event(ev);
                }
            })
            .map_err(|e| anyhow::anyhow!("registering clipboard channel: {e}"))?;
    }

    if let Some(clip_thumb_rx) = clip_thumb_rx {
        event_loop
            .handle()
            .insert_source(clip_thumb_rx, |event, _, app| {
                if let channel::Event::Msg(ev) = event {
                    app.on_clip_thumb(ev);
                }
            })
            .map_err(|e| anyhow::anyhow!("registering clip-thumb channel: {e}"))?;
    }

    if let Some(clip_unfurl_rx) = clip_unfurl_rx {
        event_loop
            .handle()
            .insert_source(clip_unfurl_rx, |event, _, app| {
                if let channel::Event::Msg(ev) = event {
                    app.on_unfurl(ev);
                }
            })
            .map_err(|e| anyhow::anyhow!("registering clip-unfurl channel: {e}"))?;
    }

    if let Some(notif_icon_rx) = notif_icon_rx {
        event_loop
            .handle()
            .insert_source(notif_icon_rx, |event, _, app| {
                if let channel::Event::Msg(res) = event {
                    app.on_notif_icon(res);
                }
            })
            .map_err(|e| anyhow::anyhow!("registering notif-icon channel: {e}"))?;
    }

    if let Some(dict_rx) = dict_rx {
        event_loop
            .handle()
            .insert_source(dict_rx, |event, _, app| {
                if let channel::Event::Msg(ev) = event {
                    app.on_dict_loaded(ev);
                }
            })
            .map_err(|e| anyhow::anyhow!("registering dict channel: {e}"))?;
    }

    // Live launcher reload: rescan when a `.desktop` file is added, removed,
    // or rewritten in any XDG application dir, so launcher changes (webapps-gen
    // regenerating from ~/.config/webapps.list, or an external package install)
    // appear without restarting the daemon. Level-triggered inotify on the one
    // event loop — no extra thread. A fresh rescan also clears the negative
    // icon cache so newly added icons resolve.
    if let Some(watch_fd) = apps::watch_application_dirs() {
        let source = Generic::new(watch_fd, Interest::READ, Mode::Level);
        if let Err(e) =
            event_loop
                .handle()
                .insert_source(source, |_readiness, fd, app: &mut App| {
                    apps::drain_inotify(fd.as_fd());
                    app.indexer.request_rescan_fresh();
                    Ok(PostAction::Continue)
                })
        {
            warn!("cannot watch application dirs for live reload: {e}");
        }
    }

    // Intellihide: watch Hyprland window events so the dock can stay
    // up while nothing overlaps its zone, plus a steady poll — this
    // Hyprland emits no event for float toggles or float moves/resizes,
    // so events alone can never catch every layout change. Optional:
    // without Hyprland IPC the dock simply always auto-hides.
    // A 1 s clock tick for the OPTIONS time pill (redraws only on change).
    if app.options_layer.is_some() {
        if let Err(e) = event_loop.handle().insert_source(
            Timer::from_duration(Duration::from_secs(1)),
            |_, _, app: &mut App| {
                if app.tick_options_clock() {
                    app.sync_options_input();
                    app.draw_options();
                }
                TimeoutAction::ToDuration(Duration::from_secs(1))
            },
        ) {
            warn!("options clock timer failed: {e}");
        }
    }

    // Watch Hyprland window events when the dock's intellihide or the OPTIONS
    // topbar (colour-match + window pill) needs them.
    if app.config.input.intellihide || app.options_layer.is_some() {
        match hypr::subscribe(&event_loop.handle()) {
            Ok(()) => {
                app.on_layout_changed();
                // The steady zone poll is intellihide-only (float moves emit no
                // event); the bar colour-match runs its own self-healing poll
                // (started from `reeval_options_bar`).
                if app.config.input.intellihide {
                    if let Err(e) = event_loop.handle().insert_source(
                        Timer::from_duration(ZONE_POLL_INTERVAL),
                        |_, _, app: &mut App| {
                            app.on_layout_changed();
                            TimeoutAction::ToDuration(ZONE_POLL_INTERVAL)
                        },
                    ) {
                        warn!("zone poll timer failed ({e}); intellihide is event-driven only");
                    }
                }
            }
            Err(e) => warn!("Hyprland IPC unavailable: {e:#}"),
        }
    }

    // Declarative installs: the package list is the source of truth. Seed
    // it once from the managed set (the migration from the old imperative
    // `nix profile` era), then adopt any attr in the list that the managed
    // cache is missing — covers a hand-edited list or a daemon killed
    // mid-install (the root helper's rebuild completes independently, so on
    // restart the app is simply present and the cache catches up).
    applier::seed_if_missing(&app.managed.all_attrs());
    app.managed.adopt_list(&applier::list_attrs());
    app.recompute_removable();
    // Surface the Recycle Bin on the dock the first time (a one-shot pin; the
    // user can move or unpin it freely afterwards — we never re-pin it).
    app.pin_trash_once();

    info!("daemon up; try: waverunner-ctl toggle");
    while !app.exit {
        event_loop
            .dispatch(None, &mut app)
            .context("event loop dispatch")?;
    }
    info!("layer surface closed, exiting");
    Ok(())
}

/// Everything the event loop mutates: Wayland state objects, the
/// renderer, and the UI state machine.
pub struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,

    conn: Connection,
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    compositor: CompositorState,
    layer: LayerSurface,
    renderer: Option<Renderer>,
    /// The "OPTIONS" topbar surface (a near-transparent top-edge strip), its
    /// renderer, and its logical size from `configure`.
    options_layer: Option<LayerSurface>,
    options_renderer: Option<Renderer>,
    options_size: (u32, u32),
    /// Smart-gaps colour-match: when a maximized window is flush under the
    /// bar, the bar is painted this sampled colour (opaque); `None` = the
    /// default transparent strip. See [`screencopy`].
    options_bar_matched: Option<[f32; 4]>,
    /// The bar's own frosted (wallpaper-tinted) colour, sampled while the bar is
    /// *transparent* and the notification drawer is open, so the box can
    /// continue the pill's colour instead of a flat slab. See [`screencopy`].
    options_pill_color: Option<[f32; 4]>,
    /// The output + row to sample for the current match (if any).
    options_match: Option<screencopy::CaptureTarget>,
    /// An in-flight screencopy of the focused output.
    capture: Option<screencopy::Capture>,
    /// Whether a resample timer is already queued.
    options_poll_pending: bool,
    /// wlr-screencopy manager + shm plumbing for the colour sampling.
    screencopy: Option<ZwlrScreencopyManagerV1>,
    shm: Option<Shm>,
    shm_pool: Option<RawPool>,
    /// OPTIONS content (pills): which surface the pointer is on, its position,
    /// the hovered pill, and the data the pills show.
    pointer_surface: options::PointerSurface,
    options_ptr: Option<(f32, f32)>,
    options_hover: Option<options::PillId>,
    options_clock: String,
    /// Full date shown when the clock pill is hovered ("Friday, 31 July 2026").
    options_date: String,
    options_title: Option<String>,
    options_active_addr: Option<String>,
    /// Measured (logical px) widths of the clock, date, and window-title text,
    /// so the proportional-font pills can be sized without re-measuring every
    /// frame.
    options_clock_w: f32,
    options_date_w: f32,
    options_title_w: f32,
    /// Fullscreen auto-hide: whether the focused window is fullscreen, whether
    /// the bar is currently concealed, and the dwell/grace timers that reveal
    /// it on a deliberate top-edge hold.
    options_fullscreen: bool,
    options_hidden: bool,
    options_reveal_deadline: Option<Instant>,
    options_hide_deadline: Option<Instant>,
    /// Reveal animation state for the window control buttons.
    options_ctrl: options::CtrlAnim,
    /// Clock↔date "metamorphosis": the pill grows horizontally on hover and
    /// crossfades HH:MM into the full date, holding 3s after leave.
    options_clock_meta: options::ClockMeta,
    /// Notification OPTION: bell + peek + history dropdown (see [`crate::notif`]).
    notif: notif::NotifState,
    /// Clipboard OPTION: watched history + copy-back (see [`crate::clipboard`]).
    clip: clipboard::ClipState,
    /// Sender for the offline dictionary load worker (clipboard "define a word"
    /// panel). `None` when the topbar is disabled; the load is kicked lazily the
    /// first time the panel opens, and the finished map arrives on the loop.
    dict_tx: Option<channel::Sender<dict::Event>>,
    /// Off-thread resolver turning a notification's icon hint into a card-tile
    /// mip chain (`None` when the topbar is disabled).
    notif_icons_handle: Option<notif_icons::NotifIcons>,
    /// Resolved notif-icon layers uploaded to the OPTIONS renderer's own icon
    /// array; `chains[i]` is texture layer `i`. Re-uploaded wholesale as new
    /// icons resolve (the set is small — one per distinct notifying app).
    notif_icon_chains: Vec<Vec<u8>>,
    /// Icon-hint key → its layer index in `notif_icon_chains`.
    notif_icon_slot: HashMap<String, u32>,
    /// Icon hints already requested from the resolver, so a redraw before the
    /// reply lands doesn't re-queue them.
    notif_icon_pending: HashSet<String>,

    ui: UiState,
    config: Config,
    /// Icon-size level (0–3). Ctrl+Plus/Minus cycles through ICON_SCALES.
    icon_size: usize,
    /// Current buffer size in physical pixels, from `configure`.
    buffer_size: (u32, u32),
    scale_factor: i32,
    /// Timestamp of the previous drawn frame, for dt computation.
    last_frame: Option<Instant>,
    /// True while a frame callback is in flight (avoid double-requesting).
    frame_pending: bool,
    /// Scene changed since the last draw; the pending frame callback
    /// (if any) will redraw.
    dirty: bool,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// cursor-shape-v1: the manager (None when the compositor lacks the
    /// protocol), the live pointer's shape device, the serial of its
    /// last Enter, and the last shape sent (dedupe per change).
    cursor_shape: Option<CursorShapeManager>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    enter_serial: u32,
    cursor_now: Option<Shape>,
    /// AGUA water bodies (1.0 at rest): the card silhouette, the dock
    /// icons, and the box content each chase the card's motion at their
    /// own tempo — integrated in `draw`, sloshing after the landing.
    agua_card: animation::Follower,
    agua_icons: animation::Follower,
    agua_content: animation::Follower,
    agua_breath: animation::Follower,
    /// Edge-spring jelly membrane for the main card: wobbles when the
    /// pointer crosses the card boundary (anticipation pre-kick, main kick,
    /// and Poisson cross-coupling, all fired as delayed velocity injections
    /// so the membrane feels like it has propagation inertia).
    jelly: jelly::JellyMembrane,
    /// Same jelly membrane for the open group box panel.
    box_jelly: jelly::JellyMembrane,
    /// Whether the pointer was inside the open box last motion event.
    pointer_inside_box: bool,
    /// AGUA splash ripple surface across the dock: per-icon wave height
    /// and velocity (0 = flat), plus last frame's per-icon crest so a
    /// collapsing crest can splash the surface. Resized with the dock.
    dock_wave_h: Vec<f32>,
    dock_wave_v: Vec<f32>,
    dock_crest_prev: Vec<f32>,
    /// Held keyboard modifiers (Ctrl+V pastes into the query).
    modifiers: Modifiers,
    /// Set only for the duration of a middle-click activation: force a
    /// fresh instance of a running app instead of activating it (the
    /// pointer-native equivalent of macOS's Cmd+click / Ctrl+click).
    force_new_instance: bool,
    /// Clipboard: the data-device manager and the seat's device (None
    /// when the compositor lacks the protocol), plus the channel paste
    /// threads answer on.
    data_device_manager: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,
    paste_tx: channel::Sender<String>,
    /// Deadline of the pending auto-hide, if the pointer has left the
    /// dock. Re-entry clears it, invalidating the in-flight timer.
    hide_deadline: Option<Instant>,
    /// A box close is settling to the dock and should rest a beat, then
    /// hide, once the collapse animation finishes.
    rest_hide_pending: bool,
    /// Window that had focus when the box grabbed the keyboard — focus
    /// returns here on a plain close. Cleared on launch (the app takes
    /// focus) and on external focus loss (the user chose another window).
    restore_window: Option<String>,
    /// Refocus deferred until the box settles. Under `follow_mouse` the
    /// compositor won't route the keyboard to a window while our layer
    /// still covers the pointer, and it only re-evaluates on real pointer
    /// motion — so we dispatch the focus only once the card has shrunk out
    /// from under the cursor and stopped committing frames.
    pending_refocus: Option<String>,
    /// Set when we just launched an app: the next window to map (within a
    /// grace window) is focused, since focus-follows-mouse won't hand
    /// keyboard focus to an app whose window opens away from the cursor.
    focus_launched: Option<Instant>,
    /// Last keyboard-interactivity value sent to the compositor.
    interactive: bool,
    /// Last input-region extent sent to the compositor.
    input_extent: Option<u32>,

    /// Discovered applications and home folders (icon texture layers
    /// are aligned with this order), plus transient file-search result
    /// entries appended past `base_len` while a query is live.
    entries: Vec<AppEntry>,
    /// What each entry is (app / file / icon asset), aligned with `entries`.
    kinds: Vec<apps::EntryKind>,
    /// Texture layer per entry: its own index for indexed entries, a
    /// generic asset layer for transient file-search results.
    icon_layers: Vec<u32>,
    /// Length of the indexed (non-transient) prefix of `entries`.
    base_len: usize,
    /// Texture layers of the generic folder/file/package icons, with
    /// their placeholder flags.
    /// Icon-carrier assets by id ("asset-folder", "asset-audio", …):
    /// (texture layer, letter-tile placeholder flag), one per scan.
    assets: HashMap<String, (u32, bool)>,
    /// Thumbnailer thread handle.
    thumbs: thumbs::Thumbs,
    /// Finished thumbnails by path: reserved-slot index + pixels (kept
    /// for re-upload after a rescan rebuilds the texture array).
    thumb_map: HashMap<String, (usize, Vec<u8>)>,
    /// Round-robin cursor over the reserved thumbnail slots.
    thumb_next: usize,
    /// Paths with a thumbnail job in flight (request deduplication).
    thumb_pending: HashSet<String>,
    /// Files classified as video by extension but found to be audio-only
    /// (no video stream): shown with the audio icon, never re-requested.
    audio_paths: HashSet<String>,
    /// Handle to the nix threads (package index + profile mutations).
    nix: nix::Nix,
    /// Top-ranked packages for `pkg_hits_query`, delivered async by the
    /// nix thread; the Install section renders these.
    pkg_hits: Vec<nix::PkgEntry>,
    /// The query `pkg_hits` answers — `""` is the recommendations
    /// storefront, `None` means no answer yet. Stale hits keep showing
    /// until the fresh rank lands (no flicker between keystrokes).
    pkg_hits_query: Option<String>,
    /// Rasterized icons for `pkg_hits` (aligned), uploaded into the
    /// texture array's reserved tail; and their letter-tile flags.
    pkg_hit_icons: Vec<Vec<u8>>,
    pkg_hit_placeholders: Vec<bool>,
    /// First reserved texture layer: the app icon count from the last
    /// `set_icons` upload.
    pkg_layer_base: u32,
    /// Whether the package index is usable yet (drives the Install hint).
    pkg_state: PkgIndexState,
    /// Entry ids (package attrs / desktop ids) with a profile mutation
    /// in flight; their cells render dimmed and ignore input.
    busy_ids: HashSet<String>,
    /// Recently failed mutations: their cells flash "Failed" for
    /// [`FAIL_FLASH`] (details go to the log).
    failed_ids: HashMap<String, Instant>,
    /// Attrs being realized for an ephemeral "try it" run (drag a package
    /// out of the box). Their Install cell shows "Launching…" until the
    /// build lands; terminal-vs-GUI is decided from the built package
    /// (see [`nix::Event::Realized`]), then the run is spawned.
    launching: HashSet<String>,
    /// The waverunner-managed home-manager package list (source of truth
    /// for what the launcher installed, and the generator of
    /// `waverunner-packages.nix`). Drives removable/installed detection.
    managed: managed::ManagedDb,
    /// Webapps installed from the catalog (dragged to the grid). Catalog
    /// entries not in this set stay in the Install section; members show on
    /// the grid. Generator of `waverunner-webapps.nix`.
    managed_webapps: managed_webapps::ManagedWebapps,
    /// Catalog slugs marked as storefront recommendations (`*` in
    /// webapps.list) — shown in the Install section on an empty query; the
    /// rest only appear when searched.
    webapp_recommended: std::collections::HashSet<String>,
    /// Ids of the currently-indexed apps that are removable — the apps
    /// whose attr is in [`Self::managed`], recomputed whenever apps or
    /// the managed list change. Only these offer the Install section as
    /// an uninstall drop target.
    removable_ids: HashSet<String>,
    /// Desktop ids of every installed app, refreshed on rescan — matched
    /// against a package's `.desktop` stems / attr to drop it from the
    /// Install list once it's installed.
    installed_app_ids: HashSet<String>,
    /// Home-tree file index the search ranks against (fresh per rescan).
    file_index: Vec<apps::FileEntry>,
    /// Directory the Files section is navigated into (`None` = the
    /// top-level home-folder strip).
    files_dir: Option<std::path::PathBuf>,
    /// Persistent app groups ("boxes") shown in the Apps grid.
    groups: groups::GroupDb,
    /// Group the Apps section is navigated into (index into `groups`).
    app_group: Option<usize>,
    /// Group whose dock-folder stack popover is open (index into `groups`).
    dock_stack: Option<usize>,
    /// A pinned directory whose content stack is open above the dock.
    dir_stack: Option<boxes::DirStack>,
    /// Whether the currently open box was opened *from the dock* (a dock
    /// folder or pinned directory), as opposed to a grid folder tile. Arms
    /// the dock hover-switch even when the box opened into the grid.
    box_from_dock: bool,
    /// In-box member reorder drag: the dragged member's entry index and the
    /// pointer position. The box stays open; drop reorders it (or, dragged
    /// out of the box, it converts to a pull-out into the grid).
    box_drag: Option<(usize, (f32, f32))>,
    /// Dwell timer for edge-paging while reordering in the box.
    box_drag_page_at: Option<Instant>,
    /// Which 3×3 page of the open box is the scroll target.
    box_page: usize,
    /// Horizontal page-scroll of the open box — the same [`pager::Pager`]
    /// the grid sections use (slide, wheel accumulation, page turns).
    box_pager: pager::Pager,
    /// Per-member animated display slots of the open box (entry index →
    /// display position, the box's make-room glide — the counterpart of
    /// `apps_slide`); pruned to the current members each frame.
    box_slide: Vec<(usize, f32)>,
    /// Group cells for the renderer: (transient entry index, member
    /// texture layers for the 2×2 mini preview). Rebuilt per refilter.
    group_minis: Vec<(usize, [Option<u32>; 4])>,
    /// Persistent Apps-grid order (install date + manual overrides).
    order: order::OrderDb,
    /// Packages dropped into the grid and installing in place. Rendered
    /// as grid tiles at their drop slots; finalized (or retried) when the
    /// install resolves.
    pending_installs: Vec<PendingInstall>,
    /// Webapps whose install progress ring is playing: already placed on the
    /// grid, this just drives the same ring for a fake ~4 s "build" so they
    /// land with the same feel as a package (see [`install::PendingWebapp`]).
    pending_webapps: Vec<crate::install::PendingWebapp>,
    /// Apps that finished installing while the launcher was closed: temp-
    /// pinned to the dock with a one-shot shine (never persisted, hidden from
    /// the grid) until opened and closed once (see [`install::InstallNotify`]).
    install_notify: Vec<crate::install::InstallNotify>,
    /// When a notify-revealed dock should auto-hide again (it popped up just
    /// to say "your app is ready"). Cleared/ignored if the user engages.
    notify_dock_hide_at: Option<Instant>,
    /// Managed-install attrs fired via `start_managed_install` (dock-drop
    /// and startup recovery) that have no grid tile.  Resolved to dock
    /// pins in `resolve_pending_installs` once the app appears in the index.
    managed_install_attrs: Vec<String>,
    /// App ids present at the last app rescan — diffed against the next
    /// scan so a pending install can resolve to the app that *newly
    /// appeared*, even when its desktop id differs from the attr and the
    /// package index carried no desktop hints (e.g. `chromium`).
    known_app_ids: HashSet<String>,
    /// Ids of the synthetic CLI-tool tiles in the current scan
    /// ([`apps::LoadedApps::cli_ids`]). Excluded when matching a pending
    /// install to its real app so a wrapped package never resolves to its
    /// own placeholder tile.
    cli_ids: HashSet<String>,
    /// Uninstalls in flight: app desktop id → the managed attr being
    /// removed. The cache entry and dock pin are dropped only once the
    /// rebuild succeeds ([`nix::Event::Done`]); on failure they stay, so the
    /// cache never diverges from the package list. Also the authoritative
    /// install-vs-uninstall discriminator for `Done`.
    uninstalling: HashMap<String, String>,
    /// An app that just resolved from a pending install: it gets a
    /// launch-style bounce as it lands in the grid (set during rescan,
    /// consumed once its cell index is known).
    just_installed: Option<String>,
    /// When the pointer settled onto the current dock icon — the name
    /// tooltip appears once this passes [`DOCK_TOOLTIP_DELAY`]. `None`
    /// when the pointer isn't on a dock icon.
    dock_hover_since: Option<Instant>,
    /// Recycle-bin reaction, eased 0→1 while an app (or box) is being
    /// dragged: the bin tile reddens and its lid opens, inviting a drop.
    /// Eased back to 0 (lid shuts, red fades) when the drag ends.
    trash_react: f32,
    /// Recycle-bin hover, eased 0→1 while a dragged icon is over the bin as a
    /// drop target: the bin swells to acknowledge the drop it would accept.
    trash_hover: f32,
    /// Drag-to-reorder: the make-room gap's display slot (`None` = no
    /// grid drag in flight).
    reorder_slot: Option<usize>,
    /// Dwell timer for edge-paging the Apps grid while a drag hovers its
    /// left/right edge — carries the dragged icon to another page (same
    /// feel as the open-box drag paging).
    grid_drag_page_at: Option<Instant>,
    /// Per-cell animated display indices for the Apps grid (the
    /// make-room glide); identity when nothing is in flight.
    apps_slide: Vec<f32>,
    /// Display slot of each visible Apps item (parallel to
    /// `search.visible[SECTION_APPS]`): `page * cap + within`. Within a
    /// page slots are dense from the page start; gaps only exist at page
    /// tails (Launchpad model). Identity while searching.
    apps_slots: Vec<usize>,
    /// Display page → storage page (index into `order.pages()`); pages
    /// whose members are all hidden are skipped on screen.
    apps_page_map: Vec<usize>,
    /// Cells per Apps page (cols × rows) as of the last refilter.
    apps_cap: usize,
    /// One past the last occupied display slot (0 = empty grid) — the
    /// Apps page count is derived from this, not the item count, so
    /// tail gaps still count toward their page.
    apps_span: usize,
    /// The id of an app just dropped by a drag: on the next refilter it
    /// starts at rest in its chosen cell instead of carrying its old
    /// animated position, so the icon lands where it was dropped rather
    /// than gliding there from its origin. One-shot (cleared on use).
    just_dropped: Option<String>,
    /// Grabbing an icon out of the Install section holds the Apps grid and
    /// Files in their resting layout even while a query is live, so the
    /// full grid is on screen to drop the new icon onto the exact slot you
    /// want. Set when a drag starts from Install, cleared when the user
    /// next edits the query (i.e. resumes searching).
    install_drag_reset: bool,
    /// Whether the previous refilter rendered the grid in its resting
    /// layout (see `grid_resting`) — a flip resets the Apps page like
    /// entering/leaving a search does.
    prev_resting_grid: bool,
    /// Animated displacement per dock slot, in slot units (the dock's
    /// make-room glide during drags).
    dock_slide: Vec<f32>,
    /// Magnification sleeps until this instant after a drop (the
    /// placement must be still; the magnify wave returns a beat later).
    mag_sleep: Option<Instant>,
    /// Magnification amplitude envelope (0 = off, 1 = full): fades out
    /// around drags/drops and fades back in — never pops.
    mag_amount: f32,
    /// Group-open transition: raw progress (0..1), its target, and the
    /// surface point the group expands from (the clicked tile).
    group_anim: f32,
    group_anim_target: f32,
    group_origin: Option<(f32, f32)>,
    /// Members (entry indices) of a box that's animating closed after a
    /// member was dragged out: the box keeps shrinking (via `group_anim`
    /// toward 0) over the live grid even though `app_group` is already
    /// cleared. Cleared when the shrink lands.
    closing_members: Option<Vec<usize>>,
    /// Icons that arrived before the renderer existed.
    pending_icons: Option<Vec<Vec<u8>>>,
    /// Item currently under the pointer.
    hover: Option<Hit>,
    /// Pointer position in surface coordinates, while inside.
    pointer_pos: Option<(f32, f32)>,
    /// Tracks whether the pointer was inside the card last motion event,
    /// so we can fire a ripple on edge-crossing.
    pointer_inside_card: bool,
    /// Grid scrolling / paging state.
    scroll: ScrollState,
    /// Press-and-drag gesture state.
    gesture: GestureState,
    /// Search box and query state.
    search: SearchState,
    /// Handle to the background indexer thread.
    indexer: apps::Indexer,
    /// Persistent launch-frequency database; drives sort order.
    usage: usage::UsageDb,
    /// Pinned app IDs in user-defined dock order.
    pins: pins::PinDb,
    /// Dock display order: maps slot index → entry index. Rebuilt
    /// whenever entries are loaded or pins change.
    dock_order: Vec<usize>,
    /// Running apps (macOS dock model): entry index → its live window
    /// addresses, most-recently-used first. Presence ⇒ the app is
    /// running (shows the indicator dot; a click activates instead of
    /// launching). Rebuilt from Hyprland on window open/close.
    running: HashMap<usize, Vec<String>>,
    /// Dock slot at which the running-but-unpinned zone begins (the
    /// divider position), or `None` when no such apps are shown. Slots
    /// `[divider..]` are ephemeral running apps that vanish on quit.
    dock_divider: Option<usize>,
    /// When the last rescan was requested, for the reveal cooldown.
    last_rescan: Instant,
    /// A launch bounce in flight: (entry index, start time).
    bounce: Option<(usize, Instant)>,
    /// Which entries have placeholder tiles (no resolved icon).
    placeholders: Vec<bool>,
    /// Intellihide: no window currently overlaps the dock zone, so the
    /// dock parks visible instead of auto-hiding.
    zone_free: bool,

    exit: bool,
}

/// Readiness of the nixpkgs package index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PkgIndexState {
    /// The nix thread is still loading or dumping the index.
    Loading,
    /// The index is searchable (rank queries get answers).
    Ready,
    /// No cache and the dump failed; package search is unavailable.
    Failed,
}

/// Grid scrolling and paging state.
#[derive(Default)]
struct ScrollState {
    /// Accumulated vertical scroll over the dock, for the expand gesture.
    accum: f64,
    /// Set when a scroll gesture triggers Expand. Events within
    /// EXPAND_BLEED_COOLDOWN are eaten so the gesture doesn't bleed
    /// through and advance a page. Cleared immediately by AxisStop so
    /// keyboard Toggle has zero cooldown.
    open_at: Option<Instant>,
    /// Per-section paging state — every popup section scrolls
    /// independently (the open box carries its own [`pager::Pager`]).
    per: [pager::Pager; content::N_SECTIONS],
}

impl ScrollState {
    /// Reset every section to page 0 and clear paging accumulators.
    fn reset_sections(&mut self) {
        self.per = Default::default();
    }
}

/// Press-and-drag gesture state.
#[derive(Default)]
struct GestureState {
    /// Item a left-button press started on; release on the same item
    /// activates it.
    pressed: Option<Hit>,
    /// Surface position where the current button press started; used to
    /// detect when a press-move crosses the drag threshold.
    press_pos: Option<(f32, f32)>,
    /// Active drag-and-drop, if any.
    dragging: Option<DragState>,
}

/// Search box and query state.
struct SearchState {
    /// Live query (empty = unfiltered).
    query: String,
    /// Fuzzy matcher (heap-heavy, allocated once per daemon).
    matcher: Searcher,
    /// Indices into `entries` shown per section, ranked by the query
    /// (every entry in order when the query is empty). Apps land in the
    /// Apps section, home folders in Files; Install is empty for now.
    visible: [Vec<usize>; content::N_SECTIONS],
    /// Auto-selected position (best match while searching) as a flat
    /// index across the sections in order; Enter launches it.
    selected: Option<usize>,
    /// Whether the search box is expanded (vs. compact circle button).
    open: bool,
    /// Animated expansion of the search widget: 0.0 = button, 1.0 = pill.
    expand: f32,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            matcher: Searcher::new(),
            visible: Default::default(),
            selected: None,
            open: false,
            expand: 0.0,
        }
    }
}

/// State of an in-progress drag-and-drop gesture.
struct DragState {
    /// Which entry is being moved (index into `App::entries`).
    entry_idx: usize,
    /// True when the drag started from a dock slot; releasing outside
    /// the dock zone then unpins the entry.
    from_dock: bool,
    /// Current pointer position in surface coordinates.
    pos: (f32, f32),
}

/// The box a click would open. Resolved from a [`content::Hit`] while a
/// box is already open, so a click on a *different* folder switches
/// straight to it (close + open in one click) while a click on the open
/// box's own icon toggles it shut.
enum BoxTarget {
    /// A group folder, by group index (grid cell or dock folder icon).
    Group(usize),
    /// A pinned directory's content stack, by the dock entry's id.
    Dir(String),
}

/// Accumulated scroll (in wl_pointer axis units; one wheel notch ≈ 15)
/// needed to trigger the dock-expand / popup-collapse gesture.
const SCROLL_THRESHOLD: f64 = 10.0;

/// How far (logical px) the OPTIONS bar overhangs the window's top edge while
/// colour-matched, to paint over the window's top border and kill the seam.
/// Kept minimal (just the ~1px border) so the colour is still sampled from the
/// window's toolbar just below it, not from content deeper down.
const OPTIONS_OVERHANG: u32 = 2;
/// Extra transparent height below the OPTIONS bar, reserved on the surface (not
/// the exclusive zone) to host the notification history dropdown — same
/// fixed-surface / animate-content discipline as the dock. Desktop layout is
/// untouched (Zero Layout Shift); the area is click-through until expanded.
pub(crate) const OPTIONS_DROPDOWN_H: u32 = 480;

/// After the box closes, the dock rests this long before it hides — a
/// brief beat parked as a dock instead of vanishing straight away.
const DOCK_REST_AFTER_CLOSE: Duration = Duration::from_millis(650);

/// Magnification blackout after a drop: the placement stays perfectly
/// still for this long before the magnify wave may return.
const MAG_SLEEP_AFTER_DROP: Duration = Duration::from_secs(1);

/// How long scroll events are eaten after a scroll gesture expands the
/// dock: the events of that same gesture keep arriving in the Open state
/// and must not page the grid. Cleared early by AxisStop (finger lift).
const EXPAND_BLEED_COOLDOWN: Duration = Duration::from_millis(300);

/// Linux evdev code for the left mouse button.
const BTN_LEFT: u32 = 0x110;

/// Linux evdev code for the right mouse button.
const BTN_RIGHT: u32 = 0x111;

/// Linux evdev code for the middle mouse button — the Linux-native
/// "open a new instance" gesture (the dock is pointer-only, so a
/// keyboard modifier like macOS's Cmd can't be read at click time).
const BTN_MIDDLE: u32 = 0x112;

/// Minimum time between app-index rescans. Summoning the dock checks
/// freshness; mashing toggle does not scan repeatedly.
const RESCAN_COOLDOWN: Duration = Duration::from_secs(30);

/// Steady re-poll interval for the dock zone (two tiny local-socket
/// queries): float toggles/moves/resizes and the re-tiling they cause
/// emit no Hyprland event, so polling is the only reliable signal. The
/// render loop stays fully idle between polls.
const ZONE_POLL_INTERVAL: Duration = Duration::from_millis(800);

/// Duration of the launch bounce (two decaying hops), after which the
/// card hides.
const BOUNCE_DURATION: Duration = Duration::from_millis(550);
/// Peak height of the launch bounce, in logical pixels.
const BOUNCE_HEIGHT: f32 = 18.0;
/// Hover dwell before a dock icon's name tooltip appears.
const DOCK_TOOLTIP_DELAY: Duration = Duration::from_millis(600);

impl App {
    /// Current icon scale multiplier (driven by `icon_size`).
    pub fn icon_scale(&self) -> f32 {
        content::ICON_SCALES[self.icon_size]
    }

    /// Logical surface dimensions for the current icon_size.
    /// DRAG_MARGIN_X/TOP are fixed; only the card area scales.
    fn scaled_surface_size(&self) -> (u32, u32) {
        let s = self.icon_scale();
        let w = (self.config.window.width as f32 * s).round() as u32
            + 2 * content::DRAG_MARGIN_X as u32;
        let h = ((self.config.window.height + self.config.window.bottom_margin) as f32 * s).round()
            as u32
            + content::MAGNIFY_HEADROOM as u32
            + content::DRAG_MARGIN_TOP as u32;
        (w, h)
    }

    /// UiState dock / full extents for the current icon_size.
    fn scaled_extents(&self) -> (f32, f32) {
        let s = self.icon_scale();
        let full = (self.config.window.height + self.config.window.bottom_margin) as f32 * s;
        let dock =
            (self.config.window.input_bar_height + self.config.window.bottom_margin) as f32 * s;
        (dock, full)
    }

    /// Resize the Wayland surface and update animation extents after an icon_size change.
    fn apply_icon_size_change(&mut self) {
        let (dock_extent, full_extent) = self.scaled_extents();
        self.ui.set_extents(dock_extent, full_extent);
        let (w, h) = self.scaled_surface_size();
        self.layer.set_size(w, h);
        self.layer.wl_surface().commit();
        save_icon_size(self.icon_size);
    }

    /// Entry point for IPC commands (called from ipc.rs) and for
    /// internally generated commands (Escape, focus loss, scroll).
    pub fn handle_command(&mut self, command: Command) {
        // Debug/verification verbs force-open an OPTIONS surface (clipboard /
        // notification box) so it can be screenshotted — they don't touch the
        // launcher rest-state machine, so handle and return before `ui.apply`.
        match command {
            Command::DebugClip => {
                self.open_clip_box();
                return;
            }
            Command::DebugClipDetail => {
                self.open_clip_box();
                self.open_clip_detail(0);
                return;
            }
            Command::DebugNotif => {
                self.open_notif_box();
                return;
            }
            Command::DebugDict => {
                self.open_clip_box();
                self.open_dict();
                // A word in both dictionaries, to exercise the bilingual answer.
                self.clip.dict_query = "pie".to_owned();
                return;
            }
            _ => {}
        }
        // Summoning or expanding is the moment freshness matters:
        // rescan (coalesced, cooldown-limited) so newly installed and
        // uninstalled apps are reflected without a restart.
        if matches!(command, Command::Toggle | Command::Expand) {
            self.maybe_rescan();
        }
        let prev = self.ui.target();
        if self.ui.apply(command) {
            let next = self.ui.target();
            self.sync_surface_state();
            // A box close (Open→Dock) parks as a dock, then hides after a
            // beat (armed once the collapse settles, in the frame loop).
            // Rising back to Open cancels that.
            match next {
                Target::Dock if prev == Target::Open => self.rest_hide_pending = true,
                Target::Open => {
                    self.rest_hide_pending = false;
                    self.hide_deadline = None;
                }
                _ => {}
            }
            // Horizontal wave when the launcher card fully opens or starts closing.
            // Fires for both keyboard (Super+Space, Escape) and pointer paths since
            // all state transitions funnel through handle_command.
            let is_opening = prev != Target::Open && next == Target::Open;
            let is_closing = prev == Target::Open && next != Target::Open;
            if is_opening || is_closing {
                if let Some(renderer) = self.renderer.as_mut() {
                    let sw = self.config.window.width as f32 + 2.0 * content::DRAG_MARGIN_X;
                    let sh = self.config.window.height as f32
                        + self.config.window.bottom_margin as f32
                        + content::MAGNIFY_HEADROOM
                        + content::DRAG_MARGIN_TOP;
                    let (wx, wy) = self.pointer_pos.unwrap_or((sw * 0.5, sh));
                    renderer.record_box_wave(wx, wy);
                }
            }
            self.schedule_frame();
        }
    }

    /// Re-evaluate the dock zone against Hyprland's window layout
    /// (intellihide). Called on relevant compositor events and from the
    /// steady zone poll: float toggles, interactive float moves/resizes,
    /// and the re-tiling they cause all emit *no* Hyprland event
    /// (verified on socket2), so events alone can never be sufficient.
    fn on_layout_changed(&mut self) {
        // Window open/close/move all land here — refresh the running set
        // regardless of intellihide (the macOS dot + activate-on-click
        // need it even when the dock never dodges).
        self.refresh_running();
        // Update the window pill + fullscreen state first, then the colour
        // match (which depends on the fullscreen state).
        self.refresh_options_content();
        self.reeval_options_bar();
        if !self.config.input.intellihide {
            return;
        }
        // On IPC failure, keep the last known state — a transient socket
        // error must not flip zone_free to false and trigger a spurious dodge.
        let last_known = self.zone_free;
        let free = (|| -> anyhow::Result<bool> {
            let mon = hypr::focused_monitor()?;
            let zone_w = self.config.window.width as f64;
            let zone_h =
                (self.config.window.input_bar_height + self.config.window.bottom_margin) as f64;
            let zone = (
                mon.x + (mon.w - zone_w) / 2.0,
                mon.y + mon.h - zone_h,
                zone_w,
                zone_h,
            );
            Ok(!hypr::zone_state(zone, mon.active_ws)?.occupied)
        })()
        .unwrap_or_else(|e| {
            debug!("dock zone query failed: {e:#}");
            last_known
        });

        if free == self.zone_free {
            return;
        }
        debug!("dock zone free: {free}");
        self.zone_free = free;
        if free {
            // Zone cleared — cancel any pending dodge and reveal the dock
            // so it parks visible (classic macOS intellihide: dock returns
            // when the covering window is moved or closed).
            self.hide_deadline = None;
            if self.ui.target() == Target::Hidden {
                self.handle_command(Command::Show);
            }
        } else if self.ui.target() == Target::Dock && self.pointer_pos.is_none() {
            // A window moved in and the user isn't on the dock: dodge.
            self.handle_command(Command::Hide);
        }
    }

    /// A window just mapped: if we launched an app moments ago, give it
    /// keyboard focus (once). With focus-follows-mouse the compositor
    /// otherwise leaves it in the background when it opens off the cursor.
    fn on_window_opened(&mut self, addr: &str) {
        const FOCUS_LAUNCH_GRACE: Duration = Duration::from_secs(10);
        if self
            .focus_launched
            .is_some_and(|t| t.elapsed() < FOCUS_LAUNCH_GRACE)
        {
            self.focus_launched = None;
            hypr::focus_window(addr);
        }
    }

    /// Request an app-index rescan unless one was requested recently.
    fn maybe_rescan(&mut self) {
        if self.last_rescan.elapsed() >= RESCAN_COOLDOWN {
            self.last_rescan = Instant::now();
            self.indexer.request_rescan();
        }
    }

    /// The indexer thread finished: adopt the entries and upload icons
    /// (or stash them until the renderer exists).
    fn on_apps_loaded(&mut self, loaded: apps::LoadedApps) {
        info!("app index ready: {} entries", loaded.entries.len());

        // Sort by descending launch frequency so the most-used entries
        // appear first in both the dock and the unfiltered grids. Ties
        // preserve the alphabetical order that comes from the indexer.
        let mut combined: Vec<(
            waverunner_core::index::AppEntry,
            apps::EntryKind,
            Vec<u8>,
            bool,
        )> = loaded
            .entries
            .into_iter()
            .zip(loaded.kinds)
            .zip(loaded.icons)
            .zip(loaded.placeholders)
            .map(|(((e, k), i), p)| (e, k, i, p))
            .collect();
        combined.sort_by_key(|(e, _, _, _)| std::cmp::Reverse(self.usage.count(&e.id)));
        let mut kinds = Vec::with_capacity(combined.len());
        let mut icons = Vec::with_capacity(combined.len());
        let mut placeholders = Vec::with_capacity(combined.len());
        self.entries = combined
            .into_iter()
            .map(|(e, k, i, p)| {
                kinds.push(k);
                icons.push(i);
                placeholders.push(p);
                e
            })
            .collect();
        self.kinds = kinds;
        self.placeholders = placeholders;
        self.base_len = self.entries.len();
        self.icon_layers = (0..self.base_len as u32).collect();
        self.file_index = loaded.files;
        // Resolve every icon-carrier asset once per scan.
        self.assets = apps::ICON_ASSETS
            .iter()
            .filter_map(|(id, _)| {
                let i = self.entries.iter().position(|e| &e.id == id)?;
                Some((id.to_string(), (i as u32, self.placeholders[i])))
            })
            .collect();

        // Record first-seen order (install date): new apps append at
        // the end of the grid, macOS-style. The very first sync seeds
        // the baseline in the current (usage) order.
        self.order.sync(
            self.entries
                .iter()
                .zip(&self.kinds)
                .filter(|(_, k)| **k == apps::EntryKind::App)
                .map(|(e, _)| e.id.as_str()),
        );
        // Which of the freshly-loaded entries are synthetic CLI tiles —
        // resolve must exclude them so a wrapped package matches its real
        // `.desktop`, not its placeholder.
        self.cli_ids = loaded.cli_ids.into_iter().collect();
        // A drag-to-install app that has now landed: reposition it into
        // its tile's slot and retire the tile (before the refilter below
        // reads the order).
        self.resolve_pending_installs();

        self.pkg_layer_base = icons.len() as u32;
        match self.renderer.as_mut() {
            Some(renderer) => {
                renderer.set_icons(&icons);
                // set_icons rebuilt the array: restore the package-hit
                // icons into its reserved tail, then the still-pending
                // install icons into theirs.
                self.upload_pkg_icons();
                self.reupload_pending_icons();
                self.reupload_thumb_icons();
            }
            None => self.pending_icons = Some(icons),
        }
        // Indices may have shifted: drop any armed click or in-flight drag,
        // rebuild the dock order, re-rank the query, re-resolve hover.
        self.gesture.pressed = None;
        self.gesture.dragging = None;
        self.recompute_dock_order();
        // Entry indices just changed — re-match live windows to them.
        self.refresh_running();
        self.recompute_removable();
        // A finished uninstall was held hidden until its `.desktop` left the
        // index; now that the rescan reflects the removal, drop the hold for
        // any id no longer present (a still-pending uninstall keeps its).
        self.uninstalling
            .retain(|id, _| self.entries.iter().any(|e| &e.id == id));
        // Boxes left with fewer than two members after uninstalls are
        // deleted (and their grid slot forgotten) so nothing undersized
        // lingers. Guarded against a degenerate empty scan wiping every box.
        //
        // A package dropped into a box placeholds its slot under the raw
        // attr until its build finishes (`resolve_pending_installs` then
        // swaps in the real app id via `replace_member`). That attr is not
        // an installed app id yet, so it must be exempted here — otherwise
        // the placeholder member is pruned on the first rescan after the
        // drop, before the (slow, or failed) rebuild ever resolves. A
        // box-destined pending tile renders *only* as that member
        // (`insert_pending_cells` claims no grid cell for it), so pruning it
        // orphans the tile: the package vanishes from the box, the grid, and
        // never reaches packages.list. Keep live pending attrs alive.
        let mut exists = self.installed_app_ids.clone();
        exists.extend(self.pending_installs.iter().map(|p| p.attr.clone()));
        if !self.installed_app_ids.is_empty() && self.groups.prune(&exists) {
            let live: std::collections::HashSet<String> = self
                .groups
                .groups()
                .iter()
                .map(|g| format!("group:{}", g.id))
                .collect();
            self.order.forget_dead_boxes(&live);
        }
        self.refilter();
        // The app index just became available (or changed): re-measure the
        // notification rows so their icon hints resolve against it. The first
        // measure can run before the index loads (empty → monogram hints); this
        // recomputes and fires the resolver requests now that entries exist.
        if self.config.options.enabled {
            self.measure_notif();
        }
        // A just-installed app lands in the dock: bounce its icon in
        // (same as a launch), and if the dock was auto-hidden, flash it
        // into view for a beat so the arrival is seen, then let it hide.
        if let Some(app_id) = self.just_installed.take() {
            if let Some(idx) = self.entries.iter().position(|e| e.id == app_id) {
                self.bounce = Some((idx, Instant::now()));
            }
            if self.ui.target() == Target::Hidden {
                self.handle_command(Command::Show);
                self.schedule_hide_after(INSTALL_REVEAL);
            }
        }
        self.schedule_frame();
    }

    /// Re-rank entries against the current query, fanning matches into
    /// their sections best-first, with the top match overall
    /// auto-selected so Enter launches it. With a query, the Files
    /// section searches the whole home-tree file index (transient
    /// entries borrowing a generic icon); without one it shows the
    /// top-level home folders, most-used first.
    /// Whether entry `idx` is a catalog webapp that has NOT been installed
    /// (dragged to the grid) — those live in the Install section, not the
    /// grid. Installed webapps and all other apps return `false`.
    fn is_catalog_webapp(&self, idx: usize) -> bool {
        self.entries
            .get(idx)
            .and_then(|e| webapps::slug_of_id(&e.id))
            .is_some_and(|slug| !self.managed_webapps.contains(slug))
    }

    /// Whether the Apps grid (and Files) should render in their resting
    /// layout: no live query, or an icon was grabbed from the Install
    /// section (`install_drag_reset`) — the latter keeps the full grid on
    /// screen so the dragged install can be dropped on an exact slot even
    /// though the query is still live for the Install list itself.
    fn grid_resting(&self) -> bool {
        self.search.query.is_empty() || self.install_drag_reset
    }

    /// Whether entry `idx` is an app whose uninstall is in flight — hidden
    /// from the grid and dock the instant it's dropped on the Install
    /// section, so it vanishes at once instead of lingering "Removing…"
    /// through the rebuild. Restored only if the rebuild fails.
    fn is_removing(&self, idx: usize) -> bool {
        self.entries
            .get(idx)
            .is_some_and(|e| self.uninstalling.contains_key(&e.id))
    }

    /// Whether entry `idx` is a catalog webapp marked as a storefront
    /// recommendation (shown on an empty query).
    fn is_recommended_webapp(&self, idx: usize) -> bool {
        self.entries
            .get(idx)
            .and_then(|e| webapps::slug_of_id(&e.id))
            .is_some_and(|slug| self.webapp_recommended.contains(slug))
    }

    fn refilter(&mut self) {
        // Snapshot the Apps cells' animated display positions (by
        // entry id — indices won't survive the rebuild) so the new
        // list can pick them up seamlessly below.
        let old_slide: Vec<(String, f32)> = self.search.visible[content::SECTION_APPS]
            .iter()
            .enumerate()
            .filter_map(|(i, &e)| {
                let d = self.apps_slide.get(i).copied()?;
                Some((self.entries.get(e)?.id.clone(), d))
            })
            .collect();
        // Drop the previous refilter's transient entries.
        self.truncate_transients();

        let searching = !self.search.query.is_empty();
        // The grid rests either on an empty query or while an icon is being
        // dragged out of Install (query still live for the Install list).
        let resting_grid = self.grid_resting();
        // A flip into or out of the resting layout reshuffles the grid
        // wholesale — the carry-over glide must skip it, and the Apps page
        // resets like entering/leaving a search.
        let grid_flip = resting_grid != self.prev_resting_grid;
        self.prev_resting_grid = resting_grid;
        let names: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        let ranked = self.search.matcher.rank(&self.search.query, &names);
        let mut visible: [Vec<usize>; content::N_SECTIONS] = Default::default();
        // Install-section catalog-webapp hits, ranked by the live query. On
        // an empty query only the recommended few show; any webapp surfaces
        // once its name is searched.
        for &idx in &ranked {
            if self.kinds.get(idx) == Some(&apps::EntryKind::App)
                && self.is_catalog_webapp(idx)
                && (searching || self.is_recommended_webapp(idx))
            {
                visible[content::SECTION_INSTALL].push(idx);
            }
        }
        // Grid apps (real, non-catalog): a live search ranks them; a resting
        // grid shows them all in install/manual order (arranged below from
        // `self.order`), so a query held only for the Install list — the
        // drag-from-Install reset — still shows the whole grid to drop onto.
        let grid_ranked = if resting_grid && searching {
            self.search.matcher.rank("", &names)
        } else {
            ranked
        };
        for idx in grid_ranked {
            // The Files section is a live directory listing (home root or a
            // navigated folder), built below — File entries add nothing here.
            if self.kinds.get(idx) == Some(&apps::EntryKind::App)
                && !self.is_catalog_webapp(idx)
                && !self.is_removing(idx)
            {
                visible[content::SECTION_APPS].push(idx);
            }
        }
        // Blend the webapp hits with the ranked package hits into the one
        // Install list: searching puts webapp name-matches first; the empty-
        // query storefront spreads the few recommendations among the programs.
        {
            let webapp_hits = std::mem::take(&mut visible[content::SECTION_INSTALL]);
            let pkgs = self.pkg_results();
            visible[content::SECTION_INSTALL] = if searching {
                let mut v = webapp_hits;
                v.extend(pkgs);
                v
            } else {
                blend_hits(webapp_hits, pkgs)
            };
        }
        self.group_minis.clear();
        // Every box gets a transient Group entry each refilter (indices
        // shift), used by both the grid and the dock; a box pinned to the
        // dock is hidden from the grid, like a pinned app.
        let group_cells = self.group_cells();
        if !resting_grid {
            visible[content::SECTION_FILES] = self.file_results();
        } else {
            // Hide pinned apps from the grid when the search box is
            // empty — they're already visible on the dock. Grouped
            // apps live inside their box, not loose in the grid.
            visible[content::SECTION_APPS].retain(|&idx| {
                let id = &self.entries[idx].id;
                !self.pins.is_pinned(id)
                    && !self.groups.is_grouped(id)
                    && !self.install_notify.iter().any(|n| n.id == *id)
            });
            // A dissolved group can't stay open.
            if self
                .app_group
                .is_some_and(|g| g >= self.groups.groups().len())
            {
                self.app_group = None;
            }
            // Boxes and loose apps share one grid order: install date with
            // manual drags on top (new boxes take their target's slot;
            // unseen ids append at the end). The grid stays put even with a
            // box open — the magnified box draws as an overlay on top of it.
            {
                // Loose (dock-unpinned) boxes join the grid.
                let mut cells: Vec<usize> = group_cells
                    .iter()
                    .copied()
                    .filter(|&i| !self.pins.is_pinned(&self.entries[i].id))
                    .collect();
                cells.append(&mut visible[content::SECTION_APPS]);
                let ids: Vec<String> = cells.iter().map(|&i| self.entries[i].id.clone()).collect();
                self.order.sync(ids.iter().map(String::as_str));
                let by_id: std::collections::HashMap<&str, usize> = ids
                    .iter()
                    .map(String::as_str)
                    .zip(cells.iter().copied())
                    .collect();
                // Cascade over-full pages by the real capacity, counting
                // only visible ids (hidden pinned/grouped ids occupy no
                // cell) — a legacy flat order loads as one big page and
                // splits here; inserts can overfill a page. Skip before
                // the first configure — a degenerate 0-size layout would
                // shred the pages into capacity-1 confetti.
                if self.renderer.is_some() {
                    let settled = self.layout_at(self.ui.extent_of(Target::Open));
                    let sec = &settled.sections[content::SECTION_APPS];
                    self.apps_cap = (sec.cols * sec.rows).max(1);
                    self.order
                        .normalize(self.apps_cap, |id| by_id.contains_key(id));
                }
                // Page-major arrangement with display slots: each storage
                // page's visible members sit dense from its page start;
                // an under-full page keeps its tail gap on screen. Pages
                // whose members are all hidden are skipped entirely.
                let cap = self.apps_cap.max(1);
                let mut arranged: Vec<usize> = Vec::with_capacity(cells.len());
                let mut slots: Vec<usize> = Vec::with_capacity(cells.len());
                let mut page_map: Vec<usize> = Vec::new();
                for (sp, page) in self.order.pages().iter().enumerate() {
                    let members: Vec<usize> = page
                        .iter()
                        .filter_map(|id| by_id.get(id.as_str()).copied())
                        .collect();
                    if members.is_empty() {
                        continue;
                    }
                    let dp = page_map.len();
                    page_map.push(sp);
                    for (w, e) in members.into_iter().enumerate() {
                        arranged.push(e);
                        slots.push(dp * cap + w);
                    }
                }
                // Safety net: a cell sync somehow missed still shows.
                for &e in &cells {
                    if !arranged.contains(&e) {
                        slots.push(slots.last().map_or(0, |s| s + 1));
                        arranged.push(e);
                    }
                }
                // Packages installing in place ride the loose grid at
                // their drop slot until the real app replaces them.
                self.insert_pending_cells(&mut arranged, &mut slots);
                self.apps_page_map = page_map;
                self.apps_span = slots.last().map_or(0, |s| s + 1);
                self.apps_slots = slots;
                visible[content::SECTION_APPS] = arranged;
            }
            // The Files section: a live listing of the navigated folder, or
            // of `$HOME` itself at the root (everything but dotfiles).
            visible[content::SECTION_FILES] = self.dir_listing();
        }
        // Pinned filesystem paths render on the dock through transient
        // entries; an open directory stack lists its contents the same way.
        self.pinned_path_entries();
        self.rebuild_dir_stack();
        self.search.visible = visible;
        // Search results are a flat ranked list: dense identity slots. (A
        // resting grid keeps the paged slots built just above.)
        if !resting_grid {
            let n = self.search.visible[content::SECTION_APPS].len();
            self.apps_slots = (0..n).collect();
            self.apps_page_map.clear();
            self.apps_span = n;
        }
        // Boxes' entry indices just changed — re-resolve pinned dock items.
        self.recompute_dock_order();
        // Visual continuity: every surviving cell keeps its current
        // animated display position and eases to its new seat from
        // there — a rebuilt list never snaps icons, not even for the
        // one synchronous frame this refilter may draw. New entries
        // start at rest (their display slot).
        let vis = &self.search.visible[content::SECTION_APPS];
        let mut slide: Vec<f32> = (0..vis.len())
            .map(|i| self.apps_slots.get(i).copied().unwrap_or(i) as f32)
            .collect();
        if resting_grid && !grid_flip {
            // (Ranked search results churn per keystroke; gliding
            // between ranks would be noise, so carry-over is for the
            // loose grid and box views only.)
            for (i, &e) in vis.iter().enumerate() {
                if let Some(entry) = self.entries.get(e) {
                    // The just-dropped icon keeps its rest position (`i`)
                    // so it appears in the chosen cell — never glides in
                    // from where it was picked up.
                    if self.just_dropped.as_deref() == Some(entry.id.as_str()) {
                        continue;
                    }
                    if let Some(&(_, d)) = old_slide.iter().find(|(oid, _)| *oid == entry.id) {
                        slide[i] = d;
                    }
                }
            }
        }
        self.just_dropped = None;
        self.apps_slide = slide;
        self.search.selected = if resting_grid || self.flat_len() == 0 {
            None
        } else {
            Some(0)
        };
        if !resting_grid || grid_flip {
            // Entering / leaving the searched grid replaces the content
            // wholesale: start every section back at its first page.
            self.scroll.reset_sections();
        } else {
            // A grid mutation (drop, rescan, box open/close, pin change)
            // must NOT yank the Apps view off the page the user is on —
            // a drop would otherwise teleport away from where it landed.
            // Other sections still reset; Apps only clamps to the
            // (possibly shrunken) page range.
            self.scroll.per[content::SECTION_INSTALL] = Default::default();
            self.scroll.per[content::SECTION_FILES] = Default::default();
            if self.renderer.is_some() {
                let settled = self.layout_at(self.ui.extent_of(Target::Open));
                let sec_l = &settled.sections[content::SECTION_APPS];
                let pages = self.apps_span.div_ceil(self.apps_cap.max(1)).max(1);
                let max_scroll = (pages - 1) as f32 * sec_l.viewport.w.max(1.0);
                let sec = &mut self.scroll.per[content::SECTION_APPS];
                sec.target = sec.target.clamp(0.0, max_scroll);
                sec.pos = sec.pos.clamp(0.0, max_scroll);
            }
        }
        self.update_hover();
        self.schedule_frame();
    }

    /// Append one transient entry, keeping the four parallel metadata
    /// arrays (`entries` / `kinds` / `placeholders` / `icon_layers`) in
    /// lockstep — the only way transients are added, so they can't
    /// desync. Returns the new entry's index.
    fn push_transient(
        &mut self,
        entry: AppEntry,
        kind: apps::EntryKind,
        placeholder: bool,
        layer: u32,
    ) -> usize {
        self.entries.push(entry);
        self.kinds.push(kind);
        self.placeholders.push(placeholder);
        self.icon_layers.push(layer);
        self.entries.len() - 1
    }

    /// Drop every transient entry (indices past `base_len`) — all four
    /// parallel arrays together.
    fn truncate_transients(&mut self) {
        self.entries.truncate(self.base_len);
        self.kinds.truncate(self.base_len);
        self.placeholders.truncate(self.base_len);
        self.icon_layers.truncate(self.base_len);
    }

    /// Hint shown centered in an empty Install section.
    fn install_hint(&self) -> &'static str {
        match self.pkg_state {
            PkgIndexState::Loading => "Indexing nixpkgs…",
            PkgIndexState::Failed => "Package search unavailable",
            PkgIndexState::Ready if self.search.query.is_empty() => {
                "Search to install from nixpkgs"
            }
            PkgIndexState::Ready if self.search.query.chars().count() < PKG_QUERY_MIN => {
                "Keep typing…"
            }
            // A live query with zero matches: the generic "No results".
            PkgIndexState::Ready => "",
        }
    }

    /// Total selectable cells across all sections (flat keyboard order:
    /// Apps, Install, Files).
    fn flat_len(&self) -> usize {
        self.search.visible.iter().map(Vec::len).sum()
    }

    /// Map a flat selection index to (section, cell within section).
    fn flat_to_pos(&self, mut i: usize) -> Option<(usize, usize)> {
        for (s, list) in self.search.visible.iter().enumerate() {
            if i < list.len() {
                return Some((s, i));
            }
            i -= list.len();
        }
        None
    }

    /// Rebuild `dock_order`: pinned entries first (in pin order), then
    /// most-used non-pinned apps (entries are already usage-sorted).
    /// Folders never auto-fill the dock, but an explicit pin works.
    /// An icon-carrier asset's (texture layer, placeholder flag).
    pub(crate) fn asset(&self, id: &str) -> Option<(u32, bool)> {
        self.assets.get(id).copied()
    }

    /// The keys a window class may match this entry by, lowercased: its
    /// `StartupWMClass`, its desktop-file id and the id's last dotted
    /// segment (`org.mozilla.firefox` → `firefox`), and the exec's
    /// program basename. Only real apps can be running windows.
    /// Close every open window of app `id` (best effort) — used on uninstall
    /// so an app that's still open goes away with its package rather than
    /// lingering on screen (and in the running-dock zone) with nothing behind
    /// it.
    fn kill_app_windows(&self, id: &str) {
        let Some(idx) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        if let Some(addrs) = self.running.get(&idx) {
            for addr in addrs {
                crate::hypr::close_window(addr);
            }
        }
    }

    fn app_match_keys(entry: &AppEntry, kind: apps::EntryKind) -> Vec<String> {
        if kind != apps::EntryKind::App {
            return Vec::new();
        }
        let mut keys: Vec<String> = Vec::new();
        let mut push = |s: &str| {
            let s = s.trim().to_lowercase();
            if !s.is_empty() && !keys.contains(&s) {
                keys.push(s);
            }
        };
        if let Some(wm) = &entry.startup_wm_class {
            push(wm);
        }
        push(&entry.id);
        if let Some(tail) = entry.id.rsplit('.').next() {
            push(tail);
        }
        // First shell token of Exec, minus any path, minus a trailing
        // extension (`/usr/bin/foo.sh` → `foo`).
        if let Some(prog) = entry.exec.split_whitespace().next() {
            let base = prog.rsplit('/').next().unwrap_or(prog);
            push(base.split('.').next().unwrap_or(base));
        }
        keys
    }

    /// Rebuild [`Self::running`] from Hyprland's live window list, matching
    /// each window's class to a dock/grid app. Cheap and best-effort: no
    /// Hyprland ⇒ every app reads as not-running (plain launcher behavior).
    fn refresh_running(&mut self) {
        // class (lowercased) → entry index, from every real app once.
        let mut by_class: HashMap<String, usize> = HashMap::new();
        for (idx, (entry, &kind)) in self.entries.iter().zip(&self.kinds).enumerate() {
            for key in Self::app_match_keys(entry, kind) {
                by_class.entry(key).or_insert(idx);
            }
        }
        let mut running: HashMap<usize, Vec<String>> = HashMap::new();
        for win in hypr::running_windows() {
            if let Some(&idx) = by_class.get(&win.class.to_lowercase()) {
                running.entry(idx).or_default().push(win.address);
            }
        }
        if running != self.running {
            self.running = running;
            // The unpinned-running dock zone depends on this set, so rebuild
            // the dock order (adds/removes ephemeral running icons + divider).
            self.recompute_dock_order();
            self.reconcile_install_notify();
            self.schedule_frame();
        }
    }

    /// A just-installed dock notify becomes "seen" the first time its app is
    /// running, and is dropped the first time a seen one has fully closed —
    /// unpinning it from the dock so the app returns to its grid slot.
    fn reconcile_install_notify(&mut self) {
        if self.install_notify.is_empty() {
            return;
        }
        let running_ids: HashSet<&String> = self
            .running
            .keys()
            .filter_map(|&i| self.entries.get(i).map(|e| &e.id))
            .collect();
        for n in &mut self.install_notify {
            if running_ids.contains(&n.id) {
                n.seen_running = true;
            }
        }
        let before = self.install_notify.len();
        self.install_notify
            .retain(|n| !n.seen_running || running_ids.contains(&n.id));
        if self.install_notify.len() != before {
            // One returned to the grid: rebuild the dock and unhide it.
            self.recompute_dock_order();
            self.refilter();
        }
    }

    /// Whether a dock notify's app is still installed — false once it has been
    /// uninstalled (webapp dropped from `managed_webapps`, package entry gone,
    /// or an uninstall in flight), so the dead temp-pin is dropped.
    fn notify_still_installed(&self, id: &str) -> bool {
        if self.uninstalling.contains_key(id) {
            return false;
        }
        if let Some(slug) = crate::webapps::slug_of_id(id) {
            return self.managed_webapps.contains(slug);
        }
        self.entries
            .iter()
            .zip(&self.kinds)
            .any(|(e, k)| e.id == id && *k == apps::EntryKind::App)
    }

    /// Arm a 3 s auto-hide after a notify pop-up: the dock only came up to
    /// flash "your app is ready", so it tucks itself away again.
    fn arm_notify_dock_hide(&mut self) {
        const NOTIFY_DOCK_DWELL: Duration = Duration::from_secs(3);
        self.notify_dock_hide_at = Some(Instant::now() + NOTIFY_DOCK_DWELL);
        let timer = Timer::from_duration(NOTIFY_DOCK_DWELL);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.notify_dock_autohide();
                TimeoutAction::Drop
            })
        {
            warn!("failed to arm notify-dock hide timer: {e}");
        }
    }

    /// Fired ~3 s after a notify pop-up: hide the dock again, unless the user
    /// has engaged with it (expanded it, or the pointer is on it) or a newer
    /// notify pushed the deadline back.
    fn notify_dock_autohide(&mut self) {
        let Some(at) = self.notify_dock_hide_at else {
            return;
        };
        if Instant::now() < at {
            return; // a later notify extended the dwell; its own timer fires
        }
        self.notify_dock_hide_at = None;
        if self.ui.target() == Target::Dock
            && self.pointer_pos.is_none()
            && self.ui.apply(Command::Hide)
        {
            self.schedule_frame();
        }
    }

    fn recompute_dock_order(&mut self) {
        self.dock_order.clear();
        // Drop any dock notify whose app is no longer installed (e.g.
        // uninstalled while it was still surfaced on the dock): its `.desktop`
        // may linger (a webapp returns to the catalog), so without this it
        // would stick to the dock as a dead icon that can't be launched or
        // removed.
        if !self.install_notify.is_empty() {
            self.install_notify = std::mem::take(&mut self.install_notify)
                .into_iter()
                .filter(|n| self.notify_still_installed(&n.id))
                .collect();
        }
        // A package installing onto the dock rides its `Package` transient
        // until the real app lands — allow that pin to match (below), while
        // other Package/File transient pins stay excluded.
        let dock_pending: std::collections::HashSet<String> = self
            .pending_installs
            .iter()
            .filter(|p| p.dock_slot.is_some())
            .map(|p| p.attr.clone())
            .collect();
        for pin_id in self.pins.pins() {
            // An uninstall in flight hides its pin at once (the app vanishes
            // from the dock the moment it's dropped on Install), restored
            // only if the rebuild fails.
            if self.uninstalling.contains_key(pin_id) {
                continue;
            }
            // A webapp that's been uninstalled (returned to the catalog) keeps
            // its materialized `.desktop`, so a stale pin would still match its
            // App entry and stick to the dock. A catalog webapp is never docked
            // — skip the pin so it can't ghost there. (`uninstall_webapp` drops
            // the pin outright; this is the belt-and-suspenders net.)
            if crate::webapps::slug_of_id(pin_id).is_some_and(|s| !self.managed_webapps.contains(s))
            {
                continue;
            }
            // Match a real App entry, a box's Group entry, or — for
            // filesystem pins only (path ids, home-strip `folder-<name>`
            // ids) — a File entry. Other pins never match File/Package
            // transients, or a stale pin for an uninstalled app would
            // ghost onto its same-named search result.
            let fs_pin = pin_id.starts_with('/') || pin_id.starts_with("folder-");
            let idx = self.entries.iter().zip(&self.kinds).position(|(e, k)| {
                &e.id == pin_id
                    && match k {
                        apps::EntryKind::App | apps::EntryKind::Group => true,
                        apps::EntryKind::File => fs_pin,
                        apps::EntryKind::Package => dock_pending.contains(pin_id),
                        apps::EntryKind::Asset => false,
                    }
            });
            if let Some(idx) = idx {
                if !self.dock_order.contains(&idx) {
                    self.dock_order.push(idx);
                }
            }
        }
        // macOS: a running app that isn't pinned rides the dock too, after
        // a divider, and vanishes when it quits. Entry order ≈ alphabetical
        // (entries are name-sorted), so the zone stays stable as unrelated
        // apps come and go rather than reshuffling.
        let pinned_count = self.dock_order.len();
        let mut running_unpinned: Vec<usize> = self
            .running
            .keys()
            .copied()
            .filter(|i| {
                self.kinds.get(*i) == Some(&apps::EntryKind::App)
                    && !self.dock_order.contains(i)
                    && !self.is_removing(*i)
            })
            .collect();
        running_unpinned.sort_unstable();
        // Freshly-installed notify apps sit in this same "in between" zone,
        // right after the divider (so they read as never-pinned): openable,
        // and gone once opened and closed. Placed first so the new app stands
        // out; skip any already pinned or already in the running zone.
        let mut zone: Vec<usize> = Vec::new();
        for n in &self.install_notify {
            let idx = self
                .entries
                .iter()
                .zip(&self.kinds)
                .position(|(e, k)| e.id == n.id && *k == apps::EntryKind::App);
            if let Some(idx) = idx {
                if !self.dock_order.contains(&idx) && !zone.contains(&idx) {
                    zone.push(idx);
                }
            }
        }
        for idx in running_unpinned {
            if !zone.contains(&idx) {
                zone.push(idx);
            }
        }
        self.dock_divider = (!zone.is_empty()).then_some(pinned_count);
        self.dock_order.extend(zone);
        // No truncation here — layout() clamps to the available width.
    }

    /// Whether `pos` is outside the popup card entirely — the drop zone
    /// for a "try it" launch (a package dragged clear of the box).
    fn outside_card(&self, layout: &content::Layout, pos: (f32, f32)) -> bool {
        let right = self.buffer_size.0 as f32 - content::DRAG_MARGIN_X;
        pos.0 < content::DRAG_MARGIN_X
            || pos.0 > right
            || pos.1 < layout.card_top
            || pos.1 > layout.card_top + layout.card_h
    }

    /// Surface-pixel center of the slot an icon will land in for the
    /// current drag. Call this *before* any drop mutation clears the
    /// drag state. Returns `None` when no drag is active or the drop
    /// position can't be resolved to a slot.
    ///
    /// Covers all three drop surfaces with the same logic:
    ///   • box  — cell center snapped from the pointer position
    ///   • dock — final slot center (accounts for the from-dock offset)
    ///   • grid — reorder-slot center (or the icon's current slot)
    fn drop_ripple_pos(&self) -> Option<(f32, f32)> {
        let pos = self.pointer_pos?;

        if self.box_drag.is_some() {
            return self.box_drag_cell_center(pos);
        }

        let drag = self.gesture.dragging.as_ref()?;
        let layout = self.current_layout();

        if let Some(i) = self.drag_dock_insert(&layout, pos) {
            let origin = self.dock_order.iter().position(|&e| e == drag.entry_idx);
            let land =
                i.saturating_sub(usize::from(drag.from_dock && origin.is_some_and(|o| o < i)));
            let s = layout
                .dock_slots
                .get(land)
                .or_else(|| layout.dock_slots.last())?;
            return Some((s.x + s.w * 0.5, s.y + s.h * 0.5));
        }

        let sec = &layout.sections[content::SECTION_APPS];
        let visible = &self.search.visible[content::SECTION_APPS];
        let orig = visible.iter().position(|&v| v == drag.entry_idx);
        let orig_slot = orig.and_then(|o| self.apps_slots.get(o).copied());
        let slot = self.reorder_slot.or(orig_slot)?;
        let cap = self.apps_cap.max(1);
        let page = slot / cap;
        let within = slot % cap;
        let cols = sec.cols.max(1);
        let cw = content::GRID_CELL_W * self.icon_scale();
        let ch = content::GRID_CELL_H * self.icon_scale();
        Some((
            sec.viewport.x - sec.scroll
                + page as f32 * sec.viewport.w
                + (within % cols) as f32 * cw
                + cw * 0.5,
            sec.viewport.y + (within / cols) as f32 * ch + ch * 0.5,
        ))
    }

    /// Drop a pointer position left stale above the collapsed dock.
    ///
    /// When our input region shrinks out from under a motionless cursor
    /// this Hyprland sends no `wl_pointer.leave` (verified quirk), so a
    /// cursor parked over the open card is still reported as being on the
    /// dock after it collapses. That suppresses both the auto-hide guard
    /// and intellihide's dodge (`pointer_pos.is_none()`), leaving the dock
    /// parked visible until a real mouse motion. On settle, if the last
    /// known pointer sits above the live input region, treat it as gone.
    fn reconcile_stale_pointer(&mut self) {
        let (Some((_, y)), Some(extent)) = (self.pointer_pos, self.input_extent) else {
            return;
        };
        let region_top = self.buffer_size.1 as f32 - extent as f32;
        if y >= region_top {
            return; // genuinely within the dock's live region
        }
        self.pointer_pos = None;
        self.hover = None;
    }

    /// What the pointer is over right now (`None` when outside).
    fn hover_at_pointer(&self) -> Option<Hit> {
        self.pointer_pos.and_then(|pos| {
            content::hit_test(
                &self.current_layout(),
                pos,
                self.search.open,
                &self.apps_slots,
            )
        })
    }

    /// Recompute which item the pointer is over; redraw on change.
    fn update_hover(&mut self) {
        let hover = self.hover_at_pointer();
        if hover != self.hover {
            debug!(
                "hover: {:?} -> {:?} at {:?} (extent {})",
                self.hover,
                hover,
                self.pointer_pos,
                self.ui.extent()
            );
            self.hover = hover;
            // Dock name tooltip: (re)start the dwell when the pointer
            // lands on a dock icon, and wake once to draw it when the
            // dwell elapses (a stationary pointer emits no more frames).
            if matches!(hover, Some(Hit::DockIcon(_))) {
                self.dock_hover_since = Some(Instant::now());
                let timer = Timer::from_duration(DOCK_TOOLTIP_DELAY);
                if let Err(e) = self
                    .loop_handle
                    .insert_source(timer, |_, _, app: &mut App| {
                        app.schedule_frame();
                        TimeoutAction::Drop
                    })
                {
                    warn!("cannot arm dock-tooltip timer: {e}");
                }
            } else {
                self.dock_hover_since = None;
            }
            // Dock hover-switch: once a box has been opened *from the dock*
            // (by an explicit click), gliding the pointer onto a different
            // dock folder or directory switches straight to it — no click
            // needed, whether the box floats over the dock or opened into
            // the grid. Armed only for dock-origin boxes, so it ends the
            // moment the box is dismissed (click-out / Escape) and never
            // fires for a box opened from a grid folder tile.
            if self.stack_open() && self.box_from_dock {
                if let Some(Hit::DockIcon(slot)) = hover {
                    self.hover_switch_dock_box(slot);
                }
            }
            self.schedule_frame();
        }
        self.apply_cursor();
    }

    /// React to the pointer gliding onto dock slot `slot` while a dock box
    /// is open: hovering a *different* folder/directory switches straight to
    /// it; hovering the already-open box's own icon keeps it open; hovering
    /// any other icon (a plain app) closes the box so that icon is clickable.
    fn hover_switch_dock_box(&mut self, slot: usize) {
        let Some(&idx) = self.dock_order.get(slot) else {
            return;
        };
        match self.kinds.get(idx) {
            Some(apps::EntryKind::Group) => {
                // The Recycle Bin switches to its trash view (a dir stack), not
                // a group folder.
                if self
                    .entries
                    .get(idx)
                    .is_some_and(|e| groups::is_trash(&e.id))
                {
                    let already = self.dir_stack.as_ref().is_some_and(|ds| ds.is_trash);
                    if !already {
                        self.open_trash_stack();
                    }
                    return;
                }
                let g = self
                    .entries
                    .get(idx)
                    .and_then(|e| e.id.strip_prefix("group:"))
                    .and_then(|gid| self.groups.index_by_id(gid));
                match g {
                    Some(g) if self.open_box_group() != Some(g) => self.open_dock_folder(g),
                    Some(_) => {} // the open box's own icon: keep it open
                    None => self.close_group(),
                }
            }
            Some(apps::EntryKind::File) => {
                let dir = self.entries.get(idx).and_then(|e| {
                    let path = e.description.clone()?;
                    std::fs::metadata(&path)
                        .ok()?
                        .is_dir()
                        .then(|| (e.id.clone(), path))
                });
                match dir {
                    Some((id, path)) => {
                        let same =
                            self.dir_stack.as_ref().map(|d| d.id.as_str()) == Some(id.as_str());
                        if !same {
                            self.open_dir_stack(id, path);
                        }
                    }
                    // A plain (non-directory) pinned file: close so it's clickable.
                    None => self.close_group(),
                }
            }
            // Any other dock icon (a plain app): close the box.
            _ => self.close_group(),
        }
    }

    /// Reflect what's under the pointer in its cursor: the pointing hand
    /// over anything clickable, a grabbing hand mid-drag, the arrow
    /// otherwise. One request per change (cursor-shape-v1; a compositor
    /// without the protocol just keeps its default cursor).
    fn apply_cursor(&mut self) {
        let Some(device) = &self.cursor_device else {
            return;
        };
        let shape = if self.gesture.dragging.is_some() || self.box_drag.is_some() {
            Shape::Grabbing
        } else if self.hover.is_some() {
            Shape::Pointer
        } else {
            Shape::Default
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }

    /// The dock slot whose name tooltip should show: the hovered dock
    /// icon, but only once the pointer has dwelt [`DOCK_TOOLTIP_DELAY`].
    fn dock_tooltip(&self) -> Option<usize> {
        match self.hover {
            Some(Hit::DockIcon(slot))
                if self
                    .dock_hover_since
                    .is_some_and(|t| t.elapsed() >= DOCK_TOOLTIP_DELAY) =>
            {
                Some(slot)
            }
            _ => None,
        }
    }

    /// The box a click would open, if any (a group folder or a pinned
    /// directory). Used to switch boxes in one click while one is open.
    fn hit_box_target(&self, hit: Hit) -> Option<BoxTarget> {
        let group_of = |idx: usize| {
            self.entries
                .get(idx)
                .and_then(|e| e.id.strip_prefix("group:"))
                .and_then(|gid| self.groups.index_by_id(gid))
                .map(BoxTarget::Group)
        };
        match hit {
            Hit::DockIcon(slot) => {
                let &idx = self.dock_order.get(slot)?;
                match self.kinds.get(idx)? {
                    // The Recycle Bin opens as a trash dir-stack (id
                    // `group:trash`), not a group box — report it as such so
                    // clicking it again toggles the open stack shut.
                    apps::EntryKind::Group
                        if self
                            .entries
                            .get(idx)
                            .is_some_and(|e| groups::is_trash(&e.id)) =>
                    {
                        Some(BoxTarget::Dir(self.entries[idx].id.clone()))
                    }
                    apps::EntryKind::Group => group_of(idx),
                    apps::EntryKind::File => {
                        let e = self.entries.get(idx)?;
                        let path = e.description.clone()?;
                        std::fs::metadata(&path)
                            .ok()?
                            .is_dir()
                            .then(|| BoxTarget::Dir(e.id.clone()))
                    }
                    _ => None,
                }
            }
            Hit::GridCell(s, i) if s == content::SECTION_APPS => {
                let idx = self.search.visible[s].get(i).copied()?;
                (self.kinds.get(idx) == Some(&apps::EntryKind::Group))
                    .then(|| group_of(idx))
                    .flatten()
            }
            _ => None,
        }
    }

    /// Resolve a hit to an action: launch an entry or toggle the search box.
    fn activate_hit(&mut self, hit: Hit) {
        // While a box is open, the grid behind is just context. Clicking a
        // *different* folder switches straight to it (the open_* call below
        // replaces the current box in one click); clicking the open box's
        // own icon toggles it shut; any other click just dismisses it.
        if self.stack_open() {
            let target = self.hit_box_target(hit);
            let same = match &target {
                Some(BoxTarget::Group(g)) => self.open_box_group() == Some(*g),
                Some(BoxTarget::Dir(id)) => {
                    self.dir_stack.as_ref().map(|ds| ds.id.as_str()) == Some(id.as_str())
                }
                None => false,
            };
            if same || (target.is_none() && !matches!(hit, Hit::OpenBoxCell(_))) {
                self.close_group();
                return;
            }
            // A different box (target is Some): fall through to open it.
        }
        match hit {
            Hit::DockIcon(slot) => {
                if let Some(&entry_idx) = self.dock_order.get(slot) {
                    // A pinned box opens its folder as a stack above the dock
                    // (the same magnified box, anchored to the icon); an app
                    // launches.
                    if self.kinds.get(entry_idx) == Some(&apps::EntryKind::Group) {
                        // The Recycle Bin opens a trash view (its FreeDesktop
                        // trash contents), not an empty folder.
                        if self
                            .entries
                            .get(entry_idx)
                            .is_some_and(|e| groups::is_trash(&e.id))
                        {
                            self.open_trash_stack();
                            return;
                        }
                        if let Some(g) = self
                            .entries
                            .get(entry_idx)
                            .and_then(|e| e.id.strip_prefix("group:"))
                            .and_then(|gid| self.groups.index_by_id(gid))
                        {
                            self.open_dock_folder(g);
                        }
                        return;
                    }
                    // A pinned directory opens its content stack the same
                    // way (a pinned plain file falls through and opens).
                    if self.kinds.get(entry_idx) == Some(&apps::EntryKind::File) {
                        let dir = self.entries.get(entry_idx).and_then(|e| {
                            let path = e.description.clone()?;
                            std::fs::metadata(&path)
                                .ok()?
                                .is_dir()
                                .then(|| (e.id.clone(), path))
                        });
                        if let Some((id, path)) = dir {
                            self.open_dir_stack(id, path);
                            return;
                        }
                    }
                    self.activate(entry_idx);
                }
            }
            Hit::GridCell(s, i) => {
                if let Some(entry_idx) = self.search.visible[s].get(i).copied() {
                    // Folders in the Files section navigate instead of
                    // launching (the popup stays open); boxes in the
                    // Apps grid open the same way.
                    if s == content::SECTION_FILES && self.try_navigate(entry_idx) {
                        return;
                    }
                    if self.kinds.get(entry_idx) == Some(&apps::EntryKind::Group) {
                        let g = self
                            .entries
                            .get(entry_idx)
                            .and_then(|e| e.id.strip_prefix("group:"))
                            .and_then(|gid| self.groups.index_by_id(gid));
                        if let Some(g) = g {
                            self.open_group(g);
                        }
                        return;
                    }
                    self.activate(entry_idx);
                }
            }
            Hit::SearchButton => {
                self.search.open = !self.search.open;
                if !self.search.open {
                    self.search.query.clear();
                    self.refilter();
                }
                self.schedule_frame();
            }
            // Click a filled box slot to launch it; an empty slot is inert
            // (the box stays open — only a click outside it closes it).
            Hit::OpenBoxCell(k) => {
                if let Some(idx) = self.open_box_member_idx(k) {
                    self.activate(idx);
                }
            }
        }
    }

    /// Get out of the way. From the open box, retreat to the dock first
    /// so it rests a beat there before hiding (the rest-then-hide timer
    /// finishes the job); from the dock, hide outright — unless nothing
    /// overlaps the zone, where intellihide keeps the dock parked.
    fn dismiss(&mut self) {
        // A dock stack (group or directory) closes with the launcher.
        self.dock_stack = None;
        self.dir_stack = None;
        let command = if self.ui.target() == Target::Open {
            Command::Collapse
        } else {
            Command::Hide
        };
        self.handle_command(command);
    }

    /// Launch an entry by index; its icon plays a bounce (macOS launch
    /// feedback), then the card gets out of the way.
    fn activate(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        // Packages aren't launchable — installing is a drag to the Apps
        // section, not a click. The one exception is a failed
        // drag-to-install tile: clicking it retries the install.
        if self.kinds.get(index) == Some(&apps::EntryKind::Package) {
            let attr = entry.id.clone();
            let desktop_ids = self
                .pending_installs
                .iter_mut()
                .find(|p| p.attr == attr && p.failed)
                .map(|p| {
                    p.failed = false;
                    p.started = std::time::Instant::now(); // restart the ring
                    p.completed_at = None;
                    p.rescan_fired = false;
                    p.desktop_ids.clone()
                });
            if let Some(desktop_ids) = desktop_ids {
                info!("retrying install of {attr}");
                // Re-stage in memory; packages.nix not written until success.
                self.managed.stage(&attr, desktop_ids);
                self.recompute_removable();
                self.busy_ids.insert(attr.clone());
                self.nix.request(nix::Request::Install {
                    id: attr.clone(),
                    attr,
                });
                self.refilter();
            }
            return;
        }
        // Catalog webapps aren't launched by a click either — like packages,
        // "try" is a drag out of the box and install is a drag to the grid.
        if self.is_catalog_webapp(index) {
            return;
        }
        let (exec, id) = (entry.exec.clone(), entry.id.clone());
        let needs_terminal = entry.needs_terminal;
        // macOS dock model: a running app activates (focus its
        // most-recently-used window) instead of launching a duplicate.
        // Ctrl-click forces a fresh instance (macOS's Cmd+click / New
        // Window), falling through to the launch path below.
        let force_new = self.modifiers.ctrl || self.force_new_instance;
        if !force_new {
            if let Some(addr) = self.running.get(&index).and_then(|w| w.first()).cloned() {
                info!("activating running app {id} -> window {addr}");
                self.usage.increment(&id);
                self.restore_window = None;
                if self.interactive {
                    surface::set_interactive(&self.layer, false);
                    self.interactive = false;
                }
                hypr::focus_window(&addr);
                self.dismiss();
                return;
            }
        }
        if let Err(e) = launch::launch(&exec, needs_terminal, &self.config.launch.terminal) {
            error!("launch failed for {id}: {e:#}");
        }
        self.usage.increment(&id);
        // Hand keyboard focus to the launched app. Drop our exclusive
        // grab now (before its window maps) and clear the return-focus
        // target so we don't pull focus back to the window we came from;
        // then focus the app's window the moment it opens — the
        // compositor won't with focus-follows-mouse if the cursor is
        // parked off it (Chrome and other slow starters especially).
        self.restore_window = None;
        self.focus_launched = Some(Instant::now());
        if self.interactive {
            surface::set_interactive(&self.layer, false);
            self.interactive = false;
        }
        self.bounce = Some((index, Instant::now()));
        self.schedule_frame();
        let timer = Timer::from_duration(BOUNCE_DURATION);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.dismiss();
                TimeoutAction::Drop
            })
        {
            warn!("failed to arm launch-hide timer: {e}");
            self.dismiss();
        }
    }

    /// Push keyboard interactivity and the pointer input region to the
    /// compositor whenever the targeted rest state changes. The input
    /// region covers the *target* rect immediately so the dock/popup is
    /// interactive without waiting for the slide to finish.
    fn sync_surface_state(&mut self) {
        if self.ui.target() == Target::Hidden {
            // Fresh card next time it rises.
            self.scroll.reset_sections();
            self.hover = None;
            self.search.open = false;
            if self.files_dir.take().is_some() {
                self.refilter();
            }
        }
        if self.ui.target() == Target::Open {
            self.scroll.reset_sections();
            // open_at is set only when expand was triggered by a scroll gesture
            // (in on_scroll), so keyboard Toggle has no cooldown.
        }
        // The search only lives while the popup is open.
        if self.ui.target() != Target::Open && !self.search.query.is_empty() {
            self.search.query.clear();
            self.search.open = false;
            self.refilter();
        }
        let interactive = self.ui.wants_keyboard();
        if interactive != self.interactive {
            if interactive {
                // Grabbing the keyboard for type-to-search steals focus
                // from whatever window has it — remember it so a plain
                // close can hand focus back.
                self.restore_window = hypr::active_window();
                debug!("grab keyboard; restore target = {:?}", self.restore_window);
                surface::set_interactive(&self.layer, true);
            } else {
                // Release the grab. Handing focus back to where we came
                // from happens in the keyboard `leave` handler, which
                // fires once the compositor has actually taken our
                // keyboard away — dispatching a focus before that races
                // the release and Hyprland drops it.
                surface::set_interactive(&self.layer, false);
            }
            self.interactive = interactive;
        }

        self.sync_input_region();
    }

    /// Size the pointer input region to whatever is currently visible,
    /// not just the target rest point: while a hide slides the dock down
    /// the still-visible bar must keep taking input (so returning to it
    /// re-summons), and once it settles the region must shrink back to
    /// the reveal strip. Run every frame while animating *and* on target
    /// change, since the visible extent moves between those.
    fn sync_input_region(&mut self) {
        let mut extent = self
            .ui
            .extent_of(self.ui.target())
            .max(self.ui.extent())
            .round() as u32;
        if self.ui.target() == Target::Hidden && self.config.input.edge_reveal {
            extent = extent.max(self.config.input.edge_reveal_px);
        }
        // While a box floats above the dock, extend the input region to cover
        // the full surface so the pointer can reach the box without leaving.
        if self.stack_open() {
            extent = extent.max(self.buffer_size.1);
        }
        if self.input_extent != Some(extent) {
            match surface::set_input_extent(
                &self.compositor,
                &self.layer,
                self.buffer_size,
                extent,
                content::DRAG_MARGIN_X as u32,
            ) {
                Ok(()) => self.input_extent = Some(extent),
                Err(e) => warn!("failed to set input region: {e:#}"),
            }
        }
    }

    /// Mark the scene damaged. Draws immediately when no frame callback
    /// is in flight; otherwise the damage is coalesced onto the pending
    /// callback, so redraw rate never exceeds the display refresh no
    /// matter how fast input events arrive.
    fn schedule_frame(&mut self) {
        if self.renderer.is_none() {
            debug!("frame requested before first configure; deferring");
            return;
        }
        self.dirty = true;
        if !self.frame_pending {
            self.last_frame = None; // waking from idle: don't count idle time as dt
            self.draw();
        }
    }

    /// One vertical-scroll step of `value` axis units.
    ///
    /// Hidden with the pointer on the reveal strip, scrolling up summons
    /// the dock (the compositor keeps our stale pointer focus when the
    /// dock hides under a parked cursor, so this — not a re-enter — is
    /// the recovery path). Docked, the wheel is the expand gesture, so
    /// one continuous scroll rides Hidden → Dock → Open. Open, it pages
    /// the grid (down = next, up = previous) and never collapses the
    /// popup — dismissal is Escape / pointer-leave / toggle only.
    fn on_scroll(&mut self, value: f64) {
        // A drag in flight owns paging (the clamped edge bands): wheel
        // events mid-drag — real or a trackpad's spurious axis noise —
        // must not wrap-cycle the pages out from under the drag.
        if self.gesture.dragging.is_some() || self.box_drag.is_some() {
            return;
        }
        // An open box (grid box or dock stack) captures scroll to page its
        // members, whatever the card state (a dock stack opens over the bar).
        if self.stack_open() {
            self.box_page_scroll(value);
            return;
        }
        let target = self.ui.target();
        match target {
            Target::Hidden | Target::Dock => {
                // While hidden, only in-strip scroll may summon (stale
                // focus can also deliver events from anywhere the open
                // card used to be — a scroll there belongs to the window
                // beneath).
                if target == Target::Hidden && !self.pointer_on_reveal_strip() {
                    return;
                }
                self.scroll.accum += value;
                let mut toward_open = self.scroll.accum;
                if self.config.input.natural_scroll {
                    toward_open = -toward_open;
                }
                if toward_open <= -SCROLL_THRESHOLD {
                    self.scroll.accum = 0.0;
                    if target == Target::Dock {
                        // Mark that expand was triggered by scroll so the
                        // bleed-through events are eaten until the gesture
                        // ends (AxisStop clears this).
                        self.scroll.open_at = Some(Instant::now());
                        self.handle_command(Command::Expand);
                    } else {
                        // No bleed cooldown: the rest of the gesture
                        // should keep accumulating toward Expand.
                        self.handle_command(Command::Show);
                    }
                } else if toward_open >= SCROLL_THRESHOLD {
                    // Scrolling away from expand: nothing to do, just
                    // keep the accumulator bounded.
                    self.scroll.accum = 0.0;
                }
            }
            Target::Open => {
                if self
                    .scroll
                    .open_at
                    .is_some_and(|t| t.elapsed() < EXPAND_BLEED_COOLDOWN)
                {
                    return;
                }
                // Page the section under the pointer (each scrolls
                // independently). An open box was already handled above.
                if let Some(section) = self
                    .pointer_pos
                    .and_then(|pos| content::section_at(&self.current_layout(), pos))
                {
                    self.page_scroll(section, value);
                }
            }
        }
    }

    /// Whether the pointer is on the edge-reveal strip (the bottom
    /// `edge_reveal_px` of the surface). False when edge reveal is off.
    fn pointer_on_reveal_strip(&self) -> bool {
        if !self.config.input.edge_reveal {
            return false;
        }
        let strip_top = self.buffer_size.1 as f32 - self.config.input.edge_reveal_px as f32;
        self.pointer_pos.is_some_and(|(_, y)| y >= strip_top)
    }

    /// Horizontal scroll: pages the section under the pointer.
    /// (Untestable in the dev VM — its virtual pointer has no horizontal
    /// axis; verify on real hardware.)
    fn on_hscroll(&mut self, value: f64) {
        // Mid-drag wheel events must not page (see `on_scroll`).
        if self.gesture.dragging.is_some() || self.box_drag.is_some() {
            return;
        }
        if self.ui.target() == Target::Open {
            if self.stack_open() {
                self.box_page_scroll(value);
            } else if let Some(section) = self
                .pointer_pos
                .and_then(|pos| content::section_at(&self.current_layout(), pos))
            {
                self.page_scroll(section, value);
            }
        }
    }

    /// Accumulate wheel scroll toward a page turn of `section` (the
    /// pager's threshold + cooldown: one notch nudges, a deliberate
    /// scroll turns one page, a fast flick can't spin the wheel).
    fn page_scroll(&mut self, section: usize, value: f64) {
        if let Some(dir) = self.scroll.per[section].wheel(value) {
            info!("wheel page turn: section {section} dir {dir}");
            self.page_by(section, dir, true);
        }
    }

    /// Slide one section's grid a page in `dir` (+1 = next, -1 =
    /// previous): wheel paging wraps cyclically, drag paging clamps at
    /// the ends (see [`pager::Pager::turn`]).
    fn page_by(&mut self, section: usize, dir: i64, wrap: bool) {
        // Use the SETTLED (full-extent) layout: mid-open-animation the
        // current layout has a tiny viewport and a bogus page count.
        let settled = self.layout_at(self.ui.extent_of(Target::Open));
        let sec_layout = &settled.sections[section];
        let page_w = sec_layout.viewport.w.max(1.0);
        if self.scroll.per[section].turn(dir, wrap, sec_layout.n_pages, page_w) {
            self.update_hover();
            self.schedule_frame();
        }
    }

    fn handle_key_event(&mut self, keysym: Keysym, utf8: Option<&str>) {
        // The clipboard "define a word" panel grabs the keyboard while open, so
        // its search field consumes every key before the launcher's shortcuts.
        if self.clip.dict_open {
            self.dict_key(keysym, utf8);
            return;
        }
        // Ctrl+Plus/Minus cycles icon size through 4 levels.
        if self.modifiers.ctrl {
            match keysym {
                Keysym::equal | Keysym::plus | Keysym::KP_Add => {
                    self.icon_size = (self.icon_size + 1).min(content::ICON_SCALES.len() - 1);
                    self.apply_icon_size_change();
                    return;
                }
                Keysym::minus | Keysym::KP_Subtract => {
                    self.icon_size = self.icon_size.saturating_sub(1);
                    self.apply_icon_size_change();
                    return;
                }
                _ => {}
            }
        }
        // Ctrl+V pastes the clipboard into the query.
        if self.modifiers.ctrl && matches!(keysym, Keysym::v | Keysym::V) {
            self.paste();
            return;
        }
        match keysym {
            Keysym::Escape => {
                // Step out of an open box first; dismiss on the next.
                if self.stack_open() && self.search.query.is_empty() {
                    self.close_group();
                    return;
                }
                self.search.query.clear();
                self.search.open = false;
                self.refilter();
                self.dismiss();
            }
            Keysym::Return | Keysym::KP_Enter => {
                if let Some((s, i)) = self.search.selected.and_then(|i| self.flat_to_pos(i)) {
                    self.activate_hit(Hit::GridCell(s, i));
                }
            }
            Keysym::BackSpace => {
                if self.search.query.pop().is_some() {
                    // Editing the query means the user is searching again —
                    // release the drag-from-Install grid hold.
                    self.install_drag_reset = false;
                    if self.search.query.is_empty() {
                        self.search.open = false;
                    }
                    self.refilter();
                }
            }
            Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                if self.flat_len() > 0 {
                    let layout = self.current_layout();
                    let cols = layout.sections[content::SECTION_APPS].cols.max(1);
                    let last = self.flat_len() - 1;
                    // When nothing is selected yet, the first arrow key
                    // lands on item 0 regardless of direction. Selection
                    // walks the sections as one flat list (Apps → Files).
                    let next = if let Some(cur) = self.search.selected {
                        match keysym {
                            Keysym::Left => cur.saturating_sub(1),
                            Keysym::Right => (cur + 1).min(last),
                            Keysym::Up => cur.saturating_sub(cols),
                            _ => (cur + cols).min(last),
                        }
                    } else {
                        0
                    };
                    self.search.selected = Some(next);
                    if let Some((s, cell)) = self.flat_to_pos(next) {
                        // Apps cells scroll by display slot (pages may
                        // have tail gaps), other sections by index.
                        let at = if s == content::SECTION_APPS {
                            self.apps_slots.get(cell).copied().unwrap_or(cell)
                        } else {
                            cell
                        };
                        self.scroll.per[s].target =
                            content::scroll_to_reveal(&layout.sections[s], at);
                    }
                    self.schedule_frame();
                }
            }
            _ => {
                if let Some(text) = utf8 {
                    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !printable.is_empty() {
                        self.search.open = true;
                        // Typing means the user is searching again — release
                        // the drag-from-Install grid hold.
                        self.install_drag_reset = false;
                        self.search.query.push_str(&printable);
                        self.refilter();
                    }
                }
            }
        }
    }

    /// Read the clipboard selection (best-effort) on a short-lived
    /// thread; the text lands in `on_paste` via the paste channel.
    fn paste(&mut self) {
        let Some(device) = &self.data_device else {
            info!("paste: no data device (compositor lacks wl_data_device?)");
            return;
        };
        let Some(offer) = device.data().selection_offer() else {
            info!("paste: clipboard is empty (no selection offer)");
            return;
        };
        let mime = offer.with_mime_types(|mimes| {
            info!("paste: selection offers {mimes:?}");
            ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
                .iter()
                .find(|want| mimes.iter().any(|m| m == *want))
                .map(|s| s.to_string())
        });
        let Some(mime) = mime else {
            info!("paste: no text mime in the selection");
            return;
        };
        let Ok(mut pipe) = offer.receive(mime) else {
            info!("paste: receive failed");
            return;
        };
        // Flush so the selection owner sees the request before the read.
        let _ = self.conn.flush();
        let tx = self.paste_tx.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            if !text.is_empty() {
                let _ = tx.send(text);
            }
        });
    }

    /// Clipboard text arrived: append its printable characters to the
    /// query, exactly like typing.
    fn on_paste(&mut self, text: &str) {
        info!("paste: {} chars", text.chars().count());
        let printable: String = text.chars().filter(|c| !c.is_control()).collect();
        if printable.is_empty() {
            return;
        }
        self.search.open = true;
        self.install_drag_reset = false;
        self.search.query.push_str(&printable);
        self.refilter();
    }

    fn schedule_autohide(&mut self) {
        let delay = Duration::from_millis(u64::from(self.config.input.autohide_delay_ms));
        self.schedule_hide_after(delay);
    }

    /// Arm a one-shot hide after `delay` (unless the pointer is back on
    /// the dock when it fires). A later call supersedes an earlier one.
    fn schedule_hide_after(&mut self, delay: Duration) {
        let deadline = Instant::now() + delay;
        self.hide_deadline = Some(deadline);
        let timer = Timer::from_duration(delay);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.hide_deadline == Some(deadline) {
                    app.hide_deadline = None;
                    // The pointer may be back on the dock without a fresh
                    // Enter to have cancelled us (stale focus) — don't hide
                    // out from under it.
                    if !matches!(app.hover_at_pointer(), Some(Hit::DockIcon(_))) {
                        // Fully hide, or just fall back to the parked dock
                        // when the zone is free (intellihide).
                        app.dismiss();
                    }
                }
                TimeoutAction::Drop
            })
        {
            warn!("failed to arm auto-hide timer: {e}");
            self.hide_deadline = None;
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // Known risk area: fractional scaling. Integer scale only for now;
        // wp-fractional-scale-v1 is P5 polish.
        info!("scale factor: {new_factor}");
        self.scale_factor = new_factor;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        self.frame_pending = false;
        if self.ui.is_animating() || self.bounce.is_some() || self.dirty {
            self.draw();
        } else {
            self.last_frame = None;
            debug!("settled in {:?}, going idle", self.ui.target());
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // Route by surface: the OPTIONS topbar has its own renderer and draw
        // path; everything below is the dock/card.
        if self
            .options_layer
            .as_ref()
            .is_some_and(|opt| opt.wl_surface() == layer.wl_surface())
        {
            self.configure_options(configure);
            return;
        }
        let (mut width, mut height) = configure.new_size;
        if width == 0 || height == 0 {
            let (sw, sh) = self.scaled_surface_size();
            if width == 0 {
                width = sw;
            }
            if height == 0 {
                height = sh;
            }
        }
        debug!("configure: {width}x{height}");
        // `new_size` is logical (surface-local). Input regions and pointer
        // hit-testing work in this space, so `buffer_size` stays logical.
        self.buffer_size = (width, height);
        // The wgpu framebuffer is physical: `logical × render_scale`, matching
        // the `set_buffer_scale` declared on the surface.
        let scale = self.config.window.render_scale.max(1);
        let (pw, ph) = (width * scale, height * scale);

        match self.renderer.as_mut() {
            Some(renderer) => renderer.resize(pw, ph),
            None => match Renderer::new(&self.conn, self.layer.wl_surface(), pw, ph, scale) {
                Ok(mut renderer) => {
                    if let Some(icons) = self.pending_icons.take() {
                        renderer.set_icons(&icons);
                    }
                    self.renderer = Some(renderer);
                    self.upload_pkg_icons();
                }
                Err(e) => {
                    error!("renderer init failed: {e:#}");
                    self.exit = true;
                    return;
                }
            },
        }
        // The buffer size is now known: restrict pointer input to the
        // visible content (empty region while hidden = click-through).
        self.input_extent = None;
        self.sync_surface_state();
        // First (and per-configure) draw: layer-shell requires a commit in
        // response to configure; presenting a frame satisfies it.
        self.draw();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let lh = self.loop_handle.clone();
            match self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                lh,
                Box::new(
                    |app: &mut App, _kb: &wl_keyboard::WlKeyboard, event: KeyEvent| {
                        app.handle_key_event(event.keysym, event.utf8.as_deref());
                    },
                ),
            ) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(e) => warn!("cannot get keyboard: {e}"),
            }
        }
        if capability == Capability::Keyboard && self.data_device.is_none() {
            // The clipboard rides the seat: create its data device once.
            self.data_device = self
                .data_device_manager
                .as_ref()
                .map(|mgr| mgr.get_data_device(qh, &seat));
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            // Raw wl_pointer (see the Dispatch impl below for why sctk's
            // frame-batched pointer helper is not used).
            let pointer = seat.get_pointer(qh, ());
            // Its cursor-shape device (pointing hand over clickables).
            self.cursor_device = self
                .cursor_shape
                .as_ref()
                .map(|mgr| mgr.get_shape_device(&pointer, qh));
            self.pointer = Some(pointer);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(device) = self.cursor_device.take() {
                device.destroy();
            }
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        debug!("keyboard focus gained");
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        debug!(
            "keyboard focus lost; target={:?}, restore={:?}",
            self.ui.target(),
            self.restore_window
        );
        if self.ui.target() == Target::Open {
            // Still open when the keyboard left us: the user focused
            // another window (alt-tab, click elsewhere). Respect their
            // choice — drop the return target and collapse to the dock.
            self.restore_window = None;
            self.handle_command(Command::Collapse);
        } else {
            // We initiated the close. Don't refocus yet: the card is
            // still a full-size layer over the cursor and mid-animation,
            // so a focus now gets clobbered by follow_mouse. Defer it to
            // the settle, when the layer has shrunk out from under the
            // pointer and gone quiet.
            self.pending_refocus = self.restore_window.take();
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key_event(event.keysym, event.utf8.as_deref());
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

/// Raw `wl_pointer` dispatch — deliberately *not* sctk's `PointerHandler`.
///
/// sctk batches pointer events and only delivers them when a
/// `wl_pointer.frame` event arrives, but this compositor (Hyprland) only
/// sends `frame` alongside enter/leave/button: plain motion arrives
/// frameless and would sit buffered forever, freezing hover on whatever
/// the enter event hit first. Processing each event as it arrives is
/// what the mainstream toolkits do and works on both behaviors.
impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Latch which surface the pointer entered; the OPTIONS topbar owns its
        // own pointer handling and must never fall through into the dock's.
        if let wl_pointer::Event::Enter { surface, .. } = &event {
            app.pointer_surface = app.classify_pointer_surface(surface);
        }
        if app.pointer_surface == options::PointerSurface::Options {
            app.options_pointer(event);
            return;
        }
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                // A fresh enter: remember its serial and re-send the shape
                // (the compositor forgot our cursor on leave).
                app.enter_serial = serial;
                app.cursor_now = None;
                // Any pending auto-hide is off: the pointer is back.
                app.hide_deadline = None;
                app.pointer_pos = Some((surface_x as f32, surface_y as f32));
                // While hidden, only the edge-reveal strip is
                // pointer-sensitive, so entering means "summon".
                if app.ui.target() == Target::Hidden && app.config.input.edge_reveal {
                    app.handle_command(Command::Show);
                }
                app.update_hover();
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let prev_pos = app.pointer_pos; // save before update for velocity
                let pos = (surface_x as f32, surface_y as f32);
                app.pointer_pos = Some(pos);
                // Edge-crossing ripple: fire whenever the pointer crosses the
                // card boundary (inside ↔ outside), regardless of click.
                {
                    let layout = app.current_layout();
                    let now_inside = !app.outside_card(&layout, pos);
                    if now_inside != app.pointer_inside_card {
                        if let Some(renderer) = app.renderer.as_mut() {
                            renderer.record_click(pos.0, pos.1);
                        }
                        if app.ui.target() == Target::Open {
                            let rect = content::Rect::new(
                                layout.card_x,
                                layout.card_top,
                                layout.card_w,
                                layout.card_h,
                            );
                            app.jelly.poke(rect, pos, prev_pos, now_inside);
                            app.schedule_frame();
                        }
                    }
                    app.pointer_inside_card = now_inside;
                }
                // Box jelly: same edge-crossing poke for the open group box panel.
                if let Some(br) = app.current_layout().open_box {
                    let now_inside_box = br.contains(pos);
                    if now_inside_box != app.pointer_inside_box {
                        if let Some(renderer) = app.renderer.as_mut() {
                            renderer.record_click(pos.0, pos.1);
                        }
                        app.box_jelly.poke(br, pos, prev_pos, now_inside_box);
                        app.schedule_frame();
                    }
                    app.pointer_inside_box = now_inside_box;
                } else {
                    app.pointer_inside_box = false;
                }
                // Hiding under a parked cursor keeps our (now stale)
                // pointer focus — the compositor never re-sends Enter for
                // the reveal strip. Treat in-strip motion as the edge
                // touch it is, or the strip stays dead until the pointer
                // fully leaves and returns.
                // Summon back from the reveal strip, or from the dock
                // itself while it is mid-hide: as the input region shrinks
                // during a hide this Hyprland keeps stale pointer focus and
                // sends motion, never a fresh Enter, so a pointer wandering
                // back onto the sliding dock would otherwise be ignored and
                // it would vanish out from under the cursor.
                if app.ui.target() == Target::Hidden
                    && (app.pointer_on_reveal_strip()
                        || matches!(app.hover_at_pointer(), Some(Hit::DockIcon(_))))
                {
                    app.hide_deadline = None;
                    app.handle_command(Command::Show);
                }
                // Detect drag start: press armed and pointer moved beyond
                // the 6-px threshold (distinguishes drag from sloppy click).
                if app.box_drag.is_some() {
                    app.update_box_drag(pos);
                } else if app.gesture.dragging.is_none() {
                    if let (Some(pp), Some(hit)) = (app.gesture.press_pos, app.gesture.pressed) {
                        let dx = pos.0 - pp.0;
                        let dy = pos.1 - pp.1;
                        if dx * dx + dy * dy > 6.0 * 6.0 {
                            if let Hit::OpenBoxCell(k) = hit {
                                // Reorder the member within the box (drag it
                                // out to pull it into the grid instead).
                                app.begin_box_drag(k, pos);
                            } else {
                                let entry_idx = match hit {
                                    Hit::DockIcon(slot) => app.dock_order.get(slot).copied(),
                                    Hit::GridCell(s, cell) => {
                                        app.search.visible[s].get(cell).copied()
                                    }
                                    Hit::SearchButton | Hit::OpenBoxCell(_) => None,
                                };
                                // Cells with a profile mutation in flight
                                // can't start a new drag.
                                // Busy cells (mutation in flight) and the
                                // ".." navigation tile never start a drag.
                                let undraggable = entry_idx.is_some_and(|i| {
                                    app.entries.get(i).is_some_and(|e| {
                                        app.busy_ids.contains(&e.id) || e.id == files::FILES_UP_ID
                                    })
                                });
                                if let (Some(entry_idx), false) = (entry_idx, undraggable) {
                                    let from_dock = matches!(hit, Hit::DockIcon(_));
                                    app.gesture.dragging = Some(DragState {
                                        entry_idx,
                                        from_dock,
                                        pos,
                                    });
                                    app.gesture.pressed = None;
                                    // Grabbing an icon out of Install resets the
                                    // grid/Files to their resting layout so it
                                    // can be dropped on an exact slot. Refilter
                                    // to rebuild the resting grid now; the
                                    // dragged transient keeps its index (built
                                    // before the grid branch) but relocate it by
                                    // id to be safe.
                                    if matches!(hit, Hit::GridCell(s, _) if s == content::SECTION_INSTALL)
                                    {
                                        let dragged_id = app.entries[entry_idx].id.clone();
                                        app.install_drag_reset = true;
                                        app.refilter();
                                        if let Some(i) =
                                            app.entries.iter().position(|e| e.id == dragged_id)
                                        {
                                            if let Some(d) = app.gesture.dragging.as_mut() {
                                                d.entry_idx = i;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(ref mut drag) = app.gesture.dragging {
                    drag.pos = pos;
                }
                if app.ui.target() != Target::Hidden {
                    if app.gesture.dragging.is_none() {
                        app.update_hover();
                    }
                    app.schedule_frame();
                }
                // Drags start/convert on motion: keep the cursor honest.
                app.apply_cursor();
            }
            wl_pointer::Event::Leave { .. } => {
                app.scroll.accum = 0.0;
                app.pointer_pos = None;
                app.gesture.pressed = None;
                app.gesture.press_pos = None;
                // An in-box reorder drag drops where it is (the box stays).
                if app.box_drag.is_some() {
                    app.drop_box_drag();
                    return;
                }
                // If a dock drag is in flight when the pointer leaves the
                // surface, treat it as a drop outside the dock: unpin and
                // leave the popup open (don't autohide).
                if let Some(drag) = app.gesture.dragging.take() {
                    app.drop_drag(drag, None, false);
                    return; // skip autohide — keep popup open
                }
                app.update_hover();
                if app.ui.target() != Target::Hidden {
                    app.schedule_frame(); // relax any magnification
                    if app.config.input.autohide {
                        if app.ui.target() == Target::Open {
                            // Full popup is up and the pointer left — the user
                            // clicked or moved to another window. Dismiss now;
                            // no grace period needed (the card is fully open,
                            // not just a slim dock sliver to accidentally graze).
                            app.dismiss();
                        } else if !app.rest_hide_pending && !app.zone_free {
                            // Zone is occupied — hide after the grace period.
                            // When zone_free the dock parks visible instead.
                            debug!("pointer left, zone occupied → scheduling autohide");
                            app.schedule_autohide();
                        } else if app.zone_free {
                            debug!("pointer left, zone free → dock parks visible");
                        }
                    }
                }
            }
            wl_pointer::Event::Button { button, state, .. } if button == BTN_LEFT => {
                match state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        app.update_hover();
                        app.gesture.pressed = app.hover;
                        app.gesture.press_pos = app.pointer_pos;
                        if let (Some((x, y)), Some(renderer)) =
                            (app.pointer_pos, app.renderer.as_mut())
                        {
                            // Click ripple only. The box wave (a full-surface
                            // darkening sweep) is reserved for box open/close,
                            // not clicks inside an open box.
                            renderer.record_click(x, y);
                        }
                    }
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        app.gesture.press_pos = None;
                        if app.box_drag.is_some() {
                            let rp = app.drop_ripple_pos();
                            app.drop_box_drag();
                            if let (Some(r), Some((x, y))) = (app.renderer.as_mut(), rp) {
                                r.record_click(x, y);
                            }
                        } else if app.gesture.dragging.is_some() {
                            let rp = app.drop_ripple_pos();
                            let drag = app.gesture.dragging.take().unwrap();
                            let layout = app.current_layout();
                            let insert = app.drag_dock_insert(&layout, drag.pos);
                            app.drop_drag(drag, insert, true);
                            if let (Some(r), Some((x, y))) = (app.renderer.as_mut(), rp) {
                                r.record_click(x, y);
                            }
                        } else {
                            // Native button behavior: activate on release,
                            // only if it happens on the item the press armed
                            // (dragging away cancels the click).
                            app.update_hover();
                            if let Some(hit) = app.gesture.pressed.take() {
                                if app.hover == Some(hit) {
                                    if let (Some(renderer), Some((x, y))) =
                                        (app.renderer.as_mut(), app.pointer_pos)
                                    {
                                        renderer.record_click(x, y);
                                    }
                                    app.activate_hit(hit);
                                }
                                // else: drag-cancel — do nothing.
                            } else if app.stack_open() {
                                // A box is open: a click off it closes the box,
                                // not the launcher.
                                app.close_group();
                            } else if app.ui.target() == Target::Open {
                                // Dismiss only if the click landed outside the
                                // card bounds (transparent surface margin).
                                // Clicks on the card background itself are inert.
                                if let Some(pos) = app.pointer_pos {
                                    if app.outside_card(&app.current_layout(), pos) {
                                        app.dismiss();
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                // Presses arm drags and releases drop them: keep the
                // cursor honest either way.
                app.apply_cursor();
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_MIDDLE
                    && state == WEnum::Value(wl_pointer::ButtonState::Released) =>
            {
                // Middle-click a dock/grid app: force a fresh instance even
                // if it is already running (Linux-native "new window").
                app.update_hover();
                if let Some(hit @ (Hit::DockIcon(_) | Hit::GridCell(..))) = app.hover {
                    app.force_new_instance = true;
                    app.activate_hit(hit);
                    app.force_new_instance = false;
                }
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_RIGHT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released) =>
            {
                app.update_hover();
                match app.hover {
                    // Right-click on a Files cell opens a terminal in that
                    // directory (a file's containing folder).
                    Some(Hit::GridCell(s, cell)) if s == content::SECTION_FILES => {
                        if let Some(entry_idx) = app.search.visible[s].get(cell).copied() {
                            app.open_terminal_at(entry_idx);
                        }
                    }
                    // Right-click a dock or grid app: force a fresh instance
                    // (the accessible, pointer-native "new window").
                    Some(hit @ (Hit::DockIcon(_) | Hit::GridCell(..) | Hit::OpenBoxCell(_))) => {
                        app.force_new_instance = true;
                        app.activate_hit(hit);
                        app.force_new_instance = false;
                    }
                    _ => {}
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => {
                app.on_scroll(value);
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                value,
                ..
            } => {
                app.on_hscroll(value);
            }
            wl_pointer::Event::AxisStop {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                ..
            } => {
                // Gesture ended: clear the expand-bleed cooldown immediately so
                // the next gesture can navigate pages without the 300 ms wait.
                app.scroll.open_at = None;
            }
            _ => {} // frame, axis metadata, other buttons/axes
        }
    }
}

// cursor-shape-v1 has no events on either object; the impls exist only
// to satisfy the binding bounds.
impl Dispatch<WpCursorShapeManagerV1, GlobalData> for App {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeManagerV1,
        _: <WpCursorShapeManagerV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        unreachable!("wp_cursor_shape_manager_v1 has no events")
    }
}

impl Dispatch<WpCursorShapeDeviceV1, GlobalData> for App {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeDeviceV1,
        _: <WpCursorShapeDeviceV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        unreachable!("wp_cursor_shape_device_v1 has no events")
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_layer!(App);
delegate_registry!(App);
delegate_shm!(App);

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        // Only reached once `shm` is bound (WlShm dispatch can't fire otherwise).
        self.shm.as_mut().expect("shm bound before wl_shm dispatch")
    }
}

// Clipboard plumbing: we only ever *read* the selection on Ctrl+V, so
// every data-device / offer / source callback is a no-op — the paste
// path pulls the current offer lazily instead of tracking events.
impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_device::WlDataDevice,
        _: f64,
        _: f64,
        _: &wl_surface::WlSurface,
    ) {
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_data_device::WlDataDevice) {}

    fn motion(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_device::WlDataDevice,
        _: f64,
        _: f64,
    ) {
    }

    fn selection(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_device::WlDataDevice,
    ) {
        debug!("clipboard: selection offer received");
    }

    fn drop_performed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_device::WlDataDevice,
    ) {
    }
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_source::WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_source::WlDataSource,
        _: String,
        _: WritePipe,
    ) {
    }

    fn cancelled(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_source::WlDataSource,
    ) {
    }

    fn dnd_dropped(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_source::WlDataSource,
    ) {
    }

    fn dnd_finished(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_source::WlDataSource,
    ) {
    }

    fn action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_data_source::WlDataSource,
        _: DndAction,
    ) {
    }
}

fn load_icon_size() -> usize {
    let path = persist::data_path("ui_state.json");
    let n: usize = persist::read_json(&path).unwrap_or(1);
    n.min(content::ICON_SCALES.len() - 1)
}

/// Interleave a few webapp hits through a longer package-hit list so the
/// storefront recommendations sit among the programs rather than clustered
/// at the top. Order within each input is preserved.
fn blend_hits(webapps: Vec<usize>, pkgs: Vec<usize>) -> Vec<usize> {
    if webapps.is_empty() {
        return pkgs;
    }
    if pkgs.is_empty() {
        return webapps;
    }
    let step = (pkgs.len() / (webapps.len() + 1)).max(1);
    let mut out = Vec::with_capacity(webapps.len() + pkgs.len());
    let mut wi = 0;
    for (i, &p) in pkgs.iter().enumerate() {
        if wi < webapps.len() && i == (wi + 1) * step {
            out.push(webapps[wi]);
            wi += 1;
        }
        out.push(p);
    }
    out.extend_from_slice(&webapps[wi..]);
    out
}

fn save_icon_size(size: usize) {
    let path = persist::data_path("ui_state.json");
    persist::write_json("ui_state", &path, &size);
}

smithay_client_toolkit::delegate_data_device!(App);
