// Sprite art, compiled from `assets/skins/*.xbm` by build.rs.
//
// This file holds only the types and lookups; the data itself is generated
// into `$OUT_DIR/sprites.rs` and included at the bottom. To change or add art,
// edit the `.xbm` files under `assets/skins/` - nothing here needs touching.
//
// To add a whole new character, create `assets/skins/<name>/` containing the
// 21 required poses plus their `_mask` companions (see build.rs's REQUIRED and
// RUN_POSES); `stretch` and `tailflick` are optional. It'll be picked up
// automatically on the next build.

use std::path::{Path, PathBuf};

use crate::cat::Dir;
use crate::xbm;

/// One pose: the sprite bits plus the mask saying which pixels are drawn.
///
/// Packed exactly as XBM stores them - 4 bytes per row, 32 rows, LSB of each
/// byte is the leftmost pixel. At draw time (see `CatSurface::draw`):
///   mask bit set + sprite bit set  => opaque black
///   mask bit set, sprite bit clear => opaque white
///   mask bit clear                 => transparent
pub struct Sprite {
    pub bits: [u8; 128],
    pub mask: [u8; 128],
}

/// A complete character: every pose the animation code can ask for.
pub struct Skin {
    pub name: &'static str,
    /// Walking frames, indexed by `Dir::index()` then by animation frame.
    run: [[Sprite; 2]; 8],
    sit: Sprite,
    wash: [Sprite; 2],
    sleep: [Sprite; 2],
    /// Optional stages of the idle sequence. A skin missing one just skips
    /// that stage, so a minimal skin still works.
    scratch: Option<[Sprite; 2]>,
    yawn: Option<Sprite>,
    awake: Option<Sprite>,
    /// Wall-scratching, one two-frame pair per wall, in `WALL_ORDER`. Absent
    /// means the cat simply waits at the edge instead of scratching.
    wall: Option<[[Sprite; 2]; 4]>,
    /// Hand-authored quirk poses, absent from upstream oneko art. A skin
    /// without them still gets the Moment's speech bubble - it just keeps its
    /// normal idle pose instead of changing sprite.
    stretch: Option<Sprite>,
    tailflick: Option<Sprite>,
}

/// The walls, in the order `Skin::wall` stores them. Only the four cardinal
/// directions can be scratched - the cat squares up to a wall, it doesn't
/// scratch a corner diagonally.
pub const WALL_ORDER: [Dir; 4] = [Dir::N, Dir::E, Dir::S, Dir::W];

/// The `assets/skins/<skin>/` filename stem for a wall's scratch frames.
pub fn wall_stem(dir: Dir) -> &'static str {
    match dir {
        Dir::N => "wall_n",
        Dir::E => "wall_e",
        Dir::S => "wall_s",
        Dir::W => "wall_w",
        // Only cardinals are stored; callers gate on `wall_index` first.
        _ => "wall_n",
    }
}

/// Index into `Skin::wall`, or `None` for a diagonal.
pub fn wall_index(dir: Dir) -> Option<usize> {
    WALL_ORDER.iter().position(|d| *d == dir)
}

/// Which pose to draw. Carries no sprite reference, so `DrawState` can compare
/// poses by value for the dirty-check.
///
/// This replaces comparing `&'static [u8; 128]` pointers, which the old code
/// had to avoid doing: LLVM's constant-merging pass is free to unify two consts
/// that happen to be byte-identical, which would have made pointer equality
/// silently wrong. Comparing a small `Pose` is both cheaper and exact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pose {
    Run(Dir, bool),
    Sit,
    Wash(bool),
    Scratch(bool),
    Yawn,
    Sleep(bool),
    Awake,
    /// Scratching the wall the cat is pressed against; the `Dir` is which wall,
    /// and is always one of `WALL_ORDER`.
    Wall(Dir, bool),
    Stretch,
    Tailflick,
}

