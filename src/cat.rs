// The cat itself: one layer-shell overlay surface per monitor, plus all the
// per-monitor behavior state (position, facing, idle progression, moments) and
// the per-tick logic that drives it.
//
// Timing note: every duration in here is wall-clock (`Duration`), not a count
// of ticks. That matters because the main loop's tick interval is no longer
// fixed - it comes from --fps and slows down further while the cat is asleep
// (see `App::motion_tick` and `crate::SLEEPING_TICK`). Tick-counted thresholds would silently rescale all
// of the cat's behavior whenever that interval changed, so "sit after 3 ticks"
// became "sit after 375 ms" and so on. The constants below were chosen to be
// exactly the old tick counts times the old fixed 125 ms tick, so behavior at
// the default rate is unchanged.

use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorState, Region},
    reexports::client::{protocol::{wl_output, wl_shm}, QueueHandle},
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface},
        WaylandSurface,
    },
    shm::slot::SlotPool,
};

use crate::app::App;
use crate::font::draw_bubble;
use crate::moments::MOMENTS;
use crate::sprites::{Pose, Skin, Sprite};

// The cat sprite itself is always exactly SIZE x SIZE pixels.
pub const SIZE: u32 = 32;

// The cursor must be within this many pixels of the cat before it'll wake up
// and chase - see the `near` check in tick_active. Cursor movement farther
// away than this is ignored entirely, so the cat settles into its idle
// animations instead of reacting to every mouse movement on screen.
pub const CHASE_RADIUS: f32 = 150.0;

// The Wayland surface/buffer is bigger than the cat (CANVAS_W x CANVAS_H),
// with BUBBLE_H extra rows of empty space above it to draw a speech bubble
// into when one is active. The cat sprite is always drawn bottom-anchored
// and horizontally centered within that canvas, at (CAT_X_OFFSET,
// CAT_Y_OFFSET) - see CatSurface::draw. To make the bubble area bigger/smaller,
// tweak CANVAS_W (width, e.g. for longer phrases) and/or BUBBLE_H (height).
pub const CANVAS_W: u32 = 72;
pub const BUBBLE_H: u32 = 16;
pub const CANVAS_H: u32 = SIZE + BUBBLE_H;
pub const CAT_X_OFFSET: i32 = ((CANVAS_W - SIZE) / 2) as i32;
pub const CAT_Y_OFFSET: i32 = BUBBLE_H as i32;

// How long the cursor must sit still before the cat steps down through its idle
// animations. The ladder is the original oneko's, whose states are (with its own
// Japanese comments) STOP 立ち止まった, JARE 顔を洗っている "washing face",
// KAKI 頭を掻いている "scratching head", AKUBI あくびをしている "yawning", then
// SLEEP 寝てしまった. Thresholds are cumulative from when the cursor went still.
//
// Sleep now arrives at 3.75s rather than 2.5s, because two stages were added
// ahead of it. That's the point: the wind-down reads as a sequence of things the
// cat is doing rather than a two-step fade.
const SIT_AFTER: Duration = Duration::from_millis(375);
const WASH_AFTER: Duration = Duration::from_millis(1250);
const SCRATCH_AFTER: Duration = Duration::from_millis(2500);
const YAWN_AFTER: Duration = Duration::from_millis(3125);
const SLEEP_AFTER: Duration = Duration::from_millis(3750);

// How long the "just woke up" pose holds when the cursor comes back to a settled
// cat (oneko's NEKO_AWAKE_TIME, 3 ticks). The cat doesn't chase during it.
const AWAKE_TIME: Duration = Duration::from_millis(375);

// Wall-scratching has no threshold of its own: it takes over the washing slot,
// so the ladder's next rung ends it. That slot's length is asserted against
// oneko's NEKO_TOGI_TIME in the tests.

// How long each frame of the 2-frame sleeping animation holds (was 4 ticks).
const SLEEP_FRAME: u128 = 500;

// How long each frame of a 2-frame walk/wash animation holds.
//
// This is deliberately independent of the tick interval. The walk cycle is only
// two frames, so flipping it once per tick was fine at 8 ticks/second but turns
// into a strobe at 60 - the cat would vibrate rather than walk. Motion now
// updates every tick while the sprite advances on this fixed clock, which is
// what lets the tick rate rise without changing how the animation reads.
const SPRITE_FRAME: Duration = Duration::from_millis(125);

// The reference tick the chase easing was originally tuned against, and the
// fraction of the remaining distance the cat closed per such tick. `EASE_KEEP`
// is the fraction left over (1 - 0.3); see the chase block in tick_active for
// why the math is expressed that way.
const EASE_REFERENCE: Duration = Duration::from_millis(125);
const EASE_KEEP: f32 = 0.7;

