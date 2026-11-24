# Writing an integration

The helper's core knows no application. An integration teaches it one program or protocol: new actions,
places inside the program, values for lights, and words and lists for the editor and the cheat sheet. The
OSC support is one: [`src/osc.rs`](src/osc.rs) and [`src/osc.js`](src/osc.js), about the size you should expect.

First check whether you need one. If the program takes keyboard shortcuts, MIDI, Mackie Control or OSC, a
profile is enough.

## Two kinds

- A **protocol** (OSC): `app()` returns `None`. Its actions can be used everywhere.
- An **application**: `app()` gives its name, its program files and its places. Its actions are offered
  when that app is being edited, and its id is its place in profiles (`[myapp.knob]`).

## Where the code goes

An integration is a Rust type with the trait `micropanel::ext::Integration`. It can live in its own crate;
nothing in the helper has to change:

```toml
# Cargo.toml
[dependencies]
micropanel = { git = "https://github.com/Bipedal/BMD-MicroColorPanel-Tools" }
serde_json = "1"
toml = "1"
```

```rust
// src/main.rs: the helper with your integration
#![cfg_attr(not(test), windows_subsystem = "windows")]
use std::sync::Arc;

fn main() {
    let mut all = micropanel::ext::builtin();   // OSC
    all.push(Arc::new(my_integration::MyApp::new()));
    micropanel::cli::main(all);
}
```

## The smallest one

```rust
use micropanel::ext::{Act, AppInfo, Integration, Io, Parsed, Place};
use micropanel::profiles::{not_allowed, num, req, Kind};
use serde_json::{json, Value as J};

#[derive(Debug)]
struct Turn { id: String, per_turn: f64 }
impl Act for Turn {
    fn turn(&self, counts: f64, per: f64, io: &mut dyn Io) {
        io.send(json!({"set": self.id, "by": counts / per * self.per_turn}));
    }
}

const ACTIONS: [(&str, &str, &str, &str); 1] = [("my.value", "knob, ring", "id, per_turn", "changes a setting")];

pub struct MyApp;
impl Integration for MyApp {
    fn id(&self) -> &'static str { "myapp" }
    fn prefix(&self) -> &'static str { "my" }          // my.value, my.<light value>
    fn app(&self) -> Option<AppInfo> {
        Some(AppInfo { label: "My App Pro".into(), short: "My App".into(), exes: vec!["myapp.exe".into()], places: vec![
            Place { ctx: "myapp".into(), label: "Everywhere".into(), more: false, tools: vec![] },
            Place { ctx: "myapp/edit".into(), label: "Edit".into(), more: false, tools: vec![] },
        ] })
    }
    fn actions(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] { &ACTIONS }
    fn parse(&self, what: &str, kind: Kind, t: &toml::Table, p: &str) -> Result<Parsed, String> {
        Ok(match (what, kind) {
            ("my.value", Kind::Knob | Kind::Ring) => (&["id", "per_turn"], Box::new(Turn { id: req(t, "id", p)?.into(), per_turn: num(t, "per_turn", 1.0, p)? })),
            _ => return Err(not_allowed(what, kind, &ACTIONS, p)),
        })
    }
    fn start(&self) { /* connect, in a thread of its own */ }
    fn send(&self, msg: &J) -> bool { /* deliver it; false = nobody there */ false }
}
```

## How it works

- **`parse`** reads one binding. It returns the fields the binding may have (others are an error) and an
  `Act`. Errors start with `p`, the binding's path.
- **`Act`** is one bound action: `press` / `release` for keys, `turn` for knobs and rings (counts and counts
  per full turn), `ball` (steps right and up). It has no state of its own: `io.send` passes a message to your
  `send`, and `io.carry` / `io.steps` keep the fractions between events. Never throw movement away.
  `allowed` says whether it may act now; `layer` names a layer that is open while the key is held.
- **`send`** is called from the thread that reads the panel. It must not wait: queue, or write with a short
  timeout. Combine fast turns if the other side is slow.
