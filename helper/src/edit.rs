//! Structured editing of profiles.toml for the visual mapping editor: set / block / clear one
//! binding in place (comments and layout kept, via toml_edit), the model as JSON, and the
//! effective binding of every control in a context (same lookup as the engine).

use crate::panel::{BUTTONS, KNOBS, LEDS};
use crate::profiles::{binding_text, parse_action, Action, Kind, LayerMode, Profiles};
use serde_json::{json, Map, Value as J};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value as EV};

const RESERVED: [&str; 8] = ["button", "knob", "ring", "ball", "layer", "leds", "apps", "settings"];

fn check_name(s: &str, what: &str) -> Result<(), String> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Err(format!("{what} {s:?}: use lower-case letters, digits, _ or -"));
    }
    if RESERVED.contains(&s) { return Err(format!("{what} {s:?} is a reserved word")); }
    Ok(())
}

pub fn kind_of(k: &str) -> Result<Kind, String> {
    Kind::from_key(k).ok_or(format!("kind must be button, knob, ring or ball, not {k:?}"))
}

/// Context stack for a context path: "app/module" -> [app/module, app, default].
pub fn stack_for(ctx: &str) -> Vec<String> {
    let mut v = vec![];
    if ctx != "default" && !ctx.is_empty() {
        let segs: Vec<&str> = ctx.split('/').collect();
        for i in (1..=segs.len()).rev() { v.push(segs[..i].join("/")); }
    }
    v.push("default".into());
    v
}

fn to_edit_value(v: &J) -> Result<EV, String> {
    Ok(match v {
        J::String(s) => EV::from(s.as_str()),
        J::Bool(b) => EV::from(*b),
        J::Number(n) if n.is_i64() => EV::from(n.as_i64().unwrap()),
        J::Number(n) => EV::from(n.as_f64().unwrap_or(0.0)),
        _ => return Err(format!("binding fields must be strings, numbers or booleans, not {v}")),
    })
}

/// Set (Some(binding)), block (Some("none")) or clear (None) one binding; returns the new text.
pub fn set_binding(text: &str, ctx: &str, layer: &str, kind: &str, name: &str, binding: Option<&J>) -> Result<String, String> {
    let kind = kind_of(kind)?;
    let name = name.to_uppercase();
    if !kind.names().contains(&name.as_str()) {
        return Err(format!("no {} named {name}", kind.key()));
    }
    for seg in ctx.split('/') { check_name(seg, "context")?; }
    if !layer.is_empty() { check_name(layer, "layer")?; }
    let path = format!("{}{}.{}.{}", ctx.replace('/', "."),
        if layer.is_empty() { String::new() } else { format!(".layer.{layer}") }, kind.key(), name);

    let new_item = match binding {
        None => None,
        Some(J::String(s)) if s == "none" => Some(Item::Value(EV::from("none"))),
        Some(J::Object(m)) => {
            // validate exactly as the loader will
            let tv: toml::Value = serde_json::from_value(J::Object(m.clone())).map_err(|e| e.to_string())?;
            parse_action(kind, &tv, &path)?;
            let mut it = InlineTable::new();
            if let Some(d) = m.get("do") { it.insert("do", to_edit_value(d)?); }
            for (k, v) in m { if k != "do" { it.insert(k, to_edit_value(v)?); } }
            Some(Item::Value(EV::InlineTable(it)))
        }
        Some(other) => return Err(format!("binding must be an object or \"none\", not {other}")),
    };

    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut keys: Vec<String> = ctx.split('/').map(String::from).collect();
    if !layer.is_empty() { keys.push("layer".into()); keys.push(layer.to_string()); }
    let mut tbl: &mut Table = doc.as_table_mut();
    for k in &keys {
        if !tbl.contains_key(k) {
            if new_item.is_none() { return Ok(text.to_string()); } // nothing to clear
            let mut t = Table::new();
            t.set_implicit(true);
            tbl.insert(k, Item::Table(t));
        }
        tbl = tbl.get_mut(k).and_then(|i| i.as_table_mut())
            .ok_or(format!("`{k}` in {path} is not a [table]; edit it in the TOML view"))?;
    }
    match new_item {
        Some(item) => {
            if !tbl.contains_key(kind.key()) { tbl.insert(kind.key(), Item::Table(Table::new())); }
            let kt = tbl.get_mut(kind.key()).and_then(|i| i.as_table_mut())
                .ok_or(format!("{} in {path} is not a [table]; edit it in the TOML view", kind.key()))?;
            match (kt.get_mut(&name).and_then(|i| i.as_value_mut()), item) {
                // keep the spacing and any comment on that line
                (Some(old), Item::Value(mut v)) => { *v.decor_mut() = old.decor().clone(); *old = v; }
                (_, item) => { kt.insert(&name, item); }
            }
        }
        None => {
            let empty = match tbl.get_mut(kind.key()).and_then(|i| i.as_table_mut()) {
                Some(kt) => { kt.remove(&name); kt.is_empty() }
                None => false,
            };
            if empty { tbl.remove(kind.key()); }
        }
    }
    let out = doc.to_string();
    Profiles::parse(&out).map_err(|e| format!("the result doesn't load: {e}"))?;
    Ok(out)
}

