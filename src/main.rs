// oneko-rust: a desktop cat that chases your cursor on Hyprland/Wayland.
//
// Module map:
//   sprites  - the 32x32 XBM-style cat pose bitmaps, embedded in the binary.
//   font     - 5x7 bitmap font + the speech-bubble renderer.
//   moments  - the random "cat says/does something cute" table. Add phrases here.
//   cursor   - where the global cursor position comes from; one backend per
//              compositor, behind the `CursorSource` trait. Add new compositor
//              support here and nothing else needs to change.
//   cat      - per-monitor cat state, the per-tick behavior logic, and drawing.
//   app      - shared Wayland/SCTK state and the event-dispatch boilerplate.
//   main     - connects to Wayland, then runs the event loop below.

mod app;
mod cat;
mod cursor;
mod font;
mod moments;
mod sprites;
mod xbm;

use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{timer::{TimeoutAction, Timer}, EventLoop},
        calloop_wayland_source::WaylandSource,
        client::{globals::registry_queue_init, Connection},
    },
    registry::RegistryState,
    seat::SeatState,
    shell::{wlr_layer::LayerShell, WaylandSurface},
    shm::{slot::SlotPool, Shm},
};

use app::App;
use cat::{tick_active, CatState, CANVAS_H, CANVAS_W};
use sprites::Skin;

// Default motion updates per second.
//
// The original X11 oneko ran everything at 8 Hz, and the sprite animation still
// does (see SPRITE_FRAME in cat.rs) - that stepped, two-frame walk is the look.
// What used to be stuck at 8 Hz along with it was the cat's *position*, which is
// what made the chase look jerky. Those are now independent, so position can
// update at display rate while the sprite keeps its retro cadence.
//
// 30 rather than 60, on measurement rather than taste. Chasing nonstop on a
// 60 Hz panel, over 60-second samples:
//
//     --fps 8    0.233% of one core
//     --fps 30   0.250%
//     --fps 60   1.000%
//     --fps 90   2.050%
//
// 30 buys visibly smoother motion for essentially nothing over the original
// 8 Hz, while 60 costs 4x that for 2x the rate - superlinear, most likely SHM
// buffer pressure as our cycle approaches how long the compositor holds each
// buffer at 60 Hz. Since this thing runs in the background forever, that's not
// a good trade by default; `--fps 60` is one flag away for anyone who wants it,
// and worth revisiting if the draw path moves to ping-ponged persistent buffers
// and frame callbacks.
//
// Position is committed as an integer layer-shell margin, so there's also
// nothing left to interpolate once the cat is within a pixel of the cursor.
// Pass `--fps 8` for exactly the old stepped chase.
const DEFAULT_FPS: u32 = 30;

// Bounds for --fps. The floor keeps the idle state machine's sub-second
// thresholds meaningful; the ceiling is well past any display refresh rate and
// exists to catch typos rather than to express a real limit.
const MIN_FPS: u32 = 1;
const MAX_FPS: u32 = 240;

// Tick used once the cat is asleep, regardless of --fps. Nothing on screen
// changes while it sleeps (the 2-frame breathing animation only flips twice a
// second, and the dirty-check in tick_active skips identical frames anyway), so
// there is no reason to poll the cursor at motion rate to find that out.
//
// Fixed rather than derived from --fps because what matters here isn't
// smoothness - there is no motion to smooth - but how long the cat takes to
// notice the cursor coming back. Half a second reads as broken rather than
// sleepy. Clicks are unaffected either way: they arrive on the Wayland socket,
// which the event loop watches continuously, so click-to-freeze stays instant no
// matter how slow the tick is.
const SLEEPING_TICK: Duration = Duration::from_millis(250);

const HELP: &str = "\
oneko-rust - a desktop cat that chases your cursor

USAGE:
    oneko-rust [OPTIONS]