// A moment shows for ~0.75-1.25s, with a ~30-100s quiet period between them.
// Tune these to change how chatty the cat is.
const MOMENT_SHOW_MIN_MS: u32 = 750;
const MOMENT_SHOW_SPREAD_MS: u32 = 500;
const MOMENT_GAP_MIN_MS: u32 = 30_000;
const MOMENT_GAP_SPREAD_MS: u32 = 70_001;
// Quiet period before the very first moment after startup.
const MOMENT_FIRST_GAP: Duration = Duration::from_millis(37_500);

// The cat's current activity, driven by how long the cursor has been idle
// (see `idle_for` and the thresholds above).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CatState {
    Chasing,
    /// Pressed against a screen edge with the cursor still beyond it, scratching
    /// the wall. The `Dir` is which wall, always a cardinal.
    WallScratch(Dir),
    Sitting,
    Washing,
    Scratching,
    Yawning,
    Sleeping,
    /// Briefly startled awake after the cursor returns to a settled cat.
    Waking,
}

impl CatState {
    /// Whether the cat has settled into its idle sequence, as opposed to
    /// chasing, waking, or working on a wall. Only settled states get random
    /// Moments - a speech bubble mid-startle or mid-scratch would step on a
    /// pose that's saying something already.
    fn is_settled(self) -> bool {
        matches!(
            self,
            CatState::Sitting
                | CatState::Washing
                | CatState::Scratching
                | CatState::Yawning
                | CatState::Sleeping
        )
    }
}

// 8-way compass direction the cat is facing/running, used to pick which
// CAT_* sprite pair to show while Chasing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Dir {
    /// All eight directions, in the same order as a `Skin`'s `run` table.
    pub const ALL: [Dir; 8] = [
        Dir::N, Dir::NE, Dir::E, Dir::SE, Dir::S, Dir::SW, Dir::W, Dir::NW,
    ];

    /// Index into a skin's `run` table. Written as an explicit match rather
    /// than `self as usize` so the mapping can't silently change if the enum
    /// is ever reordered - build.rs's RUN_POSES order is the other half of this
    /// contract, and a test in sprites.rs pins them together.
    /// The `assets/skins/<skin>/` filename stem for this direction's walk
    /// frames, minus the trailing frame number. Must match build.rs's
    /// RUN_POSES, which a test pins.
    pub fn asset_name(self) -> &'static str {
        match self {
            Dir::N => "run_n",
            Dir::NE => "run_ne",
            Dir::E => "run_e",
            Dir::SE => "run_se",
            Dir::S => "run_s",
            Dir::SW => "run_sw",
            Dir::W => "run_w",
            Dir::NW => "run_nw",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Dir::N => 0,
            Dir::NE => 1,
            Dir::E => 2,
            Dir::SE => 3,
            Dir::S => 4,
            Dir::SW => 5,
            Dir::W => 6,
            Dir::NW => 7,
        }
    }
}

// Turns a cursor-relative offset into one of the 8 compass directions.
// Uses a 2:1 ratio to bias toward the 4 cardinal directions (N/E/S/W) over
// the diagonals, so the cat doesn't flicker between them too easily.
pub fn dir_from_delta(dx: f32, dy: f32) -> Dir {
    let adx = dx.abs();
    let ady = dy.abs();
    if adx > ady * 2.0 {
        if dx > 0.0 { Dir::E } else { Dir::W }
    } else if ady > adx * 2.0 {
        if dy > 0.0 { Dir::S } else { Dir::N }
    } else {
        match (dx >= 0.0, dy <= 0.0) {
            (true,  true)  => Dir::NE,
            (false, true)  => Dir::NW,
            (true,  false) => Dir::SE,
            (false, false) => Dir::SW,
        }
    }
}

/// Which wall the cat is pressed against and still trying to walk through, if
/// any. `dx`/`dy` are the cursor relative to the cat's centre.
///
/// This is oneko's own rule - `NekoMoveDx < 0 && NekoX <= 0` and its three
/// mirrors - rather than "did the move overshoot": the cat has already stopped
/// by the time this matters, so there is no movement left to overshoot with.
/// Checked in oneko's order, so a corner resolves to the horizontal wall.
fn blocked_wall(
    win_x: f32,
    win_y: f32,
    dx: f32,
    dy: f32,
    max_x: f32,
    max_y: f32,
) -> Option<Dir> {
    if dx < 0.0 && win_x <= 0.0 {
        Some(Dir::W)
    } else if dx > 0.0 && win_x >= max_x {
        Some(Dir::E)
    } else if dy < 0.0 && win_y <= 0.0 {
        Some(Dir::N)
    } else if dy > 0.0 && win_y >= max_y {
        Some(Dir::S)
    } else {
        None
    }
}