fn raw_json(b: &crate::profiles::Binding) -> J {
    serde_json::to_value(&b.raw).unwrap_or_else(|_| json!(binding_text(b)))
}

/// Set (Some(condition)) or clear (None) the LED rule `ctx.leds.LED`. A list-form `leds = [..]`
/// in that context is converted to a table first (each entry = "true").
pub fn set_led(text: &str, ctx: &str, led: &str, rule: Option<&str>) -> Result<String, String> {
    let led = led.to_uppercase();
    if !LEDS.iter().any(|(n, _)| *n == led) {
        return Err(format!("{led} has no LED (LEDs: {})", LEDS.iter().map(|x| x.0).collect::<Vec<_>>().join(" ")));
    }
    for seg in ctx.split('/') { check_name(seg, "context")?; }
    if let Some(r) = rule { crate::leds::parse(r).map_err(|e| format!("{ctx} LED {led}: {e}"))?; }
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut tbl: &mut Table = doc.as_table_mut();
    for k in ctx.split('/') {
        if !tbl.contains_key(k) {
            if rule.is_none() { return Ok(text.to_string()); }
            let mut t = Table::new();
            t.set_implicit(true);
            tbl.insert(k, Item::Table(t));
        }
        tbl = tbl.get_mut(k).and_then(|i| i.as_table_mut()).ok_or(format!("`{k}` in {ctx} is not a [table]"))?;
    }
    // list form -> table
    if let Some(arr) = tbl.get("leds").and_then(|i| i.as_array()).cloned() {
        let mut lt = Table::new();
        for v in arr.iter() { if let Some(s) = v.as_str() { lt.insert(s, Item::Value(EV::from("true"))); } }
        tbl.insert("leds", Item::Table(lt));
    }
    match rule {
        Some(r) => {
            if !tbl.contains_key("leds") { tbl.insert("leds", Item::Table(Table::new())); }
            let lt = tbl.get_mut("leds").and_then(|i| i.as_table_mut()).ok_or("leds is not a table")?;
            lt.insert(&led, Item::Value(EV::from(r)));
        }
        None => {
            let empty = match tbl.get_mut("leds").and_then(|i| i.as_table_mut()) {
                Some(lt) => { lt.remove(&led); lt.is_empty() }
                None => false,
            };
            if empty { tbl.remove("leds"); }
        }
    }
    let out = doc.to_string();
    Profiles::parse(&out).map_err(|e| format!("the result doesn't load: {e}"))?;
    Ok(out)
}

/// Several changes as one: {ctx, layer, kind, name, binding} or {ctx, led, rule}; null clears.
/// All of them or none.
pub fn apply_batch(text: &str, items: &[J]) -> Result<String, String> {
    let mut out = text.to_string();
    for v in items {
        let s = |k: &str| v[k].as_str().unwrap_or("");
        out = if v["led"].is_string() {
            set_led(&out, s("ctx"), s("led"), v["rule"].as_str().filter(|x| !x.trim().is_empty()))?
        } else {
            set_binding(&out, s("ctx"), s("layer"), s("kind"), s("name"), if v["binding"].is_null() { None } else { Some(&v["binding"]) })?
        };
    }
    Ok(out)
}

