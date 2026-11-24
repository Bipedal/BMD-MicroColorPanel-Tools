// Lightroom Classic in the mapping editor.
(function () {
  "use strict";
  var api, params = [], cmds = [], pmap = {}, cmap = {}, was = false, loaded = false;
  var PHOTO = "Photos, flags and ratings", VIEWS = "Views and tools", EDIT = "Undo, reset, copy";
  // Lightroom's own panel order; settings, commands and wheels of one panel belong together
  var G_MAP = { "HSL": "Color mixer", "Tone Curve": "Tone curve", "Color Grading": "Color grading", "Lens": "Lens corrections", "Mask": "Masking", "Masks": "Masking", "White Balance": "White balance",
    "Navigation": PHOTO, "Flags": PHOTO, "Rating": PHOTO, "Labels": PHOTO, "Photo": PHOTO, "Modules": VIEWS, "View": VIEWS, "Tools": VIEWS, "Edit": EDIT, "Develop": EDIT };
  var PRESENCE = ["Texture", "Clarity2012", "Dehaze", "Vibrance", "Saturation"];
  function groupOf(it) {
    if (it.group === "Basic") { return it.id === "Temperature" || it.id === "Tint" ? "White balance" : PRESENCE.indexOf(it.id) >= 0 ? "Presence" : "Tone"; }
    return G_MAP[it.group] || it.group || "Other";
  }
  // shortcuts Lightroom users know by name
  var KEYS = { "ctrl+shift+c": "Copy settings", "ctrl+shift+v": "Paste settings", "backslash": "Before/after", "f": "Full-screen preview" };
  var SHORT = { "Spot removal tool": "Remove tool", "Reset all adjustments": "Reset all", "Crop move left/right / Crop move up/down": "Move crop",
    "Mask overlay on/off": "Mask overlay", "White balance: As Shot": "WB as shot", "Mask temperature / Mask tint": "Temp / Tint", "Temperature / Tint": "Temp / Tint",
    "Before/after": "Before / after" };
  var PER_SPECIAL = { Exposure2012: 2, Temperature: 2000, Tint: 50, Sharpness: 150, SplitToningBalance: 200, PerspectiveScale: 50,
    straightenAngle: 20, 'crop.size': 1, 'crop.x': 0.5, 'crop.y': 0.5, 'mask.size': 50, 'remove.size': 50, 'remove.feather': 50, local_Exposure: 2, local_Temperature: 100, local_Tint: 100 };
  var TOOLS = { masking: "Masking", crop: "Crop", dust: "Remove", redeye: "Red eye" };
  var MODULES = ["library", "develop", "map", "book", "slideshow", "print", "web"];
  var COLORS = ["red", "yellow", "green", "blue", "purple"];

  function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); }
  function plab(id) { return id && pmap[id] ? pmap[id].label || id : id || ""; }
  function clab(id) { return id && cmap[id] ? cmap[id].label || id : id || ""; }
  function lower(t) { return t.replace(/^[A-Z](?=[a-z])/, function (c) { return c.toLowerCase(); }); }
  function isReset(b) { return b.reset || !b.value; }
  function perTurn(id) { return has(PER_SPECIAL, id) ? PER_SPECIAL[id] : /hue/i.test(id) ? 360 : 100; }
  function resetOf(id) { if (/^(remove|mask)\./.test(id)) { return null; } return /^local_/.test(id) ? { do: "lr.param", id: id, value: 0 } : { do: "lr.param", id: id, reset: true }; }

  MP.register({
    id: "lightroom", prefix: "lr", name: "Lightroom", icon: "Lr", app: true,
    setup: function (a) { api = a; },
    load: function () {
      return api.fetchJson("/api/ext/lightroom/catalog").then(function (c) {
        params = c.params || []; cmds = c.cmds || []; pmap = {}; cmap = {}; loaded = true;
        params.forEach(function (p) { pmap[p.id] = p; });
        cmds.forEach(function (p) { cmap[p.id] = p; });
      });
    },
    // refreshed each time Lightroom connects
    stale: function (s) {
      var c = !!s.connected, old = was;
      was = c;
      return c && (!old || !params.length);
    },
    chip: function (s) { return s.old_plugin ? ["Lightroom: " + (s.installed === "same" ? "restart it to load the new plugin" : "plugin out of date"), "warn", "set"] : !s.connected && s.installed === "missing" ? ["Lightroom: install the plugin", "warn", "set"] : s.connected ? ["Lightroom: " + (s.module || "?") + (s.tool ? " / " + s.tool : ""), "ok"] : ["Lightroom: not connected"]; },
    needs: function (s) {
      var t = "Lightroom Classic", p = "install", r = { button: "Remove the plugin", path: "remove", confirm: "Really remove it?" };
      if (s.connected && s.installed === "missing") {
        return { title: t, text: s.old_plugin ? "Lightroom runs an older plugin from another folder. Replace it there." : "Lightroom runs the plugin from another folder, or until its next start." };
      }
      if (s.installed === "missing") { return { title: t, text: "The helper reaches Lightroom through a plugin. Installing it copies a folder into Lightroom’s own plugin folder; Lightroom loads it the next time it starts.", button: "Install the plugin", path: p, primary: true }; }
      if (s.installed === "different") { return { title: t, text: "The installed plugin isn’t the one that belongs to this helper.", button: "Replace the plugin", path: p, primary: true, also: r }; }
      if (s.installed === "same") { return { title: t, text: s.old_plugin ? "The new plugin is installed. Restart Lightroom to load it." : s.connected ? "The plugin is installed and connected." : "The plugin is installed. Start Lightroom, or restart it if it is already running.", also: r }; }
      return null;
    },
    note: function () { return loaded && !params.length ? "Start Lightroom once to get the list of its settings and commands." : ""; },
    catalogue: function () { return [{ title: "Parameters", items: params }, { title: "Commands", items: cmds }]; },
    settings: [{ key: "lr_actions_need_focus", type: "bool", label: "Lightroom actions only when Lightroom is in front", def: true },
      { key: "lr_show_values", type: "bool", label: "Lightroom shows the value while you adjust it", def: true },
      { key: "lr_denoise_default", type: "num", label: "Lightroom: switch Denoise on at (1 to 100)", def: 20 }],
    template: function (kind) {
      return kind === "ball" ? '{ do = "lr.wheel", zone = "midtone", speed = 1.0 }' : kind === "button" ? "" : '{ do = "lr.param", id = "", per_turn = 1.0 }';
    },

    describe: function (b, kind) {
      var turn = kind === "knob" || kind === "ring";
      switch (b.do) {
        case "lr.param":
          if (turn) { return plab(b.id); }
          return (b.reset || (!b.value && /^local_/.test(b.id))) ? "Reset " + lower(plab(b.id)) : plab(b.id) + " = " + api.num(b.value === undefined ? 0 : b.value);
        case "lr.xy": return [b.x, b.y].filter(Boolean).map(plab).join(" / ") || "Two settings";
        case "lr.wheel": return api.cap(b.zone || "midtone") + (b.zone === "global" ? "" : "s") + " wheel";
        case "lr.cmd": return turn ? clab(b.cw) + " / " + clab(b.ccw) : clab(b.id);
      }
      return "";
    },
    shorten: function (t) {
      if (has(SHORT, t)) { return SHORT[t]; }
      var s = t.replace(/^Mask (?!overlay)(?=[a-z])/, "").replace(/^Reset mask (?=[a-z])/, "Reset ").replace(/^(Post-crop|Profile) /, "")
        .replace(/^(New|Add to|Subtract from) mask: /, function (m, v) { return { "New": "New ", "Add to": "Add ", "Subtract from": "Subtract " }[v]; });
      return s === t ? "" : s;
    },
    shortcut: function (keys) { return has(KEYS, keys) ? KEYS[keys] : ""; },
    settingRef: function (b, kind) {
      if (b.do !== "lr.param") { return null; }
      return { id: b.id, role: kind !== "button" ? "turn" : isReset(b) ? "reset" : "set", value: b.value };
    },
    settingName: plab,
    resetOf: resetOf,
    speed: function (b) { return b.do === "lr.param" ? perTurn(b.id) : 0; },
    unit: function (b) { return /Exposure/.test(b.id) ? "EV" : b.id === "Temperature" ? "K" : /Angle|Hue$/.test(b.id) ? "°" : "units"; },

    fields: function (kind, what) {
      var turn = kind === "knob" || kind === "ring";
      switch (what) {
        case "lr.param":
          return turn
            ? [{ n: "id", t: "pick", items: params, l: "Setting", req: 1, adv: 1 }, { n: "per_turn", t: "num", l: "One full turn", req: 1, def: 100, speed: 1 }]
            : [{ n: "id", t: "pick", items: params, l: "Setting", req: 1, adv: 1 }, { n: "reset", t: "bool", l: "Reset to default" }, { n: "value", t: "num", l: "Or set it to" }];
        case "lr.xy":
          return [{ n: "x", t: "pick", items: params, l: "Left / right" }, { n: "y", t: "pick", items: params, l: "Up / down" },
            { n: "x_per_unit", t: "num", l: "Speed left / right", ph: "0.1", help: "Setting units per ball step." },
            { n: "y_per_unit", t: "num", l: "Speed up / down", ph: "0.1", help: "Setting units per ball step." },
            { n: "per_unit", t: "num", l: "Both (fallback)", ph: "0.1", adv: 1, help: "Used for an axis without its own value." },
            { n: "x_dir", t: "num", l: "Direction L/R°", ph: "0", adv: 1, help: "Roll direction that raises X: 0 = right, 90 = up. Temp like a colour wheel: 135." },
            { n: "y_dir", t: "num", l: "Direction U/D°", ph: "90", adv: 1, help: "Roll direction that raises Y. Tint like a colour wheel: 45." }];
        case "lr.wheel":
          return [{ n: "zone", t: "sel", l: "Zone", opts: ["shadow", "midtone", "highlight", "global"], def: "midtone", adv: 1 }, { n: "speed", t: "num", l: "Speed", ph: "1" }];
        case "lr.cmd":
          return turn
            ? [{ n: "cw", t: "pick", items: cmds, l: "Clockwise" }, { n: "ccw", t: "pick", items: cmds, l: "Anticlockwise" },
              { n: "per", t: "num", l: "Per step", ph: "default", adv: 1, help: "Raw counts per command (optional)." }]
            : [{ n: "id", t: "pick", items: cmds, l: "Command", req: 1, adv: 1 }];
      }
      return [];
    },
    check: function (b, kind) {
      if (b.do === "lr.param" && kind === "button") {
        if (b.reset && b.value !== undefined) { delete b.value; }
        if (!b.reset && b.value === undefined) { return "Choose Reset or enter a value."; }
      }
      if (b.do === "lr.xy" && !b.x && !b.y) { return "Choose at least one of Left / right and Up / down."; }
      if (b.do === "lr.cmd" && kind !== "button" && !b.cw && !b.ccw) { return "Give at least one of Clockwise and Anticlockwise."; }
      return "";
    },

    groups: ["Crop", "White balance", "Tone", "Presence", "Tone curve", "Color mixer", "Color grading", "Detail", "Lens corrections", "Transform", "Effects", "Calibration", "Masking", "Remove", PHOTO, VIEWS, EDIT],
    placeGroups: { masking: "Masking", crop: "Crop", dust: "Remove", library: PHOTO },
    ownPlaceGroups: ["Masking", "Remove"],
    entries: function (add) {
      var B = ["button"], BA = ["ball"], KR = api.knobAndRing, fix = api.fixed, cap = api.cap;
      ["shadow", "midtone", "highlight", "global"].forEach(function (z) {
        add("Color grading", cap(z) + (z === "global" ? "" : "s") + " wheel", BA, fix({ do: "lr.wheel", zone: z, speed: 1 }), function (b) { return b.do === "lr.wheel" && (b.zone || "midtone") === z; });
      });
      params.forEach(function (p) {
        var lb = p.group === "Mask" ? cap((p.label || p.id).replace(/^Mask /, "")) : (p.label || p.id);
        var brush = /^(remove|mask)\./.test(p.id);     // no default to go back to: not for keys
        add(groupOf(p), lb, brush ? KR.concat(BA) : api.all, function (kind, name) {
          if (kind === "button") { return resetOf(p.id); }
          if (kind !== "ball") { return { do: "lr.param", id: p.id, per_turn: perTurn(p.id) }; }
          var own = api.own("ball." + name);     // fills the free direction of a ball that has one setting
          if (own && own.do === "lr.xy" && own.x && !own.y && own.x !== p.id) {
            var b = api.copy(own);
            b.y = p.id; b.y_per_unit = perTurn(p.id) / 1000;
            return b;
          }
          return { do: "lr.xy", x: p.id, x_per_unit: perTurn(p.id) / 1000 };
        }, function (b, kind) { return kind === "ball" ? b.do === "lr.xy" && (b.x === p.id || b.y === p.id) : b.do === "lr.param" && b.id === p.id; }, { sid: p.id, resets: !brush });
      });
      cmds.forEach(function (c) {
        add(groupOf(c), (c.label || c.id).replace(/^White balance: /, "").replace(/^Color grading: /, "View: "), B, fix({ do: "lr.cmd", id: c.id }), function (b) { return b.do === "lr.cmd" && b.id === c.id; });
      });
      [["Next / previous photo", "nextPhoto", "previousPhoto", PHOTO], ["Rating up / down", "increaseRating", "decreaseRating", PHOTO], ["Zoom in / out", "zoomIn", "zoomOut", VIEWS],
       ["Redo / undo", "redo", "undo", EDIT], ["Next / previous mask", "maskNext", "maskPrev", "Masking"], ["Next / previous preset", "wbNext", "wbPrev", "White balance"]].forEach(function (x) {
        add(x[3], x[0], KR, function (kind) { return { do: "lr.cmd", cw: x[1], ccw: x[2], per: api.perStep(kind) }; }, function (b) { return b.do === "lr.cmd" && b.cw === x[1]; });
      });
      add(VIEWS, "Two commands, one per direction", KR, function (kind) { return { do: "lr.cmd", cw: "nextPhoto", ccw: "previousPhoto", per: api.perStep(kind) }; }, function () { return false; });
      [["Copy settings", "ctrl+shift+c", EDIT], ["Paste settings", "ctrl+shift+v", EDIT], ["Before/after", "backslash", VIEWS], ["Full-screen preview", "f", VIEWS]].forEach(function (x) {
        add(x[2], x[0], B, fix({ do: "key", keys: x[1] }), function (b) { return b.do === "key" && !b.hold && b.keys === x[1]; });
      });
    },

    lights: function () {
      function photo(rule, label) { return [rule, label, "The photo " + label.charAt(0).toLowerCase() + label.slice(1)]; }
      return [
        ["The photo", [photo("lr.flag == pick", "Is flagged"), photo("lr.flag == reject", "Is rejected"), photo("lr.flag != none", "Is flagged or rejected")]
          .concat([1, 2, 3, 4, 5].map(function (n) { return photo("lr.rating >= " + n, "Has " + n + (n < 5 ? " or more stars" : " stars")); }))
          .concat([photo("lr.label != none", "Has a colour label")], COLORS.map(function (c) { return photo("lr.label == " + c, "Is labelled " + c); }))],
        ["The tool in use", Object.keys(TOOLS).map(function (t) { return ["lr.tool == " + t, TOOLS[t] + " is open"]; })],
        ["AI Denoise", [["lr.denoise", "Is on", "Denoise is on"]]],
        ["The module", MODULES.map(function (m) { return ["lr.module == " + m, api.cap(m), api.cap(m) + " module"]; })],
        ["A setting", [["?lr.set", "Is changed from 0…"]]]
      ];
    },
    asks: [{ v: "?lr.set", match: /^lr\.(\w+) != 0$/, label: "Setting", ph: "Exposure2012", items: function () { return params; },
      words: function (id) { return plab(id) + " is changed from 0"; }, rule: function (v) { return /^\w+$/.test(v) ? "lr." + v + " != 0" : ""; } }],
    operands: "lr.flag, lr.label, lr.rating, lr.module, lr.tool, lr.photo, lr.<setting>",
    ruleExample: "lr.rating >= 3 and not layer.fine"
  });
})();
