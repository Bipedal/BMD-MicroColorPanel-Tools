//! OSC and OSCQuery: the built-in integration, and the worked example for INTEGRATIONS.md.
//!
//! Values go out as OSC over UDP to `osc_target`. If the other program also speaks OSCQuery
//! (`oscquery = "http://host:port"`), its list of parameters with ranges, units and current values
//! is read over HTTP and changes are followed over its WebSocket. Without OSCQuery, values the
//! program sends back over UDP are used, and ranges come from the binding (0..1 if it says nothing).
//!
//! Layout: the actions (`Act`) only describe a change; `Osc::send` knows the current values and
//! turns the change into an OSC message. Two threads keep the values up to date.

use crate::ext::{Act, Integration, Io, Parsed, Role, SettingRef};
use crate::leds::Val;
use crate::profiles::{not_allowed, num, req, text, Kind, Settings};
use crate::util::log;
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use toml::{Table, Value};
use tungstenite::Message;

// ------------------------------------------------------------------ OSC packets

#[derive(Debug, Clone, PartialEq)]
pub enum Arg { F(f32), I(i32), S(String), B(bool) }

impl Arg {
    fn val(&self) -> Val {
        match self { Arg::F(x) => Val::Num(*x as f64), Arg::I(x) => Val::Num(*x as f64), Arg::S(s) => Val::Str(s.clone()), Arg::B(b) => Val::Bool(*b) }
    }
}

fn pad(v: &mut Vec<u8>, s: &str) {
    v.extend(s.bytes().filter(|b| *b != 0));
    v.push(0);
    while v.len() % 4 != 0 { v.push(0); }
}

pub fn encode(address: &str, args: &[Arg]) -> Vec<u8> {
    let mut v = vec![];
    pad(&mut v, address);
    let tags: String = args.iter().map(|a| match a { Arg::F(_) => 'f', Arg::I(_) => 'i', Arg::S(_) => 's', Arg::B(true) => 'T', Arg::B(false) => 'F' }).collect();
    pad(&mut v, &format!(",{tags}"));
    for a in args {
        match a {
            Arg::F(x) => v.extend(x.to_be_bytes()),
            Arg::I(x) => v.extend(x.to_be_bytes()),
            Arg::S(s) => pad(&mut v, s),
            Arg::B(_) => {}
        }
    }
    v
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if b.len() < n { return None; }
    let (a, z) = b.split_at(n);
    *b = z;
    Some(a)
}
fn take_str(b: &mut &[u8]) -> Option<String> {
    let n = b.iter().position(|c| *c == 0)?;
    let s = String::from_utf8_lossy(&b[..n]).into_owned();
    // a last string without its padding is accepted
    let whole = ((n + 4) / 4 * 4).min(b.len());
    take(b, whole)?;
    Some(s)
}
fn take4(b: &mut &[u8]) -> Option<[u8; 4]> { take(b, 4)?.try_into().ok() }
fn take8(b: &mut &[u8]) -> Option<[u8; 8]> { take(b, 8)?.try_into().ok() }

/// The messages of a packet (a message, or a bundle of them). Anything broken ends the reading.
pub fn decode(p: &[u8]) -> Vec<(String, Vec<Arg>)> {
    let mut out = vec![];
    decode_into(p, &mut out, 0);
    out
}
fn decode_into(p: &[u8], out: &mut Vec<(String, Vec<Arg>)>, depth: u32) {
    if depth > 8 { return; }
    if p.starts_with(b"#bundle\0") {
        let Some(mut b) = p.get(16..) else { return };
        while let Some(n) = take4(&mut b) {
            let Some(m) = take(&mut b, u32::from_be_bytes(n) as usize) else { return };
            decode_into(m, out, depth + 1);
        }
        return;
    }
    let mut b = p;
    let Some(addr) = take_str(&mut b).filter(|a| a.starts_with('/')) else { return };
    let tags = take_str(&mut b).unwrap_or_default();
    let mut args = vec![];
    for t in tags.chars().skip(1) {
        let a = match t {
            'f' => take4(&mut b).map(|x| Arg::F(f32::from_be_bytes(x))),
            'i' | 'c' | 'r' | 'm' => take4(&mut b).map(|x| Arg::I(i32::from_be_bytes(x))),
            'd' => take8(&mut b).map(|x| Arg::F(f64::from_be_bytes(x) as f32)),
            'h' => take8(&mut b).map(|x| Arg::I(i64::from_be_bytes(x) as i32)),
            't' => { if take8(&mut b).is_none() { break; } continue; }
            's' | 'S' => take_str(&mut b).map(Arg::S),
            'T' => Some(Arg::B(true)),
            'F' => Some(Arg::B(false)),
            'N' | 'I' | '[' | ']' => continue,
            _ => None, // blobs and whatever else: nothing after it can be placed
        };
        let Some(a) = a else { break };
        args.push(a);
    }
    out.push((addr, args));
}

/// Plain addresses only: no patterns, nothing a message couldn't carry.
fn good_address(a: &str) -> bool {
    a.len() > 1 && a.starts_with('/') && a.chars().all(|c| c.is_ascii_graphic() && !"#*,?[]{}\"'".contains(c))
}

// ------------------------------------------------------------------ what is known about the other side

/// One parameter of an OSCQuery tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node { pub desc: String, pub ty: String, pub min: Option<f64>, pub max: Option<f64>, pub unit: String, pub write: bool }

