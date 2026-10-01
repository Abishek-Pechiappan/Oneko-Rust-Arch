// Just enough JSON to read Hyprland's IPC replies.
//
// The `j/` IPC requests answer in JSON, and pulling in serde for two small
// replies would double the dependency list. This is a plain recursive-descent
// parser into a `Value` tree - not fast, not streaming, but the replies are a
// few KB at most and are read twice a second. Scraping fields out with string
// searches would be shorter and would break the first time Hyprland reorders
// or reformats a reply; parsing the structure properly doesn't.

#[derive(Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// The value under `key`, if this is an object that has it.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Num(n) if n.fract() == 0.0 => Some(*n as i64),
            _ => None,
        }
    }
}

/// Parses a complete JSON document, or `None` if it's malformed or has
/// anything but whitespace after the value.
pub fn parse(text: &str) -> Option<Value> {
    let mut p = Parser { b: text.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.b.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn literal(&mut self, word: &str, v: Value) -> Option<Value> {
        self.b[self.i..].starts_with(word.as_bytes()).then(|| {
            self.i += word.len();
            v
        })
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Value::Str),
            b't' => self.literal("true", Value::Bool(true)),
            b'f' => self.literal("false", Value::Bool(false)),
            b'n' => self.literal("null", Value::Null),
            _ => self.number(),
        }
    }

    fn object(&mut self) -> Option<Value> {
        self.eat(b'{')?;
        let mut fields = Vec::new();
        if self.eat(b'}').is_some() {
            return Some(Value::Obj(fields));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.eat(b':')?;
            fields.push((key, self.value()?));
            if self.eat(b',').is_none() {
                self.eat(b'}')?;
                return Some(Value::Obj(fields));
            }
        }
    }

    fn array(&mut self) -> Option<Value> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        if self.eat(b']').is_some() {
            return Some(Value::Arr(items));
        }
        loop {
            items.push(self.value()?);
            if self.eat(b',').is_none() {
                self.eat(b']')?;
                return Some(Value::Arr(items));
            }
        }
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        while self
            .b
            .get(self.i)
            .is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).ok()?;
        text.parse().ok().map(Value::Num)
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        // Collected as bytes so multi-byte UTF-8 (window titles are full of
        // it) passes through untouched; only escapes need decoding.
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => return None,
                    };
                    let mut buf = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(c),
            }
        }
    }

    /// The `XXXX` after `\u`, including a following low surrogate when the
    /// first one is high. A lone or mismatched surrogate becomes U+FFFD rather
    /// than failing the whole reply over one odd window title.
    fn unicode_escape(&mut self) -> Option<char> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) && self.b[self.i..].starts_with(b"\\u") {
            self.i += 2;
            let lo = self.hex4()?;
            let code = 0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
            return Some(char::from_u32(code).unwrap_or('\u{FFFD}'));
        }
        Some(char::from_u32(hi).unwrap_or('\u{FFFD}'))
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(digits, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_structures() {
        let v = parse(r#"[{"id": -2, "name": "x", "ok": true, "s": {"id": 0}, "l": [], "n": null}]"#)
            .unwrap();
        let first = &v.as_array().unwrap()[0];
        assert_eq!(first.get("id").and_then(Value::as_i64), Some(-2));
        assert_eq!(first.get("name").and_then(Value::as_str), Some("x"));
        assert_eq!(first.get("ok"), Some(&Value::Bool(true)));
        assert_eq!(first.get("s").and_then(|s| s.get("id")).and_then(Value::as_i64), Some(0));
        assert_eq!(first.get("l").and_then(Value::as_array).map(<[_]>::len), Some(0));
        assert_eq!(first.get("n"), Some(&Value::Null));
        assert_eq!(first.get("missing"), None);
    }

    #[test]
    fn decodes_escapes_and_keeps_raw_utf8() {
        let v = parse(r#""a\"b\\c\n\u00e9\ud83d\ude00 ◐""#).unwrap();
        assert_eq!(v.as_str(), Some("a\"b\\c\né😀 ◐"));
    }

    #[test]
    fn reads_hyprland_number_formats() {
        let v = parse(r#"{"refreshRate": 60.03000, "x": -2560, "big": 1e3}"#).unwrap();
        assert_eq!(v.get("x").and_then(Value::as_i64), Some(-2560));
        assert_eq!(v.get("big").and_then(Value::as_i64), Some(1000));
        // Fractional numbers aren't integers.
        assert_eq!(v.get("refreshRate").and_then(Value::as_i64), None);
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["", "{", "[1,]", r#"{"a" 1}"#, r#""unterminated"#, "tru", "[1] x", r#""\q""#] {
            assert_eq!(parse(bad), None, "accepted {bad:?}");
        }
    }
}
