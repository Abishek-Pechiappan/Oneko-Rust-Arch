//! Compiles the XBM sprite art under `assets/skins/` into Rust sprite tables.
//!
//! The sprites used to be 529 lines of hand-written hex literals in
//! `src/sprites.rs`, with a comment warning that hand-flipping bits was
//! "extremely error-prone" and pointing at throwaway scripts in git history.
//! Now the art is checked in as real `.xbm` files - a format GIMP, ImageMagick
//! and friends read and write - and this script turns them into the exact same
//! `[u8; 128]` arrays at build time. Runtime representation and cost are
//! unchanged; only the authoring story improves.
//!
//! Output goes to `$OUT_DIR/sprites.rs`, included by `src/sprites.rs`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// The XBM decoder is shared verbatim with the runtime skin loader. A build
// script can't `use` the crate it builds, so it's pulled in textually - see the
// header comment in src/xbm.rs.
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/xbm.rs"));

/// Every pose the program needs, in the order `Skin`'s fields expect.
///
/// The `run_*` names are indexed by compass direction; `Dir::index()` in
/// `src/cat.rs` must agree with the order of `RUN_POSES` below, which a test
/// in `src/sprites.rs` checks.
const RUN_POSES: [&str; 8] = [
    "run_n", "run_ne", "run_e", "run_se", "run_s", "run_sw", "run_w", "run_nw",
];

/// Poses every skin must provide, or the build fails.
const REQUIRED: &[&str] = &["sit", "wash1", "wash2", "sleep1", "sleep2"];

/// Single-sprite poses a skin may omit.
///
/// `stretch`/`tailflick` are hand-authored additions used by the Moment system;
/// a skin without them still shows the phrase and keeps its normal idle pose.
/// `awake`/`yawn` are stages of the idle sequence, skipped when absent.
const OPTIONAL: &[&str] = &["stretch", "tailflick", "awake", "yawn"];

/// The four walls the cat can scratch, in `Skin::wall` order. Either all eight
/// files are present or the skin simply doesn't wall-scratch.
const WALL_POSES: [&str; 4] = ["wall_n", "wall_e", "wall_s", "wall_w"];

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skins");
    println!("cargo:rerun-if-changed={}", assets.display());
    println!("cargo:rerun-if-changed=src/xbm.rs");

    let mut skins: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();

    // BTreeMap via sorted dir listing so generated output is deterministic -
    // an unstable order would make the build non-reproducible and churn the
    // generated file for no reason.
    for dir in sorted_dirs(&assets) {
        let skin = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_else(|| panic!("non-UTF8 skin directory name: {}", dir.display()))
            .to_owned();

        println!("cargo:rerun-if-changed={}", dir.display());

        let mut poses: BTreeMap<String, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
        for pose in RUN_POSES
            .iter()
            .flat_map(|d| [format!("{d}1"), format!("{d}2")])
            .chain(REQUIRED.iter().map(|s| s.to_string()))
        {
            poses.insert(pose.clone(), load_pair(&dir, &skin, &pose));
        }

        let mut optional: BTreeMap<String, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
        for pose in OPTIONAL {
            if dir.join(format!("{pose}.xbm")).exists() {
                optional.insert((*pose).to_string(), load_pair(&dir, &skin, pose));
            }
        }

        let mut s = String::new();
        writeln!(s, "    Skin {{").unwrap();
        writeln!(s, "        name: {skin:?},").unwrap();

        writeln!(s, "        run: [").unwrap();
        for d in RUN_POSES {
            writeln!(s, "            [").unwrap();
            for frame in ["1", "2"] {
                let (bits, mask) = &poses[&format!("{d}{frame}")];
                writeln!(s, "{},", sprite_literal(bits, mask, 16)).unwrap();
            }
            writeln!(s, "            ],").unwrap();
        }
        writeln!(s, "        ],").unwrap();

        let (bits, mask) = &poses["sit"];
        writeln!(s, "        sit:").unwrap();
        writeln!(s, "{},", sprite_literal(bits, mask, 12)).unwrap();
        for (field, a, b) in [("wash", "wash1", "wash2"), ("sleep", "sleep1", "sleep2")] {
            writeln!(s, "        {field}: [").unwrap();
            for pose in [a, b] {
                let (bits, mask) = &poses[pose];
                writeln!(s, "{},", sprite_literal(bits, mask, 12)).unwrap();
            }
            writeln!(s, "        ],").unwrap();
        }
        // Head-scratching: a two-frame pair, all or nothing.
        if dir.join("scratch1.xbm").exists() {
            writeln!(s, "        scratch: Some([").unwrap();
            for frame in ["scratch1", "scratch2"] {
                let (bits, mask) = load_pair(&dir, &skin, frame);
                writeln!(s, "{},", sprite_literal(&bits, &mask, 12)).unwrap();
            }
            writeln!(s, "        ]),").unwrap();
        } else {
            writeln!(s, "        scratch: None,").unwrap();
        }

        // Wall scratching: all four directions, two frames each, or nothing.
        if WALL_POSES.iter().all(|w| dir.join(format!("{w}1.xbm")).exists()) {
            writeln!(s, "        wall: Some([").unwrap();
            for w in WALL_POSES {
                writeln!(s, "            [").unwrap();
                for frame in ["1", "2"] {
                    let (bits, mask) = load_pair(&dir, &skin, &format!("{w}{frame}"));
                    writeln!(s, "{},", sprite_literal(&bits, &mask, 16)).unwrap();
                }
                writeln!(s, "            ],").unwrap();
            }
            writeln!(s, "        ]),").unwrap();
        } else {
            writeln!(s, "        wall: None,").unwrap();
        }

        for pose in OPTIONAL {
            match optional.get(*pose) {
                Some((bits, mask)) => {
                    writeln!(s, "        {pose}: Some(").unwrap();
                    writeln!(s, "{}", sprite_literal(bits, mask, 12)).unwrap();
                    writeln!(s, "        ),").unwrap();
                }
                None => writeln!(s, "        {pose}: None,").unwrap(),
            }
        }
        writeln!(s, "    }},").unwrap();

        skins.push(s);
        names.push(skin);
    }

    assert!(
        names.iter().any(|n| n == "neko"),
        "assets/skins must contain the default `neko` skin; found: {names:?}"
    );

    let mut out = String::new();
    out.push_str("// @generated by build.rs from assets/skins/ - do not edit.\n\n");
    writeln!(out, "pub static SKINS: &[Skin] = &[").unwrap();
    for s in &skins {
        out.push_str(s);
    }
    writeln!(out, "];").unwrap();

    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"))
        .join("sprites.rs");
    std::fs::write(&dest, out)
        .unwrap_or_else(|e| panic!("writing {}: {e}", dest.display()));
}