impl Skin {
    /// The sprite for `pose`, or `None` if this skin doesn't provide it (only
    /// possible for the optional quirk poses).
    pub fn sprite(&self, pose: Pose) -> Option<&Sprite> {
        Some(match pose {
            Pose::Run(dir, frame) => &self.run[dir.index()][usize::from(frame)],
            Pose::Sit => &self.sit,
            Pose::Wash(frame) => &self.wash[usize::from(frame)],
            Pose::Scratch(frame) => &self.scratch.as_ref()?[usize::from(frame)],
            Pose::Yawn => self.yawn.as_ref()?,
            Pose::Sleep(frame) => &self.sleep[usize::from(frame)],
            Pose::Awake => self.awake.as_ref()?,
            Pose::Wall(dir, frame) => {
                &self.wall.as_ref()?[wall_index(dir)?][usize::from(frame)]
            }
            Pose::Stretch => self.stretch.as_ref()?,
            Pose::Tailflick => self.tailflick.as_ref()?,
        })
    }

    /// Whether this skin can render `pose` - used to skip a Moment's sprite
    /// override rather than fall back to mismatched art.
    pub fn has(&self, pose: Pose) -> bool {
        self.sprite(pose).is_some()
    }
}

/// The skin used when none is requested: the original oneko cat.
pub const DEFAULT_SKIN: &str = "neko";

/// Looks up a skin by name, as passed on the command line.
pub fn by_name(name: &str) -> Option<&'static Skin> {
    SKINS.iter().find(|s| s.name == name)
}

/// The default skin. Panics only if the `neko` assets went missing, which
/// build.rs already refuses to build without.
pub fn default_skin() -> &'static Skin {
    by_name(DEFAULT_SKIN).expect("default skin present (enforced by build.rs)")
}

/// Every built-in skin name, for `--help` and error messages.
pub fn names() -> impl Iterator<Item = &'static str> {
    SKINS.iter().map(|s| s.name)
}

/// Where user-supplied skins live: `$XDG_CONFIG_HOME/oneko-rust/skins`,
/// falling back to `~/.config/oneko-rust/skins`.
pub fn user_skin_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("oneko-rust/skins"))
}

/// Names of user skins found on disk, sorted. Errors are swallowed: a missing
/// or unreadable skins directory just means there are none.
pub fn user_skin_names() -> Vec<String> {
    let Some(dir) = user_skin_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Resolves a skin by name: built-ins first, then `~/.config/oneko-rust/skins`.
///
/// A user skin shadowing a built-in name would be surprising and hard to
/// debug, so built-ins win and the loader says so.
pub fn resolve(name: &str) -> Result<&'static Skin, String> {
    if let Some(skin) = by_name(name) {
        return Ok(skin);
    }
    let dir = user_skin_dir()
        .map(|d| d.join(name))
        .ok_or_else(|| format!("unknown skin {name:?} and no home directory to search"))?;
    if !dir.is_dir() {
        return Err(format!(
            "unknown skin {name:?}\n  built-in: {}\n  looked for user art in: {}",
            names().collect::<Vec<_>>().join(", "),
            dir.display(),
        ));
    }
    let skin = load_from_dir(name, &dir)?;
    // Loaded once at startup and used for the life of the process, so leaking
    // it to get a `\'static` reference matches how built-in skins are held and
    // avoids threading a lifetime through every cat.
    Ok(Box::leak(Box::new(skin)))
}

