# oneko-rust for GNOME/Ubuntu

GNOME's Mutter compositor doesn't support `wlr-layer-shell` or `hyprctl`, so
the [Hyprland backend](../hyprland) can't run here. This directory has a
different backend built the only way GNOME allows an always-on-top overlay
that tracks the global cursor: a GNOME Shell extension.

## How it's split

- **`daemon/`** - a Rust background process (`oneko-daemon`) that owns the
  cat's animation state and does all the pixel rendering, via the
  [`oneko-core`](../oneko-core) crate shared with the Hyprland binary. It
  can't get the cursor position or draw on screen itself, so it exposes a
  D-Bus service instead.
- **`extension/`** - a small GNOME Shell extension (JavaScript) that
  supplies the two things the daemon can't get on its own: the global
  cursor position and an always-on-top actor to draw the cat into. It calls
  the daemon once per 125ms animation tick and relays clicks back for the
  freeze toggle.

See the comments at the top of [`daemon/src/main.rs`](daemon/src/main.rs)
and [`extension/extension.js`](extension/extension.js) for the D-Bus
contract between them.

## Requirements

- GNOME Shell 45 or newer (Ubuntu 24.04 ships GNOME 46). Older GNOME (e.g.
  Ubuntu 22.04's GNOME 42) uses a different extension API and isn't
  supported here.
- Rust toolchain (`rustup`, or `sudo apt install rustc cargo`)

Works under both GNOME/Wayland and GNOME/Xorg sessions - the extension API
this uses is the same either way.

## Install

```sh
./install.sh
```

Builds `oneko-daemon`, installs it to `~/.local/bin`, copies the extension
to `~/.local/share/gnome-shell/extensions/`, and enables it. The extension
spawns/kills the daemon itself in its enable()/disable() lifecycle, so
there's nothing separate to start - toggling the extension (Extensions app,
or `gnome-extensions enable/disable oneko-rust@abishek-pechiappan.github.io`)
is the only control needed.

If the cat doesn't appear right after install, a brand-new extension
directory sometimes isn't picked up until you log out and back in.

## Known rough edges

This backend has not been run against a live GNOME session during
development - only compiled and reviewed. The most likely first-run issues:

- **Wrong colors / a solid block instead of a cat**: a pixel byte-order
  mismatch between what the daemon writes and what
  `Cogl.PixelFormat.BGRA_8888_PRE` expects - see the comment at the top of
  `extension.js`.
- **Cat never appears**: check `journalctl --user -f` (or `Looking Glass`,
  `Alt+F2` -> `lg`) while the extension is enabled for D-Bus connection
  errors - most likely the daemon failing to start or claim its bus name.
- **Cat position drifts on multi-monitor setups**: the monitor-geometry
  lookup in `extension.js`'s `_tick()` may need adjusting for your monitor
  arrangement.

Please open an issue (or just report back) with what you see - this will
need a couple of iterations against a real GNOME session to shake out.