- **`places`** returns where in the app the user is now, most specific first, without the app itself.
- **`facts`** and **`watch`**: values for lights, `("my.name", value)`. `watch` is told about ten times a
  second which ones some light uses.
- **`setting`** / **`configure`**: your keys in `[settings]`.
- **`describe`, `shorten`, `shortcut`, `setting_ref`**: words for the cheat sheet. `setting_ref` lets the core
  say "Reset" when a press resets what its knob turns.
- **`status`** and **`get`**: JSON for your editor script, in `/api/state` under `ext.<id>` and at
  `/api/ext/<id>/<path>`.
- **`starter`**: bindings a new user starts with, as TOML tables of your own place.

The comments in [`src/ext.rs`](src/ext.rs) are the reference. If the program has a parameter tree, think of it
as OSCQuery does: path, type, range, unit, description, value, and changes pushed to you.

## The editor script

`script()` returns JavaScript that calls `MP.register` once. Everything but `id` is optional; a hook may also
be a plain value. A hook that throws is logged and ignored.

```js
MP.register({
  id: "myapp", prefix: "my", name: "My App", icon: "My", app: true,   // app: false for a protocol
  setup: function (api) {},          // keep api, see below
  load: function () {},              // fetch your lists; return the promise
  stale: function (status) {},       // true = call load again
  chip: function (status) {},        // ["My App: connected", "ok" | "warn" | "err"] for the top line
  note: function () {},              // one sentence under your list, e.g. why it is empty
  settings: [{ key, type: "bool" | "num" | undefined, label, help, ph, def }],

  describe: function (b, kind) {},   // what a binding does, in words
  shorten: function (text) {},       // what fits on a key
  shortcut: function (keys) {},      // what a keyboard shortcut is called in your app
  settingRef: function (b, kind) {}, // {id, role: "turn" | "reset" | "set", value}
  settingName: function (id) {},
  resetOf: function (id) {},         // the binding that resets a setting

  fields: function (kind, what) {},  // [{n, t, l, ...}], see below
  check: function (b, kind) {},      // an error, or ""; may tidy b up
  speed: function (b) {},            // "Normal" for a field with speed: 1
  unit: function (b) {},

  // something to install or set up first: shown under Settings; the button does PUT /api/ext/<id>/<path>
  needs: function (status) {},       // null or {title, text, button, path, primary, also: {button, path, confirm}}

  entries: function (add) {},        // add(group, label, kinds, make(kind, name), match(b, kind), {sid, resets})
  groups: [], placeGroups: {}, ownPlaceGroups: [],   // order; group for a place; groups only suggested there
  lights: function () {},            // [[group, [[rule, label, words]]]]
  asks: [{ v: "?my.x", match: /regexp/, label, ph, items(), words(arg), rule(value) }],   // lights that need a value
  operands: "my.flag, my.<setting>", ruleExample: "my.flag == pick",
  catalogue: function () {},         // [{title, items: [{id, label, group}]}] for the TOML tab
  template: function (kind) {}       // a binding as TOML, for the TOML tab
});
```

A field: `n` name, `l` label, `t` type (`text`, `num`, `bool`, `sel` with `opts`, `keys`, `pick` with `items`),
`req`, `def`, `ph` placeholder, `min`, `max`, `help`, `unit`, `adv: 1` (behind "More settings"), `speed: 1`
(Fine / Normal / Fast buttons).

An entry's `sid` is the setting it is about; `resets: true` says that on a key it resets it.

`api`: `fetchJson(url)`, `cap(text)`, `num(number)`, `perStep(kind)`, `knobAndRing`, `all` (the four kinds),
`copy(object)`, `fixed(binding)` (a `make` that always gives this binding), `own(key)` (what is set for a
control here, or null), `place()`, `bindings()`.

## Trying it

`micropanel --editor-only --port 58791 --connect` runs your build's editor and integrations next to a running
helper, with `MICROPANEL_HOME` pointing at another folder. `--no-tray --dry` logs what would be sent.
Test `parse` and the `Act`s with `micropanel::engine::Engine` and an `Out` that records; the tests in
`src/engine.rs` show how.