/// Contexts, layers and bindings, for the editor's pickers and picture.
/// "dxo-photolab" -> "Dxo photolab"; known apps by their real name.
pub fn app_label(app: &str) -> String {
    match app {
        "default" => "All apps".into(),
        "explorer" => "File Explorer".into(),
        _ => crate::ext::app(app).map(|a| a.label).unwrap_or_else(|| title(app)),
    }
}

fn title(s: &str) -> String {
    let s = s.replace(['-', '_'], " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// "app/module/tool" -> "App, Tool", for titles.
pub fn place_title(ctx: &str) -> String {
    let app = ctx.split('/').next().unwrap_or(ctx);
    let info = crate::ext::app(app);
    let name = info.as_ref().map(|a| if a.places.is_empty() { a.label.clone() } else { a.short.clone() }).unwrap_or_else(|| app_label(app));
    if ctx == app { return name; }
    let places = info.map(|a| a.places).unwrap_or_default();
    let place = places.iter().flat_map(|p| p.tools.iter().skip(1)).find(|t| t.0 == ctx).map(|t| t.1.clone())
        .or(places.iter().find(|p| p.ctx == ctx).map(|p| p.label.clone()))
        .unwrap_or_else(|| { let l = ctx.rsplit('/').next().unwrap_or(""); if l == "dialog" { "Dialog box".into() } else { title(l) } });
    format!("{name}, {place}")
}

/// Context name for an exe that has none yet: "Adobe Photoshop 2026.exe" -> "adobe-photoshop-2026".
pub fn app_slug(exe: &str) -> String {
    let e = exe.to_lowercase();
    let mut out = String::new();
    for c in e.trim_end_matches(".exe").chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { out.push(c); }
        else if !out.ends_with('-') { out.push('-'); }
    }
    out.trim_matches('-').to_string()
}

/// Number of bindings and LED rules set in `ctx` or (with `below`) anywhere under it.
fn used(p: &Profiles, ctx: &str, below: bool) -> usize {
    let sub = format!("{ctx}/");
    p.ctxs.iter().filter(|(c, _)| *c == ctx || (below && c.starts_with(&sub))).map(|(_, x)| x.bind.len() + x.leds.len()).sum()
}

/// The editor's navigation: apps, the places inside each app, the tools inside a place.
fn apps_tree(p: &Profiles) -> Vec<J> {
    let mut apps: Vec<String> = std::iter::once("default".to_string()).chain(crate::ext::apps().map(|(i, _)| i.id().to_string())).collect();
    let mut more: Vec<String> = p.apps.values().cloned()
        .chain(p.ctxs.keys().map(|c| c.split('/').next().unwrap_or(c).to_string())).collect();
    more.sort();
    for a in more { if !apps.contains(&a) { apps.push(a); } }
    apps.iter().map(|app| {
        let mut places: Vec<J> = vec![];
        let mut seen: Vec<String> = vec![app.clone()];
        let info = crate::ext::app(app);
        for pl in info.iter().flat_map(|a| &a.places) {
            let tools: Vec<J> = pl.tools.iter().map(|(tc, tl)| { seen.push(tc.clone()); json!({"ctx": tc, "label": tl, "used": used(p, tc, false)}) }).collect();
            seen.push(pl.ctx.clone());
            places.push(json!({"ctx": pl.ctx, "label": pl.label, "more": pl.more, "used": used(p, &pl.ctx, &pl.ctx != app), "tools": tools}));
        }
        // anything else the profile names under this app (e.g. app/dialog, or a tool the integration doesn't list)
        let sub = format!("{app}/");
        let mut extra: Vec<&String> = p.ctxs.keys().filter(|c| c.starts_with(&sub) && !seen.contains(c)).collect();
        extra.sort();
        if !extra.is_empty() && places.is_empty() {
            places.push(json!({"ctx": app, "label": "Everywhere", "more": false, "used": used(p, app, false), "tools": []}));
        }
        for c in extra {
            let l = c[sub.len()..].split('/').map(|s| if s == "dialog" { "Dialog boxes".into() } else { title(s) }).collect::<Vec<_>>().join(", ");
            places.push(json!({"ctx": c, "label": l, "more": true, "used": used(p, c, false), "tools": []}));
        }
        let removable = app != "default" && info.is_none() && p.apps.values().any(|a| a == app) && used(p, app, true) == 0;
        json!({"ctx": app, "label": app_label(app), "places": places, "short": info.as_ref().map(|a| a.short.clone()), "name": if app == "default" { app_label(app) } else { place_title(app) }, "used": used(p, app, true), "removable": removable})
    }).collect()
}

/// Which buttons open which layer.
fn layer_openers(p: &Profiles) -> Vec<J> {
    let mut v = vec![];
    for (c, x) in &p.ctxs {
        for ((l, k, n), b) in &x.bind {
            let (name, mode) = match &b.action {
                Action::Layer { name, mode } => (name.clone(), match mode { LayerMode::Hold => "hold", LayerMode::Toggle => "toggle", LayerMode::Auto => "auto" }),
                Action::Ext(a) => match a.act.layer() { Some(name) => (name.to_string(), "hold"), None => continue },
                _ => continue,
            };
            v.push((c.clone(), l.clone(), n.clone(), json!({"layer": name, "ctx": c, "in_layer": l, "kind": k.key(), "name": n, "mode": mode})));
        }
    }
    v.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    v.into_iter().map(|x| x.3).collect()
}

/// Add an app (`exe` = its program file) to [apps], or remove one nothing is set for (`exe` = None).
pub fn set_app(text: &str, name: &str, exe: Option<&str>) -> Result<String, String> {
    check_name(name, "app")?;
    let p = Profiles::parse(text)?;
    let builtin = || if name == "default" || crate::ext::app(name).is_some() { Err(format!("{} is built in", app_label(name))) } else { Ok(()) };
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    match exe {
        Some(exe) => {
            let e = exe.trim().to_lowercase();
            let e = if e.ends_with(".exe") { e } else { format!("{e}.exe") };
            if e == ".exe" || e.contains(['/', '\\', ':', '"']) { return Err(format!("{exe:?} is not a program file name")); }
            if let Some(a) = p.apps.get(&e) { return Err(format!("{e} already belongs to {}", app_label(a))); }
            builtin()?;
            if p.apps.values().any(|a| a == name) { return Err(format!("there is already an app named {name}")); }
            if !doc.contains_key("apps") { doc.insert("apps", Item::Table(Table::new())); }
            let t = doc.get_mut("apps").and_then(|i| i.as_table_mut()).ok_or("[apps] is not a table")?;
            let mut arr = toml_edit::Array::new();
            arr.push(e);
            t.insert(name, Item::Value(EV::Array(arr)));
        }
        None => {
            builtin()?;
            let n = used(&p, name, true);
            if n > 0 { return Err(format!("{} still has {n} assignments", app_label(name))); }
            if let Some(t) = doc.get_mut("apps").and_then(|i| i.as_table_mut()) { t.remove(name); }
        }
    }
    let out = doc.to_string();
    Profiles::parse(&out).map_err(|e| format!("the result doesn't load: {e}"))?;
    Ok(out)
}

/// Whether unset controls keep their normal job in layer `name` (`[layers]`), keeping the rest of the file.
pub fn set_layer_inherit(text: &str, name: &str, on: bool) -> Result<String, String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') { return Err(format!("bad layer name {name:?}")); }
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    if on {
        if !doc.contains_key("layers") { doc.insert("layers", Item::Table(Table::new())); }
        let t = doc.get_mut("layers").and_then(|i| i.as_table_mut()).ok_or("[layers] is not a table")?;
        let mut v = toml_edit::InlineTable::new();
        v.insert("inherit", true.into());
        t.insert(name, Item::Value(toml_edit::Value::InlineTable(v)));
    } else if let Some(t) = doc.get_mut("layers").and_then(|i| i.as_table_mut()) {
        t.remove(name);
        if t.is_empty() { doc.remove("layers"); }
    }
    let out = doc.to_string();
    Profiles::parse(&out).map_err(|e| format!("the result doesn't load: {e}"))?;
    Ok(out)
}

