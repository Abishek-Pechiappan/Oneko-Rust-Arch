# Sprite assets

Each directory here is one skin. `build.rs` compiles every skin in this
directory into the binary at build time, so adding a character is just adding a
directory — no code changes.

The same layout works for skins you don't want to compile in: drop a directory
into `$XDG_CONFIG_HOME/oneko-rust/skins/` (usually
`~/.config/oneko-rust/skins/`) and run `oneko-rust --skin <dirname>`. Copying
one of these directories is the easiest way to start.

## Format

Every pose is a pair of 32×32 XBM files:

| file | meaning |
| --- | --- |
| `<pose>.xbm` | the sprite bits |
| `<pose>_mask.xbm` | which pixels are drawn at all |

They combine per pixel:

| mask bit | sprite bit | result |
| --- | --- | --- |
| set | set | opaque black |
| set | clear | opaque white |
| clear | — | transparent |

XBM is a plain-text C array that GIMP, ImageMagick and most image tools read and
write, so you can edit a pose with `convert`, or in an editor if you're brave:

```sh
# PNG -> XBM (1-bit, 32x32)
magick pose.png -resize 32x32 -monochrome pose.xbm
```

## Required poses

Every skin must provide all of these, each with its `_mask` companion. A missing
or malformed file fails the build (for skins here) or startup (for user skins),
naming the file.

- **Walking**, two frames each, for eight directions:
  `run_n1`/`run_n2`, `run_ne1`/`run_ne2`, `run_e1`/`run_e2`, `run_se1`/`run_se2`,
  `run_s1`/`run_s2`, `run_sw1`/`run_sw2`, `run_w1`/`run_w2`, `run_nw1`/`run_nw2`
- **Idle**: `sit`, `wash1`, `wash2`, `sleep1`, `sleep2`

## Optional poses

A skin missing any of these simply skips that behavior, so a minimal skin still
works.

- `scratch1`, `scratch2` — scratching its head, a stage of the idle sequence.
  Both frames or neither.
- `yawn` — yawning, the stage just before sleep.
- `awake` — the startled pose shown briefly when the cursor returns.
- `wall_n1`/`wall_n2`, `wall_e1`/…, `wall_s1`/…, `wall_w1`/… — scratching the
  screen edge the cat is stuck against, one pair per edge. All eight or none.
- `stretch`, `tailflick` — used by the random "moment" system. A skin without
  them still shows the speech bubble; it just keeps its normal idle pose. Only
  `neko` has these, since they were drawn for this project rather than carried
  over from oneko.

## Where the art came from

`neko`, `tora`, `dog`, `sakura`, `tomoyo` and `bsd` are the original
[oneko](https://github.com/tie/oneko) bitmaps by Masayuki Koba, which oneko's
maintainers describe as public domain (the project carries no formal license
file). They were copied unmodified — only renamed from oneko's names to the
pose names above:

| this repo | oneko |
| --- | --- |
| `run_n`, `run_s`, `run_e`, `run_w` | `up`, `down`, `right`, `left` |
| `run_ne`, `run_nw` | `upright`, `upleft` |
| `run_se`, `run_sw` | `dwright`, `dwleft` |
| `sit` | `mati2` |
| `wash1`, `wash2` | `jare2`, `mati2` |
| `scratch1`, `scratch2` | `kaki1`, `kaki2` |
| `yawn` | `mati3` |
| `awake` | `awake` |
| `sleep1`, `sleep2` | `sleep1`, `sleep2` |
| `wall_n`, `wall_e`, `wall_s`, `wall_w` | `utogi`, `rtogi`, `dtogi`, `ltogi` |

Two details worth knowing if you go back to the upstream art:

- **`tora` has no masks of its own.** Upstream reuses `neko`'s, because tora is
  a striped repaint of the same silhouette. They're copied in here so every skin
  directory is self-contained. A few tora stripe pixels fall outside that shared
  mask and render as transparent — that's upstream's behavior, preserved.
- **`wash2` is a copy of `sit`.** Upstream's washing state alternates `jare2`
  with `mati2`, which is the same bitmap as the sit pose. It's duplicated here
  rather than aliased so every pose is one editable file, and so a skin can make
  the two differ if it wants to.

`stretch` and `tailflick` are original to this project, built from the `sit`
pose, and are covered by the repository's GPL-3.0 license.