/// Fraction of the remaining distance to close over `dt`.
///
/// The original code did `pos += delta * 0.3` once per fixed 125 ms tick, which
/// silently means "speed" is defined per tick rather than per second: double the
/// tick rate and the cat closes distance twice as fast. Now that the tick
/// interval varies - it already stretches while the cat sleeps, and is
/// configurable - that has to be expressed as a rate.
///
/// Exponential decay is the form that composes correctly: keeping 70% of the
/// distance once per 125 ms is the same as keeping `0.7^(dt/125ms)` over any
/// `dt`, so two 62.5 ms steps land exactly where one 125 ms step would. At
/// `dt == 125ms` this returns 0.3 and behavior is bit-for-bit the old behavior.
///
/// Saturates at 1.0 for a large enough `dt` (f32 underflow in the power), which
/// is the right limit: after a suspend/resume the cat should arrive, not crawl
/// back across the missing seconds. The result is always in `(0.0, 1.0]`, so it
/// can never overshoot the cursor.
fn ease_alpha(dt: Duration) -> f32 {
    let steps = dt.as_secs_f32() / EASE_REFERENCE.as_secs_f32();
    1.0 - EASE_KEEP.powf(steps)
}

// Tiny xorshift32 PRNG (no external `rand` dependency needed for
// occasionally picking a random Moment). Must be seeded with a nonzero
// value once at startup - see `seed` in main().
pub fn next_u32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

// Everything about a tick that actually affects the rendered frame. Compared
// against the previous tick's state so tick_active can skip the SHM
// allocate/blit/damage/commit entirely when nothing would visually change -
// see the dirty-check at the end of tick_active.
//
// `pose` identifies the art by value rather than by reference. The old version
// held `&'static [u8; 128]` pairs and had to compare their contents, because
// LLVM's constant-merging pass may unify two byte-identical consts, which would
// make pointer equality silently wrong. A `Pose` sidesteps that entirely and is
// cheaper to compare.
#[derive(Clone, Copy, PartialEq)]
pub struct DrawState {
    margin_top: i32,
    margin_left: i32,
    pose: Pose,
    bubble_text: &'static str,
}

// One wlr-layer-shell overlay surface bound to a single output, plus all the
// per-monitor cat behavior state (position, animation, idle/moment timers).
// Created in OutputHandler::new_output when a monitor appears, dropped in
// output_destroyed/LayerShellHandler::closed when it goes away.
pub struct CatSurface {
    /// The character this cat draws. Chosen once at startup and shared by every
    /// monitor's cat, so it lives behind a `'static` reference.
    pub skin: &'static Skin,
    pub output: wl_output::WlOutput,
    pub output_id: u32, // OutputInfo.id - stable key, since wl_output's own Eq/Hash isn't relied on
    pub layer: LayerSurface,
    pub input_region: Region, // covers the cat's 32x32 sub-rect; re-applied when unhiding
    empty_region: Region,     // zero-area region swapped in while hidden so the
                              // invisible-but-still-mapped surface never eats clicks
    pub configured: bool, // true once the compositor has sent an initial configure event
    pub visible: bool,    // true while this is the monitor currently showing the cat

    pub logical_position: (i32, i32), // this output's offset in the global/layout coordinate space
    pub logical_size: (f32, f32),     // this output's size in that same space; used to clamp movement

    win_x: f32, // cat's on-screen position, local to this monitor (top-left of
    win_y: f32, // its own 32x32 box, not the enlarged canvas - see CAT_X/Y_OFFSET)

    last_cursor: (f32, f32), // local cursor position as of the previous active tick, to detect idling
    dir: Dir,                // facing direction while chasing
    frame: bool,             // picks between each pose's 2 animation frames
    sprite_clock: Duration,  // time accumulated toward the next `frame` flip
    idle_for: Duration,      // how long the cursor has been still
    waking_for: Duration,    // time left in the "just woke up" pose
    pub frozen: bool,        // toggled by clicking the cat; see PointerHandler

    next_moment_in: Duration,     // time until the next random speech/quirk moment
    moment_remaining: Duration,   // time left in the currently active moment, if any
    active_moment: Option<usize>, // index into MOMENTS while a moment is active

    last_drawn: Option<DrawState>, // what's currently actually committed to the compositor;
                                   // None forces the next tick to draw regardless - see hide()
}

// Creates a brand-new overlay surface bound to a specific output (so it can
// only ever render on that monitor - see LayerShell::create_layer_surface's
// `Some(&output)` below, the actual fix for the cat being pinned to a single
// monitor), plus its own input region and freshly-seeded per-monitor state.
// Called from OutputHandler::new_output whenever a monitor appears.
#[allow(clippy::too_many_arguments)]
pub fn spawn_cat_surface(
    skin: &'static Skin,
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

    // Deliberately empty (no rects added) - see CatSurface::hide.
    let empty_region = Region::new(compositor).expect("create empty region");

    layer.commit();

    let max_x = (logical_size.0 - SIZE as f32).max(0.0);
    let max_y = (logical_size.1 - SIZE as f32).max(0.0);
    let win_x = (init_cursor.0 - logical_position.0 as f32).clamp(0.0, max_x);
    let win_y = (init_cursor.1 - logical_position.1 as f32).clamp(0.0, max_y);

    CatSurface {
        skin,
        output,
        output_id,
        layer,
        input_region,
        empty_region,
        configured: false,
        visible: true,
        logical_position,
        logical_size,
        win_x,
        win_y,
        last_cursor: (win_x, win_y),
        dir: Dir::E,
        frame: false,
        sprite_clock: Duration::ZERO,
        idle_for: Duration::ZERO,
        waking_for: Duration::ZERO,
        frozen: false,
        next_moment_in: MOMENT_FIRST_GAP,
        moment_remaining: Duration::ZERO,
        active_moment: None,
        last_drawn: None,
    }
}