/// Set one `[settings]` value, keeping the rest of the file (comments included).
pub fn set_setting(text: &str, key: &str, value: &J) -> Result<String, String> {
    let v: toml_edit::Value = match (key, value) {
        ("start_deg", J::Number(n)) => n.as_f64().unwrap_or(0.0).into(),
        ("backlight" | "pointer_smoothing_ms" | "backlight_off_after_s" | "start_ball" | "press_ms" | "sleep_after_min" | "restart_hold_s", J::Number(n)) =>
            n.as_i64().ok_or(format!("{key} must be a whole number"))?.into(),
        ("restart_keys", J::String(s)) => crate::profiles::restart_keys(s)?.join("+").into(),
        ("resolve" | "midi_out" | "midi_in", J::String(s)) => s.as_str().into(),
        (_, J::Bool(_) | J::Number(_) | J::String(_)) if crate::ext::all().iter().any(|i| {
            toml::Value::try_from(value).ok().and_then(|v| i.setting(key, &v)).map_or(false, |r| r.is_ok())
        }) => to_edit_value(value)?,
        _ => return Err(format!("unknown setting or bad value: {key} = {value}")),
    };
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    if !doc.contains_key("settings") { doc.insert("settings", Item::Table(Table::new())); }
    let t = doc.get_mut("settings").and_then(|i| i.as_table_mut()).ok_or("[settings] is not a table")?;
    match t.get_mut(key).and_then(|i| i.as_value_mut()) {
        Some(old) => { let d = old.decor().clone(); *old = v; *old.decor_mut() = d; }
        None => { t.insert(key, Item::Value(v)); }
    }
    let out = doc.to_string();
    Profiles::parse(&out).map_err(|e| format!("the result doesn't load: {e}"))?;
    Ok(out)
}

