// oneko-core: everything about the cat that doesn't depend on how it's
// actually put on screen - sprites, animation state machine, and pixel
// rendering. Shared between the Hyprland/wlr-layer-shell binary (../hyprland)
// and the GNOME daemon (../ubuntu-version/daemon), which each own their own
// way of getting the cursor position and presenting an overlay surface.
//
// How this file is laid out, top to bottom:
//   1. sprites module              - hand-embedded pixel art (see sprites.rs).
//   2. Dir / CatState / helpers    - direction + animation-state logic.
//   3. Canvas/bubble layout consts - sizing for the cat + speech bubble.
//   4. Bitmap font + draw_text/bubble - renders the little speech bubbles.
//   5. Moment / MOMENTS            - the random "cat says/does something
//                                    cute" system; add new phrases here.
//   6. struct Cat                  - per-cat animation/position state, and
//                                    Cat::new for a freshly spawned cat.
//   7. tick()                      - runs one 125ms animation step; a
//                                    backend calls this once per cat per
//                                    tick with the cursor position (already
//                                    local to whatever area the cat is
//                                    confined to) and gets back the sprite
//                                    to show, or None if nothing changed.
//   8. render_frame()              - turns a sprite/mask/bubble-text combo
//                                    into a plain ARGB8888 pixel buffer a
//                                    backend can blit however it likes.

mod sprites;
use sprites::*;

// The cat's current activity, driven by how long the cursor has been idle
// (see idle_ticks / the thresholds in tick()).
#[derive(Clone, Copy, PartialEq)]
enum CatState { Chasing, Sitting, Washing, Sleeping }

// 8-way compass direction the cat is facing/running, used to pick which
// CAT_* sprite pair to show while Chasing.
#[derive(Clone, Copy, PartialEq)]
pub enum Dir { N, NE, E, SE, S, SW, W, NW }

// Turns a cursor-relative offset into one of the 8 compass directions.
// Uses a 2:1 ratio to bias toward the 4 cardinal directions (N/E/S/W) over
// the diagonals, so the cat doesn't flicker between them too easily.
fn dir_from_delta(dx: f32, dy: f32) -> Dir {
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

// The cat sprite itself is always exactly SIZE x SIZE pixels.
pub const SIZE: u32 = 32;

// The cursor must be within this many pixels of the cat before it'll wake up
// and chase - see the `near` check in tick(). Cursor movement farther away
// than this is ignored entirely, so the cat settles into its idle
// animations instead of reacting to every mouse movement on screen.
pub const CHASE_RADIUS: f32 = 150.0;

// The rendered canvas is bigger than the cat (CANVAS_W x CANVAS_H), with
// BUBBLE_H extra rows of empty space above it to draw a speech bubble into
// when one is active. The cat sprite is always drawn bottom-anchored and
// horizontally centered within that canvas, at (CAT_X_OFFSET, CAT_Y_OFFSET)
// - see render_frame. To make the bubble area bigger/smaller, tweak
// CANVAS_W (width, e.g. for longer phrases) and/or BUBBLE_H (height).
pub const CANVAS_W: u32 = 72;
pub const BUBBLE_H: u32 = 16;
pub const CANVAS_H: u32 = SIZE + BUBBLE_H;
pub const CAT_X_OFFSET: i32 = ((CANVAS_W - SIZE) / 2) as i32;
pub const CAT_Y_OFFSET: i32 = BUBBLE_H as i32;

// Size of one character cell in the speech-bubble font, in pixels.
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

// 5x7 pixel font, one row per byte, bit 4 = leftmost column (so the binary
// literals read left-to-right the same as the glyph shape, e.g. 0b01110 is
// ".###."). Only covers the characters actually used by MOMENTS below - if
// you add a phrase with a new character, add its glyph here too, or
// draw_text will silently skip that character (see glyph_for).
const FONT: &[(char, [u8; GLYPH_H])] = &[
    ('m', [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b00000]),
    ('e', [0b11110, 0b10000, 0b11110, 0b10000, 0b10000, 0b11110, 0b00000]),
    ('o', [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000]),
    ('w', [0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001, 0b00000]),
    ('z', [0b11111, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111, 0b00000]),
    ('!', [0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100, 0b00000]),
    ('?', [0b01110, 0b10001, 0b00010, 0b00100, 0b00000, 0b00100, 0b00000]),
    ('~', [0b00000, 0b00000, 0b01001, 0b10110, 0b00000, 0b00000, 0b00000]),
    ('*', [0b00000, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0b00000]),
    ('r', [0b11110, 0b10001, 0b10001, 0b11110, 0b10010, 0b10001, 0b00000]),
    ('p', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b00000]),
    ('u', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000]),
    ('n', [0b10001, 0b11001, 0b10101, 0b10101, 0b10011, 0b10001, 0b00000]),
    ('y', [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00000]),
    ('a', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b00000]),
    ('h', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b00000]),
    ('i', [0b00100, 0b00000, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000]),
    ('s', [0b01111, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110, 0b00000]),
    ('c', [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111, 0b00000]),
    ('t', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000]),
];