OPTIONS:
    -s, --skin <NAME>    Character to draw [default: neko]
        --fps <N>        Motion updates per second, 1-240 [default: 30]
                         The sprite animation keeps its original 8 Hz cadence
                         regardless; this only affects how smoothly the cat
                         moves. Use --fps 8 for the classic stepped chase.
        --list-skins     List available skins and exit
    -h, --help           Print this help and exit
    -V, --version        Print version and exit

SKINS:
    Built-in skins come from the original oneko. Extra skins are read from
    $XDG_CONFIG_HOME/oneko-rust/skins/<name>/ (default
    ~/.config/oneko-rust/skins/<name>/) as .xbm pose files - copy a directory
    out of this repo's assets/skins/ to see the layout and pose names.
";

/// Hand-rolled argument parsing, to keep the dependency list at one crate.
///
/// Returns the chosen skin, or `None` when the program has done what was asked
/// (help, version, listing) and should exit without starting a cat.
fn parse_args() -> Result<Option<(&'static Skin, Duration)>, String> {
    let mut skin_name: Option<String> = None;
    let mut fps = DEFAULT_FPS;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("oneko-rust {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--list-skins" => {
                list_skins();
                return Ok(None);
            }
            "-s" | "--skin" => {
                skin_name = Some(
                    args.next()
                        .ok_or_else(|| format!("{arg} needs a skin name"))?,
                );
            }
            "--fps" => {
                let v = args.next().ok_or_else(|| format!("{arg} needs a number"))?;
                fps = parse_fps(&v)?;
            }
            // Accept --opt=VALUE as well as --opt VALUE; people expect both.
            other => {
                if let Some(v) = other.strip_prefix("--skin=") {
                    skin_name = Some(v.to_owned());
                } else if let Some(v) = other.strip_prefix("--fps=") {
                    fps = parse_fps(v)?;
                } else {
                    return Err(format!("unexpected argument {other:?}\n\n{HELP}"));
                }
            }
        }
    }

    let skin = match skin_name {
        Some(name) => sprites::resolve(&name)?,
        None => sprites::default_skin(),
    };
    // Truncating division is fine and slightly favours the faster rate: 60 fps
    // becomes 16ms (62.5 Hz), and the easing is time-correct either way.
    Ok(Some((skin, Duration::from_millis(1000 / u64::from(fps)))))
}

fn parse_fps(v: &str) -> Result<u32, String> {
    let n: u32 = v
        .parse()
        .map_err(|_| format!("--fps: {v:?} is not a whole number"))?;
    if !(MIN_FPS..=MAX_FPS).contains(&n) {
        return Err(format!("--fps: {n} is outside {MIN_FPS}-{MAX_FPS}"));
    }
    Ok(n)
}

fn list_skins() {
    println!("built-in:");
    for name in sprites::names() {
        println!("  {name}");
    }
    match sprites::user_skin_names() {
        names if names.is_empty() => {
            if let Some(dir) = sprites::user_skin_dir() {
                println!("\nno user skins in {}", dir.display());
            }
        }
        names => {
            println!("\nuser:");
            for name in names {
                println!("  {name}");
            }
        }
    }
}

fn main() {
    // Errors here are things the user can act on - a bad skin name, no
    // supported compositor - so print them plainly rather than letting
    // `Box<dyn Error>` debug-format escaped newlines into the terminal.
    if let Err(e) = run() {
        eprintln!("oneko: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let Some((skin, motion_tick)) = parse_args()? else { return Ok(()) };

    // Pick a cursor backend before touching Wayland: without one there is
    // nothing for the cat to chase, and failing here with a clear message beats
    // mapping an overlay surface and then sitting motionless forever.
    let cursor = cursor::detect().ok_or(
        "oneko: no supported cursor source found.\n\
         \x20 Currently supported: Hyprland (via its IPC socket, detected through\n\
         \x20 HYPRLAND_INSTANCE_SIGNATURE). See src/cursor.rs for how to add a\n\
         \x20 backend for another compositor.",
    )?;
    eprintln!(
        "oneko: skin {}, cursor backend {}, motion {:?}",
        skin.name,
        cursor.name(),
        motion_tick,
    );

    // Connect to the Wayland compositor and discover available globals
    // (compositor, layer-shell, shm, seat, output - the protocols we need).
    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init(&conn)?;
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
        skin,
        motion_tick,
        cursor,
        last_tick: Instant::now(),
    };

    // The event loop watches two things at once: the Wayland socket (so
    // pointer clicks and output hotplug are handled the moment they arrive)
    // and an animation timer. Previously this was a `roundtrip` + 125 ms
    // `thread::sleep`, which forced a server round trip 8x/second whether or
    // not anything had been drawn, and made click-to-freeze wait out the rest
    // of the sleep - up to a full tick of latency on every click.
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let handle = event_loop.handle();

    // WaylandSource also flushes pending requests before the loop blocks, so
    // the margin/buffer commits made in a tick get sent without an explicit
    // flush or a round trip.
    WaylandSource::new(conn, event_queue).insert(handle.clone())?;

    handle.insert_source(Timer::immediate(), |_deadline, _, app: &mut App| {
        let interval = tick(app);
        TimeoutAction::ToDuration(interval)
    })?;

    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
    }

    Ok(())
}

