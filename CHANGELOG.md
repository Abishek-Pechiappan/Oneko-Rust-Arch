# Changelog

## Unreleased

Nothing has been tagged yet, so everything below is on `main`.

### Hides over fullscreen windows

The cat now disappears from a monitor while it's showing a fullscreen window — a game, a video — and comes back where it left off once the window leaves fullscreen. It's per monitor, so a fullscreen video on one screen doesn't hide the cat on the other, and maximized windows don't count.

On Hyprland this reads `j/monitors` and `j/clients` from the IPC socket twice a second. While hidden the tick drops to 500 ms, so a game doesn't pay for a cat it can't see. Other compositors need a `FullscreenSource` backend; without one the cat just never hides, as before. The startup line says which applies.

### Less buffer work while chasing, fewer wakeups while idle

- **Moving without redrawing.** The sprite changes 8×/second but the position changes every tick, and every position change used to allocate, fill and attach a new SHM buffer. Now a position-only change just updates the layer-shell margin and commits; the pixels already on screen are reused. A protocol trace of 7 s of chasing at `--fps 30` went from 114 buffer attaches to 67.
- **Settled cats tick at 8 Hz.** Sitting, washing, scratching and yawning involve no motion, so the tick drops to the 125 ms sprite cadence instead of running at `--fps`. That's about 4× fewer cursor polls (7.5× at `--fps 60`) and wakeups during the wind-down. Noticing the cursor come back can take up to 125 ms — the original oneko's own reaction time.
- **Release profile.** LTO, one codegen unit, `panic = "abort"` and stripping take the binary from 1.76 MB to 786 KB.

The process's own CPU was already around 0.1% of one core, so the difference there is within measurement noise. The saving is mostly compositor-side: fewer buffers to import and upload.

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

`src/main.rs` was a single 1,461-line file, 529 lines of which were sprite hex.
It's now eight modules, so the compositor and sprite work can proceed
independently — see [the module map](docs/architecture.md#modules).

There are unit tests now (`cargo test`), including one that catches a phrase in
`moments.rs` using a character with no glyph in the font — which previously
failed silently, rendering a blank gap on screen.

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

Wayland deliberately doesn't expose the global cursor position, so every
compositor needs its own answer. That's now behind a `CursorSource` trait in
`src/cursor.rs`: supporting a new compositor means writing one implementation
and adding one line to `detect()`, with nothing else in the program changing.
See [cursor tracking](docs/architecture.md#cursor-tracking) for the trait and
its contract, and the [roadmap](docs/roadmap.md#running-on-other-desktops) for
which compositors are reachable.