// Looks up a character's glyph in FONT; None for anything not in the table
// (draw_text just skips those, so unsupported characters render as blanks).
fn glyph_for(c: char) -> Option<&'static [u8; GLYPH_H]> {
    FONT.iter().find(|(fc, _)| *fc == c).map(|(_, g)| g)
}

// Writes one solid-color pixel block into an ARGB8888 canvas, matching the
// byte layout used by render_frame (4 bytes per pixel, row-major).
fn put_pixel(canvas: &mut [u8], canvas_w: usize, x: i32, y: i32, argb: u32) {
    if x < 0 || y < 0 || x as usize >= canvas_w {
        return;
    }
    let idx = (y as usize * canvas_w + x as usize) * 4;
    if idx + 4 <= canvas.len() {
        canvas[idx..idx + 4].copy_from_slice(&argb.to_le_bytes());
    }
}

// Pixel width `text` would render at, at the given integer upscale factor
// (1 glyph pixel becomes `scale` x `scale` screen pixels). Used to center
// the speech bubble box around its text; see draw_bubble.
fn text_width(text: &str, scale: i32) -> i32 {
    text.chars().count() as i32 * (GLYPH_W as i32 + 1) * scale - scale
}

// Draws `text` in solid black starting at (x0, y0), one glyph after another,
// each glyph pixel blown up to a `scale` x `scale` block.
fn draw_text(canvas: &mut [u8], canvas_w: usize, text: &str, x0: i32, y0: i32, scale: i32) {
    let mut cx = x0;
    for ch in text.chars() {
        if let Some(glyph) = glyph_for(ch) {
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..GLYPH_W {
                    if bits & (1 << (GLYPH_W - 1 - col)) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                put_pixel(
                                    canvas,
                                    canvas_w,
                                    cx + col as i32 * scale + sx,
                                    y0 + row as i32 * scale + sy,
                                    0xFF00_0000,
                                );
                            }
                        }
                    }
                }
            }
        }
        cx += (GLYPH_W as i32 + 1) * scale;
    }
}

// Draws a small white speech-bubble box (black 1px border + downward nub)
// sized to fit `text`, then the text itself in black on top.
// SCALE is the font upscale factor (1 = native glyph pixels, 2 = double
// size, etc.) and PAD is the empty margin in pixels around the text inside
// the box - bump either up if the bubble/text ever needs to look bigger.
fn draw_bubble(canvas: &mut [u8], canvas_w: usize, text: &str) {
    const SCALE: i32 = 1;
    const PAD: i32 = 2;
    let text_w = text_width(text, SCALE);
    let box_w = text_w + PAD * 2;
    let box_h = GLYPH_H as i32 * SCALE + PAD * 2;
    let box_x0 = (canvas_w as i32 - box_w) / 2;
    let box_y0 = 1;

    for y in box_y0..box_y0 + box_h {
        for x in box_x0..box_x0 + box_w {
            let on_border = x == box_x0 || x == box_x0 + box_w - 1 || y == box_y0 || y == box_y0 + box_h - 1;
            let argb = if on_border { 0xFF00_0000 } else { 0xFFFF_FFFF };
            put_pixel(canvas, canvas_w, x, y, argb);
        }
    }
    // Small downward-pointing nub connecting the bubble to the cat.
    let nub_x = canvas_w as i32 / 2;
    let nub_y0 = box_y0 + box_h;
    for (i, y) in (nub_y0..nub_y0 + 2).enumerate() {
        for x in (nub_x - 1 + i as i32)..=(nub_x + 1 - i as i32) {
            put_pixel(canvas, canvas_w, x, y, 0xFF00_0000);
        }
    }

    draw_text(canvas, canvas_w, text, box_x0 + PAD, box_y0 + PAD, SCALE);
}

// One entry in the random "flavor" table below: a speech-bubble phrase
// and/or a sprite override to briefly show instead of the normal idle pose.
// `text: ""` means no bubble (quirk-only); `quirk: None` means no sprite
// change (speech-only, cat keeps its normal idle animation).
struct Moment {
    text: &'static str,
    quirk: Option<(&'static [u8; 128], &'static [u8; 128])>,
}

