// XBM decoding, shared by two callers with no dependency between them:
//
//   - build.rs, which `include!`s this file to compile `assets/skins/` into the
//     binary. Build scripts can't use the crate they build, so `include!` is
//     how the two stay in sync.
//   - the runtime skin loader in sprites.rs, for art dropped into
//     ~/.config/oneko-rust/skins/.
//
// Because build.rs includes it textually, this file must stay standalone: no
// `use crate::...`, no dependencies, nothing but std.

/// Sprites are 1-bit 32x32, which is 4 bytes per row over 32 rows.
pub const SPRITE_SIZE: usize = 32;
pub const SPRITE_BYTES: usize = SPRITE_SIZE * SPRITE_SIZE / 8;

/// Parses an XBM file's pixel data.
///
/// Deliberately lenient about the surrounding C - `static char` vs
/// `static unsigned char`, line wrapping, trailing commas, `0X` vs `0x` - so
/// files written by any of the usual image tools load. Strict about the two
/// things that would silently corrupt rendering: the declared dimensions and
/// the byte count.
pub fn parse(text: &str) -> Result<[u8; SPRITE_BYTES], String> {
    for key in ["width", "height"] {
        if let Some(v) = find_define(text, key) {
            if v != SPRITE_SIZE {
                return Err(format!(
                    "{key} is {v}, but sprites must be {SPRITE_SIZE}x{SPRITE_SIZE}"
                ));
            }
        }
    }

    let bytes = parse_hex_bytes(text);
    <[u8; SPRITE_BYTES]>::try_from(bytes.as_slice()).map_err(|_| {
        format!(
            "got {} bytes, expected {SPRITE_BYTES} ({SPRITE_SIZE}x{SPRITE_SIZE} at 1 bit/px)",
            bytes.len(),
        )
    })
}

/// Reads an XBM `#define <name>_<key> <int>` line, e.g. `#define sit_width 32`.
pub fn find_define(text: &str, key: &str) -> Option<usize> {
    text.lines()
        .filter(|l| l.trim_start().starts_with("#define"))
        .find_map(|l| {
            let (name, value) = l.trim_end().rsplit_once(char::is_whitespace)?;
            name.trim_end().ends_with(key).then(|| value.trim().parse().ok())?
        })
}

/// Pulls every `0x..` literal out of the text, in order.
///
/// Scanning for hex literals rather than parsing C means a stray `#define` or
/// an unusual declaration can't throw the reader off; the only thing that
/// matters is the byte list, and its length is checked by the caller.
pub fn parse_hex_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(SPRITE_BYTES);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'0' && (bytes[i + 1] | 0x20) == b'x' {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && (bytes[end] as char).is_ascii_hexdigit() {
                end += 1;
            }
            // XBM is byte data, so cap at two digits: a stray longer literal
            // shouldn't silently merge two pixels' worth of bits into one.
            let stop = end.min(start + 2);
            if stop > start {
                if let Ok(v) = u8::from_str_radix(&text[start..stop], 16) {
                    out.push(v);
                }
            }
            i = end.max(start + 1);
        } else {
            i += 1;
        }
    }
    out
}