/// Flatten an OSCQuery tree: path -> parameter, and the values it came with.
pub fn read_tree(root: &J) -> (BTreeMap<String, Node>, HashMap<String, Arg>) {
    let (mut nodes, mut vals) = (BTreeMap::new(), HashMap::new());
    walk(root, "", &mut nodes, &mut vals, 0);
    (nodes, vals)
}
fn walk(n: &J, path: &str, nodes: &mut BTreeMap<String, Node>, vals: &mut HashMap<String, Arg>, depth: u32) {
    if depth > 60 || nodes.len() >= 20000 { return; }
    let full = n["FULL_PATH"].as_str().unwrap_or(path).to_string();
    if let Some(ty) = n["TYPE"].as_str() {
        if good_address(&full) {
            let access = n["ACCESS"].as_u64().unwrap_or(3);
            let r = &n["RANGE"][0];
            let unit = n["UNIT"].as_str().or(n["UNIT"][0].as_str()).unwrap_or("");
            nodes.insert(full.clone(), Node {
                desc: n["DESCRIPTION"].as_str().unwrap_or("").to_string(), ty: ty.to_string(),
                min: r["MIN"].as_f64(), max: r["MAX"].as_f64(), unit: unit.to_string(), write: access & 2 != 0,
            });
            let v = if n["VALUE"].is_array() { &n["VALUE"][0] } else { &n["VALUE"] };
            if let Some(a) = arg_of(v, ty) { vals.insert(full.clone(), a); }
        }
    }
    if let Some(c) = n["CONTENTS"].as_object() {
        for (k, x) in c { walk(x, &format!("{}/{k}", if full == "/" { "" } else { &full }), nodes, vals, depth + 1); }
    }
}
fn arg_of(v: &J, ty: &str) -> Option<Arg> {
    Some(match v {
        J::Bool(b) => Arg::B(*b),
        J::Number(n) if ty.starts_with('i') => Arg::I(n.as_f64()? as i32),
        J::Number(n) => Arg::F(n.as_f64()? as f32),
        J::String(s) => Arg::S(s.clone()),
        _ if ty.starts_with('T') => Arg::B(true),
        _ if ty.starts_with('F') => Arg::B(false),
        _ => return None,
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Cfg { target: String, listen: u16, query: String }

#[derive(Default)]
struct St {
    cfg: Cfg,
    /// where messages go, and through which socket
    out: Option<(Arc<UdpSocket>, SocketAddr)>,
    /// the target the OSCQuery server names, used when `osc_target` is empty
    query_target: Option<SocketAddr>,
    nodes: BTreeMap<String, Node>,
    values: HashMap<String, Arg>,
    /// what we turned a moment ago: its exact value wins over an echo that is still on its way
    ours: HashMap<String, (f64, Instant)>,
    /// paths the OSCQuery server should report
    want: HashSet<String>,
    /// paths a light depends on
    watched: HashSet<String>,
    connected: bool,
    live: bool,
    name: String,
    error: String,
    sent: u64,
    received: u64,
    /// counts changes of `nodes`, for the editor
    rev: u64,
}

const ECHO: Duration = Duration::from_millis(400);

impl St {
    fn number(&self, a: &str) -> Option<f64> {
        if let Some((v, t)) = self.ours.get(a) { if t.elapsed() < ECHO { return Some(*v); } }
        match self.values.get(a)? { Arg::F(x) => Some(*x as f64), Arg::I(x) => Some(*x as f64), Arg::B(b) => Some(*b as u8 as f64), Arg::S(_) => None }
    }
    fn arg(&self, a: &str, v: f64, ty: Option<&str>) -> Arg {
        match ty.or(self.nodes.get(a).map(|n| n.ty.as_str())).and_then(|t| t.chars().next()) {
            Some('i' | 'h') => Arg::I(v.round() as i32),
            Some('T' | 'F' | 'b') => Arg::B(v != 0.0),
            _ => Arg::F(v as f32),
        }
    }
    fn got(&mut self, msgs: Vec<(String, Vec<Arg>)>) {
        for (a, args) in msgs {
            self.received += 1;
            if self.values.len() >= 50000 && !self.values.contains_key(&a) { continue; }
            if let Some(x) = args.into_iter().next() { self.values.insert(a, x); }
        }
    }
    fn put(&mut self, a: &str, args: Vec<Arg>) -> bool {
        let Some((s, to)) = &self.out else { return false };
        match s.send_to(&encode(a, &args), to) {
            Ok(_) => { self.sent += 1; if let Some(x) = args.into_iter().next() { self.values.insert(a.to_string(), x); } true }
            Err(e) => { self.error = format!("can't send to {to}: {e}"); false }
        }
    }
    fn fail(&mut self, e: String) { if self.error != e { log(&format!("osc: {e}")); self.error = e; } }
}

// ------------------------------------------------------------------ actions

#[derive(Debug, Clone)]
struct Range { min: Option<f64>, max: Option<f64>, ty: Option<String>, rel: bool }

/// knob, ring: change a value by so much per turn
#[derive(Debug)]
struct Turn { address: String, per_turn: Option<f64>, r: Range }
impl Act for Turn {
    fn turn(&self, counts: f64, per: f64, io: &mut dyn Io) { io.send(change(&self.address, counts / per, self.per_turn, 1.0, &self.r)); }
}
fn change(a: &str, n: f64, per: Option<f64>, share: f64, r: &Range) -> J {
    json!({"t": "change", "address": a, "n": n, "per": per, "share": share, "min": r.min, "max": r.max, "type": r.ty, "relative": r.rel})
}

/// ball: two values
#[derive(Debug)]
struct Xy { x: Option<String>, y: Option<String>, per: Option<f64>, r: Range }
impl Act for Xy {
    fn ball(&self, dx: f64, dy: f64, io: &mut dyn Io) {
        for (a, d) in [(&self.x, dx), (&self.y, dy)] {
            if let (Some(a), true) = (a, d != 0.0) { io.send(change(a, d, self.per, 0.001, &self.r)); }
        }
    }
}

/// button: a value
#[derive(Debug)]
struct Press { address: String, value: f64, off: f64, mode: String, ty: Option<String> }
impl Act for Press {
    fn press(&self, io: &mut dyn Io) {
        io.send(match self.mode.as_str() {
            "toggle" => json!({"t": "toggle", "address": self.address, "on": self.value, "off": self.off, "type": self.ty}),
            _ => json!({"t": "set", "address": self.address, "value": self.value, "type": self.ty}),
        });
    }
    fn release(&self, io: &mut dyn Io) {
        if self.mode == "hold" { io.send(json!({"t": "set", "address": self.address, "value": self.off, "type": self.ty})); }
    }
}

/// button: any message
#[derive(Debug)]
struct Send { address: String, args: Vec<J> }
impl Act for Send {
    fn press(&self, io: &mut dyn Io) { io.send(json!({"t": "send", "address": self.address, "args": self.args})); }
}

/// `1 0.5 true play 'two words'` -> integers, floats, true/false, text
fn read_args(t: &str) -> Result<Vec<J>, String> {
    let (mut out, mut c) = (vec![], t.chars().peekable());
    loop {
        while c.next_if(|x| x.is_whitespace()).is_some() {}
        let Some(&f) = c.peek() else { return Ok(out) };
        if f == '\'' || f == '"' {
            c.next();
            let mut w = String::new();
            loop { match c.next() { Some(x) if x == f => break, Some(x) => w.push(x), None => return Err("a quote is not closed".into()) } }
            out.push(J::String(w));
            continue;
        }
        let mut w = String::new();
        while let Some(x) = c.next_if(|x| !x.is_whitespace()) { w.push(x); }
        out.push(match (w.as_str(), w.parse::<i32>(), w.parse::<f64>()) {
            ("true", ..) => J::Bool(true),
            ("false", ..) => J::Bool(false),
            (_, Ok(i), _) => json!(i),
            (_, _, Ok(x)) if x.is_finite() => json!(x),
            _ => J::String(w),
        });
    }
}

const ACTIONS: [(&str, &str, &str, &str); 3] = [
    ("osc.value", "knob, ring, button", "address, per_turn, min, max, type, mode | address, value, off, mode, type",
     "OSC value. knob/ring: changes it by per_turn per full turn (default: its whole range) between min and max (default: from OSCQuery, else 0..1); mode relative sends only the change. button: sends value (default 1); mode set (default), hold (off when let go) or toggle. type int, float or bool (default: from OSCQuery, else float)"),
    ("osc.xy", "ball", "x, y, per_unit, min, max, type, mode", "ball changes two OSC values, per_unit per ball step (default: a thousandth of the range)"),
    ("osc.send", "button", "address, args", "sends one OSC message; args as text: 1 0.5 true play 'two words'"),
];

// ------------------------------------------------------------------ the integration

#[derive(Default)]
pub struct Osc { st: Arc<Mutex<St>> }

impl Osc {
    pub fn new() -> Self { Self::default() }

    fn label(&self, a: &str) -> String {
        let st = self.st.lock().unwrap();
        st.nodes.get(a).map(|n| n.desc.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| a.trim_start_matches('/').to_string())
    }
}

fn address(t: &Table, k: &str, p: &str) -> Result<Option<String>, String> {
    match text(t, k, p)? {
        None => Ok(None),
        Some(a) if good_address(a) => Ok(Some(a.to_string())),
        Some(a) => Err(format!("{p}: {a:?} is not an OSC address (it starts with / and has no spaces or patterns)")),
    }
}
fn one_of(t: &Table, k: &str, list: &[&str], p: &str) -> Result<String, String> {
    let v = text(t, k, p)?.unwrap_or(list[0]);
    if list.contains(&v) { Ok(v.into()) } else { Err(format!("{p}: {k} must be {}, not {v:?}", list.join(", "))) }
}
fn some_num(t: &Table, k: &str, p: &str) -> Result<Option<f64>, String> { if t.contains_key(k) { num(t, k, 0.0, p).map(Some) } else { Ok(None) } }
fn kind_of(t: &Table, p: &str) -> Result<Option<String>, String> {
    if !t.contains_key("type") { return Ok(None); }
    one_of(t, "type", &["float", "int", "bool"], p).map(Some)
}
fn range(t: &Table, p: &str) -> Result<Range, String> {
    let r = Range { min: some_num(t, "min", p)?, max: some_num(t, "max", p)?, ty: kind_of(t, p)?, rel: one_of(t, "mode", &["absolute", "relative"], p)? == "relative" };
    if let (Some(a), Some(b)) = (r.min, r.max) { if a >= b { return Err(format!("{p}: min must be below max")); } }
    Ok(r)
}

impl Integration for Osc {
    fn id(&self) -> &'static str { "osc" }
    fn actions(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] { &ACTIONS }

    fn parse(&self, what: &str, kind: Kind, t: &Table, p: &str) -> Result<Parsed, String> {
        let rel = matches!(kind, Kind::Knob | Kind::Ring);
        let must = |k: &str| { req(t, k, p)?; Ok::<_, String>(address(t, k, p)?.unwrap_or_default()) };
        Ok(match (what, kind) {
            ("osc.value", _) if rel => (&["address", "per_turn", "min", "max", "type", "mode"],
                Box::new(Turn { address: must("address")?, per_turn: some_num(t, "per_turn", p)?, r: range(t, p)? })),
            ("osc.value", Kind::Button) => (&["address", "value", "off", "type", "mode"], Box::new(Press { address: must("address")?,
                value: num(t, "value", 1.0, p)?, off: num(t, "off", 0.0, p)?, ty: kind_of(t, p)?, mode: one_of(t, "mode", &["set", "hold", "toggle"], p)? })),
            ("osc.xy", Kind::Ball) => {
                let (x, y) = (address(t, "x", p)?, address(t, "y", p)?);
                if x.is_none() && y.is_none() { return Err(format!("{p}: `x` and/or `y` required")); }
                (&["x", "y", "per_unit", "min", "max", "type", "mode"], Box::new(Xy { x, y, per: some_num(t, "per_unit", p)?, r: range(t, p)? }))
            }
            ("osc.send", Kind::Button) => (&["address", "args"], Box::new(Send { address: must("address")?,
                args: read_args(text(t, "args", p)?.unwrap_or("")).map_err(|e| format!("{p}: args: {e}"))? })),
            _ => return Err(not_allowed(what, kind, &ACTIONS, p)),
        })
    }

    fn setting(&self, key: &str, v: &Value) -> Option<Result<(), String>> {
        Some(match (key, v) {
            ("osc_target", Value::String(s)) => if s.is_empty() || host_port(s, 0).is_some_and(|x| x.1 != 0) { Ok(()) } else { Err("write it as host:port, e.g. 127.0.0.1:9000".into()) },
            ("oscquery", Value::String(s)) => if s.is_empty() || url(s).is_some() { Ok(()) } else { Err("write it as http://host:port".into()) },
            ("osc_listen", Value::Integer(n)) => if (0..=65535).contains(n) { Ok(()) } else { Err("a port, 0 to 65535".into()) },
            ("osc_target" | "oscquery" | "osc_listen", _) => Err("osc_target \"host:port\", oscquery \"http://host:port\", osc_listen a port number".into()),
            _ => return None,
        })
    }
    fn configure(&self, s: &Settings) {
        let t = |k: &str| s.ext.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let c = Cfg { target: t("osc_target"), query: t("oscquery"), listen: s.ext.get("osc_listen").and_then(|v| v.as_integer()).unwrap_or(0) as u16 };
        let mut st = self.st.lock().unwrap();
        if st.cfg != c { st.cfg = c; st.error.clear(); }
    }
    fn start(&self) {
        let (a, b) = (self.st.clone(), self.st.clone());
        let _ = std::thread::Builder::new().name("osc-udp".into()).spawn(move || run_udp(a)).map_err(|e| log(&format!("osc: {e}")));
        let _ = std::thread::Builder::new().name("oscquery".into()).spawn(move || run_query(b)).map_err(|e| log(&format!("osc: {e}")));
    }

    fn send(&self, m: &J) -> bool {
        let mut st = self.st.lock().unwrap();
        let Some(a) = m["address"].as_str() else { return false };
        let ty = m["type"].as_str();
        if !st.want.contains(a) { st.want.insert(a.to_string()); }
        match m["t"].as_str().unwrap_or("") {
            "change" => {
                let n = st.nodes.get(a).cloned().unwrap_or_default();
                let rel = m["relative"] == true;
                let lo = m["min"].as_f64().or(n.min).unwrap_or(0.0);
                let hi = m["max"].as_f64().or(n.max).unwrap_or(lo + 1.0).max(lo);
                let per = m["per"].as_f64().unwrap_or((hi - lo) * m["share"].as_f64().unwrap_or(1.0));
                let by = m["n"].as_f64().unwrap_or(0.0) * per;
                if rel {
                    // whole numbers only go out when there is one; the rest waits
                    let key = format!("{a} rel");
                    let all = by + st.ours.get(&key).map_or(0.0, |x| x.0);
                    let whole = matches!(st.arg(a, 0.0, ty), Arg::I(_));
                    let out = if whole { (all + all.signum() * 1e-9).trunc() } else { all };
                    st.ours.insert(key, (all - out, Instant::now()));
                    if out == 0.0 { return st.out.is_some(); }
                    let x = st.arg(a, out, ty);
                    // a change is not a value
                    let old = st.values.get(a).cloned();
                    let ok = st.put(a, vec![x]);
                    match old { Some(o) => { st.values.insert(a.into(), o); } None => { st.values.remove(a); } }
                    return ok;
                }
                let old = st.number(a).unwrap_or(lo);
                let new = (old + by).clamp(lo, hi);
                let (x, y) = (st.arg(a, old, ty), st.arg(a, new, ty));
                st.ours.insert(a.to_string(), (new, Instant::now()));
                if x == y { st.out.is_some() } else { st.put(a, vec![y]) }
            }
            "set" | "toggle" => {
                let v = if m["t"] == "set" { m["value"].as_f64() } else {
                    let (on, off) = (m["on"].as_f64().unwrap_or(1.0), m["off"].as_f64().unwrap_or(0.0));
                    Some(if st.number(a).is_some_and(|x| (x - on).abs() < 1e-6) { off } else { on })
                }.unwrap_or(0.0);
                let x = st.arg(a, v, ty);
                st.ours.insert(a.to_string(), (v, Instant::now()));
                st.put(a, vec![x])
            }
            "send" => {
                let args = m["args"].as_array().map(|l| l.iter().filter_map(|v| match v {
                    J::Bool(b) => Some(Arg::B(*b)),
                    J::Number(n) if n.is_i64() => n.as_i64().map(|i| Arg::I(i as i32)),
                    J::Number(n) => n.as_f64().map(|f| Arg::F(f as f32)),
                    J::String(s) => Some(Arg::S(s.clone())),
                    _ => None,
                }).collect()).unwrap_or_default();
                st.put(a, args)
            }
            _ => false,
        }
    }

    fn facts(&self) -> Vec<(String, Val)> {
        let st = self.st.lock().unwrap();
        st.watched.iter().filter_map(|a| st.values.get(a).map(|v| (format!("osc.{a}"), v.val()))).collect()
    }
    fn watch(&self, ops: &[String]) {
        let mut st = self.st.lock().unwrap();
        if st.watched.len() == ops.len() && ops.iter().all(|o| st.watched.contains(o)) { return; }
        st.watched = ops.iter().cloned().collect();
        st.want.extend(ops.iter().cloned());
    }

    fn describe(&self, b: &J, kind: Kind) -> Option<String> {
        let t = |k: &str| b[k].as_str().unwrap_or("");
        Some(match t("do") {
            "osc.value" if kind != Kind::Button => self.label(t("address")),
            "osc.value" => {
                let l = self.label(t("address"));
                match t("mode") { "toggle" => format!("{l} on / off"), "hold" => format!("{l} (held)"), _ => format!("{l} = {}", b["value"].as_f64().unwrap_or(1.0)) }
            }
            "osc.xy" => [t("x"), t("y")].iter().filter(|x| !x.is_empty()).map(|x| self.label(x)).collect::<Vec<_>>().join(" / "),
            "osc.send" => format!("Send {}", t("address")),
            _ => return None,
        })
    }
    fn setting_ref(&self, b: &J, kind: Kind) -> Option<SettingRef> {
        if b["do"] != "osc.value" { return None; }
        let role = if kind != Kind::Button { Role::Turn } else if b["mode"].is_null() || b["mode"] == "set" { Role::Set(b["value"].as_f64().unwrap_or(1.0)) } else { return None };
        Some(SettingRef { id: b["address"].as_str().unwrap_or("").into(), role })
    }

    fn script(&self) -> &'static str { include_str!("osc.js") }
    fn status(&self) -> J {
        let st = self.st.lock().unwrap();
        json!({"configured": !st.cfg.target.is_empty() || !st.cfg.query.is_empty(), "target": st.out.as_ref().map(|o| o.1.to_string()), "connected": st.connected, "live": st.live,
            "name": st.name, "parameters": st.nodes.len(), "sent": st.sent, "received": st.received, "error": st.error, "rev": st.rev})
    }
    fn get(&self, path: &str) -> Option<J> {
        if path != "tree" { return None; }
        let st = self.st.lock().unwrap();
        Some(st.nodes.iter().filter(|(_, n)| n.write).map(|(a, n)| json!({"id": a, "label": n.desc, "type": n.ty, "min": n.min, "max": n.max, "unit": n.unit, "value": st.number(a)})).collect())
    }
}

// ------------------------------------------------------------------ UDP

/// "host:port" -> its parts; `def` if there is no port
fn host_port(s: &str, def: u16) -> Option<(String, u16)> {
    let (h, p) = match s.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') || h.ends_with(']') => (h, p.parse().ok()?),
        Some(_) => return None, // a bare IPv6 address
        None => (s, def),
    };
    let h = h.trim_start_matches('[').trim_end_matches(']');
    (!h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-_:".contains(c))).then(|| (h.to_string(), p))
}
/// "http://host:port" -> (host, port)
fn url(s: &str) -> Option<(String, u16)> {
    let s = s.strip_prefix("http://").unwrap_or(s);
    if s.contains("://") { return None; }
    host_port(s.split('/').next().unwrap_or(""), 80)
}
fn resolve(h: &str, p: u16) -> Result<SocketAddr, String> {
    let mut l: Vec<SocketAddr> = (h, p).to_socket_addrs().map_err(|e| format!("{h}: {e}"))?.collect();
    l.sort_by_key(|a| !a.is_ipv4()); // "localhost" first as 127.0.0.1, where OSC programs listen
    l.into_iter().next().ok_or(format!("{h}: no address"))
}

/// Keeps the socket to the target and reads what comes back.
fn run_udp(st: Arc<Mutex<St>>) {
    let mut have: Option<(Cfg, Option<SocketAddr>)> = None;
    let mut retry = Instant::now();
    let mut buf = vec![0u8; 65536];
    loop {
        let (cfg, qt) = { let s = st.lock().unwrap(); (s.cfg.clone(), s.query_target) };
        let now = (cfg.clone(), qt);
        let sock = st.lock().unwrap().out.as_ref().map(|o| o.0.clone());
        if have.as_ref() != Some(&now) || (sock.is_none() && Instant::now() >= retry) {
            retry = Instant::now() + Duration::from_secs(5);
            have = Some(now);
            st.lock().unwrap().out = None; // frees the port before it is bound again
            drop(sock);
            let to = if cfg.target.is_empty() { Ok(qt) } else { host_port(&cfg.target, 0).ok_or("bad target".to_string()).and_then(|(h, p)| resolve(&h, p)).map(Some) };
            match to {
                Ok(None) => {}
                Ok(Some(to)) => {
                    // only this PC when the other program is on it
                    let ip: std::net::IpAddr = match (to.ip().is_loopback(), to.is_ipv4()) {
                        (true, true) => [127, 0, 0, 1].into(), (true, false) => std::net::Ipv6Addr::LOCALHOST.into(),
                        (false, true) => [0, 0, 0, 0].into(), (false, false) => std::net::Ipv6Addr::UNSPECIFIED.into() };
                    let r = UdpSocket::bind((ip, cfg.listen)).and_then(|s| s.set_read_timeout(Some(Duration::from_millis(100))).map(|_| s));
                    let mut s = st.lock().unwrap();
                    match r {
                        Ok(u) => { log(&format!("osc: sending to {to}, listening on port {}", u.local_addr().map(|a| a.port()).unwrap_or(0))); s.out = Some((Arc::new(u), to)); s.error.clear(); }
                        Err(e) => s.fail(format!("can't listen on port {}: {e}", cfg.listen)),
                    }
                }
                Err(e) => st.lock().unwrap().fail(e),
            }
            continue;
        }
        let Some(u) = sock else { std::thread::sleep(Duration::from_millis(250)); continue };
        match u.recv_from(&mut buf) {
            Ok((n, _)) => { let m = decode(&buf[..n]); st.lock().unwrap().got(m); }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::ConnectionReset) => {} // reset: nobody listened to our last message
            Err(e) => { st.lock().unwrap().fail(format!("receiving failed: {e}")); std::thread::sleep(Duration::from_millis(250)); }
        }
    }
}

// ------------------------------------------------------------------ OSCQuery

const MAX_BODY: usize = 16 << 20;

fn connect(h: &str, p: u16) -> Result<TcpStream, String> {
    let a = resolve(h, p)?;
    let s = TcpStream::connect_timeout(&a, Duration::from_secs(1)).map_err(|e| e.to_string())?;
    let _ = s.set_nodelay(true);
    s.set_read_timeout(Some(Duration::from_secs(3))).and_then(|_| s.set_write_timeout(Some(Duration::from_secs(1)))).map_err(|e| e.to_string())?;
    Ok(s)
}

/// One GET that answers with JSON. OSCQuery servers are small; so is this.
fn get_json(h: &str, p: u16, path: &str) -> Result<J, String> {
    let mut s = connect(h, p)?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: {h}:{p}\r\nAccept: application/json\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let (mut data, mut buf) = (vec![], [0u8; 16384]);
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(b) = body(&data, false)? { return serde_json::from_slice(&b).map_err(|e| format!("not JSON: {e}")); }
        if data.len() > MAX_BODY || Instant::now() > until { return Err("the answer is too long or too slow".into()); }
        match s.read(&mut buf) {
            Ok(0) => { let b = body(&data, true)?.ok_or("the connection closed early")?; return serde_json::from_slice(&b).map_err(|e| format!("not JSON: {e}")); }
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(e) => return Err(e.to_string()),
        }
    }
}
/// The body once it is complete (`end` = the connection closed).
fn body(d: &[u8], end: bool) -> Result<Option<Vec<u8>>, String> {
    let Some(i) = d.windows(4).position(|w| w == b"\r\n\r\n") else { return Ok(None) };
    let head = String::from_utf8_lossy(&d[..i]).to_string();
    let code = head.split_whitespace().nth(1).unwrap_or("");
    if code != "200" { return Err(format!("the server answered {code}")); }
    let field = |n: &str| head.lines().find_map(|l| l.split_once(':').filter(|x| x.0.trim().eq_ignore_ascii_case(n)).map(|x| x.1.trim().to_ascii_lowercase()));
    let mut b = &d[i + 4..];
    if field("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        let mut out = vec![];
        loop {
            let Some(e) = b.windows(2).position(|w| w == b"\r\n") else { return Ok(None) };
            let n = usize::from_str_radix(String::from_utf8_lossy(&b[..e]).split(';').next().unwrap_or("").trim(), 16).map_err(|_| "bad chunk")?;
            if n == 0 { return Ok(Some(out)); }
            if n > MAX_BODY { return Err("the answer is too long".into()); }
            let Some(c) = b.get(e + 2..e + 2 + n) else { return Ok(None) };
            out.extend_from_slice(c);
            b = b.get(e + 4 + n..).unwrap_or(&[]);
        }
    }
    match field("content-length").and_then(|v| v.parse::<usize>().ok()) {
        Some(n) => Ok(b.get(..n).map(|x| x.to_vec())),
        None => Ok(end.then(|| b.to_vec())),
    }
}