/// Advances the cat on the monitor that currently contains the cursor; every
/// other monitor's cat is just hidden, not ticked (see the loop in `main`).
///
/// `local_x`/`local_y` are the global cursor position already converted into
/// this monitor's own coordinate space, so all the math below is identical to
/// single-monitor oneko - it just runs against whichever monitor is active
/// right now. `dt` is the wall-clock time since this cat's previous tick.
///
/// Returns the state the cat ended up in, which `main` uses to decide how long
/// to wait before the next tick.
pub fn tick_active(
    rng_state: &mut u32,
    pool: &mut SlotPool,
    local_x: f32,
    local_y: f32,
    dt: Duration,
    cat: &mut CatSurface,
) -> CatState {
    // The cat only wakes up for the cursor once it's within CHASE_RADIUS;
    // movement farther away than that is treated the same as no movement
    // at all, so the cat isn't yanked out of its idle animations by every
    // mouse movement on screen (see CHASE_RADIUS's doc comment). Once it's
    // already chasing, though, don't re-check the distance every tick - a
    // fast-moving cursor can easily outrun the cat's 30%-per-tick easing
    // and briefly land outside the radius mid-chase, which would otherwise
    // make it give up and fall asleep instead of catching up.
    let (cx, cy) = cat.center();
    let dist = ((local_x - cx).powi(2) + (local_y - cy).powi(2)).sqrt();
    let was_chasing = cat.idle_for < SIT_AFTER;
    let near = was_chasing || dist <= CHASE_RADIUS;

    // "Idle" means the cursor has barely moved since last tick (and is near).
    let cursor_moved = near
        && ((local_x - cat.last_cursor.0).abs() > 2.0
            || (local_y - cat.last_cursor.1).abs() > 2.0);
    cat.last_cursor = (local_x, local_y);

    // The cursor came back to a cat that had already settled: play the startled
    // "just woke up" pose before resuming the chase. `was_chasing` is the
    // pre-update view of `idle_for`, so this fires on the transition only.
    if cursor_moved && !was_chasing && cat.skin.has(Pose::Awake) {
        cat.waking_for = AWAKE_TIME;
    }
    cat.waking_for = cat.waking_for.saturating_sub(dt);

    if cursor_moved {
        cat.idle_for = Duration::ZERO;
    } else {
        cat.idle_for += dt;
    }

    // While frozen, keep the cat pinned in place but still let it settle
    // into its idle animations if the cursor stops moving elsewhere.
    let idle_for = if cat.frozen { cat.idle_for.max(SIT_AFTER) } else { cat.idle_for };

    // Everything below aims the cat's centre at the cursor, matching oneko's
    // `MouseX - NekoX - BITMAP_WIDTH / 2`. Tracking by top-left corner instead
    // would park the cat down-and-right of the cursor, and - because the cat
    // could then never want to be further left or higher than the cursor - would
    // make the left and top walls impossible to reach.
    let dx = local_x - cx;
    let dy = local_y - cy;
    let max_x = (cat.logical_size.0 - SIZE as f32).max(0.0);
    let max_y = (cat.logical_size.1 - SIZE as f32).max(0.0);

    // Pressed against an edge with the cursor still beyond it.
    let wall = blocked_wall(cat.win_x, cat.win_y, dx, dy, max_x, max_y)
        .filter(|w| !cat.frozen && cat.skin.has(Pose::Wall(*w, false)));

    // Idle-duration thresholds stepping the cat down through its idle sequence.
    // Stages whose art this skin lacks are skipped rather than substituted, so a
    // minimal skin still works - it just has a shorter wind-down.
    //
    // Waking and wall-scratching sit above the ladder: both describe what the
    // cat is doing right now, which outranks how long the cursor has been still.
    let state = if !cat.waking_for.is_zero() {
        CatState::Waking
    } else if idle_for >= SLEEP_AFTER {
        CatState::Sleeping
    } else if idle_for >= YAWN_AFTER && cat.skin.has(Pose::Yawn) {
        CatState::Yawning
    } else if idle_for >= SCRATCH_AFTER && cat.skin.has(Pose::Scratch(false)) {
        CatState::Scratching
    } else if idle_for >= WASH_AFTER {
        // Upstream reaches the wall-scratch states from STOP in place of JARE,
        // so scratching a wall occupies the washing slot rather than adding a
        // stage: a cat stuck against an edge grooms less and complains more.
        match wall {
            Some(w) => CatState::WallScratch(w),
            None => CatState::Washing,
        }
    } else if idle_for >= SIT_AFTER {
        CatState::Sitting
    } else {
        CatState::Chasing
    };

    // Occasional, non-distracting flavor: a speech bubble and/or a
    // quirky sprite override, only while the cat is already idle.
    // `moment_remaining` counts down while one is showing; `next_moment_in`
    // counts down the (much longer) quiet period between moments. Tune the
    // MOMENT_* constants above to change how often moments happen and how
    // long each one lasts.
    if !cat.moment_remaining.is_zero() {
        cat.moment_remaining = cat.moment_remaining.saturating_sub(dt);
    } else {
        cat.active_moment = None;
        if state.is_settled() {
            if cat.next_moment_in.is_zero() {
                let idx = (next_u32(rng_state) as usize) % MOMENTS.len();
                cat.active_moment = Some(idx);
                cat.moment_remaining = Duration::from_millis(
                    (MOMENT_SHOW_MIN_MS + next_u32(rng_state) % MOMENT_SHOW_SPREAD_MS) as u64,
                );
                cat.next_moment_in = Duration::from_millis(
                    (MOMENT_GAP_MIN_MS + next_u32(rng_state) % MOMENT_GAP_SPREAD_MS) as u64,
                );
            } else {
                cat.next_moment_in = cat.next_moment_in.saturating_sub(dt);
            }
        }
    }

    // Chase the cursor: ease toward it, closing a fraction of the remaining
    // distance so movement looks smooth rather than snapping, clamped so the cat
    // can't leave this monitor. Skipped while frozen or while the cursor is
    // outside CHASE_RADIUS.
    if !cat.frozen && near && cat.waking_for.is_zero() {
        if dx.abs() > 1.0 || dy.abs() > 1.0 {
            cat.dir = dir_from_delta(dx, dy);
        }

        let alpha = ease_alpha(dt);
        cat.win_x = (cat.win_x + dx * alpha).clamp(0.0, max_x);
        cat.win_y = (cat.win_y + dy * alpha).clamp(0.0, max_y);
    }


    // Advance the 2-frame sprite animation on its own clock rather than once per
    // tick, so the tick rate and the animation speed are independent.
    cat.sprite_clock += dt;
    while cat.sprite_clock >= SPRITE_FRAME {
        cat.sprite_clock -= SPRITE_FRAME;
        cat.frame = !cat.frame;
    }

    // Pick which pose to show for the current state/direction/frame.
    let mut pose = match state {
        CatState::Sitting => Pose::Sit,
        CatState::Washing => Pose::Wash(cat.frame),
        CatState::Scratching => Pose::Scratch(cat.frame),
        CatState::Yawning => Pose::Yawn,
        CatState::Waking => Pose::Awake,
        CatState::WallScratch(wall) => Pose::Wall(wall, cat.frame),
        // The sleeping animation runs on its own clock rather than `frame`, so
        // it breathes at a fixed rate regardless of the tick interval.
        CatState::Sleeping => {
            Pose::Sleep((cat.idle_for.as_millis() / SLEEP_FRAME).is_multiple_of(2))
        }
        CatState::Chasing => Pose::Run(cat.dir, cat.frame),
    };

    // If a Moment is currently active, let it supply the speech-bubble text
    // and optionally override the pose. A quirk pose the active skin doesn't
    // provide is skipped, keeping the normal idle pose - only the `neko` art
    // has the hand-authored quirk poses.
    let mut bubble_text: &'static str = "";
    if let Some(idx) = cat.active_moment {
        bubble_text = MOMENTS[idx].text;
        if let Some(quirk) = MOMENTS[idx].quirk {
            if cat.skin.has(quirk) {
                pose = quirk;
            }
        }
    }

    // Anchored TOP|LEFT; the cat's own sub-rect sits CAT_X/Y_OFFSET into
    // the (larger, bubble-carrying) canvas, so shift the margin back by
    // that offset to keep the cat itself tracking win_x/win_y exactly.
    let new_state = DrawState {
        margin_top: cat.win_y as i32 - CAT_Y_OFFSET,
        margin_left: cat.win_x as i32 - CAT_X_OFFSET,
        pose,
        bubble_text,
    };

    // Skip the SHM allocate/blit/damage/commit entirely when the frame
    // would be pixel-for-pixel identical to what's already on screen (e.g.
    // Sitting, most of Sleeping, or a frozen/motionless cat) - this is what
    // was forcing the compositor to recomposite 8x/second forever even
    // while the cat visually never changed.
    if cat.last_drawn != Some(new_state) {
        cat.layer.set_margin(new_state.margin_top, 0, 0, new_state.margin_left);
        cat.draw(pool, pose, bubble_text);
        cat.last_drawn = Some(new_state);
    }

    state
}

