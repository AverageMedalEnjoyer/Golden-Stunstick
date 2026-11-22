//! Valve KeyValues (text) parser, used for gameinfo.txt, VMT materials and
//! the embedded keyvalue blocks inside MDL files.
//!
//! The parser is forgiving: it accepts quoted and unquoted tokens, `//`
//! comments, `[$CONDITION]` suffixes (ignored) and `#include`/`#base`
//! directives (kept as plain keys so callers may resolve them).

#[derive(Debug, Clone, PartialEq)]
pub enum KvValue {
    Str(String),
    Block(Vec<Kv>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Kv {
    pub key: String,
    pub value: KvValue,
}

impl Kv {
    pub fn children(&self) -> &[Kv] {
        match &self.value {
            KvValue::Block(c) => c,
            KvValue::Str(_) => &[],
        }
    }

    pub fn str(&self) -> Option<&str> {
        match &self.value {
            KvValue::Str(s) => Some(s),
            KvValue::Block(_) => None,
        }
    }

    /// Case-insensitive lookup of the first child with this key.
    pub fn get(&self, key: &str) -> Option<&Kv> {
        self.children().iter().find(|c| c.key.eq_ignore_ascii_case(key))
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|k| k.str())
    }

    /// Lookup through a path like `["FileSystem", "SearchPaths"]`.
    pub fn path(&self, keys: &[&str]) -> Option<&Kv> {
        let mut cur = self;
        for k in keys {
            cur = cur.get(k)?;
        }
        Some(cur)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
    Cond,
}

fn tokenize(text: &str) -> Vec<(Tok, usize)> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
        } else if c == b'{' {
            out.push((Tok::Open, line));
            i += 1;
        } else if c == b'}' {
            out.push((Tok::Close, line));
            i += 1;
        } else if c == b'[' {
            // conditional like [$WIN32] - skip
            while i < b.len() && b[i] != b']' && b[i] != b'\n' {
                i += 1;
            }
            i += 1;
            out.push((Tok::Cond, line));
        } else if c == b'"' {
            i += 1;
            let mut s = Vec::new();
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'\\') {
                    // Valve's KV only escapes when enabled; accept \" defensively
                    if b[i + 1] == b'"' {
                        s.push(b'"');
                        i += 2;
                        continue;
                    }
                }
                if b[i] == b'\n' {
                    line += 1;
                }
                s.push(b[i]);
                i += 1;
            }
            i += 1;
            out.push((Tok::Str(String::from_utf8_lossy(&s).into_owned()), line));
        } else {
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'{' && b[i] != b'}' && b[i] != b'"' {
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
                    break;
                }
                i += 1;
            }
            out.push((Tok::Str(String::from_utf8_lossy(&b[start..i]).into_owned()), line));
        }
    }
    out
}

/// Parse a KeyValues document. Returns the list of top level entries.
pub fn parse(text: &str) -> Vec<Kv> {
    let text = text.trim_start_matches('\u{feff}');
    let toks: Vec<Tok> = tokenize(text).into_iter().map(|t| t.0).filter(|t| *t != Tok::Cond).collect();
    let mut pos = 0;
    parse_block(&toks, &mut pos, true)
}

fn parse_block(toks: &[Tok], pos: &mut usize, top: bool) -> Vec<Kv> {
    let mut out = Vec::new();
    while *pos < toks.len() {
        match &toks[*pos] {
            Tok::Close => {
                *pos += 1;
                if !top {
                    return out;
                }
            }
            Tok::Open => {
                // anonymous block
                *pos += 1;
                let children = parse_block(toks, pos, false);
                out.push(Kv { key: String::new(), value: KvValue::Block(children) });
            }
            Tok::Str(k) => {
                let key = k.clone();
                *pos += 1;
                match toks.get(*pos) {
                    Some(Tok::Open) => {
                        *pos += 1;
                        let children = parse_block(toks, pos, false);
                        out.push(Kv { key, value: KvValue::Block(children) });
                    }
                    Some(Tok::Str(v)) => {
                        out.push(Kv { key, value: KvValue::Str(v.clone()) });
                        *pos += 1;
                    }
                    _ => {
                        out.push(Kv { key, value: KvValue::Str(String::new()) });
                    }
                }
            }
            Tok::Cond => *pos += 1,
        }
    }
    out
}

/// Serialize entries back to KeyValues text.
pub fn write(entries: &[Kv], indent: usize) -> String {
    let mut s = String::new();
    let pad = "\t".repeat(indent);
    for e in entries {
        match &e.value {
            KvValue::Str(v) => s.push_str(&format!("{pad}\"{}\" \"{}\"\n", e.key, v)),
            KvValue::Block(c) => {
                s.push_str(&format!("{pad}\"{}\"\n{pad}{{\n", e.key));
                s.push_str(&write(c, indent + 1));
                s.push_str(&format!("{pad}}}\n"));
            }
        }
    }
    s
}

/// Parse and return the first root entry.
pub fn parse_root(text: &str) -> Option<Kv> {
    parse(text).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basic() {
        let kv = parse_root("\"GameInfo\" { game \"X\" FileSystem { SearchPaths { game |gameinfo_path|. // c\n game+mod \"a b\" } } }").unwrap();
        assert_eq!(kv.get_str("game"), Some("X"));
        let sp = kv.path(&["FileSystem", "SearchPaths"]).unwrap();
        assert_eq!(sp.children().len(), 2);
        assert_eq!(sp.children()[1].str(), Some("a b"));
    }
}
