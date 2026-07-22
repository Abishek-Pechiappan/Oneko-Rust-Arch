// oneko-rust: a desktop cat that chases your cursor on Hyprland/Wayland.
//
// Sprites, the animation state machine, and pixel rendering live in the
// shared `oneko-core` crate (../../oneko-core) - this binary is just the
// Wayland/wlr-layer-shell plumbing around it: creating one overlay surface
// per connected monitor, reading the cursor position via `hyprctl`, and
// blitting whatever oneko_core::tick() says to draw into an SHM buffer.
//
// How this file is laid out, top to bottom:
//   1. mouse_pos()                     - the one Hyprland-specific call.
//   2. struct App / struct CatSurface  - Wayland state; CatSurface wraps an
//                                        oneko_core::Cat with its own
//                                        layer-shell surface.
//   3. spawn_cat_surface / tick_active - creates a monitor's overlay surface;
//                                        runs one animation tick against it.
//   4. impl CatSurface                 - draw (blit a frame) / hide.
//   5. *Handler impls + delegate_*!    - boilerplate wiring for SCTK/Wayland
//                                        event dispatch; skip unless you're
//                                        changing what Wayland events we react to.
//   6. main()                          - connects to Wayland, creates the
//                                        overlay surface(s), then loops tick().
use std::{thread, time::Duration};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::client::{
        globals::registry_queue_init,
        protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        Connection, Proxy, QueueHandle,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};

use oneko_core::{Cat, DrawState, CANVAS_H, CANVAS_W, CAT_X_OFFSET, CAT_Y_OFFSET, SIZE};

// linux/input-event-codes.h
const BTN_LEFT: u32 = 0x110;

// Global cursor position in screen (layout) coordinates, via Hyprland's own
// CLI. This is the one Hyprland-specific dependency in the whole program;
// porting to another wlroots compositor means replacing this function with
// whatever that compositor exposes for global pointer position. Returns
// (0, 0) if hyprctl isn't available or its output can't be parsed.
fn mouse_pos() -> (f32, f32) {
    let Ok(output) = std::process::Command::new("hyprctl")
        .args(["cursorpos"])
        .output()
    else {
        return (0.0, 0.0);
    };
    let s = String::from_utf8_lossy(&output.stdout);
    let mut parts = s.trim().splitn(2, ", ");
    let x = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let y = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (x, y)
}

// Shared Wayland/SCTK plumbing (registry/output/seat/shm/pool/globals),
// owned once for the whole process. Per-monitor cat behavior state lives in
// CatSurface below - one instance per currently-connected output.
struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    pointer: Option<wl_pointer::WlPointer>,
    shm: Shm,
    pool: SlotPool,
    compositor: CompositorState,
    layer_shell: LayerShell,
    exit: bool, // currently never set to true; the app just idles with zero cats if every output disappears

    cats: Vec<CatSurface>,
    active_output_id: Option<u32>, // which cat is currently chasing the cursor; the rest stay hidden
    rng_state: u32,                // xorshift32 seed/state, shared since only the active cat ever draws from it
}

// One wlr-layer-shell overlay surface bound to a single output, plus the
// shared oneko_core::Cat animation/position state for that monitor.
// Created in OutputHandler::new_output when a monitor appears, dropped in
// output_destroyed/LayerShellHandler::closed when it goes away.
struct CatSurface {
    output: wl_output::WlOutput,
    output_id: u32, // OutputInfo.id - stable key, since wl_output's own Eq/Hash isn't relied on
    layer: LayerSurface,
    _input_region: Region, // kept alive only; never read after setup
    configured: bool,      // true once the compositor has sent an initial configure event
    visible: bool,         // true while this is the monitor currently showing the cat

    logical_position: (i32, i32), // this output's offset in the global/layout coordinate space
    logical_size: (f32, f32),     // this output's size in that same space; used to clamp movement

    state: Cat, // animation/position state, shared with the GNOME backend - see oneko-core
}

// Creates a brand-new overlay surface bound to a specific output (so it can
// only ever render on that monitor - see LayerShell::create_layer_surface's
// `Some(&output)` below, the actual fix for the cat being pinned to a single
// monitor), plus its own input region and freshly-seeded per-monitor state.
// Called from OutputHandler::new_output whenever a monitor appears.
fn spawn_cat_surface(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    qh: &QueueHandle<App>,
    output: wl_output::WlOutput,
    output_id: u32,
    logical_position: (i32, i32),
    logical_size: (f32, f32),
    init_cursor: (f32, f32),
) -> CatSurface {
    let surface = compositor.create_surface(qh);
    let layer = layer_shell.create_layer_surface(
        qh,
        surface,
        Layer::Overlay,
        Some("oneko"),
        Some(&output),
    );
    layer.set_anchor(Anchor::TOP | Anchor::LEFT);
    // Canvas is bigger than the cat (room for a speech bubble above it).
    layer.set_size(CANVAS_W, CANVAS_H);
    // Position relative to the full output, ignoring bars' reserved space.
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);

    // Input region covers only the cat's own sub-rect so it can receive
    // clicks (to toggle frozen state) without the bubble area above it ever
    // blocking clicks.
    let input_region = Region::new(compositor).expect("create input region");
    input_region.add(CAT_X_OFFSET, CAT_Y_OFFSET, SIZE as i32, SIZE as i32);
    layer.wl_surface().set_input_region(Some(input_region.wl_region()));

    layer.commit();

    let max_x = (logical_size.0 - SIZE as f32).max(0.0);
    let max_y = (logical_size.1 - SIZE as f32).max(0.0);
    let win_x = (init_cursor.0 - logical_position.0 as f32).clamp(0.0, max_x);
    let win_y = (init_cursor.1 - logical_position.1 as f32).clamp(0.0, max_y);

    CatSurface {
        output,
        output_id,
        layer,
        _input_region: input_region,
        configured: false,
        visible: true,
        logical_position,
        logical_size,
        state: Cat::new(win_x, win_y),
    }
}

