//! LED rules: `[ctx.leds] LED = "condition"` (PROFILES.md).
//!
//! condition := or ; or := and ("or" and)* ; and := unary ("and" unary)*
//! unary := "not" unary | "(" or ")" | cmp ; cmp := operand [("=="|"!="|">"|">="|"<"|"<=") operand]
//! operands: layer.<name>, sheet (the cheat sheet is open), midi.cc.<n> / midi.note.<n> / midi.ch<c>.cc.<n> (last value sent or received), true, false, numbers, words (pick, red, masking) or 'quoted text'.
//! <prefix>.<name> of an integration (ext.rs; one this build doesn't have counts as missing).
//! A bare operand is true when it is true / non-zero / non-empty and not "none".

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Lit(Val),
    Var(String),
    Not(Box<Cond>),
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
    Cmp(Box<Cond>, Op, Box<Cond>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op { Eq, Ne, Gt, Ge, Lt, Le }

#[derive(Debug, Clone, PartialEq)]
pub enum Val { Num(f64), Str(String), Bool(bool), Null }

impl Val {
    fn truthy(&self) -> bool {
        match self {
            Val::Bool(b) => *b,
            Val::Num(n) => *n != 0.0,
            Val::Str(s) => !s.is_empty() && !s.eq_ignore_ascii_case("none"),
            Val::Null => false,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            Val::Str(s) => s.parse().ok(),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Val::Null => None,
        }
    }
}

/// What a rule can look at.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// values of integrations: "prefix.name"
    pub ext: HashMap<String, Val>,
    pub layers: Vec<String>,
    /// the cheat sheet is open
    pub sheet: bool,
    /// MIDI values sent or received: "ch.cc.n" / "ch.note.n"
    pub midi: HashMap<String, f64>,
}

fn tokens(s: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() { i += 1; continue; }
        if ch == '(' || ch == ')' { out.push(ch.to_string()); i += 1; continue; }
        if "=!<>".contains(ch) {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["==", "!=", ">=", "<="].contains(&two.as_str()) { out.push(two); i += 2; continue; }
            if ch == '>' || ch == '<' { out.push(ch.to_string()); i += 1; continue; }
            return Err(format!("unexpected {ch:?} (comparisons are == != > >= < <=)"));
        }
        if ch == '\'' || ch == '"' {
            let end = c[i + 1..].iter().position(|&x| x == ch).ok_or("unterminated quote")?;
            out.push(format!("'{}", c[i + 1..i + 1 + end].iter().collect::<String>()));
            i += end + 2;
            continue;
        }
        let start = i;
        while i < c.len() && !c[i].is_whitespace() && !"()=!<>'\"".contains(c[i]) { i += 1; }
        out.push(c[start..i].iter().collect());
    }
    Ok(out)
}

struct P { t: Vec<String>, i: usize }