fn run_query(st: Arc<Mutex<St>>) {
    loop {
        let q = st.lock().unwrap().cfg.query.clone();
        if !q.is_empty() {
            let r = match url(&q) { Some((h, p)) => follow(&st, &q, &h, p), None => Err("bad address".into()) };
            let mut s = st.lock().unwrap();
            let was = s.connected;
            s.connected = false;
            s.live = false;
            if let Err(e) = r { if was { log(&format!("osc: OSCQuery lost: {e}")); } s.fail(format!("OSCQuery at {q}: {e}")); }
        } else {
            let mut s = st.lock().unwrap();
            if !s.nodes.is_empty() { s.nodes.clear(); s.rev += 1; s.query_target = None; }
        }
        // sooner when the setting changes
        for _ in 0..12 { std::thread::sleep(Duration::from_millis(250)); if st.lock().unwrap().cfg.query != q { break; } }
    }
}

fn load_tree(st: &Arc<Mutex<St>>, h: &str, p: u16) -> Result<(), String> {
    let (nodes, vals) = read_tree(&get_json(h, p, "/")?);
    let mut s = st.lock().unwrap();
    for (k, v) in vals { s.values.insert(k, v); }
    if s.nodes != nodes { s.nodes = nodes; s.rev += 1; }
    Ok(())
}