/// One animation tick. Reads the cursor once, figures out which monitor
/// contains it, animates only that monitor's cat and hides the rest.
///
/// Returns how long to wait before the next tick.
fn tick(app: &mut App) -> Duration {
    let now = Instant::now();
    let dt = now.duration_since(app.last_tick);
    app.last_tick = now;

    // No answer this poll means exactly that: leave the cat where it is and try
    // again next tick. The old code substituted (0, 0) on failure, which on a
    // layout with a monitor left of the primary (negative x) is a real
    // on-screen point - so a transient IPC hiccup teleported the cat.
    let Some((cursor_x, cursor_y)) = app.cursor.position() else {
        return app.motion_tick;
    };

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

    let mut state = None;
    for cat in app.cats.iter_mut() {
        if !cat.configured {
            continue;
        }
        if Some(cat.output_id) == app.active_output_id {
            if !cat.visible {
                // Coming back from hidden: restore the cat's click region
                // (hide() swapped in the empty one). Applied by the commit
                // tick_active is guaranteed to make, since hide() cleared
                // last_drawn.
                cat.layer
                    .wl_surface()
                    .set_input_region(Some(cat.input_region.wl_region()));
            }
            let local_x = cursor_x - cat.logical_position.0 as f32;
            let local_y = cursor_y - cat.logical_position.1 as f32;
            state = Some(tick_active(
                &mut app.rng_state,
                &mut app.pool,
                local_x,
                local_y,
                dt,
                cat,
            ));
            cat.visible = true;
        } else if cat.visible {
            cat.hide(&mut app.pool);
            cat.visible = false;
        }
    }

    match state {
        // Never tick slower than asked: at --fps 1 the motion tick is already
        // longer than SLEEPING_TICK, and speeding up to sleep would be absurd.
        Some(CatState::Sleeping) => SLEEPING_TICK.max(app.motion_tick),
        _ => app.motion_tick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_accepts_the_documented_range() {
        assert_eq!(parse_fps("8"), Ok(8));
        assert_eq!(parse_fps("60"), Ok(60));
        assert_eq!(parse_fps(&MIN_FPS.to_string()), Ok(MIN_FPS));
        assert_eq!(parse_fps(&MAX_FPS.to_string()), Ok(MAX_FPS));
    }

    #[test]
    fn fps_rejects_nonsense_with_a_message_naming_the_input() {
        for bad in ["0", "241", "-1", "", "60.5", "sixty", "1e3"] {
            let err = parse_fps(bad).expect_err("should reject {bad:?}");
            assert!(err.starts_with("--fps:"), "unhelpful error for {bad:?}: {err}");
        }
    }

    #[test]
    fn the_default_fps_is_inside_its_own_bounds() {
        assert!((MIN_FPS..=MAX_FPS).contains(&DEFAULT_FPS));
        // A zero interval would spin the event loop at 100% CPU.
        assert!(!Duration::from_millis(1000 / u64::from(MAX_FPS)).is_zero());
    }
}
