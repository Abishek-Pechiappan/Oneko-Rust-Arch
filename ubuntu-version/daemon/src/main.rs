// oneko-daemon: the Rust half of the GNOME/Ubuntu backend. Owns all the same
// animation/position state and pixel rendering as the Hyprland binary (both
// go through ../../oneko-core), but instead of talking to Wayland directly,
// it's a D-Bus service that the GNOME Shell extension in ../extension calls
// once per 125ms tick - the extension is the only thing that can get the
// global cursor position and draw an always-on-top overlay on GNOME/Mutter.
//
// Bus name: com.onekorust.Cat, object path: /com/onekorust/Cat, interface:
// com.onekorust.Cat1 - see CatService below for the two methods.
//
// There's no independent poll loop here: the extension's own GLib timer
// drives the 125ms cadence by calling Tick every time it fires, so this
// process just blocks serving D-Bus requests.

use zbus::{blocking::connection, interface};

struct CatService {
    // None until the first Tick call, which spawns the cat at the cursor's
    // position that tick - mirrors the Hyprland backend's
    // spawn_cat_surface, which seeds win_x/win_y from the cursor position
    // at the moment a monitor's surface is created rather than (0, 0).
    cat: Option<oneko_core::Cat>,
    rng_state: u32,
}

#[interface(name = "com.onekorust.Cat1")]
impl CatService {
    // Called by the extension once per animation tick with the current
    // global cursor position and the geometry (in global coordinates) of
    // whichever monitor the cursor is currently on - mirrors the Hyprland
    // backend's per-output clamping, see oneko_core::tick's `bounds` param.
    //
    // Returns `changed = false` (and empty `pixels`) when nothing about the
    // frame changed since the last tick - same dirty-check the Hyprland
    // backend uses to skip redundant redraws - in which case the extension
    // should just leave its actor exactly where it is. When `changed` is
    // true, `actor_x`/`actor_y` are already in global coordinates, ready to
    // set directly on the overlay actor.
    fn tick(
        &mut self,
        x: i32,
        y: i32,
        monitor_x: i32,
        monitor_y: i32,
        monitor_w: i32,
        monitor_h: i32,
    ) -> (bool, i32, i32, u32, u32, Vec<u8>) {
        let local_x = (x - monitor_x) as f32;
        let local_y = (y - monitor_y) as f32;
        let bounds = (monitor_w as f32, monitor_h as f32);

        let cat = self.cat.get_or_insert_with(|| {
            let max_x = (bounds.0 - oneko_core::SIZE as f32).max(0.0);
            let max_y = (bounds.1 - oneko_core::SIZE as f32).max(0.0);
            oneko_core::Cat::new(local_x.clamp(0.0, max_x), local_y.clamp(0.0, max_y))
        });

        match oneko_core::tick(&mut self.rng_state, local_x, local_y, bounds, cat) {
            Some(draw_state) => {
                let actor_x = monitor_x + draw_state.margin_left;
                let actor_y = monitor_y + draw_state.margin_top;
                let pixels = oneko_core::render_frame(draw_state.sprite, draw_state.mask, draw_state.bubble_text);
                (true, actor_x, actor_y, oneko_core::CANVAS_W, oneko_core::CANVAS_H, pixels)
            }
            None => (false, 0, 0, 0, 0, Vec::new()),
        }
    }

    // Called by the extension when a press lands inside the cat's 32x32
    // sub-rect (it does the hit-testing, same trick as the Hyprland
    // backend's Wayland input region - see spawn_cat_surface there).
    fn click(&mut self) {
        if let Some(cat) = &mut self.cat {
            cat.frozen = !cat.frozen;
        }
    }
}

fn main() -> zbus::Result<()> {
    // Seed the PRNG from the clock; xorshift32 needs a nonzero seed, hence `| 1`.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u32)
        .unwrap_or(0x9E37_79B9)
        | 1;

    let service = CatService {
        cat: None,
        rng_state: seed,
    };

    let _conn = connection::Builder::session()?
        .name("com.onekorust.Cat")?
        .serve_at("/com/onekorust/Cat", service)?
        .build()?;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
