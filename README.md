# oneko in rust

A rewrite of the classic [**oneko**](https://github.com/tie/oneko) desktop cat, in **Rust**, for **Arch Linux + Hyprland** - plus a [GNOME/Ubuntu backend](ubuntu-version/) for everyone else.

A little pixel-art cat chases your cursor around the screen. When you stop moving the mouse it sits down, washes itself, and eventually falls asleep — just like the 1990s X11 original, but running natively on Wayland, with multi-monitor support and a proximity-based chase so it doesn't chase your cursor across the whole desktop. Click the cat to freeze it in place; click again to let it resume chasing.

![demo](demo.gif)

## Features

- **Cursor chasing** with smooth easing and full 8-directional walking sprites
- **Proximity-based chasing** — the cat only wakes up and starts chasing once the cursor comes within range; cursor movement elsewhere on screen is ignored, so a parked cat stays parked instead of being yanked around by every mouse movement. Once it's actively chasing, it won't give up mid-pursuit even if you move the cursor fast
- **Idle animation stages** — sits, then washes itself, then falls asleep the longer the cursor stays away/still, in that order
- **Random idle "moments"** — occasional speech bubbles (`meow`, `purrr~`, `nya~`, ...) and quirky animations (stretch, tail-flick) while idle
- **Click-to-freeze** — left-click the cat to pin it in place (it still runs through its idle animations), click again to release it
- **Multi-monitor aware** — a layer-shell surface is created per connected output; only the monitor currently containing the cursor shows/animates the cat, and it hands off cleanly as the cursor crosses monitors, including hotplug of new/removed outputs
- **Efficient redraws** — skips the whole allocate/blit/commit pass whenever a frame would be pixel-identical to what's already on screen (e.g. while sitting or asleep), instead of recompositing 8x/second forever
- **Zero external assets** — the original 32×32 XBM sprites are embedded directly in the binary

## Why a rewrite?

The original oneko (and most clones) rely on X11 tricks — override-redirect windows and the SHAPE extension — that don't work under Wayland compositors like Hyprland. This version uses:

- **`wlr-layer-shell`** (via [smithay-client-toolkit](https://crates.io/crates/smithay-client-toolkit)) for an always-on-top overlay surface
- **ARGB transparency** instead of the X11 SHAPE extension
- **`hyprctl cursorpos`** to track the cursor globally
- A **small input region matching the cat's 32×32 box**, so it can catch clicks to toggle freezing without stealing focus or blocking anything outside its own bounds

## Usage

Move the cursor near the cat to wake it up and get chased; leave it alone (or stay far away) and it'll sit down, wash itself, and eventually fall asleep. Left-click the cat to freeze it in place; click again to unfreeze. While frozen it still sits, washes, and sleeps if left alone — it just won't chase. Every so often while idle, the cat may pop up a tiny speech bubble ("meow", "purrr~"...) or do a quirky animation like a stretch or tail-flick, then carry on as normal.

## Requirements

- Arch Linux (or any Linux distro, really)
- [Hyprland](https://hypr.land) — the cursor tracking uses `hyprctl`; any other wlroots-based compositor would need a different cursor source
- Rust toolchain (`rustup` or `pacman -S rust`)

On GNOME (e.g. stock Ubuntu), none of the above applies — see
[`ubuntu-version/`](ubuntu-version/) for a separate backend built around a
GNOME Shell extension instead.

## Build & run

This repo is a Cargo workspace; the Hyprland binary is the `oneko-rust`
package within it (`hyprland/`), alongside the shared `oneko-core` crate and
the GNOME backend's `oneko-daemon` crate:

```sh
cargo build --release -p oneko-rust
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

- Hyprland-specific: cursor tracking uses `hyprctl cursorpos`, so porting to another wlroots compositor means swapping out that one function for whatever that compositor exposes. GNOME/Mutter doesn't support `wlr-layer-shell` or `hyprctl` at all — see [`ubuntu-version/`](ubuntu-version/) for that platform's separate GNOME Shell extension-based backend instead.
- Clicks landing inside the cat's current 32×32 box are consumed to detect the freeze toggle, so anything beneath the cat at that instant won't receive that click.
- Only one cat is shown at a time, on whichever monitor currently contains the cursor — it's not simultaneously visible/independent on every monitor.

## Credits

Sprites and behavior are taken from the original [oneko](https://github.com/tie/oneko) by Masayuki Koba, which its maintainers describe as public domain software (no formal license file).

## License

This rewrite is licensed under the [GNU General Public License v3.0](LICENSE).