// The pool of things the cat can randomly say/do while idle (see the
// "Moment" trigger logic in tick()). To add a new phrase, just add an entry
// here - make sure every character in it has a glyph in FONT above. To add
// a new quirky animation, hand-author a sprite pair (see the comment above
// CAT_TAILFLICK in sprites.rs) and reference it here.
const MOMENTS: &[Moment] = &[
    Moment { text: "meow", quirk: None },
    Moment { text: "meow?", quirk: None },
    Moment { text: "meow!", quirk: None },
    Moment { text: "mrow", quirk: None },
    Moment { text: "purrr~", quirk: None },
    Moment { text: "nya~", quirk: None },
    Moment { text: "mew", quirk: None },
    Moment { text: "zzz", quirk: None },
    Moment { text: "hiss!", quirk: None },
    Moment { text: "?!", quirk: None },
    Moment { text: "*stretch*", quirk: Some((&CAT_STRETCH, &CAT_STRETCH_MASK)) },
    Moment { text: "", quirk: Some((&CAT_TAILFLICK, &CAT_TAILFLICK_MASK)) },
];

// Everything about a tick that actually affects the rendered frame. Compared
// against the previous tick's state so tick() can report "nothing changed"
// and let the caller skip re-rendering/re-committing a frame - see the
// dirty-check at the end of tick(). Sprite/mask are compared by value (not
// pointer identity): they're plain `const [u8; 128]` arrays, and LLVM's
// constant-merging pass is free to unify two consts that ever become
// byte-identical, which would make pointer comparison silently wrong.
//
// margin_top/margin_left are the cat's own 32x32 sub-rect's offset from the
// top-left of the confined area (monitor, for both current backends), *not*
// from the top-left of the (larger, bubble-carrying) canvas - subtract
// CAT_X/Y_OFFSET to convert from win_x/win_y, see tick(). A backend
// positions its overlay using these directly (Hyprland: layer-surface
// margin; GNOME: actor x/y relative to the monitor origin).
#[derive(Clone, Copy, PartialEq)]
pub struct DrawState {
    pub margin_top: i32,
    pub margin_left: i32,
    pub sprite: &'static [u8; 128],
    pub mask: &'static [u8; 128],
    pub bubble_text: &'static str,
}

// All per-cat animation/position state, independent of how it's actually
// displayed. One instance per on-screen cat - the Hyprland backend keeps one
// per connected output, the GNOME backend keeps exactly one (GNOME Shell
// already spans the whole desktop in one coordinate space).
pub struct Cat {
    pub win_x: f32, // cat's on-screen position, local to whatever area it's confined
    pub win_y: f32, // to (top-left of its own 32x32 box, not the enlarged canvas)

    pub last_cursor: (f32, f32), // local cursor position as of the previous tick, to detect idling
    pub dir: Dir,                // facing direction while chasing
    pub frame: bool,             // flips every tick; picks between each pose's 2 animation frames
    pub idle_ticks: u32,         // consecutive ticks the cursor has been still
    pub frozen: bool,            // toggled by clicking the cat

    pub next_moment_in: u32,          // ticks until the next random speech/quirk moment
    pub moment_ticks_remaining: u32,  // ticks left in the currently active moment, if any
    pub active_moment: Option<usize>, // index into MOMENTS while a moment is active

    pub last_drawn: Option<DrawState>, // what's currently actually on screen;
                                        // None forces the next tick to report a change regardless
}

impl Cat {
    // Freshly spawned cat at (win_x, win_y), matching the original oneko's
    // just-appeared state: facing east, not moving, no moment active yet.
    pub fn new(win_x: f32, win_y: f32) -> Self {
        Cat {
            win_x,
            win_y,
            last_cursor: (win_x, win_y),
            dir: Dir::E,
            frame: false,
            idle_ticks: 0,
            frozen: false,
            next_moment_in: 300,
            moment_ticks_remaining: 0,
            active_moment: None,
            last_drawn: None,
        }
    }
}

// Tiny xorshift32 PRNG (no external `rand` dependency needed for
// occasionally picking a random Moment). Must be seeded with a nonzero
// value once at startup by the caller.
pub fn next_u32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

