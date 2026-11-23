// SPDX-License-Identifier: AGPL-3.0-or-later
//! The small part of gettext this project uses: `.po` entries (msgid, msgid_plural, msgstr,
//! msgstr[n], the fuzzy flag; no msgctxt) and C-like `Plural-Forms` expressions. Shared by
//! build.rs and the tests.

#[derive(Debug, Default)]
pub struct Entry {
    pub msgid: String,
    pub msgid_plural: Option<String>,
    pub msgstr: Vec<String>,
    pub fuzzy: bool,
}

#[derive(Debug, Default)]
pub struct File {
    pub headers: Vec<(String, String)>,
    pub entries: Vec<Entry>,
}

impl File {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn unquote(s: &str, line: usize) -> Result<String, String> {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(format!("line {line}: expected a quoted string"))?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            other => return Err(format!("line {line}: unknown escape \\{other:?}")),
        }
    }
    Ok(out)
}

pub fn parse(text: &str) -> Result<File, String> {
    #[derive(Clone, Copy)]
    enum Field {
        None,
        Id,
        Plural,
        Str(usize),
    }
    let mut file = File::default();
    let mut cur = Entry::default();
    let mut field = Field::None;
    let mut started = false;
    let flush = |cur: &mut Entry, started: &mut bool, file: &mut File| {
        if *started {
            let e = std::mem::take(cur);
            if e.msgid.is_empty() {
                for l in e.msgstr.first().map(String::as_str).unwrap_or("").lines() {
                    if let Some((k, v)) = l.split_once(':') {
                        file.headers
                            .push((k.trim().to_owned(), v.trim().to_owned()));
                    }
                }
            } else {
                file.entries.push(e);
            }
        }
        *started = false;
    };
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let l = raw.trim();
        if l.is_empty() {
            flush(&mut cur, &mut started, &mut file);
            field = Field::None;
            continue;
        }
        if let Some(flags) = l.strip_prefix("#,") {
            if started && !matches!(field, Field::None) {
                flush(&mut cur, &mut started, &mut file);
            }
            cur.fuzzy |= flags.split(',').any(|f| f.trim() == "fuzzy");
            continue;
        }
        if l.starts_with('#') {
            // Comments, references, obsolete (#~) entries.
            continue;
        }
        if let Some(rest) = l.strip_prefix("msgctxt") {
            return Err(format!("line {n}: msgctxt is not used here ({rest})"));
        }
        if let Some(rest) = l.strip_prefix("msgid_plural") {
            cur.msgid_plural = Some(unquote(rest, n)?);
            field = Field::Plural;
        } else if let Some(rest) = l.strip_prefix("msgid") {
            if started && !matches!(field, Field::Id) {
                let fuzzy = cur.fuzzy && matches!(field, Field::None);
                flush(&mut cur, &mut started, &mut file);
                cur.fuzzy = fuzzy;
            }
            started = true;
            cur.msgid = unquote(rest, n)?;
            field = Field::Id;
        } else if let Some(rest) = l.strip_prefix("msgstr[") {
            let (idx, s) = rest
                .split_once(']')
                .ok_or(format!("line {n}: msgstr[ without ]"))?;
            let idx: usize = idx.parse().map_err(|_| format!("line {n}: bad index"))?;
            if cur.msgstr.len() <= idx {
                cur.msgstr.resize(idx + 1, String::new());
            }
            cur.msgstr[idx] = unquote(s, n)?;
            field = Field::Str(idx);
        } else if let Some(rest) = l.strip_prefix("msgstr") {
            cur.msgstr = vec![unquote(rest, n)?];
            field = Field::Str(0);
        } else if l.starts_with('"') {
            let more = unquote(l, n)?;
            match field {
                Field::Id => cur.msgid.push_str(&more),
                Field::Plural => cur.msgid_plural.get_or_insert_default().push_str(&more),
                Field::Str(i) => cur.msgstr[i].push_str(&more),
                Field::None => return Err(format!("line {n}: string outside an entry")),
            }
        } else {
            return Err(format!("line {n}: unexpected '{l}'"));
        }
    }
    flush(&mut cur, &mut started, &mut file);
    Ok(file)
}

/// Evaluates a gettext plural expression (`n != 1`, `n > 1`,
/// `(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)`…) for `n`.
pub fn plural_index(expr: &str, n: u64) -> Result<usize, String> {
    let tokens = lex(expr)?;
    let mut p = Parser {
        t: &tokens,
        i: 0,
        n,
    };
    let v = p.ternary()?;
    if p.i != tokens.len() {
        return Err(format!("unexpected '{}' in '{expr}'", tokens[p.i]));
    }
    Ok(v as usize)
}

fn lex(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let st = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push(b[st..i].iter().collect());
        } else if c == 'n' {
            out.push("n".into());
            i += 1;
        } else {
            let two: String = b[i..(i + 2).min(b.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||"].contains(&two.as_str()) {
                out.push(two);
                i += 2;
            } else if "<>%?:()!".contains(c) {
                out.push(c.to_string());
                i += 1;
            } else {
                return Err(format!("unexpected '{c}' in plural expression '{s}'"));
            }
        }
    }
    Ok(out)
}

struct Parser<'a> {
    t: &'a [String],
    i: usize,
    n: u64,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&str> {
        self.t.get(self.i).map(String::as_str)
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.peek() == Some(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn ternary(&mut self) -> Result<u64, String> {
        let c = self.or()?;
        if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err("expected ':'".into());
            }
            let b = self.ternary()?;
            return Ok(if c != 0 { a } else { b });
        }
        Ok(c)
    }
    fn or(&mut self) -> Result<u64, String> {
        let mut v = self.and()?;
        while self.eat("||") {
            let r = self.and()?;
            v = u64::from(v != 0 || r != 0);
        }
        Ok(v)
    }
    fn and(&mut self) -> Result<u64, String> {
        let mut v = self.cmp()?;
        while self.eat("&&") {
            let r = self.cmp()?;
            v = u64::from(v != 0 && r != 0);
        }
        Ok(v)
    }
    fn cmp(&mut self) -> Result<u64, String> {
        let mut v = self.modulo()?;
        loop {
            let op = match self.peek() {
                Some(o @ ("==" | "!=" | "<" | ">" | "<=" | ">=")) => o.to_owned(),
                _ => return Ok(v),
            };
            self.i += 1;
            let r = self.modulo()?;
            v = u64::from(match op.as_str() {
                "==" => v == r,
                "!=" => v != r,
                "<" => v < r,
                ">" => v > r,
                "<=" => v <= r,
                _ => v >= r,
            });
        }
    }
    fn modulo(&mut self) -> Result<u64, String> {
        let mut v = self.unary()?;
        while self.eat("%") {
            let r = self.unary()?;
            v = v.checked_rem(r).ok_or("% 0")?;
        }
        Ok(v)
    }
    fn unary(&mut self) -> Result<u64, String> {
        if self.eat("!") {
            return Ok(u64::from(self.unary()? == 0));
        }
        if self.eat("(") {
            let v = self.ternary()?;
            if !self.eat(")") {
                return Err("expected ')'".into());
            }
            return Ok(v);
        }
        match self.peek() {
            Some("n") => {
                self.i += 1;
                Ok(self.n)
            }
            Some(d) if d.chars().all(|c| c.is_ascii_digit()) => {
                let v = d.parse().map_err(|_| "number too big")?;
                self.i += 1;
                Ok(v)
            }
            other => Err(format!("unexpected {other:?}")),
        }
    }
}