impl CatSurface {
    /// The centre of the cat's 32x32 box, which is what chases the cursor.
    fn center(&self) -> (f32, f32) {
        let half = SIZE as f32 / 2.0;
        (self.win_x + half, self.win_y + half)
    }

    // Renders one frame: allocates a fresh ARGB8888 buffer sized to the full
    // canvas, unpacks `sprite`/`mask` into the cat's sub-rect within it
    // (everywhere else starts transparent), optionally draws a speech
    // bubble on top, then attaches and commits the buffer to the surface.
    //
    // XBM layout: 4 bytes per row, LSB of each byte is the leftmost pixel.
    // mask bit set + sprite bit set => black, mask only => white, else transparent.
    fn draw(&mut self, pool: &mut SlotPool, pose: Pose, text: &str) {
        // `pose` is always one this skin provides: tick_active only selects a
        // quirk pose after checking `skin.has`, and every other pose is
        // required of every skin (enforced by build.rs and a test).
        let Sprite { bits, mask } = self
            .skin
            .sprite(pose)
            .expect("tick_active only picks poses the skin provides");

        let (buffer, canvas) = pool
            .create_buffer(
                CANVAS_W as i32,
                CANVAS_H as i32,
                (CANVAS_W * 4) as i32,
                wl_shm::Format::Argb8888,
            )
            .expect("create shm buffer");

        canvas.fill(0); // fully transparent by default

        // Unpack the cat's 32x32 bitmap into its offset sub-rect of the canvas.
        for y in 0..SIZE as usize {
            for x in 0..SIZE as usize {
                let byte = y * 4 + x / 8;
                let bit = 1u8 << (x % 8);
                let px: u32 = if mask[byte] & bit != 0 {
                    if bits[byte] & bit != 0 { 0xFF00_0000 } else { 0xFFFF_FFFF }
                } else {
                    0
                };
                let idx = ((y + CAT_Y_OFFSET as usize) * CANVAS_W as usize
                    + (x + CAT_X_OFFSET as usize))
                    * 4;
                canvas[idx..idx + 4].copy_from_slice(&px.to_le_bytes());
            }
        }

        if !text.is_empty() {
            draw_bubble(canvas, CANVAS_W as usize, text);
        }

        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, CANVAS_W as i32, CANVAS_H as i32);
        buffer.attach_to(surface).expect("attach buffer");
        self.layer.commit();
    }

    // Makes the cat invisible by committing a fully transparent frame - used
    // to hide the cat on every monitor except the one the cursor is on.
    //
    // Deliberately does NOT unmap (attach(None)): unmapping resets the layer
    // surface's configured state compositor-side, requiring a full
    // initial-commit/configure round-trip before a buffer may legally be
    // attached again. A configure event already in flight when the unmap
    // commit lands can then race our `configured` flag back to true early,
    // and the next draw dies with "layerSurface was not configured, but a
    // buffer was attached" (reproducibly hit when moving the cursor between
    // monitors). Staying mapped with transparent pixels sidesteps that whole
    // protocol state machine; the one-off blank frame is cheap since hide()
    // only runs once per hide, not per tick (gated by `visible`).
    pub fn hide(&mut self, pool: &mut SlotPool) {
        // Swap in the empty input region so the invisible surface can't
        // intercept clicks meant for whatever is underneath the cat's spot.
        // Double-buffered state - applied by the commit below.
        let surface = self.layer.wl_surface();
        surface.set_input_region(Some(self.empty_region.wl_region()));

        let (buffer, canvas) = pool
            .create_buffer(
                CANVAS_W as i32,
                CANVAS_H as i32,
                (CANVAS_W * 4) as i32,
                wl_shm::Format::Argb8888,
            )
            .expect("create shm buffer");
        canvas.fill(0); // fully transparent

        surface.damage_buffer(0, 0, CANVAS_W as i32, CANVAS_H as i32);
        buffer.attach_to(surface).expect("attach buffer");
        self.layer.commit();

        // The surface is now blank regardless of what was last drawn, so
        // force the next active tick to redraw even if it computes the same
        // DrawState this monitor had before being hidden.
        self.last_drawn = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinal_directions_win_over_diagonals() {
        // The 2:1 bias means a shallow angle still reads as pure E/W/N/S.
        assert_eq!(dir_from_delta(10.0, 1.0), Dir::E);
        assert_eq!(dir_from_delta(-10.0, 1.0), Dir::W);
        assert_eq!(dir_from_delta(1.0, -10.0), Dir::N);
        assert_eq!(dir_from_delta(1.0, 10.0), Dir::S);
    }

    #[test]
    fn diagonals_inside_the_bias_band() {
        assert_eq!(dir_from_delta(10.0, -10.0), Dir::NE);
        assert_eq!(dir_from_delta(-10.0, -10.0), Dir::NW);
        assert_eq!(dir_from_delta(10.0, 10.0), Dir::SE);
        assert_eq!(dir_from_delta(-10.0, 10.0), Dir::SW);
    }

    #[test]
    fn zero_delta_is_stable() {
        // Never panics / never NaNs; the exact quadrant doesn't matter since
        // tick_active only updates `dir` once the delta exceeds 1px.
        assert_eq!(dir_from_delta(0.0, 0.0), Dir::NE);
    }

    #[test]
    fn ease_alpha_reproduces_the_original_rate_at_the_original_tick() {
        // The whole point of the rewrite is that 125 ms behaves exactly as the
        // old hardcoded `* 0.3` did.
        let a = ease_alpha(EASE_REFERENCE);
        assert!((a - 0.3).abs() < 1e-6, "expected 0.3 at the reference tick, got {a}");
    }

    #[test]
    fn ease_alpha_composes_so_framerate_does_not_change_speed() {
        // This is the property that makes --fps safe: splitting a step into N
        // smaller steps must land the cat in the same place. Expressed on the
        // remaining distance, since that's what multiplies.
        for divisor in [2u32, 4, 8, 15] {
            let whole = 1.0 - ease_alpha(EASE_REFERENCE);
            let part = EASE_REFERENCE / divisor;
            let split = (1.0 - ease_alpha(part)).powi(divisor as i32);
            assert!(
                (whole - split).abs() < 1e-5,
                "splitting into {divisor} steps changed the result: {whole} vs {split}",
            );
        }
    }

    #[test]
    fn ease_alpha_stays_a_sane_fraction() {
        // Never overshoots (>1 would fling the cat past the cursor) and never
        // stalls (<=0 would freeze it), across the full --fps range and beyond.
        //
        // 1.0 is allowed and is the correct limit: for a large enough gap - a
        // suspend/resume, or the process stopped and continued - the cat should
        // simply arrive rather than crawl back over the intervening seconds.
        for ms in [1u64, 4, 8, 16, 125, 250, 1000, 10_000] {
            let a = ease_alpha(Duration::from_millis(ms));
            assert!(a > 0.0 && a <= 1.0, "alpha out of range at {ms}ms: {a}");
        }
        assert_eq!(ease_alpha(Duration::ZERO), 0.0);
        // Monotonic: a longer step always closes more distance.
        let mut prev = 0.0;
        for ms in [1u64, 2, 4, 8, 16, 32, 64, 125, 250] {
            let a = ease_alpha(Duration::from_millis(ms));
            assert!(a > prev, "not monotonic at {ms}ms");
            prev = a;
        }
    }

    #[test]
    fn sprite_clock_advances_at_a_fixed_rate_regardless_of_tick_size() {
        // Simulates the accumulator in tick_active: over one second the sprite
        // must flip the same number of times whether ticks are 125ms or 4ms,
        // otherwise the walk cycle speeds up with --fps.
        fn flips_in_one_second(tick: Duration) -> u32 {
            let mut clock = Duration::ZERO;
            let mut flips = 0;
            let mut elapsed = Duration::ZERO;
            while elapsed < Duration::from_secs(1) {
                elapsed += tick;
                clock += tick;
                while clock >= SPRITE_FRAME {
                    clock -= SPRITE_FRAME;
                    flips += 1;
                }
            }
            flips
        }
        let baseline = flips_in_one_second(Duration::from_millis(125));
        assert_eq!(baseline, 8, "8 Hz sprite cadence");
        for ms in [1u64, 4, 8, 16, 25, 50] {
            assert_eq!(
                flips_in_one_second(Duration::from_millis(ms)),
                baseline,
                "sprite cadence changed at a {ms}ms tick",
            );
        }
    }

    #[test]
    fn the_idle_ladder_is_strictly_ordered() {
        // The state selection reads these longest-first, so any pair out of
        // order would make a stage unreachable rather than misbehave visibly.
        let ladder = [SIT_AFTER, WASH_AFTER, SCRATCH_AFTER, YAWN_AFTER, SLEEP_AFTER];
        for pair in ladder.windows(2) {
            assert!(pair[0] < pair[1], "ladder out of order: {:?}", ladder);
        }
    }

    #[test]
    fn idle_thresholds_are_whole_ticks_of_the_original() {
        // oneko drove everything off a 125 ms tick; keeping these on that grid
        // means the sequence still lines up with the 8 Hz sprite cadence.
        for t in [SIT_AFTER, WASH_AFTER, SCRATCH_AFTER, YAWN_AFTER, SLEEP_AFTER,
                  AWAKE_TIME] {
            assert_eq!(
                t.as_millis() % 125, 0,
                "{t:?} is not a whole number of 125ms ticks",
            );
        }
        // The two stages carried over unchanged from before this ladder existed.
        assert_eq!(SIT_AFTER, Duration::from_millis(3 * 125));
        assert_eq!(WASH_AFTER, Duration::from_millis(10 * 125));
        // Matching oneko's own NEKO_AWAKE_TIME.
        assert_eq!(AWAKE_TIME, Duration::from_millis(3 * 125));
        // Wall-scratching replaces washing, so the washing slot is how long the
        // cat works on a wall. oneko's NEKO_TOGI_TIME is 10 ticks.
        assert_eq!(SCRATCH_AFTER - WASH_AFTER, Duration::from_millis(10 * 125));
    }

    #[test]
    fn no_wall_when_the_cat_is_not_against_one() {
        let (max_x, max_y) = (1888.0, 1048.0);
        // Mid-screen, cursor anywhere: nothing to scratch.
        assert_eq!(blocked_wall(900.0, 500.0, -50.0, -50.0, max_x, max_y), None);
        assert_eq!(blocked_wall(900.0, 500.0, 50.0, 50.0, max_x, max_y), None);
        // At a wall but no longer pushing into it: the cat is free to leave.
        assert_eq!(blocked_wall(0.0, 500.0, 50.0, 0.0, max_x, max_y), None);
        assert_eq!(blocked_wall(max_x, 500.0, -50.0, 0.0, max_x, max_y), None);
    }

    #[test]
    fn each_wall_is_reachable() {
        // The bug this pins: with the cat tracking the cursor by its top-left
        // corner it could never want to be further left or higher than the
        // cursor, so W and N were unreachable. Chasing by centre fixes that.
        let (max_x, max_y) = (1888.0, 1048.0);
        assert_eq!(blocked_wall(0.0, 500.0, -10.0, 0.0, max_x, max_y), Some(Dir::W));
        assert_eq!(blocked_wall(max_x, 500.0, 10.0, 0.0, max_x, max_y), Some(Dir::E));
        assert_eq!(blocked_wall(900.0, 0.0, 0.0, -10.0, max_x, max_y), Some(Dir::N));
        assert_eq!(blocked_wall(900.0, max_y, 0.0, 10.0, max_x, max_y), Some(Dir::S));
    }

    #[test]
    fn a_corner_resolves_to_one_wall_deterministically() {
        // Both axes blocked. oneko checks left/right before up/down, so the
        // horizontal wall wins - the specific choice matters less than it being
        // stable, since a tie that flipped would alternate poses every frame.
        let (max_x, max_y) = (1888.0, 1048.0);
        assert_eq!(blocked_wall(0.0, 0.0, -10.0, -10.0, max_x, max_y), Some(Dir::W));
        assert_eq!(blocked_wall(max_x, max_y, 10.0, 10.0, max_x, max_y), Some(Dir::E));
    }

    #[test]
    fn every_reachable_wall_has_art_storage() {
        let (max_x, max_y) = (1888.0, 1048.0);
        let cases = [
            (0.0, 500.0, -10.0, 0.0),
            (max_x, 500.0, 10.0, 0.0),
            (900.0, 0.0, 0.0, -10.0),
            (900.0, max_y, 0.0, 10.0),
        ];
        for (x, y, dx, dy) in cases {
            let w = blocked_wall(x, y, dx, dy, max_x, max_y).unwrap();
            assert!(
                crate::sprites::wall_index(w).is_some(),
                "{w:?} has no slot in Skin::wall",
            );
        }
    }

    #[test]
    fn settled_states_are_exactly_the_idle_ladder() {
        // Moments fire only while settled; waking and wall-scratching are
        // deliberately excluded so a bubble can't step on those poses.
        for s in [CatState::Sitting, CatState::Washing, CatState::Scratching,
                  CatState::Yawning, CatState::Sleeping] {
            assert!(s.is_settled(), "{s:?} should be settled");
        }
        for s in [CatState::Chasing, CatState::Waking, CatState::WallScratch(Dir::N)] {
            assert!(!s.is_settled(), "{s:?} should not be settled");
        }
    }

    #[test]
    fn rng_never_gets_stuck_at_zero() {
        // xorshift32 has no escape from a zero state, which would freeze every
        // moment on MOMENTS[0] forever - main() seeds with `| 1` to avoid it.
        let mut s = 1u32;
        for _ in 0..1000 {
            assert_ne!(next_u32(&mut s), 0);
        }
    }

    #[test]
    fn every_moment_phrase_is_renderable() {
        // draw_text silently skips characters missing from FONT, so a new
        // phrase with an unlisted character would render as a blank gap with
        // no error anywhere. Catch that here instead of on screen.
        for moment in MOMENTS {
            for ch in moment.text.chars() {
                assert!(
                    crate::font::glyph_for(ch).is_some(),
                    "MOMENTS phrase {:?} uses {ch:?}, which has no glyph in FONT",
                    moment.text,
                );
            }
        }
    }
}