impl P {
    fn peek(&self) -> Option<&str> { self.t.get(self.i).map(|s| s.as_str()) }
    fn next(&mut self) -> Option<String> { let v = self.t.get(self.i).cloned(); self.i += 1; v }
    fn or(&mut self) -> Result<Cond, String> {
        let mut a = self.and()?;
        while self.peek().map_or(false, |x| x.eq_ignore_ascii_case("or")) { self.i += 1; a = Cond::Or(Box::new(a), Box::new(self.and()?)); }
        Ok(a)
    }
    fn and(&mut self) -> Result<Cond, String> {
        let mut a = self.unary()?;
        while self.peek().map_or(false, |x| x.eq_ignore_ascii_case("and")) { self.i += 1; a = Cond::And(Box::new(a), Box::new(self.unary()?)); }
        Ok(a)
    }
    fn unary(&mut self) -> Result<Cond, String> {
        match self.peek() {
            Some(x) if x.eq_ignore_ascii_case("not") => { self.i += 1; Ok(Cond::Not(Box::new(self.unary()?))) }
            Some("(") => {
                self.i += 1;
                let e = self.or()?;
                if self.next().as_deref() != Some(")") { return Err("missing )".into()); }
                Ok(e)
            }
            _ => self.cmp(),
        }
    }
    fn cmp(&mut self) -> Result<Cond, String> {
        let a = self.operand()?;
        let op = match self.peek() {
            Some("==") => Op::Eq, Some("!=") => Op::Ne, Some(">") => Op::Gt, Some(">=") => Op::Ge,
            Some("<") => Op::Lt, Some("<=") => Op::Le, _ => return Ok(a),
        };
        self.i += 1;
        let b = self.operand()?;
        Ok(Cond::Cmp(Box::new(a), op, Box::new(b)))
    }
    fn operand(&mut self) -> Result<Cond, String> {
        let t = self.next().ok_or("condition ends too early")?;
        if let Some(q) = t.strip_prefix('\'') { return Ok(Cond::Lit(Val::Str(q.to_string()))); }
        if ["(", ")", "==", "!=", ">", ">=", "<", "<="].contains(&t.as_str()) || ["and", "or", "not"].contains(&t.to_lowercase().as_str()) {
            return Err(format!("expected a value, found {t:?}"));
        }
        if let Ok(n) = t.parse::<f64>() { return Ok(Cond::Lit(Val::Num(n))); }
        match t.to_lowercase().as_str() {
            "true" => return Ok(Cond::Lit(Val::Bool(true))),
            "false" => return Ok(Cond::Lit(Val::Bool(false))),
            "sheet" => return Ok(Cond::Var("sheet".into())),
            _ => {}
        }
        if let Some(l) = t.strip_prefix("layer.") {
            if l.is_empty() { return Err("layer. needs a layer name".into()); }
            return Ok(Cond::Var(format!("layer.{}", l.to_lowercase())));
        }
        if let Some(m) = t.to_lowercase().strip_prefix("midi.") {
            // midi.cc.20, midi.note.60, midi.ch2.cc.20
            let parts: Vec<&str> = m.split('.').collect();
            let (ch, rest) = match parts.first() {
                Some(c) if c.starts_with("ch") => (c[2..].parse::<u8>().ok().filter(|c| (1..=16).contains(c)), &parts[1..]),
                _ => (Some(1), &parts[..]),
            };
            let ok = ch.is_some() && rest.len() == 2 && (rest[0] == "cc" || rest[0] == "note")
                && rest[1].parse::<u8>().map_or(false, |n| n <= 127);
            if !ok { return Err(format!("{t:?}: use midi.cc.<0-127>, midi.note.<0-127> or midi.ch<1-16>.cc.<n>")); }
            return Ok(Cond::Var(format!("midi.{}.{}.{}", ch.unwrap(), rest[0], rest[1])));
        }
        if let Some((p, n)) = t.split_once('.') {
            if p.is_empty() || n.is_empty() || !p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!("unknown name {t:?} (use layer.<name>, midi.cc.<n> or <integration>.<name>)"));
            }
            return Ok(Cond::Var(t));
        }
        Ok(Cond::Lit(Val::Str(t)))
    }
}

pub fn parse(s: &str) -> Result<Cond, String> {
    let mut p = P { t: tokens(s)?, i: 0 };
    if p.t.is_empty() { return Err("empty condition".into()); }
    let e = p.or()?;
    if p.i < p.t.len() { return Err(format!("unexpected {:?}", p.t[p.i])); }
    Ok(e)
}

fn value(c: &Cond, f: &Facts) -> Val {
    match c {
        Cond::Lit(v) => v.clone(),
        Cond::Var(n) => {
            if n == "sheet" { return Val::Bool(f.sheet); }
            if let Some(l) = n.strip_prefix("layer.") { return Val::Bool(f.layers.iter().any(|x| x == l)); }
            if let Some(k) = n.strip_prefix("midi.") { return f.midi.get(k).map(|v| Val::Num(*v)).unwrap_or(Val::Null); }
            f.ext.get(n).cloned().unwrap_or(Val::Null)
        }
        other => Val::Bool(eval(other, f)),
    }
}

pub fn eval(c: &Cond, f: &Facts) -> bool {
    match c {
        Cond::Not(a) => !eval(a, f),
        Cond::And(a, b) => eval(a, f) && eval(b, f),
        Cond::Or(a, b) => eval(a, f) || eval(b, f),
        Cond::Cmp(a, op, b) => {
            let (x, y) = (value(a, f), value(b, f));
            if matches!(op, Op::Eq | Op::Ne) {
                let eq = match (x.num(), y.num(), &x, &y) {
                    (Some(p), Some(q), _, _) if !matches!((&x, &y), (Val::Str(_), Val::Str(_))) => (p - q).abs() < 1e-9,
                    (_, _, Val::Str(s), Val::Str(t)) => s.eq_ignore_ascii_case(t),
                    (_, _, Val::Null, Val::Str(t)) | (_, _, Val::Str(t), Val::Null) => t.eq_ignore_ascii_case("none"),
                    _ => x == y,
                };
                return if *op == Op::Eq { eq } else { !eq };
            }
            match (x.num(), y.num()) {
                (Some(p), Some(q)) => match op { Op::Gt => p > q, Op::Ge => p >= q, Op::Lt => p < q, Op::Le => p <= q, _ => false },
                _ => false,
            }
        }
        other => value(other, f).truthy(),
    }
}