/// Skin directories, sorted, so the generated table order is stable.
fn sorted_dirs(assets: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(assets)
        .unwrap_or_else(|e| panic!("reading {}: {e}", assets.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no skins found in {}", assets.display());
    dirs
}

/// Loads a pose's `<name>.xbm` plus its `<name>_mask.xbm` companion.
fn load_pair(dir: &Path, skin: &str, pose: &str) -> (Vec<u8>, Vec<u8>) {
    let bits = parse_xbm(&dir.join(format!("{pose}.xbm")), skin, pose);
    let mask = parse_xbm(&dir.join(format!("{pose}_mask.xbm")), skin, pose);
    (bits, mask)
}

/// Reads one pose file, turning any decode failure into a build error that
/// names the offending file - the author is editing art, so the message needs
/// to point at the art, not at a byte offset.
fn parse_xbm(path: &Path, skin: &str, pose: &str) -> Vec<u8> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("{skin}/{pose}: cannot read {}: {e}", path.display())
    });
    parse(&text)
        .unwrap_or_else(|e| panic!("{skin}/{pose} ({}): {e}", path.display()))
        .to_vec()
}

/// Renders one `Sprite { bits, mask }` literal, indented by `indent` spaces.
/// No trailing comma - call sites add one where the grammar needs it.
fn sprite_literal(bits: &[u8], mask: &[u8], indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut s = String::new();
    writeln!(s, "{pad}Sprite {{").unwrap();
    for (field, data) in [("bits", bits), ("mask", mask)] {
        writeln!(s, "{pad}    {field}: [").unwrap();
        for row in data.chunks(16) {
            let cells: Vec<String> = row.iter().map(|b| format!("0x{b:02x}")).collect();
            writeln!(s, "{pad}        {},", cells.join(", ")).unwrap();
        }
        writeln!(s, "{pad}    ],").unwrap();
    }
    write!(s, "{pad}}}").unwrap();
    s
}