// Runs one 125ms animation step for `cat`. `cursor_x`/`cursor_y` must already
// be local to `bounds` (the size of the area the cat is confined to - e.g.
// one monitor), and `bounds` is used to clamp the cat's movement the same
// way regardless of backend. Returns the DrawState to render this tick, or
// None if it's pixel-for-pixel identical to what's already been reported as
// drawn (see DrawState's doc comment) - the caller can skip re-rendering and
// re-displaying the frame entirely in that case, e.g. while sitting or
// asleep, instead of doing so every single tick forever.
pub fn tick(rng_state: &mut u32, cursor_x: f32, cursor_y: f32, bounds: (f32, f32), cat: &mut Cat) -> Option<DrawState> {
    // The cat only wakes up for the cursor once it's within CHASE_RADIUS;
    // movement farther away than that is treated the same as no movement
    // at all, so the cat isn't yanked out of its idle animations by every
    // mouse movement on screen (see CHASE_RADIUS's doc comment). Once it's
    // already chasing, though, don't re-check the distance every tick - a
    // fast-moving cursor can easily outrun the cat's 30%-per-tick easing
    // and briefly land outside the radius mid-chase, which would otherwise
    // make it give up and fall asleep instead of catching up.
    let dist = ((cursor_x - cat.win_x).powi(2) + (cursor_y - cat.win_y).powi(2)).sqrt();
    let was_chasing = cat.idle_ticks < 3;
    let near = was_chasing || dist <= CHASE_RADIUS;

    // "Idle" means the cursor has barely moved since last tick (and is near).
    let cursor_moved = near
        && ((cursor_x - cat.last_cursor.0).abs() > 2.0
            || (cursor_y - cat.last_cursor.1).abs() > 2.0);
    cat.last_cursor = (cursor_x, cursor_y);

    if cursor_moved { cat.idle_ticks = 0; } else { cat.idle_ticks += 1; }

    // While frozen, keep the cat pinned in place but still let it settle
    // into its idle animations if the cursor stops moving elsewhere.
    let idle_ticks = if cat.frozen { cat.idle_ticks.max(3) } else { cat.idle_ticks };

    // Idle-duration thresholds (in ticks @ 125ms each) that step the cat
    // through its idle animations: Sitting -> Washing -> Sleeping.
    let state = if idle_ticks >= 20 {
        CatState::Sleeping
    } else if idle_ticks >= 10 {
        CatState::Washing
    } else if idle_ticks >= 3 {
        CatState::Sitting
    } else {
        CatState::Chasing
    };

    // Occasional, non-distracting flavor: a speech bubble and/or a
    // quirky sprite override, only while the cat is already idle.
    // `moment_ticks_remaining` counts down while one is showing;
    // `next_moment_in` counts down the (much longer) quiet period
    // between moments. Tune the two "~Ns" ranges below to change how
    // often moments happen and how long each one lasts.
    if cat.moment_ticks_remaining > 0 {
        cat.moment_ticks_remaining -= 1;
    } else {
        cat.active_moment = None;
        if state != CatState::Chasing {
            if cat.next_moment_in == 0 {
                let idx = (next_u32(rng_state) as usize) % MOMENTS.len();
                cat.active_moment = Some(idx);
                cat.moment_ticks_remaining = 6 + next_u32(rng_state) % 5; // ~0.75-1.25s
                cat.next_moment_in = 240 + next_u32(rng_state) % 561; // ~30-100s
            } else {
                cat.next_moment_in -= 1;
            }
        }
    }

    // Chase the cursor: ease toward it (30% of the remaining distance
    // per tick, so movement looks smooth rather than snapping), clamped
    // so the cat can't leave the confined area. Skipped while frozen or
    // while the cursor is outside CHASE_RADIUS.
    if !cat.frozen && near {
        let dx = cursor_x - cat.win_x;
        let dy = cursor_y - cat.win_y;

        if dx.abs() > 1.0 || dy.abs() > 1.0 {
            cat.dir = dir_from_delta(dx, dy);
        }

        let max_x = (bounds.0 - SIZE as f32).max(0.0);
        let max_y = (bounds.1 - SIZE as f32).max(0.0);
        cat.win_x = (cat.win_x + dx * 0.3).clamp(0.0, max_x);
        cat.win_y = (cat.win_y + dy * 0.3).clamp(0.0, max_y);
    }

    cat.frame = !cat.frame; // alternates true/false each tick, for 2-frame poses

    // Pick which sprite to show for the current state/direction/frame.
    let (mut sprite, mut mask): (&'static [u8; 128], &'static [u8; 128]) = match state {
        CatState::Sitting  => (&CAT_SITTING,    &CAT_SITTING_MASK),
        CatState::Washing  => if cat.frame { (&CAT_WASHING_1,  &CAT_WASHING_1_MASK)  }
                              else        { (&CAT_WASHING_2,  &CAT_WASHING_2_MASK)  },
        CatState::Sleeping => if (cat.idle_ticks / 4) % 2 == 0
                                   { (&CAT_SLEEPING_1, &CAT_SLEEPING_1_MASK) }
                              else { (&CAT_SLEEPING_2, &CAT_SLEEPING_2_MASK) },
        CatState::Chasing  => match (cat.dir, cat.frame) {
            (Dir::E,  true)  => (&CAT_RIGHT1, &CAT_RIGHT1_MASK),
            (Dir::E,  false) => (&CAT_RIGHT2, &CAT_RIGHT2_MASK),
            (Dir::W,  true)  => (&CAT_LEFT1,  &CAT_LEFT1_MASK),
            (Dir::W,  false) => (&CAT_LEFT2,  &CAT_LEFT2_MASK),
            (Dir::N,  true)  => (&CAT_UP1,    &CAT_UP1_MASK),
            (Dir::N,  false) => (&CAT_UP2,    &CAT_UP2_MASK),
            (Dir::S,  true)  => (&CAT_DOWN1,  &CAT_DOWN1_MASK),
            (Dir::S,  false) => (&CAT_DOWN2,  &CAT_DOWN2_MASK),
            (Dir::NE, true)  => (&CAT_NE1,    &CAT_NE1_MASK),
            (Dir::NE, false) => (&CAT_NE2,    &CAT_NE2_MASK),
            (Dir::NW, true)  => (&CAT_NW1,    &CAT_NW1_MASK),
            (Dir::NW, false) => (&CAT_NW2,    &CAT_NW2_MASK),
            (Dir::SE, true)  => (&CAT_SE1,    &CAT_SE1_MASK),
            (Dir::SE, false) => (&CAT_SE2,    &CAT_SE2_MASK),
            (Dir::SW, true)  => (&CAT_SW1,    &CAT_SW1_MASK),
            (Dir::SW, false) => (&CAT_SW2,    &CAT_SW2_MASK),
        },
    };

    // If a Moment is currently active, let it override the sprite
    // and/or supply the speech-bubble text picked above.
    let mut bubble_text: &'static str = "";
    if let Some(idx) = cat.active_moment {
        bubble_text = MOMENTS[idx].text;
        if let Some((qs, qm)) = MOMENTS[idx].quirk {
            sprite = qs;
            mask = qm;
        }
    }

    // Anchored TOP|LEFT; the cat's own sub-rect sits CAT_X/Y_OFFSET into
    // the (larger, bubble-carrying) canvas, so shift the margin back by
    // that offset to keep the cat itself tracking win_x/win_y exactly.
    let new_state = DrawState {
        margin_top: cat.win_y as i32 - CAT_Y_OFFSET,
        margin_left: cat.win_x as i32 - CAT_X_OFFSET,
        sprite,
        mask,
        bubble_text,
    };

    // Report "nothing changed" when the frame would be pixel-for-pixel
    // identical to what's already reported as drawn (e.g. Sitting, most of
    // Sleeping, or a frozen/motionless cat) - this is what lets a backend
    // skip recompositing 8x/second forever while the cat visually never
    // changes.
    if cat.last_drawn != Some(new_state) {
        cat.last_drawn = Some(new_state);
        Some(new_state)
    } else {
        None
    }
}

// Renders one frame: a fresh ARGB8888 buffer sized to the full canvas, with
// `sprite`/`mask` unpacked into the cat's sub-rect (everywhere else
// transparent), and a speech bubble drawn on top if `text` isn't empty. Pure
// pixel data - what a backend does with it (attach to a Wayland SHM buffer,
// upload to a Clutter.Image, ...) is up to it.
//
// XBM layout: 4 bytes per row, LSB of each byte is the leftmost pixel.
// mask bit set + sprite bit set => black, mask only => white, else transparent.
pub fn render_frame(sprite: &[u8; 128], mask: &[u8; 128], text: &str) -> Vec<u8> {
    let mut canvas = vec![0u8; (CANVAS_W * CANVAS_H * 4) as usize]; // fully transparent by default

    for y in 0..SIZE as usize {
        for x in 0..SIZE as usize {
            let byte = y * 4 + x / 8;
            let bit = 1u8 << (x % 8);
            let px: u32 = if mask[byte] & bit != 0 {
                if sprite[byte] & bit != 0 { 0xFF00_0000 } else { 0xFFFF_FFFF }
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
        draw_bubble(&mut canvas, CANVAS_W as usize, text);
    }

    canvas
}