/// Builds a `Skin` from a directory of `.xbm` files, using the same pose names
/// and layout as `assets/skins/` - so a user skin is authored exactly like a
/// built-in one, and a built-in can be copied out as a starting point.
pub fn load_from_dir(name: &str, dir: &Path) -> Result<Skin, String> {
    let pose_file = |pose: &str| -> Result<Sprite, String> {
        let load = |suffix: &str| -> Result<[u8; 128], String> {
            let path = dir.join(format!("{pose}{suffix}.xbm"));
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("skin {name:?}: cannot read {}: {e}", path.display()))?;
            xbm::parse(&text)
                .map_err(|e| format!("skin {name:?}: {}: {e}", path.display()))
        };
        Ok(Sprite { bits: load("")?, mask: load("_mask")? })
    };

    // Driven by `Dir::ALL` rather than a second hardcoded list, so the run
    // table can't get out of order with `Dir::index` - the same contract
    // build.rs relies on for the built-in skins.
    let mut run: Vec<[Sprite; 2]> = Vec::with_capacity(Dir::ALL.len());
    for d in Dir::ALL {
        let stem = d.asset_name();
        run.push([pose_file(&format!("{stem}1"))?, pose_file(&format!("{stem}2"))?]);
    }

    // Quirk poses are optional - a skin without them keeps its idle pose and
    // still shows the Moment's phrase.
    let optional = |pose: &str| -> Result<Option<Sprite>, String> {
        if dir.join(format!("{pose}.xbm")).exists() {
            pose_file(pose).map(Some)
        } else {
            Ok(None)
        }
    };

    Ok(Skin {
        // Leaked for the same reason as the skin itself; a skin name lives as
        // long as the process.
        name: Box::leak(name.to_owned().into_boxed_str()),
        run: run.try_into().map_err(|_| "internal: run table arity".to_string())?,
        sit: pose_file("sit")?,
        wash: [pose_file("wash1")?, pose_file("wash2")?],
        sleep: [pose_file("sleep1")?, pose_file("sleep2")?],
        // Head-scratching is a pair: both frames or neither.
        scratch: if dir.join("scratch1.xbm").exists() {
            Some([pose_file("scratch1")?, pose_file("scratch2")?])
        } else {
            None
        },
        yawn: optional("yawn")?,
        awake: optional("awake")?,
        // Wall-scratching is all four walls or none, so the cat can't end up
        // able to scratch only some edges.
        wall: if WALL_ORDER
            .iter()
            .all(|d| dir.join(format!("{}1.xbm", wall_stem(*d))).exists())
        {
            let mut walls = Vec::with_capacity(WALL_ORDER.len());
            for d in WALL_ORDER {
                let stem = wall_stem(d);
                walls.push([pose_file(&format!("{stem}1"))?, pose_file(&format!("{stem}2"))?]);
            }
            Some(walls.try_into().map_err(|_| "internal: wall arity".to_string())?)
        } else {
            None
        },
        stretch: optional("stretch")?,
        tailflick: optional("tailflick")?,
    })
}

include!(concat!(env!("OUT_DIR"), "/sprites.rs"));