pub fn model(p: &Profiles) -> J {
    let mut ctxs: Vec<String> = p.ctxs.keys().cloned().collect();
    // parents of nested contexts too (e.g. "app" for "app/module")
    for c in p.ctxs.keys() {
        let segs: Vec<&str> = c.split('/').collect();
        for i in 1..segs.len() { ctxs.push(segs[..i].join("/")); }
    }
    ctxs.push("default".into());
    ctxs.sort();
    ctxs.dedup();
    let mut layers: Vec<String> = vec![];
    let mut binds = vec![];
    for (c, ctx) in &p.ctxs {
        for ((l, k, n), b) in &ctx.bind {
            if !l.is_empty() { layers.push(l.clone()); }
            if let Action::Layer { name, .. } = &b.action { layers.push(name.clone()); }
            binds.push(json!({"ctx": c, "layer": l, "kind": k.key(), "name": n, "binding": raw_json(b)}));
        }
    }
    layers.sort();
    layers.dedup();
    let mut leds = vec![];
    for (c, ctx) in &p.ctxs {
        for (l, _, src) in &ctx.leds { leds.push(json!({"ctx": c, "led": l, "rule": src})); }
    }
    let mut inherit: Vec<&String> = p.inherit.iter().collect();
    inherit.sort();
    json!({"contexts": ctxs, "apps": apps_tree(p), "layer_openers": layer_openers(p), "layers": layers, "layer_inherit": inherit, "bindings": binds, "leds": leds,
           "led_keys": LEDS.iter().map(|(n, b)| json!({"name": n, "bit": b, "color": if *n == "BYPASS" || *n == "DISABLE" { "red" } else { "green" }})).collect::<Vec<_>>(),
           "settings": {"backlight": p.settings.backlight, "resolve_yield": p.settings.resolve_yield,
                        "ext": p.settings.ext,
                        "pointer_smoothing_ms": p.settings.pointer_smoothing_ms,
                        "backlight_off_after_s": p.settings.backlight_off_after_s,
                        "start_deg": p.settings.start_deg, "start_ball": p.settings.start_ball, "press_ms": p.settings.press_ms, "sleep_after_min": p.settings.sleep_after_min, "restart_keys": p.settings.restart_keys.join("+"), "restart_hold_s": p.settings.restart_hold_s,
                        "midi_out": p.settings.midi_out, "midi_in": p.settings.midi_in}, "warnings": p.warnings})
}