// Runs once per 125ms main-loop iteration, but only for the CatSurface whose
// output currently contains the cursor (see the `while` loop in main());
// every other monitor's cat is just hidden, not ticked. `local_x`/`local_y`
// are the global cursor position already converted into this monitor's own
// coordinate space, so all the math below is identical to single-monitor
// oneko - it just runs against whichever monitor is active right now.
fn tick_active(rng_state: &mut u32, pool: &mut SlotPool, local_x: f32, local_y: f32, cat: &mut CatSurface) {
    if let Some(new_state) = oneko_core::tick(rng_state, local_x, local_y, cat.logical_size, &mut cat.state) {
        cat.layer.set_margin(new_state.margin_top, 0, 0, new_state.margin_left);
        cat.draw(pool, &new_state);
    }
}

impl CatSurface {
    // Renders one frame: gets the pixel bytes from oneko_core::render_frame,
    // copies them into a fresh SHM buffer, then attaches and commits it to
    // the surface.
    fn draw(&mut self, pool: &mut SlotPool, ds: &DrawState) {
        let (buffer, canvas) = pool
            .create_buffer(
                CANVAS_W as i32,
                CANVAS_H as i32,
                (CANVAS_W * 4) as i32,
                wl_shm::Format::Argb8888,
            )
            .expect("create shm buffer");

        canvas.copy_from_slice(&oneko_core::render_frame(ds.sprite, ds.mask, ds.bubble_text));

        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, CANVAS_W as i32, CANVAS_H as i32);
        buffer.attach_to(surface).expect("attach buffer");
        self.layer.commit();
    }

    // Unmaps the surface (no buffer attached) so it's invisible, without the
    // cost of allocating/blitting a blank frame - used to hide the cat on
    // every monitor except the one the cursor is currently on.
    fn hide(&mut self) {
        let surface = self.layer.wl_surface();
        surface.attach(None, 0, 0);
        surface.commit();
        // The surface is now blank regardless of what was last drawn, so
        // force the next active tick to redraw even if it computes the same
        // DrawState this monitor had before being hidden.
        self.state.last_drawn = None;
    }
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

        let init_cursor = mouse_pos();
        let cat = spawn_cat_surface(
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
                    cat.state.frozen = !cat.state.frozen;
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the Wayland compositor and discover available globals
    // (compositor, layer-shell, shm, seat, output - the protocols we need).
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;

    // Shared-memory pool every monitor's cat frames get drawn into (see
    // CatSurface::draw). Sized generously since multiple monitors can each
    // have a buffer in flight at once; SlotPool grows further on demand.
    let pool = SlotPool::new((CANVAS_W * CANVAS_H * 4 * 4) as usize, &shm)?;

    // Seed the PRNG from the clock; xorshift32 needs a nonzero seed, hence `| 1`.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u32)
        .unwrap_or(0x9E37_79B9)
        | 1;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        pointer: None,
        shm,
        pool,
        compositor,
        layer_shell,
        exit: false,
        cats: Vec::new(),
        active_output_id: None,
        rng_state: seed,
    };

    // Main loop: one animation tick every 125ms. Every connected monitor
    // gets its own overlay surface, created reactively in
    // OutputHandler::new_output as outputs are announced. Each iteration we
    // read the global cursor position once, figure out which monitor
    // currently contains it, chase/animate only that one, and hide the cat
    // on every other monitor.
    while !app.exit {
        let (cursor_x, cursor_y) = mouse_pos();

        let active_id = app
            .cats
            .iter()
            .find(|c| {
                let (lx, ly) = c.logical_position;
                let (lw, lh) = c.logical_size;
                cursor_x >= lx as f32 && cursor_x < lx as f32 + lw
                    && cursor_y >= ly as f32 && cursor_y < ly as f32 + lh
            })
            .map(|c| c.output_id);
        if active_id.is_some() {
            app.active_output_id = active_id;
        }

        for cat in app.cats.iter_mut() {
            if !cat.configured {
                continue;
            }
            if Some(cat.output_id) == app.active_output_id {
                let local_x = cursor_x - cat.logical_position.0 as f32;
                let local_y = cursor_y - cat.logical_position.1 as f32;
                tick_active(&mut app.rng_state, &mut app.pool, local_x, local_y, cat);
                cat.visible = true;
            } else if cat.visible {
                cat.hide();
                cat.visible = false;
            }
        }

        // Flushes pending requests (margin + buffer commit) and processes events.
        event_queue.roundtrip(&mut app)?;
        thread::sleep(Duration::from_millis(125));
    }

    Ok(())
}
