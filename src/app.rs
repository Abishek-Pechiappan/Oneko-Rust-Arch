// Shared Wayland/SCTK state plus the event-dispatch boilerplate.
//
// `App` owns everything process-wide (registry/output/seat/shm/pool/globals,
// the cursor backend, and the list of per-monitor cats). The *Handler impls
// below just wire it up to receive protocol events; most methods are no-ops
// because this app doesn't care about those events. Only edit these if you're
// changing what Wayland events the cat reacts to.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::client::{
        protocol::{wl_output, wl_pointer, wl_seat, wl_surface},
        Connection, Proxy, QueueHandle,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};

use std::time::Instant;

use crate::cat::{spawn_cat_surface, CatSurface};
use crate::cursor::CursorSource;
use crate::sprites::Skin;

// linux/input-event-codes.h
const BTN_LEFT: u32 = 0x110;

// Shared Wayland/SCTK plumbing (registry/output/seat/shm/pool/globals),
// owned once for the whole process. Per-monitor cat behavior state lives in
// CatSurface below - one instance per currently-connected output.
pub struct App {
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub seat_state: SeatState,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub shm: Shm,
    pub pool: SlotPool,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub exit: bool, // currently never set to true; the app just idles with zero cats if every output disappears

    pub cats: Vec<CatSurface>,
    pub active_output_id: Option<u32>, // which cat is currently chasing the cursor; the rest stay hidden
    pub rng_state: u32,                // xorshift32 seed/state, shared since only the active cat ever draws from it

    // The character every cat on every monitor draws, chosen once at startup.
    pub skin: &'static Skin,

    // Interval between motion updates while the cat is awake, from --fps.
    // Sleeping overrides this with a fixed slower tick - see `main::tick`.
    pub motion_tick: std::time::Duration,

    // Where global cursor coordinates come from - one impl per compositor.
    // Boxed so adding an X11 / wlroots / KWin backend needs no change here.
    pub cursor: Box<dyn CursorSource>,

    // When the previous tick ran, so each tick can pass a real elapsed time
    // into tick_active instead of assuming a fixed interval. The tick interval
    // is no longer constant (it stretches while the cat sleeps), and every
    // animation threshold in cat.rs is wall-clock, so this has to be measured
    // rather than inferred.
    pub last_tick: Instant,
}
// --- SCTK/Wayland event-dispatch boilerplate below ---
// These trait impls just wire App up to receive protocol events; most
// methods are no-ops because this app doesn't care about those events
// (e.g. we don't need to react to scale/transform changes). Only edit
// these if you're changing what Wayland events the cat reacts to.

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

// Tracks monitor add/remove/geometry-change, keeping `App.cats` in sync: one
// CatSurface (its own layer-shell surface bound to that specific output) per
// currently-connected monitor.
impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&output) else { return };

        let logical_position = info.logical_position.unwrap_or((0, 0));
        let logical_size = info.logical_size.map(|(w, h)| (w as f32, h as f32)).unwrap_or_else(|| {
            eprintln!("oneko: output {} has no logical size yet, defaulting to 1920x1080", info.id);
            (1920.0, 1080.0)
        });

        let init_cursor = self.cursor.position().unwrap_or((0.0, 0.0));
        let cat = spawn_cat_surface(
            self.skin,
            &self.compositor,
            &self.layer_shell,
            qh,
            output,
            info.id,
            logical_position,
            logical_size,
            init_cursor,
        );
        self.cats.push(cat);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&output) else { return };
        let Some(cat) = self.cats.iter_mut().find(|c| c.output_id == info.id) else { return };

        // Only overwrite cached geometry when the fresh info actually has
        // it - a transient `None` here shouldn't clobber a good cached value.
        if let Some(pos) = info.logical_position {
            cat.logical_position = pos;
        }
        if let Some((w, h)) = info.logical_size {
            cat.logical_size = (w as f32, h as f32);
        }
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // Don't rely on output_state.info() here - it may already be gone by
        // the time this fires. Match on the wl_output proxy's own id instead.
        let removed_id = output.id();
        let mut removed_output_id = None;
        self.cats.retain(|c| {
            if c.output.id() == removed_id {
                removed_output_id = Some(c.output_id);
                false
            } else {
                true
            }
        });
        if self.active_output_id == removed_output_id {
            self.active_output_id = None;
        }
    }
}

// The two events that matter for our layer-shell surfaces: the compositor
// telling us one is ready for content (`configure`, gates the first draw for
// that monitor) and telling us one was closed (`closed` - just drop that
// monitor's CatSurface; the app keeps running with whatever monitors remain,
// showing zero cats if none are left, and resumes via new_output on reconnect).
impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        let mut removed_output_id = None;
        self.cats.retain(|c| {
            if c.layer == *layer {
                removed_output_id = Some(c.output_id);
                false
            } else {
                true
            }
        });
        if self.active_output_id == removed_output_id {
            self.active_output_id = None;
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, _: LayerSurfaceConfigure, _: u32) {
        if let Some(cat) = self.cats.iter_mut().find(|c| &c.layer == layer) {
            cat.configured = true;
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

// Binds a pointer as soon as one becomes available, so we can receive click
// events (see PointerHandler below) to toggle `frozen`.
impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = Some(self.seat_state.get_pointer(qh, &seat).expect("create pointer"));
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

// This is the click-to-freeze feature: any left-button press received on a
// cat's surface (only possible within its input region - see
// spawn_cat_surface) toggles that monitor's `frozen`, routed by matching the
// event's surface id against each CatSurface's own wl_surface id.
impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let PointerEventKind::Press { button: BTN_LEFT, .. } = event.kind {
                if let Some(cat) = self
                    .cats
                    .iter_mut()
                    .find(|c| c.layer.wl_surface().id() == event.surface.id())
                {
                    cat.frozen = !cat.frozen;
                }
            }
        }
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
delegate_shm!(App);
delegate_layer!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_registry!(App);
