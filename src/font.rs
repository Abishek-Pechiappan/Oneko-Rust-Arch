// The 5x7 bitmap font and the speech-bubble renderer used by the Moment
// system. Self-contained: everything here operates on a caller-supplied
// ARGB8888 canvas and its width, so it knows nothing about Wayland.
// Size of one character cell in the speech-bubble font, in pixels.
pub const GLYPH_W: usize = 5;
pub const GLYPH_H: usize = 7;

// 5x7 pixel font, one row per byte, bit 4 = leftmost column (so the binary
// literals read left-to-right the same as the glyph shape, e.g. 0b01110 is
// ".###."). Only covers the characters actually used by MOMENTS below - if
// you add a phrase with a new character, add its glyph here too, or
// draw_text will silently skip that character (see glyph_for).
pub const FONT: &[(char, [u8; GLYPH_H])] = &[
    ('m', [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b00000]),
    ('e', [0b11110, 0b10000, 0b11110, 0b10000, 0b10000, 0b11110, 0b00000]),
    ('o', [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000]),
    ('w', [0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001, 0b00000]),
    ('z', [0b11111, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111, 0b00000]),
    ('!', [0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100, 0b00000]),
    ('?', [0b01110, 0b10001, 0b00010, 0b00100, 0b00000, 0b00100, 0b00000]),
    ('~', [0b00000, 0b00000, 0b01001, 0b10110, 0b00000, 0b00000, 0b00000]),
    ('*', [0b00000, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0b00000]),
    ('r', [0b11110, 0b10001, 0b10001, 0b11110, 0b10010, 0b10001, 0b00000]),
    ('p', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b00000]),
    ('u', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000]),
    ('n', [0b10001, 0b11001, 0b10101, 0b10101, 0b10011, 0b10001, 0b00000]),
    ('y', [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00000]),
    ('a', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b00000]),
    ('h', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b00000]),
    ('i', [0b00100, 0b00000, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000]),
    ('s', [0b01111, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110, 0b00000]),
    ('c', [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111, 0b00000]),
    ('t', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000]),
];

// Looks up a character's glyph in FONT; None for anything not in the table
// (draw_text just skips those, so unsupported characters render as blanks).
pub fn glyph_for(c: char) -> Option<&'static [u8; GLYPH_H]> {
    FONT.iter().find(|(fc, _)| *fc == c).map(|(_, g)| g)
}

// Writes one solid-color pixel block into an ARGB8888 canvas, matching the
// byte layout used by draw()/create_buffer (4 bytes per pixel, row-major).
pub fn put_pixel(canvas: &mut [u8], canvas_w: usize, x: i32, y: i32, argb: u32) {
    if x < 0 || y < 0 || x as usize >= canvas_w {
        return;
    }
    let idx = (y as usize * canvas_w + x as usize) * 4;
    if idx + 4 <= canvas.len() {
        canvas[idx..idx + 4].copy_from_slice(&argb.to_le_bytes());
    }
}

// Pixel width `text` would render at, at the given integer upscale factor
// (1 glyph pixel becomes `scale` x `scale` screen pixels). Used to center
// the speech bubble box around its text; see draw_bubble.
pub fn text_width(text: &str, scale: i32) -> i32 {
    text.chars().count() as i32 * (GLYPH_W as i32 + 1) * scale - scale
}

// Draws `text` in solid black starting at (x0, y0), one glyph after another,
// each glyph pixel blown up to a `scale` x `scale` block.
pub fn draw_text(canvas: &mut [u8], canvas_w: usize, text: &str, x0: i32, y0: i32, scale: i32) {
    let mut cx = x0;
    for ch in text.chars() {
        if let Some(glyph) = glyph_for(ch) {
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..GLYPH_W {
                    if bits & (1 << (GLYPH_W - 1 - col)) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                put_pixel(
                                    canvas,
                                    canvas_w,
                                    cx + col as i32 * scale + sx,
                                    y0 + row as i32 * scale + sy,
                                    0xFF00_0000,
                                );
                            }
                        }
                    }
                }
            }
        }
        cx += (GLYPH_W as i32 + 1) * scale;
    }
}

// Draws a small white speech-bubble box (black 1px border + downward nub)
// sized to fit `text`, then the text itself in black on top.
// SCALE is the font upscale factor (1 = native glyph pixels, 2 = double
// size, etc.) and PAD is the empty margin in pixels around the text inside
// the box - bump either up if the bubble/text ever needs to look bigger.
pub fn draw_bubble(canvas: &mut [u8], canvas_w: usize, text: &str) {
    const SCALE: i32 = 1;
    const PAD: i32 = 2;
    let text_w = text_width(text, SCALE);
    let box_w = text_w + PAD * 2;
    let box_h = GLYPH_H as i32 * SCALE + PAD * 2;
    let box_x0 = (canvas_w as i32 - box_w) / 2;
    let box_y0 = 1;

    for y in box_y0..box_y0 + box_h {
        for x in box_x0..box_x0 + box_w {
            let on_border = x == box_x0 || x == box_x0 + box_w - 1 || y == box_y0 || y == box_y0 + box_h - 1;
            let argb = if on_border { 0xFF00_0000 } else { 0xFFFF_FFFF };
            put_pixel(canvas, canvas_w, x, y, argb);
        }
    }
    // Small downward-pointing nub connecting the bubble to the cat.
    let nub_x = canvas_w as i32 / 2;
    let nub_y0 = box_y0 + box_h;
    for (i, y) in (nub_y0..nub_y0 + 2).enumerate() {
        for x in (nub_x - 1 + i as i32)..=(nub_x + 1 - i as i32) {
            put_pixel(canvas, canvas_w, x, y, 0xFF00_0000);
        }
    }

    draw_text(canvas, canvas_w, text, box_x0 + PAD, box_y0 + PAD, SCALE);
}