/// Recorded art digests, asserted by the test below. Update only when art
/// is changed on purpose.
#[cfg(test)]
const EXPECTED_DIGESTS: &[(&str, u64)] = &[
    ("bsd", 2_686_659_434_751_408_177),
    ("dog", 13_015_649_302_522_873_897),
    ("neko", 4_099_426_467_513_268_140),
    ("sakura", 16_335_495_987_527_238_928),
    ("tomoyo", 9_004_108_221_273_561_833),
    ("tora", 1_152_095_044_623_147_164),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_order_matches_the_generated_run_table() {
        // build.rs emits `run` in RUN_POSES order and `Dir::index` reads it
        // back; if either side is reordered the cat walks facing the wrong way,
        // which is easy to miss and maddening to debug. Pin both halves.
        for (i, d) in Dir::ALL.iter().enumerate() {
            assert_eq!(d.index(), i, "{d:?} index");
        }
        assert_eq!(
            Dir::ALL.map(|d| d.asset_name()),
            ["run_n", "run_ne", "run_e", "run_se", "run_s", "run_sw", "run_w", "run_nw"],
        );
    }

    #[test]
    fn the_expected_skins_are_compiled_in() {
        let mut got: Vec<&str> = names().collect();
        got.sort_unstable();
        assert_eq!(got, ["bsd", "dog", "neko", "sakura", "tomoyo", "tora"]);
    }

    #[test]
    fn every_skin_has_every_required_pose() {
        for skin in SKINS {
            for dir in Dir::ALL {
                for frame in [false, true] {
                    assert!(
                        skin.has(Pose::Run(dir, frame)),
                        "{}: missing Run({dir:?}, {frame})",
                        skin.name,
                    );
                }
            }
            for pose in [Pose::Sit, Pose::Wash(false), Pose::Wash(true),
                         Pose::Sleep(false), Pose::Sleep(true)] {
                assert!(skin.has(pose), "{}: missing {pose:?}", skin.name);
            }
        }
    }

    #[test]
    fn only_neko_has_the_hand_authored_quirk_poses() {
        // The quirk poses were authored for this project, not carried over
        // from oneko, so they exist for neko alone. If that ever changes,
        // update this - it exists to document the asymmetry, not to forbid it.
        for skin in SKINS {
            let expected = skin.name == "neko";
            assert_eq!(skin.has(Pose::Stretch), expected, "{} stretch", skin.name);
            assert_eq!(skin.has(Pose::Tailflick), expected, "{} tailflick", skin.name);
        }
    }

    #[test]
    fn run_frames_differ_so_the_walk_cycle_animates() {
        // Two identical frames would make a direction look frozen while
        // walking - a plausible copy/paste slip when authoring a new skin.
        for skin in SKINS {
            for dir in Dir::ALL {
                let a = skin.sprite(Pose::Run(dir, false)).unwrap();
                let b = skin.sprite(Pose::Run(dir, true)).unwrap();
                assert_ne!(
                    (a.bits, a.mask), (b.bits, b.mask),
                    "{}: Run({dir:?}) frames are identical", skin.name,
                );
            }
        }
    }

    #[test]
    fn no_pose_is_entirely_blank() {
        // An all-zero mask draws nothing at all, which on screen looks exactly
        // like the cat vanishing. Catch an empty or misnamed file here.
        for skin in SKINS {
            for dir in Dir::ALL {
                for frame in [false, true] {
                    let s = skin.sprite(Pose::Run(dir, frame)).unwrap();
                    assert!(
                        s.mask.iter().any(|&b| b != 0),
                        "{}: Run({dir:?}, {frame}) has an empty mask", skin.name,
                    );
                }
            }
            for pose in [Pose::Sit, Pose::Wash(false), Pose::Wash(true),
                         Pose::Sleep(false), Pose::Sleep(true)] {
                let s = skin.sprite(pose).unwrap();
                assert!(
                    s.mask.iter().any(|&b| b != 0),
                    "{}: {pose:?} has an empty mask", skin.name,
                );
            }
        }
    }

    #[test]
    fn neko_art_is_byte_identical_to_the_previously_hardcoded_sprites() {
        // Guards the build.rs migration. These values were read out of the
        // hand-written consts this file replaced: `CAT_SITTING` (oneko's
        // mati2.xbm) and the hand-authored `CAT_STRETCH`.
        let neko = default_skin();

        let sit = neko.sprite(Pose::Sit).unwrap();
        assert_eq!(sit.bits[..12], [0x00; 12]);
        assert_eq!(
            &sit.bits[48..56],
            &[0x00, 0x22, 0x88, 0x00, 0x00, 0x02, 0x80, 0x00],
        );

        let stretch = neko.sprite(Pose::Stretch).unwrap();
        assert_eq!(&stretch.bits[4..8], &[0x00, 0x00, 0x00, 0x18]);
    }

    /// FNV-1a over a sprite's bits then mask. Enough to pin exact art without
    /// checking in 128 bytes per pose, and dependency-free.
    fn digest(s: &Sprite) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in s.bits.iter().chain(s.mask.iter()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h
    }

    #[test]
    fn all_skin_art_matches_its_recorded_digest() {
        // Pins every sprite of every skin byte-for-byte, so a corrupted asset,
        // a wrong mask pairing, or a decoder change is caught here rather than
        // noticed on screen weeks later. If you intentionally change art, run
        // the test and paste the reported digest in.
        let expected: &[(&str, u64)] = EXPECTED_DIGESTS;
        let mut actual: Vec<(&str, u64)> = Vec::new();
        for skin in SKINS {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for dir in Dir::ALL {
                for frame in [false, true] {
                    h ^= digest(skin.sprite(Pose::Run(dir, frame)).unwrap());
                }
            }
            for pose in [Pose::Sit, Pose::Wash(false), Pose::Wash(true),
                         Pose::Sleep(false), Pose::Sleep(true)] {
                h ^= digest(skin.sprite(pose).unwrap());
            }
            for wall in WALL_ORDER {
                for frame in [false, true] {
                    if let Some(s) = skin.sprite(Pose::Wall(wall, frame)) {
                        h ^= digest(s);
                    }
                }
            }
            for pose in [Pose::Scratch(false), Pose::Scratch(true), Pose::Yawn,
                         Pose::Awake, Pose::Stretch, Pose::Tailflick] {
                if let Some(s) = skin.sprite(pose) {
                    h ^= digest(s);
                }
            }
            actual.push((skin.name, h));
        }
        assert_eq!(
            actual, expected,
            "skin art changed; if deliberate, update EXPECTED_DIGESTS to the left-hand values",
        );
    }
}