/// Read the tree, then follow the values until the connection or the setting changes.
fn follow(st: &Arc<Mutex<St>>, q: &str, h: &str, p: u16) -> Result<(), String> {
    load_tree(st, h, p)?;
    // where the server listens for OSC; without HOST_INFO: the same address and port
    let info = get_json(h, p, "/?HOST_INFO").unwrap_or(J::Null);
    let port = |k: &str| info[k].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|n| *n != 0).unwrap_or(p);
    let host = |k: &str| info[k].as_str().filter(|x| !x.is_empty() && *x != "0.0.0.0").unwrap_or(h).to_string();
    {
        let to = resolve(&host("OSC_IP"), port("OSC_PORT")).ok();
        let mut s = st.lock().unwrap();
        s.query_target = to;
        s.connected = true;
        s.error.clear();
        s.name = info["NAME"].as_str().unwrap_or("").chars().take(60).collect();
        log(&format!("osc: OSCQuery {q} ({}): {} parameters", s.name, s.nodes.len()));
    }
    let mut ws = None;
    if info["EXTENSIONS"]["LISTEN"] != false {
        let (wh, wp) = (host("WS_IP"), port("WS_PORT"));
        match connect(&wh, wp).and_then(|s| tungstenite::client(format!("ws://{wh}:{wp}/"), s).map_err(|e| e.to_string())) {
            Ok((w, _)) => { let _ = w.get_ref().set_read_timeout(Some(Duration::from_millis(50))); ws = Some(w); }
            Err(e) => log(&format!("osc: no live values from {q}: {e}")),
        }
    }
    st.lock().unwrap().live = ws.is_some();
    let mut asked: HashSet<String> = HashSet::new();
    let mut reload: Option<Instant> = None;
    let mut last = Instant::now();
    loop {
        if st.lock().unwrap().cfg.query != q { return Ok(()); }
        // without live values: look again now and then
        if reload.is_some_and(|t| Instant::now() >= t) || (ws.is_none() && last.elapsed() > Duration::from_secs(5)) {
            reload = None;
            last = Instant::now();
            load_tree(st, h, p)?;
        }
        let Some(w) = ws.as_mut() else { std::thread::sleep(Duration::from_millis(250)); continue };
        let new: Vec<String> = { let s = st.lock().unwrap(); s.want.iter().filter(|a| s.nodes.contains_key(*a) && !asked.contains(*a)).cloned().collect() };
        for a in new {
            w.send(Message::text(json!({"COMMAND": "LISTEN", "DATA": a}).to_string())).map_err(|e| e.to_string())?;
            asked.insert(a);
        }
        match w.read() {
            Ok(Message::Binary(b)) => { let m = decode(&b); st.lock().unwrap().got(m); }
            Ok(Message::Text(t)) => {
                let v: J = serde_json::from_str(t.as_str()).unwrap_or(J::Null);
                if v["COMMAND"].as_str().is_some_and(|c| c.starts_with("PATH_")) { reload = Some(Instant::now() + Duration::from_millis(500)); }
            }
            Ok(Message::Close(_)) => return Err("the server closed the connection".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn parse(o: &Osc, kind: Kind, t: &str) -> Result<Box<dyn Act>, String> {
        let t: Table = t.parse().unwrap();
        o.parse(t["do"].as_str().unwrap(), kind, &t, "x").map(|x| x.1)
    }
    struct Direct<'a>(&'a Osc);
    impl Io for Direct<'_> {
        fn send(&mut self, m: J) -> bool { self.0.send(&m) }
        fn carry(&mut self, _s: &str, v: f64) -> i32 { v as i32 }
        fn steps(&mut self, _c: f64, _p: f64) -> Vec<bool> { vec![] }
    }
    fn wait(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..400 { if f() { return; } std::thread::sleep(Duration::from_millis(20)); }
        panic!("timed out: {what}");
    }

    #[test]
    fn packets() {
        assert_eq!(encode("/a", &[Arg::F(1.0)]), b"/a\0\0,f\0\0\x3f\x80\0\0");
        let m = encode("/ab/c", &[Arg::I(-3), Arg::S("hey!".into()), Arg::B(true), Arg::F(0.5)]);
        assert_eq!(m.len() % 4, 0);
        assert_eq!(decode(&m), vec![("/ab/c".to_string(), vec![Arg::I(-3), Arg::S("hey!".into()), Arg::B(true), Arg::F(0.5)])]);
        let mut b = b"#bundle\0\0\0\0\0\0\0\0\x01".to_vec();
        for x in [&m, &encode("/z", &[])] { b.extend((x.len() as u32).to_be_bytes()); b.extend(x); }
        assert_eq!(decode(&b).len(), 2);
        for n in 0..b.len() { decode(&b[..n]); } // cut anywhere: no panic
        assert!(decode(b"#bundle\0\0\0\0\0\0\0\0\x01\xff\xff\xff\xff").is_empty());
        assert!(decode(b"nonsense").is_empty());
    }

    #[test]
    fn bindings_are_checked() {
        let o = Osc::new();
        assert!(parse(&o, Kind::Knob, "do = \"osc.value\"\naddress = \"/a\"\nper_turn = -2\nmin = 0\nmax = 10").is_ok());
        for (k, t) in [(Kind::Knob, "do = \"osc.value\""), (Kind::Knob, "do = \"osc.value\"\naddress = \"a\""), (Kind::Knob, "do = \"osc.value\"\naddress = \"/a b\""),
            (Kind::Ring, "do = \"osc.value\"\naddress = \"/a\"\nmin = 2\nmax = 1"), (Kind::Ball, "do = \"osc.xy\""), (Kind::Ball, "do = \"osc.value\"\naddress = \"/a\""),
            (Kind::Button, "do = \"osc.value\"\naddress = \"/a\"\nmode = \"blink\""), (Kind::Button, "do = \"osc.send\"\naddress = \"/a\"\nargs = \"'open\"")] {
            assert!(parse(&o, k, t).is_err(), "{t}");
        }
        assert_eq!(read_args("1 0.5 true play 'two words' -3").unwrap(), vec![json!(1), json!(0.5), json!(true), json!("play"), json!("two words"), json!(-3)]);
        for (k, v, ok) in [("osc_target", Value::from("127.0.0.1:9000"), true), ("osc_target", Value::from("host"), false), ("osc_target", Value::from("[::1]:9"), true), ("osc_target", Value::from(""), true),
            ("oscquery", Value::from("http://localhost:5678/"), true), ("oscquery", Value::from("https://x"), false), ("osc_listen", Value::from(9001), true), ("osc_listen", Value::from(70000), false), ("osc_listen", Value::from("x"), false)] {
            assert_eq!(o.setting(k, &v).unwrap().is_ok(), ok, "{k} {v}");
        }
        assert!(o.setting("backlight", &Value::from(1)).is_none());
    }

    #[test]
    fn http_bodies() {
        let h = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}x";
        assert_eq!(body(h, false).unwrap().unwrap(), b"{}");
        assert!(body(&h[..h.len() - 2], false).unwrap().is_none());
        assert_eq!(body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n1\r\n}\r\n0\r\n\r\n", false).unwrap().unwrap(), b"{}");
        assert!(body(b"HTTP/1.1 200 OK\r\n\r\n{}", false).unwrap().is_none());
        assert_eq!(body(b"HTTP/1.1 200 OK\r\n\r\n{}", true).unwrap().unwrap(), b"{}");
        assert!(body(b"HTTP/1.1 404 No\r\n\r\n", true).is_err());
    }

    #[test]
    fn plain_osc() {
        // without OSCQuery: ranges from the binding, values from what comes back
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let o = Osc::new();
        let mut set = Settings::default();
        set.ext.insert("osc_target".into(), Value::from(rx.local_addr().unwrap().to_string()));
        assert!(!o.send(&json!({"t": "set", "address": "/a", "value": 1})), "nothing set up yet");
        o.configure(&set);
        o.start();
        wait("socket", || o.st.lock().unwrap().out.is_some());
        let mut buf = [0u8; 1000];
        let mut next = || { let (n, from) = rx.recv_from(&mut buf).unwrap(); (decode(&buf[..n]).remove(0), from) };
        let knob = parse(&o, Kind::Knob, "do = \"osc.value\"\naddress = \"/vol\"\nmax = 10").unwrap();
        knob.turn(1024.0, 4096.0, &mut Direct(&o));
        let (m, from) = next();
        assert_eq!(m, ("/vol".to_string(), vec![Arg::F(2.5)]));
        knob.turn(-40960.0, 4096.0, &mut Direct(&o));
        assert_eq!(next().0.1, [Arg::F(0.0)], "stops at the end");
        rx.send_to(&encode("/vol", &[Arg::F(7.0)]), from).unwrap();
        wait("feedback", || o.st.lock().unwrap().values.get("/vol") == Some(&Arg::F(7.0)));
        std::thread::sleep(ECHO);
        knob.turn(409.6, 4096.0, &mut Direct(&o));
        assert_eq!(next().0.1, [Arg::F(8.0)], "goes on from what the program reported");
        let t = parse(&o, Kind::Button, "do = \"osc.value\"\naddress = \"/mute\"\nmode = \"toggle\"\ntype = \"int\"").unwrap();
        t.press(&mut Direct(&o));
        assert_eq!(next().0.1, [Arg::I(1)]);
        t.press(&mut Direct(&o));
        assert_eq!(next().0.1, [Arg::I(0)]);
        let j = parse(&o, Kind::Ring, "do = \"osc.value\"\naddress = \"/jog\"\nmode = \"relative\"\ntype = \"int\"\nper_turn = 10").unwrap();
        for _ in 0..3 { j.turn(20.0, 300.0, &mut Direct(&o)); } // 0.67 each: 2 in all
        assert_eq!((next().0.1, next().0.1), (vec![Arg::I(1)], vec![Arg::I(1)]));
        parse(&o, Kind::Button, "do = \"osc.send\"\naddress = \"/go\"\nargs = \"3 x\"").unwrap().press(&mut Direct(&o));
        assert_eq!(next().0, ("/go".to_string(), vec![Arg::I(3), Arg::S("x".into())]));
        let h = parse(&o, Kind::Button, "do = \"osc.value\"\naddress = \"/flash\"\nmode = \"hold\"\nvalue = 0.5").unwrap();
        h.press(&mut Direct(&o));
        h.release(&mut Direct(&o));
        assert_eq!((next().0.1, next().0.1), (vec![Arg::F(0.5)], vec![Arg::F(0.0)]));
    }

    /// A small OSCQuery server: HTTP and WebSocket on one port.
    fn server(osc_port: u16) -> (u16, Arc<Mutex<Vec<String>>>, std::sync::mpsc::Sender<Vec<u8>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let heard: Arc<Mutex<Vec<String>>> = Arc::default();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let rx = Arc::new(Mutex::new(rx));
        let h2 = heard.clone();
        std::thread::spawn(move || for s in l.incoming() {
            let (mut s, heard, rx) = (s.unwrap(), h2.clone(), rx.clone());
            std::thread::spawn(move || {
                let mut b = [0u8; 2000];
                let n = s.peek(&mut b).unwrap();
                let head = String::from_utf8_lossy(&b[..n]).to_string();
                if head.to_lowercase().contains("upgrade: websocket") {
                    let mut w = tungstenite::accept(s).unwrap();
                    w.get_ref().set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                    loop {
                        if let Ok(p) = rx.lock().unwrap().try_recv() { w.send(Message::binary(p)).unwrap(); }
                        match w.read() { Ok(Message::Text(t)) => heard.lock().unwrap().push(t.to_string()), Ok(_) => {}, Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}, Err(_) => return }
                    }
                }
                let _ = s.read(&mut b);
                let j = if head.starts_with("GET /?HOST_INFO") { json!({"NAME": "Test rig", "OSC_PORT": osc_port, "OSC_TRANSPORT": "UDP", "EXTENSIONS": {"LISTEN": true, "VALUE": true}}) } else {
                    json!({"FULL_PATH": "/", "CONTENTS": {"mix": {"CONTENTS": {
                        "gain": {"FULL_PATH": "/mix/gain", "TYPE": "f", "ACCESS": 3, "VALUE": [-6.0], "RANGE": [{"MIN": -60, "MAX": 12}], "UNIT": ["dB"], "DESCRIPTION": "Main gain"},
                        "mute": {"FULL_PATH": "/mix/mute", "TYPE": "F", "ACCESS": 3},
                        "steps": {"TYPE": "i", "VALUE": [4], "RANGE": [{"MIN": 0, "MAX": 16}]},
                        "meter": {"FULL_PATH": "/mix/meter", "TYPE": "f", "ACCESS": 1, "VALUE": [0.1]}}}}})
                }.to_string();
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{j}", j.len());
            });
        });
        (port, heard, tx)
    }

    #[test]
    fn oscquery_gives_ranges_units_and_values() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (port, heard, push) = server(rx.local_addr().unwrap().port());
        let o = Osc::new();
        let mut set = Settings::default();
        set.ext.insert("oscquery".into(), Value::from(format!("http://127.0.0.1:{port}")));
        o.configure(&set);
        o.watch(&["/mix/mute".to_string()]);
        o.start();
        wait("tree", || { let s = o.st.lock().unwrap(); s.live && s.out.is_some() });
        let s = o.status();
        assert_eq!((s["name"].as_str(), s["parameters"].as_u64(), s["connected"].as_bool()), (Some("Test rig"), Some(4), Some(true)));
        let t = o.get("tree").unwrap();
        assert_eq!(t.as_array().unwrap().len(), 3, "the meter can't be written: {t}");
        assert_eq!((t[0]["label"].as_str(), t[0]["unit"].as_str(), t[0]["min"].as_f64(), t[0]["value"].as_f64()), (Some("Main gain"), Some("dB"), Some(-60.0), Some(-6.0)));
        // a turn without per_turn = the whole range, from the value the server reported
        let mut buf = [0u8; 1000];
        let mut next = || { let n = rx.recv_from(&mut buf).unwrap().0; decode(&buf[..n]).remove(0) };
        parse(&o, Kind::Knob, "do = \"osc.value\"\naddress = \"/mix/gain\"").unwrap().turn(512.0, 4096.0, &mut Direct(&o));
        assert_eq!(next(), ("/mix/gain".to_string(), vec![Arg::F(3.0)]));
        parse(&o, Kind::Knob, "do = \"osc.value\"\naddress = \"/mix/steps\"\nper_turn = 8").unwrap().turn(1024.0, 4096.0, &mut Direct(&o));
        assert_eq!(next(), ("/mix/steps".to_string(), vec![Arg::I(6)]));
        // live values: for lights, and for the next turn
        wait("listen", || { let h = heard.lock().unwrap(); ["/mix/mute", "/mix/gain", "/mix/steps"].iter().all(|a| h.iter().any(|x| x.contains("LISTEN") && x.contains(a))) });
        assert_eq!(o.facts(), vec![("osc./mix/mute".to_string(), Val::Bool(false))]);
        push.send(encode("/mix/mute", &[Arg::B(true)])).unwrap();
        wait("pushed value", || o.facts() == vec![("osc./mix/mute".to_string(), Val::Bool(true))]);
        assert!(crate::leds::eval(&crate::leds::parse("osc./mix/mute").unwrap(), &crate::leds::Facts { ext: o.facts().into_iter().collect(), ..Default::default() }));
        parse(&o, Kind::Button, "do = \"osc.value\"\naddress = \"/mix/mute\"\nmode = \"toggle\"").unwrap().press(&mut Direct(&o));
        assert_eq!(next(), ("/mix/mute".to_string(), vec![Arg::B(false)]));
        // words
        assert_eq!(o.describe(&json!({"do": "osc.value", "address": "/mix/gain"}), Kind::Knob).unwrap(), "Main gain");
        assert_eq!(o.describe(&json!({"do": "osc.value", "address": "/x/y", "mode": "toggle"}), Kind::Button).unwrap(), "x/y on / off");
        // taken out of the settings: gone
        o.configure(&Settings::default());
        wait("gone", || { let s = o.status(); s["parameters"] == 0 && s["target"].is_null() });
    }
}