fn vars(c: &Cond, out: &mut Vec<String>) {
    match c {
        Cond::Var(n) => if !out.contains(n) { out.push(n.clone()); },
        Cond::Not(a) => vars(a, out),
        Cond::And(a, b) | Cond::Or(a, b) | Cond::Cmp(a, _, b) => { vars(a, out); vars(b, out); }
        Cond::Lit(_) => {}
    }
}

/// What a condition reads of the integration with this prefix (without the prefix).
pub fn watched(c: &Cond, prefix: &str, out: &mut Vec<String>) {
    let mut v = vec![];
    vars(c, &mut v);
    for n in v {
        if let Some(("", k)) = n.strip_prefix(prefix).map(|r| if let Some(k) = r.strip_prefix('.') { ("", k) } else { ("x", r) }) {
            if !out.iter().any(|x| x == k) { out.push(k.to_string()); }
        }
    }
}

/// The integration prefixes a condition uses.
pub fn prefixes(c: &Cond) -> Vec<String> {
    let mut v = vec![];
    vars(c, &mut v);
    let mut out: Vec<String> = v.iter().filter_map(|n| n.split_once('.')).map(|x| x.0.to_string()).filter(|p| !crate::ext::CORE_PREFIXES.contains(&p.as_str())).collect();
    out.sort();
    out.dedup();
    out
}

/// What is so right now.
pub fn facts(layers: &[String], sheet: bool) -> Facts {
    Facts { ext: crate::ext::facts(), layers: layers.to_vec(), sheet, midi: crate::midi::values() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> Facts {
        let mut ext = HashMap::new();
        ext.insert("ph.flag".into(), Val::Str("reject".into()));
        ext.insert("ph.label".into(), Val::Str("red".into()));
        ext.insert("ph.rating".into(), Val::Num(3.0));
        ext.insert("ph.tool".into(), Val::Str("masking".into()));
        ext.insert("ph.Exposure2012".into(), Val::Num(0.5));
        let mut midi = HashMap::new();
        midi.insert("1.cc.20".to_string(), 100.0);
        Facts { ext, layers: vec!["fine".into()], sheet: true, midi }
    }
    fn t(s: &str) -> bool { eval(&parse(s).unwrap_or_else(|e| panic!("{s}: {e}")), &f()) }

    #[test]
    fn conditions() {
        assert!(t("ph.flag == reject"));
        assert!(!t("ph.flag == pick"));
        assert!(t("ph.flag != pick"));
        assert!(t("ph.label == RED"));
        assert!(t("ph.label == 'red'"));
        assert!(t("ph.rating >= 3") && !t("ph.rating > 3"));
        assert!(t("ph.Exposure2012 > 0") && t("ph.Exposure2012 != 0"));
        assert!(t("layer.fine") && !t("layer.more"));
        assert!(t("not layer.more and (ph.tool == masking or ph.flag == pick)"));
        assert!(t("ph.label"), "bare value: set and not none");
        assert!(!t("ph.cg_view"), "unknown/absent value is false");
        assert!(t("ph.cg_view == none"), "absent equals none");
        assert!(t("true") && !t("false"));
        assert!(t("sheet") && t("sheet and layer.fine") && !t("not sheet"));
    }

    #[test]
    fn midi_operands() {
        assert!(eval(&parse("midi.cc.20 > 50").unwrap(), &f()));
        assert!(!eval(&parse("midi.note.60").unwrap(), &f()), "never seen = off");
        assert!(parse("midi.ch17.cc.1").is_err() && parse("midi.cc.300").is_err() && parse("midi.pb").is_err());
        assert_eq!(parse("midi.ch2.note.60").unwrap(), Cond::Var("midi.2.note.60".into()));
    }

    #[test]
    fn errors_and_watch() {
        for bad in ["", "ph.flag ==", "ph.flag = pick", "(ph.flag", "Foo.bar", "ph.", "ph.flag == pick extra"] {
            assert!(parse(bad).is_err(), "{bad:?} should fail");
        }
        let mut w = vec![];
        let c = parse("ph.Exposure2012 > 0 or ph.flag == pick or other.x and layer.fine").unwrap();
        watched(&c, "ph", &mut w);
        assert_eq!(w, vec!["Exposure2012", "flag"]);
        assert_eq!(prefixes(&c), vec!["other", "ph"]);
    }
}
