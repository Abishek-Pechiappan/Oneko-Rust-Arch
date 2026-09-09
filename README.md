# oneko in rust

A rewrite of the classic [**oneko**](https://github.com/tie/oneko) desktop cat, in **Rust**, for **Arch Linux + Hyprland**.

A little pixel-art cat chases your cursor around the screen. When you stop moving the mouse it sits down, washes itself, and eventually falls asleep — just like the 1990s X11 original, but running natively on Wayland, with multi-monitor support and a proximity-based chase so it doesn't chase your cursor across the whole desktop. Click the cat to freeze it in place; click again to let it resume chasing.

![demo](demo.gif)

## Features

- **Cursor chasing** with smooth, framerate-independent easing and full 8-directional walking sprites; motion rate is tunable with `--fps` while the sprite animation keeps its original 8 Hz cadence
- **Proximity-based chasing** — the cat only wakes up and starts chasing once the cursor comes within range; cursor movement elsewhere on screen is ignored, so a parked cat stays parked instead of being yanked around by every mouse movement. Once it's actively chasing, it won't give up mid-pursuit even if you move the cursor fast
- **Full idle sequence** — sits, washes its face, scratches its head, yawns, then falls asleep, and wakes with a startled pose when the cursor comes back; the complete progression from the X11 original
- **Random idle "moments"** — occasional speech bubbles (`meow`, `purrr~`, `nya~`, ...) and quirky animations (stretch, tail-flick) while idle
- **Wall scratching** — a cat pressed against a screen edge with the cursor beyond it scratches at the edge, with a distinct pose per side, exactly as the original does
- **Click-to-freeze** — left-click the cat to pin it in place (it still runs through its idle animations), click again to release it
- **Multi-monitor aware** — a layer-shell surface is created per connected output; only the monitor currently containing the cursor shows/animates the cat, and it hands off cleanly as the cursor crosses monitors, including hotplug of new/removed outputs
- **Efficient redraws** — skips the whole allocate/blit/commit pass whenever a frame would be pixel-identical to what's already on screen (e.g. while sitting or asleep), instead of recompositing 8x/second forever
- **Cheap to leave running** — event-driven loop plus direct IPC cursor polling; measured at 0.15% of one core while idle, down from 9.25% (see [What's new](#whats-new))
- **Six characters** — the original oneko cat plus `tora`, `dog`, `sakura`, `tomoyo` and the BSD daemon, selected with `--skin`; drop your own art in `~/.config/oneko-rust/skins/` and it's picked up without a rebuild
- **Zero external assets at runtime** — sprites are compiled into the binary from `assets/skins/`, so the default build needs no files on disk

## What's new

### ~60× less idle CPU

Cursor tracking no longer shells out to `hyprctl`. It talks to Hyprland's IPC socket directly, and the main loop is event-driven instead of a fixed sleep/round-trip cycle.

Measured on a dual-monitor Hyprland session, release builds, 20 s idle — child-process CPU included, so `hyprctl`'s forks are counted fairly:

| | idle CPU |
| --- | --- |
| before — `hyprctl` subprocess, fixed 125 ms loop | **9.25%** of one core |
| after — IPC socket, event-driven loop | **0.15%** of one core |

The old version spawned a process 8×/second forever — fork, exec and dynamic-link of an entire separate binary — just to ask where the mouse was. The socket answers the same question in about 10 µs.

What changed:

- **Hyprland IPC socket instead of `hyprctl cursorpos`.** Same request, same reply format, no process spawn. Both socket locations are probed (`$XDG_RUNTIME_DIR/hypr/…` and the older `/tmp/hypr/…`), so one binary works across Hyprland versions.
- **Event-driven main loop.** The loop now waits on the Wayland socket and an animation timer together, rather than `roundtrip()` + `thread::sleep(125ms)` — which forced a server round trip 8×/second whether or not anything had been drawn, and made click-to-freeze wait out the rest of the sleep. Clicks are now handled the moment they arrive, at any tick rate.
- **Adaptive tick rate.** Once the cat is asleep the tick stretches from 125 ms to 250 ms — nothing on screen is changing, so there's no reason to wake up 8×/second to confirm it. Kept deliberately modest: that interval is also how quickly a sleeping cat notices the cursor coming back.
- **Wall-clock timings.** Every animation threshold is a `Duration` now rather than a count of ticks, so a varying tick interval can't silently rescale the cat's behavior. The values match the old tick counts exactly — sit at 375 ms, wash at 1.25 s, sleep at 2.5 s — and a test pins that equivalence.
- **A failed cursor read no longer moves the cat.** It used to fall back to `(0, 0)`, which on a layout with a monitor to the left of the primary is a real on-screen point — so a transient hiccup teleported the cat into the corner. A failed read now simply skips the tick.

### Split into modules

`src/main.rs` was a single 1,461-line file. It's now split, so the compositor and sprite work below can proceed independently:

| file | what |
| --- | --- |
| `main.rs` | startup and the event loop |
| `app.rs` | shared Wayland/SCTK state and event-dispatch boilerplate |
| `cat.rs` | per-monitor state, per-tick behavior, drawing |
| `cursor.rs` | the `CursorSource` trait and one backend per compositor |
| `font.rs` | 5×7 bitmap font and the speech-bubble renderer |
| `moments.rs` | the random phrase/quirk table — add phrases here |
| `sprites.rs` | skin/pose types and lookup; the art itself is generated by `build.rs` |
| `xbm.rs` | the XBM decoder, shared by `build.rs` and the runtime skin loader |

There are unit tests now (`cargo test`), including one that catches a phrase in `moments.rs` using a character with no glyph in the font — which previously failed silently, rendering a blank gap on screen.

### The original cat's full behavior

The idle sequence used to be `sit → wash → sleep`. It's now the one oneko
actually has, including the states this rewrite had never implemented — the art
for them was in the upstream bitmaps all along, just unused:

| stage | oneko's name | what it is |
| --- | --- | --- |
| sit | `STOP` 立ち止まった | stopped |
| wash | `JARE` 顔を洗っている | washing its face |
| **scratch** | `KAKI` 頭を掻いている | **scratching its head** |
| **yawn** | `AKUBI` あくびをしている | **yawning** |
| sleep | `SLEEP` 寝てしまった | asleep |
| **wake** | `AWAKE` 目が覚めた | **startled awake when the cursor returns** |
| **wall** | `*_TOGI` 壁を引っ掻いている | **scratching the screen edge it's stuck against** |

**Wall scratching** is the one you'll notice most. Push the cursor into a screen
edge and the cat, unable to get closer, scratches at it — the same behavior the
X11 original has, with a pose per edge. It replaces washing rather than adding a
stage, which is what upstream does too: a cat stuck against an edge grooms less
and complains more.

**Waking up** puts a brief startled pose between "asleep" and "chasing" instead
of snapping straight into a run.

Sleep now arrives at 3.75 s rather than 2.5 s, since two stages were added ahead
of it. That's the point — the wind-down reads as a sequence of things the cat is
doing rather than a two-step fade.

Two fixes fell out of implementing this against the original source:

- **The cat now chases with its centre, not its top-left corner.** oneko computes
  `MouseX - NekoX - BITMAP_WIDTH / 2`; this rewrite had dropped the half-width,
  so the cat parked down-and-right of the cursor. It also made two walls
  unreachable: a cat whose corner tracks the cursor can never *want* to be
  further left or higher than it, so it could only ever scratch the right and
  bottom edges.
- **Washing now alternates `jare2` with the sit pose,** as upstream does. It had
  been pairing `jare2` with `awake`, which is a separate state, not a grooming
  frame.

Every new pose is optional in the skin format, so a skin that doesn't ship them
simply skips those stages instead of failing to load.

### Smoother chasing, without losing the retro look

The cat's *position* used to update at the same 8 Hz as its sprite animation,
which is what made the chase look stepped. Those are now independent: position
updates at `--fps` (default 30) while the two-frame walk cycle stays on its
original 8 Hz clock. Verified by instrumenting the loop — at `--fps 30` the
motion tick runs at 30 Hz and the sprite still flips 8.0 times a second.

```sh
oneko-rust --fps 8     # the classic stepped chase, exactly as before
oneko-rust --fps 60    # smoother, at a real cost - see below
```

**Speed is now framerate-independent.** The chase easing was
`position += remaining * 0.3` applied once per tick, which quietly defines speed
*per tick* rather than per second — at 60 Hz the cat would have closed distance
7× faster. It's now exponential decay over elapsed time, so splitting a step into
N smaller steps lands the cat in exactly the same place, and 125 ms still gives
precisely the old 0.3. Tests pin both the equivalence and the composition
property. This was a latent bug even at the old fixed rate, since the tick
already stretches while the cat sleeps.

**Why 30 and not 60.** Measured chasing nonstop on a 60 Hz panel, 60-second
samples:

| `--fps` | idle CPU |
| --- | --- |
| 8 | 0.233% of one core |
| 30 | **0.250%** |
| 60 | 1.000% |
| 90 | 2.050% |

30 buys visibly smoother motion for essentially nothing over the original 8 Hz.
60 costs 4× that for 2× the rate — superlinear, most likely SHM buffer pressure
as the update cycle approaches how long the compositor holds each buffer at
60 Hz. For something that runs in the background forever that's a poor default,
so 60 is available but not the default. Worth revisiting once the draw path uses
ping-ponged persistent buffers and frame callbacks.

### Six characters, and skins you can add yourself

The sprites are no longer 529 lines of hand-written hex. The art now lives in
`assets/skins/` as real `.xbm` files, and `build.rs` compiles them into the
binary — the same `[u8; 128]` arrays as before, so the runtime representation
and cost are unchanged. What changes is that the art is editable with ordinary
image tools instead of by flipping bits in hex literals.

That unlocked the rest of the original oneko cast:

```sh
oneko-rust --skin tora      # the striped cat
oneko-rust --skin dog
oneko-rust --skin sakura
oneko-rust --skin tomoyo
oneko-rust --skin bsd       # the BSD daemon
oneko-rust --list-skins
```

**Your own skins** go in `$XDG_CONFIG_HOME/oneko-rust/skins/<name>/` (usually
`~/.config/oneko-rust/skins/<name>/`) and are loaded by name without rebuilding:

```sh
cp -r assets/skins/dog ~/.config/oneko-rust/skins/mydog
oneko-rust --skin mydog
```

Both paths use the same format and the same decoder, so a directory that builds
in also works as a user skin. See [`assets/README.md`](assets/README.md) for the
pose list, the mask convention, and a `magick` one-liner for converting PNG art.
A malformed or missing pose fails with a message naming the file rather than
rendering an invisible cat.

The migration is verified rather than assumed: all 42 of the previously
hardcoded sprites were matched byte-for-byte against the upstream oneko bitmaps
they came from, and a test pins every skin's art to a recorded digest so
corrupted or mis-paired assets are caught by `cargo test` rather than on screen.

### Command-line options

`--skin`, `--fps`, `--list-skins`, `--help` and `--version`, parsed by hand so
the dependency list stays at one crate.

### Cursor tracking is pluggable

Wayland deliberately doesn't expose the global cursor position, so every compositor needs its own answer. That's now a single trait in `src/cursor.rs`:

```rust
pub trait CursorSource {
    fn position(&mut self) -> Option<(f32, f32)>;
    fn name(&self) -> &'static str;
}
```

Supporting a new compositor means writing one implementation and adding one line to `detect()`. Nothing else in the program changes.

## Planned

- **X11 backend** (`XQueryPointer`) — the most reach for the least work: i3, XFCE, MATE, Cinnamon, and KDE or GNOME on X11, all at once. It also covers the rendering half, since `wlr-layer-shell` isn't available on GNOME either.
- **Generic wlroots backend** for sway, river, Wayfire, niri and labwc — a layer-shell surface plus `zwlr_virtual_pointer_v1`, the approach [wl-find-cursor](https://github.com/cjacker/wl-find-cursor) uses. sway's own `get_cursor` IPC proposal was left unmerged in favor of exactly this.
- **KWin backend** — a KWin script polling `workspace.cursorPos` over D-Bus.
- **HiDPI** — honor the output scale factor and render at integer scale, so the cat stays crisp instead of being bilinearly upscaled on 2× displays.
- **More CLI flags and a config file** — `--scale`, `--speed`, `--radius`, `--all-monitors`, and a config file so the flags don't have to live in a compositor autostart line.
- **Packaging** — a PKGBUILD for the AUR, a systemd user unit, and an XDG autostart entry, so setup doesn't require editing a compositor config.

GNOME on Wayland isn't planned: no layer-shell, no virtual-pointer, and `Shell.Eval` is locked down, so it would need a GNOME Shell extension — a separate project.

## Why a rewrite?

The original oneko (and most clones) rely on X11 tricks — override-redirect windows and the SHAPE extension — that don't work under Wayland compositors like Hyprland. This version uses:

- **`wlr-layer-shell`** (via [smithay-client-toolkit](https://crates.io/crates/smithay-client-toolkit)) for an always-on-top overlay surface
- **ARGB transparency** instead of the X11 SHAPE extension
- **Hyprland's IPC socket** to track the cursor globally, behind a `CursorSource` trait so other compositors can be plugged in
- A **small input region matching the cat's 32×32 box**, so it can catch clicks to toggle freezing without stealing focus or blocking anything outside its own bounds

## Usage

Run `oneko-rust` for the classic cat, or pick another character with `--skin` (`oneko-rust --list-skins` shows what's available). Move the cursor near the cat to wake it up and get chased; leave it alone (or stay far away) and it'll sit down, wash itself, and eventually fall asleep. Left-click the cat to freeze it in place; click again to unfreeze. While frozen it still sits, washes, and sleeps if left alone — it just won't chase. Every so often while idle, the cat may pop up a tiny speech bubble ("meow", "purrr~"...) or do a quirky animation like a stretch or tail-flick, then carry on as normal.

## Requirements

- Arch Linux (or any Linux distro, really)
- [Hyprland](https://hypr.land) — cursor tracking talks to Hyprland's IPC socket; any other compositor needs its own `CursorSource` backend (see [Planned](#planned))
- Rust toolchain (`rustup` or `pacman -S rust`)

## Build & run

```sh
cargo build --release
./target/release/oneko-rust
```

## Install

Run the install script to build the release binary, copy it to `~/.local/bin`, and optionally add a Hyprland autostart entry:

```sh
./install.sh
```

## Autostart with Hyprland

Add the binary to your Hyprland autostart. Classic config (`hyprland.conf`):

```ini
exec-once = /path/to/oneko-rust/target/release/oneko-rust
```

Lua config (`hyprland.lua`, Hyprland ≥ 0.55):

```lua
hl.on("hyprland.start", function()
    hl.exec_cmd("/path/to/oneko-rust/target/release/oneko-rust")
end)
```

Stop it with `pkill oneko-rust`.

## Limitations

- Hyprland-specific: cursor tracking reads Hyprland's IPC socket. Wayland deliberately doesn't expose the global cursor position, so every compositor needs its own answer — porting means adding one `CursorSource` impl in `src/cursor.rs` (see [Planned](#planned)).
- Clicks landing inside the cat's current 32×32 box are consumed to detect the freeze toggle, so anything beneath the cat at that instant won't receive that click.
- Only one cat is shown at a time, on whichever monitor currently contains the cursor — it's not simultaneously visible/independent on every monitor.

## Credits

Sprites and behavior are taken from the original [oneko](https://github.com/tie/oneko) by Masayuki Koba, which its maintainers describe as public domain software (no formal license file). All six character sets in `assets/skins/` — `neko`, `tora`, `dog`, `sakura`, `tomoyo` and `bsd` — are that original art, copied unmodified and renamed; see [`assets/README.md`](assets/README.md) for the pose-name mapping and the one place the animation deviates from upstream. The `stretch` and `tailflick` poses are original to this project.

The BSD daemon is copyright 1988 Marshall Kirk McKusick.

## License

This rewrite is licensed under the [GNU General Public License v3.0](LICENSE).
