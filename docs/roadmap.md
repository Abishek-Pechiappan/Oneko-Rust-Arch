# Roadmap

What's planned, what isn't, and why. See [`architecture.md`](architecture.md)
for how the current code is put together.

## Running on other desktops

Two separate problems, and it's worth keeping them apart:

- **Rendering** needs `wlr-layer-shell`. That exists on wlroots compositors
  (Hyprland, sway, river, Wayfire, niri, labwc) and on KWin. It does not exist
  on GNOME, and it doesn't exist on X11 at all.
- **Cursor position** needs a `CursorSource` backend, because Wayland
  deliberately withholds the global pointer position.

| target | rendering | cursor | status |
| --- | --- | --- | --- |
| Hyprland | layer-shell | IPC socket | works |
| KDE Plasma (Wayland) | layer-shell ✓ | needs a KWin script | cursor backend only |
| sway, river, Wayfire, niri, labwc | layer-shell ✓ | no known clean method | blocked, see below |
| XFCE, Cinnamon, MATE, i3, LXQt, KDE/GNOME on X11 | needs an X11 backend | `XQueryPointer` | needs both halves |
| GNOME (Wayland) | no layer-shell | no method | not planned |

### X11 backend

The biggest reach by far — XFCE, Cinnamon, MATE, i3, LXQt and the X11 sessions
of KDE and GNOME, all at once. It is not just a `CursorSource` implementation
though: X11 needs its own window creation (override-redirect plus the SHAPE
extension, as the original oneko did), its own RandR monitor enumeration, and
its own event loop. `wlr-layer-shell` has no counterpart there.

It also uses a different coordinate origin. Probing a dual-monitor Hyprland
session where the second monitor sits at global x = −2560, X reports a single
4480×1440 screen with a non-negative origin.

This needs a seam between the cat's behavior and the platform that draws it;
today `cat.rs` and `app.rs` are welded to `LayerSurface`, `SlotPool` and
`wl_output`.

`Xephyr` makes it testable without leaving a Wayland session: a nested X server
has a real root window and working `XQueryPointer`.

### KDE Plasma on Wayland

The cheapest remaining target. KWin implements `wlr-layer-shell`, so the
rendering path already works — only a cursor backend is missing. The known
approach is loading a KWin script over D-Bus that polls `workspace.cursorPos`,
which is what `kdotool` does.

### Why sway, niri and river are blocked

An earlier version of this document proposed the
[wl-find-cursor](https://github.com/cjacker/wl-find-cursor) technique — a
fullscreen layer surface plus `zwlr_virtual_pointer_v1`, nudged by a
zero-distance relative motion to provoke a `wl_pointer.motion` event carrying
surface-local coordinates.

**That doesn't work for a program that stays running.** Receiving pointer motion
requires an input region covering the screen, and a layer surface with one
swallows every click underneath it. That's acceptable for a one-shot tool that
prints a coordinate and exits; it is not acceptable for a desktop pet.

No wlroots compositor exposes cursor position over IPC either — sway's own
`get_cursor` proposal ([PR #8780](https://github.com/swaywm/sway/pull/8780)) was
left unmerged, with maintainers pointing at exactly the layer-shell/virtual-
pointer approach above.

So these compositors are blocked on someone finding a method that doesn't steal
input. Reading `/dev/input` through libinput is the remaining option, but it
gives relative deltas rather than absolute position and needs the user in the
`input` group.

### Why GNOME Wayland isn't planned

No layer-shell, no virtual-pointer, and `org.gnome.Shell.Eval` is locked down
outside unsafe mode. Doing it properly means writing a GNOME Shell extension,
which is a separate project in a different language.

## Other work

- **HiDPI** — honor the output scale factor and render at integer scale, so the
  cat stays crisp instead of being bilinearly upscaled on 2× displays. Rendering
  at 2× would also give sub-pixel motion for free, which is the remaining limit
  on smoothness: position is committed as an integer layer-shell margin.
- **Frame callbacks and persistent buffers** — drive redraws from
  `wl_surface::frame()` instead of a timer, so the cat animates at exactly the
  display's refresh rate and never submits a frame that gets dropped, and
  ping-pong two buffers instead of allocating per frame. Together these are what
  would make `--fps 60` cost what `--fps 30` does today — see the measurements
  in the [changelog](../CHANGELOG.md).
- **Don't draw over fullscreen windows** — hide the cat when the focused window
  is fullscreen, which Hyprland will report. Probably the highest-value item
  here for whether people keep it running.
- **Idle-aware sleeping** via `ext-idle-notify-v1` — sleep because the user is
  away, not just because the cursor is still, and stop polling entirely while
  they are.
- **More CLI flags and a config file** — `--scale`, `--speed`, `--radius`,
  `--all-monitors`, `--fg`/`--bg` colors, and a config file so the flags don't
  have to live in a compositor autostart line.
- **Drag to reposition** — click already toggles freeze; click-and-drag to place
  the cat is a small extension of plumbing that exists.
- **PNG skins** — the runtime loader takes `.xbm` today. PNG needs a decoder
  dependency and conventions for how alpha and luminance map to the bit/mask
  pair, plus cell size and grid order for sprite sheets.
- **Packaging** — a PKGBUILD for the AUR, a systemd user unit, and an XDG
  autostart entry, so setup doesn't require editing a compositor config.
- **CI** — build, `clippy -D warnings`, `fmt --check`, and `cargo test`.

## Not planned

- **Sound.** Needs an audio dependency, and a desktop toy that makes noise gets
  uninstalled.
- **Tray icon or settings GUI.** Would multiply the dependency count for a
  layer-shell app with no toolkit.
- **Food, petting, stat-tracking.** That's a different product wearing oneko's
  sprites.
