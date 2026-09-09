// The pool of things the cat can randomly say/do while idle (see the Moment
// trigger logic in cat.rs).
//
// To add a new phrase, just add an entry - every character in it must have a
// glyph in `font::FONT`, which a test enforces. To add a new quirky pose, drop
// the art into `assets/skins/<skin>/<pose>.xbm` (plus its `_mask` companion),
// add a variant to `sprites::Pose`, and reference it here.

use crate::sprites::Pose;

// One entry: a speech-bubble phrase and/or a pose to briefly show instead of
// the normal idle pose. `text: ""` means no bubble (quirk-only); `quirk: None`
// means no pose change (speech-only, cat keeps its normal idle animation).
//
// A quirk pose that the active skin doesn't provide is simply skipped - the
// phrase still shows. Only the `neko` art has the hand-authored quirk poses,
// so this is the normal case for every other skin.
pub struct Moment {
    pub text: &'static str,
    pub quirk: Option<Pose>,
}

pub const MOMENTS: &[Moment] = &[
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
    Moment { text: "*stretch*", quirk: Some(Pose::Stretch) },
    Moment { text: "", quirk: Some(Pose::Tailflick) },
];