/// For every control: what it does in `ctx` with `layers` held, and where that comes from.
pub fn effective(p: &Profiles, ctx: &str, layers: &[String]) -> J {
    let stack = stack_for(ctx);
    let mut out = Map::new();
    let all: Vec<(Kind, Vec<&str>)> = vec![
        (Kind::Button, BUTTONS.iter().map(|(_, n)| *n).collect()),
        (Kind::Knob, KNOBS.to_vec()),
        (Kind::Ring, vec!["LEFT", "MIDDLE", "RIGHT"]),
        (Kind::Ball, vec!["LEFT", "MIDDLE", "RIGHT"]),
    ];
    for (k, names) in all {
        for n in names {
            let v = match p.lookup(k, n, &stack, layers) {
                Some(f) => json!({"binding": raw_json(f.binding), "ctx": f.ctx, "layer": f.layer,
                                  "here": f.ctx == stack[0] && f.layer == layers.last().map(|s| s.as_str())}),
                None => J::Null,
            };
            out.insert(format!("{}.{n}", k.key()), v);
        }
    }
    let led_rules: Map<String, J> = p.led_rules(&stack).into_iter().map(|(l, _)| {
        let (c, src) = stack.iter().find_map(|c| p.ctxs.get(c).and_then(|x| x.leds.iter().find(|(n, _, _)| n == l).map(|(_, _, s)| (c.clone(), s.clone())))).unwrap_or_default();
        let here = c == stack[0];
        (l.to_string(), json!({"rule": src, "ctx": c, "here": here}))
    }).collect();
    json!({"stack": stack, "layers": layers, "controls": out, "leds": led_rules})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() { crate::ext::testing::init(); }
    const SRC: &str = "# my comment\n[default.button]\nSTOP = { do = \"key\", keys = \"space\" }  # keep me\n";

    #[test]
    fn set_keeps_comments_and_creates_tables() {
        init();
        let b = json!({"do": "ph.param", "id": "Exposure2012", "per_turn": 2});
        let out = set_binding(SRC, "photo/develop", "", "knob", "y_gamma", Some(&b)).unwrap();
        assert!(out.contains("# my comment") && out.contains("# keep me"), "{out}");
        assert!(out.contains("[photo.develop.knob]"), "{out}");
        assert!(!out.contains("[photo]\n") && !out.contains("[photo.develop]\n"), "parents implicit: {out}");
        assert!(out.contains("Y_GAMMA = { do = \"ph.param\""), "do first: {out}");
        let p = Profiles::parse(&out).unwrap();
        assert!(p.ctxs.contains_key("photo/develop"));
    }

    #[test]
    fn replacing_keeps_the_comment() {
        init();
        let out = set_binding(SRC, "default", "", "button", "STOP", Some(&json!("none"))).unwrap();
        assert!(out.contains("STOP = \"none\"  # keep me"), "{out}");
        let back = set_binding(&out, "default", "", "button", "STOP", Some(&json!({"do": "key", "keys": "space"}))).unwrap();
        assert_eq!(back, SRC);
    }

    #[test]
    fn layer_block_clear_and_validation() {
        init();
        let out = set_binding(SRC, "photo", "mouse", "ball", "RIGHT", Some(&json!({"do": "mouse.move", "speed": 0.75}))).unwrap();
        assert!(out.contains("[photo.layer.mouse.ball]"), "{out}");
        let out = set_binding(&out, "default", "", "button", "STOP", Some(&json!("none"))).unwrap();
        assert!(out.contains("STOP = \"none\""), "{out}");
        let out = set_binding(&out, "default", "", "button", "STOP", None).unwrap();
        assert!(!out.contains("STOP"), "{out}");
        assert!(set_binding(SRC, "default", "", "ball", "RIGHT", Some(&json!({"do": "key", "keys": "a"}))).unwrap_err().contains("can't be bound"));
        assert!(set_binding(SRC, "Bad Ctx", "", "knob", "HUE", Some(&json!("none"))).is_err());
        assert!(set_binding(SRC, "default", "", "knob", "NOPE", Some(&json!("none"))).is_err());
    }

    #[test]
    fn batches_are_all_or_nothing() {
        init();
        let ok = json!([{"ctx":"photo","layer":"fine","kind":"knob","name":"HUE","binding":"none"}, {"ctx":"photo","led":"USER","rule":"layer.fine"},
                        {"ctx":"default","layer":"","kind":"button","name":"STOP","binding":null}]);
        let out = apply_batch(SRC, ok.as_array().unwrap()).unwrap();
        assert!(out.contains("[photo.layer.fine.knob]") && out.contains("USER = \"layer.fine\"") && !out.contains("STOP"), "{out}");
        let bad = json!([{"ctx":"photo","layer":"","kind":"knob","name":"HUE","binding":"none"}, {"ctx":"photo","led":"STOP","rule":"true"}]);
        assert!(apply_batch(SRC, bad.as_array().unwrap()).is_err());
    }

    #[test]
    fn led_rules_set_convert_clear() {
        init();
        let src = "[photo]\nleds = [\"USER\"]\n";
        let out = set_led(src, "photo", "bypass", Some("ph.flag == reject")).unwrap();
        assert!(out.contains("[photo.leds]") && out.contains("USER = \"true\"") && out.contains("BYPASS = \"ph.flag == reject\""), "{out}");
        let p = Profiles::parse(&out).unwrap();
        assert_eq!(p.led_rules(&["photo".into()]).len(), 2);
        assert!(set_led(&out, "photo", "STOP", Some("true")).unwrap_err().contains("no LED"));
        assert!(set_led(&out, "photo", "USER", Some("ph.flag = x")).is_err());
        let out = set_led(&out, "photo", "USER", None).unwrap();
        assert!(!out.contains("USER"), "{out}");
        let e = effective(&Profiles::parse(&out).unwrap(), "photo/develop", &[]);
        assert_eq!(e["leds"]["BYPASS"]["ctx"], "photo");
    }

    #[test]
    fn settings_set_in_place() {
        init();
        let src = "# top\n[settings]\nbacklight = 60   # brightness\n";
        let out = set_setting(src, "backlight", &json!(25)).unwrap();
        assert!(out.contains("backlight = 25   # brightness"), "{out}");
        let out = set_setting(&out, "start_deg", &json!(2)).unwrap();
        let out = set_setting(&out, "press_ms", &json!(600)).unwrap();
        assert_eq!((Profiles::parse(&out).unwrap().settings.start_deg, Profiles::parse(&out).unwrap().settings.press_ms), (2.0, 600));
        assert!(set_setting(&out, "start_ball", &json!(1000)).is_err());
        let out = set_setting(&out, "backlight_off_after_s", &json!(300)).unwrap();
        assert_eq!(Profiles::parse(&out).unwrap().settings.backlight_off_after_s, 300);
        assert!(set_setting(&out, "backlight", &json!(300)).is_err());
        assert!(set_setting(&out, "nope", &json!(1)).is_err());
    }

    #[test]
    fn apps_tree_groups_places() {
        init();
        let src = "[apps]\nphoto = [\"photo pro.exe\"]\nphotolab = [\"dxo photolab.exe\"]\n\
                   [photo.develop.masking.button]\nLOOP = \"none\"\n[explorer.ring]\nRIGHT = \"none\"\n\
                   [cutter.dialog.button]\nSTOP = \"none\"\n";
        let m = model(&Profiles::parse(src).unwrap());
        let apps: Vec<&str> = m["apps"].as_array().unwrap().iter().map(|a| a["ctx"].as_str().unwrap()).collect();
        assert_eq!(apps, ["default", "photo", "cutter", "explorer", "photolab"]);
        let lr = &m["apps"][1];
        assert_eq!(lr["label"], "Photo Pro");
        let places = lr["places"].as_array().unwrap();
        let dev = places.iter().find(|x| x["ctx"] == "photo/develop").unwrap();
        assert_eq!(dev["used"], 1, "counts the tools below it");
        assert_eq!(dev["tools"][2]["label"], "Masking");
        assert_eq!(places[0]["used"], 0, "Everywhere counts only itself");
        assert_eq!(places.len(), 5, "masking is a tool, not an extra place");
        assert_eq!(m["apps"][2]["places"][1]["label"], "Dialog boxes");
        assert_eq!(m["apps"][3]["places"].as_array().unwrap().len(), 0);
        assert_eq!((m["apps"][4]["removable"].as_bool(), m["apps"][3]["removable"].as_bool()), (Some(true), Some(false)));
    }

    #[test]
    fn openers_list_layer_keys() {
        init();
        let src = "[default.button]\nSHIFT_UP = { do = \"layer\", name = \"more\", mode = \"auto\" }\n\
                   [cutter.button]\nAUTO_COLOR = { do = \"ph.cmd\", id = \"groupHere\", layer = \"spread\" }\n\
                   [photo.layer.fine.knob]\nHUE = \"none\"\n";
        let m = model(&Profiles::parse(src).unwrap());
        let o = m["layer_openers"].as_array().unwrap();
        assert_eq!(o.len(), 2);
        let (o0, o1) = (&o[1], &o[0]);
        assert_eq!((o0["layer"].as_str(), o0["name"].as_str(), o0["mode"].as_str()), (Some("more"), Some("SHIFT_UP"), Some("auto")));
        assert_eq!((o1["layer"].as_str(), o1["ctx"].as_str(), o1["mode"].as_str()), (Some("spread"), Some("cutter"), Some("hold")));
        assert!(m["layers"].as_array().unwrap().iter().any(|l| l == "fine"), "a layer without a key is still listed");
    }

    #[test]
    fn apps_add_and_remove() {
        init();
        let src = "# top\n[apps]\nphoto = [\"photo pro.exe\"]  # LR\n";
        let out = set_app(src, "photoshop", Some("Photoshop.exe")).unwrap();
        assert!(out.contains("# LR") && out.contains("photoshop = [\"photoshop.exe\"]"), "{out}");
        assert_eq!(set_app(&out, "photoshop", None).unwrap(), src);
        assert!(set_app(&out, "ps2", Some("photoshop")).unwrap_err().contains("already belongs"));
        assert!(set_app(&out, "photoshop", Some("ps.exe")).unwrap_err().contains("already an app"));
        assert!(set_app(src, "x", Some("c:\\x.exe")).is_err());
        assert!(set_app(src, "x", Some("")).is_err());
        assert!(set_app(src, "photo", None).is_err());
        assert!(set_app(src, "apps", Some("a.exe")).is_err());
        let with = set_binding(&out, "photoshop", "", "knob", "HUE", Some(&json!("none"))).unwrap();
        assert!(set_app(&with, "photoshop", None).unwrap_err().contains("1 assignments"));
        assert!(set_app("", "photolab", Some("dxo photolab.exe")).unwrap().contains("[apps]"));
        assert_eq!(app_slug("DxO PhotoLab 9.exe"), "dxo-photolab-9");
    }

    #[test]
    fn effective_shows_source() {
        init();
        let p = Profiles::parse(crate::profiles::default_toml()).unwrap();
        let e = effective(&p, "photo/develop", &[]);
        assert_eq!(e["controls"]["knob.Y_GAMMA"]["ctx"], "photo/develop");
        assert_eq!(e["controls"]["knob.Y_GAMMA"]["here"], true);
        assert_eq!(e["controls"]["knob.Y_GAMMA"]["binding"]["id"], "Exposure");
        let e = effective(&p, "chrome", &[]);
        assert_eq!(e["controls"]["ball.RIGHT"]["ctx"], "default");
        assert_eq!(e["controls"]["ball.RIGHT"]["here"], false);
        let p = Profiles::parse("[photo.knob]
HUE = \"none\"
[photo.layer.fine.knob]
HUE = { do = \"mouse.scroll\" }
").unwrap();
        let e = effective(&p, "photo/develop", &["fine".to_string()]);
        assert_eq!(e["controls"]["knob.HUE"]["layer"], "fine");
        assert!(model(&p)["layers"].as_array().unwrap().iter().any(|l| l == "fine"));
    }
}
