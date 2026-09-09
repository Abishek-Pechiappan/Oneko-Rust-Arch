# oneko in rust

A rewrite of the classic [**oneko**](https://github.com/tie/oneko) desktop cat, in **Rust**, for **Arch Linux + Hyprland**.

A little pixel-art cat chases your cursor around the screen. Leave the mouse alone and it sits down, washes its face, scratches its head, yawns, and falls asleep — just like the 1990s X11 original, but running natively on Wayland.

![demo](demo.gif)

## Features

- **Chases your cursor** with smooth, framerate-independent easing and 8-directional walking sprites
- **The original's full idle sequence** — sit, wash, scratch, yawn, sleep, and a startled wake-up
- **Wall scratching** when it's pressed against a screen edge and can't reach you
- **Proximity-based** — a parked cat stays parked instead of reacting to every mouse movement
- **Random moments** — occasional speech bubbles (`meow`, `purrr~`, `nya~`) and quirky poses
- **Click to freeze** it in place; click again to release
- **Multi-monitor aware**, including hotplug
- **Six characters** — `neko`, `tora`, `dog`, `sakura`, `tomoyo`, `bsd` — plus your own
- **Cheap to leave running** — 0.15% of one core while idle
- **No runtime assets** — sprites are compiled into the binary

## Requirements

- [Hyprland](https://hypr.land) — cursor tracking uses its IPC socket; other compositors need a [`CursorSource` backend](docs/roadmap.md)
- Rust toolchain (`rustup`, or `pacman -S rust`)

## Install

```sh
./install.sh
```

Builds it, installs to `~/.local/bin`, then asks which cat you want and how smooth the motion should be, offers to add a Hyprland autostart entry with those options, and starts it. Re-run it any time to change your choices — it updates its own entry rather than adding a second one, and backs up your config first.

It handles both `hyprland.lua` and `hyprland.conf`, and takes flags if you'd rather not be asked:

```sh
./install.sh -y                        # accept defaults, no prompts
./install.sh --skin bsd --fps 8        # pick options up front
./install.sh --no-autostart            # build and install only
```

Or do it by hand:

```sh
cargo build --release
./target/release/oneko-rust
```

## Usage

Move the cursor near the cat to get chased. Leave it alone and it settles into its idle sequence. Left-click the cat to freeze it in place; click again to unfreeze.

```
-s, --skin <NAME>    Character to draw [default: neko]
    --fps <N>        Motion updates per second, 1-240 [default: 30]
    --list-skins     List available skins and exit
-h, --help           Print help
-V, --version        Print version
```

The sprite animation always keeps its original 8 Hz cadence; `--fps` only changes how smoothly the cat moves. Use `--fps 8` for the classic stepped chase.

Drop your own art in `~/.config/oneko-rust/skins/<name>/` and it's picked up without a rebuild — see [writing a skin](assets/README.md).

## Autostart

`hyprland.conf`:

```ini
exec-once = ~/.local/bin/oneko-rust
```

`hyprland.lua` (Hyprland ≥ 0.55):

```lua
hl.on("hyprland.start", function()
    hl.exec_cmd("~/.local/bin/oneko-rust")
end)
```

Stop it with `pkill oneko-rust`.

## Documentation

- [Changelog](CHANGELOG.md) — what's changed, with measurements
- [Architecture](docs/architecture.md) — how it works, for hacking on it
- [Roadmap](docs/roadmap.md) — other desktops, planned work, and what isn't planned
- [Writing a skin](assets/README.md) — pose list, mask convention, PNG conversion

## Limitations

- **Hyprland only** for now. Wayland deliberately doesn't expose the global cursor position, so every compositor needs its own backend — see the [roadmap](docs/roadmap.md).
- Clicks landing on the cat's 32×32 box are consumed, so whatever is underneath won't receive that click.
- One cat at a time, on whichever monitor holds the cursor.

## Credits

Sprites and behavior come from the original [oneko](https://github.com/tie/oneko) by Masayuki Koba, which its maintainers describe as public domain (no formal license file). All six character sets in `assets/skins/` are that original art, copied unmodified and renamed — see [`assets/README.md`](assets/README.md) for the pose mapping. The `stretch` and `tailflick` poses are original to this project.

The BSD daemon is copyright 1988 Marshall Kirk McKusick.

## License

[GNU General Public License v3.0](LICENSE).
