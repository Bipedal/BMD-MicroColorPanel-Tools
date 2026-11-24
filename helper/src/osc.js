// OSC in the mapping editor.
(function () {
  "use strict";
  var api, tree = [], map = {}, rev = -1, st = {};
  function has(a) { return Object.prototype.hasOwnProperty.call(map, a); }
  function label(a) { return (has(a) && map[a].label) || String(a || "").replace(/^\//, ""); }
  function span(a) { return has(a) && map[a].min !== null && map[a].max !== null ? map[a].max - map[a].min : 1; }
  function round(x) { return Math.round(x * 1e6) / 1e6; }
  function isBool(n) { return /^[TF]/.test(n.type); }
  function isNum(n) { return /^[fidh]/.test(n.type); }
  var TYPE = { n: "type", t: "sel", l: "Type", adv: 1, opts: [["", "As the program says, else decimal"], ["float", "Decimal number"], ["int", "Whole number"], ["bool", "True / false"]], def: "" };
  var MIN = { n: "min", t: "num", l: "Lowest", ph: "as the program says, else 0", adv: 1 }, MAX = { n: "max", t: "num", l: "Highest", ph: "as the program says, else 1", adv: 1 };
  var MODE = { n: "mode", t: "sel", l: "Sends", adv: 1, opts: [["absolute", "The new value"], ["relative", "Only the change"]], def: "absolute" };

  MP.register({
    id: "osc", name: "OSC",
    setup: function (a) { api = a; },
    load: function () {
      return api.fetchJson("/api/ext/osc/tree").then(function (t) { tree = t || []; map = {}; tree.forEach(function (n) { map[n.id] = n; }); });
    },
    stale: function (s) { st = s; var old = rev; rev = s.rev; return old !== s.rev; },
    chip: function (s) {
      if (!s.configured) { return null; }
      if (s.error) { return ["OSC: " + s.error, "warn"]; }
      return s.connected ? ["OSC: " + (s.name || "connected") + ", " + s.parameters + " parameters", "ok"] : s.target ? ["OSC: " + s.target] : null;
    },
    note: function () { return st.configured ? "" : "Say where OSC messages go under Settings."; },
    settings: [
      { key: "osc_target", label: "OSC: send to", ph: "127.0.0.1:9000", help: "Address and port of the program that takes OSC messages." },
      { key: "osc_listen", type: "num", label: "OSC: listen on port", ph: "0", help: "For values the program sends back. 0 = it answers to wherever the messages came from." },
      { key: "oscquery", label: "OSCQuery server", ph: "http://127.0.0.1:5678", help: "If the program has one: its parameters, ranges and current values are read from there, and “send to” can stay empty." }],
    catalogue: function () { return [{ title: "OSC", items: tree.map(function (n) { return { id: n.id, label: n.label, group: n.id.split("/")[1] }; }) }]; },

    describe: function (b, kind) {
      switch (b.do) {
        case "osc.value":
          if (kind !== "button") { return label(b.address); }
          return b.mode === "toggle" ? label(b.address) + " on / off" : b.mode === "hold" ? label(b.address) + " (held)" : label(b.address) + " = " + (b.value === undefined ? 1 : b.value);
        case "osc.xy": return [b.x, b.y].filter(Boolean).map(label).join(" / ");
        case "osc.send": return "Send " + b.address;
      }
      return "";
    },
    settingRef: function (b, kind) {
      if (b.do !== "osc.value" || (kind === "button" && b.mode && b.mode !== "set")) { return null; }
      return { id: b.address, role: kind === "button" ? "set" : "turn", value: b.value === undefined ? 1 : b.value };
    },
    settingName: label,
    speed: function (b) { return b.do === "osc.value" && b.mode !== "relative" ? span(b.address) : 0; },
    unit: function (b) { return (has(b.address) && map[b.address].unit) || ""; },
    fields: function (kind, what) {
      var A = { n: "address", t: "text", l: "Address", req: 1, ph: "/path/to/value" };
      if (what === "osc.send") { return [A, { n: "args", t: "text", l: "With", ph: "nothing", help: "Values separated by spaces: 1 0.5 true play 'two words'." }]; }
      if (what === "osc.xy") {
        return [{ n: "x", t: "text", l: "Left / right", ph: "/path" }, { n: "y", t: "text", l: "Up / down", ph: "/path" },
          { n: "per_unit", t: "num", l: "Per ball step", ph: "a thousandth of the range" }, MIN, MAX, MODE, TYPE];
      }
      if (kind === "button") {
        return [A, { n: "value", t: "num", l: "Value", ph: "1" }, { n: "mode", t: "sel", l: "Press", opts: [["set", "Sends the value"], ["hold", "Value while held"], ["toggle", "Value on and off"]], def: "set" },
          { n: "off", t: "num", l: "Off value", ph: "0", adv: 1 }, TYPE];
      }
      return [A, { n: "per_turn", t: "num", l: "One full turn", ph: "the whole range", speed: 1 }, MIN, MAX, MODE, TYPE];
    },
    check: function (b) {
      var bad = ["address", "x", "y"].filter(function (k) { return b[k] !== undefined && !/^\/[^\s#*,?\[\]{}"']+$/.test(b[k]); })[0];
      if (bad) { return "An OSC address starts with / and has no spaces, e.g. /mix/gain."; }
      if (b.do === "osc.xy" && !b.x && !b.y) { return "Give at least one address."; }
      if (b.min !== undefined && b.max !== undefined && b.min >= b.max) { return "Lowest must be below highest."; }
      return "";
    },

    entries: function (add) {
      var B = ["button"], KR = api.knobAndRing, KRB = KR.concat(["ball"]), own = "Any address";
      function is(b, what, a) { return b.do === what && b.address === a; }
      tree.forEach(function (n) {
        var g = n.id.split("/")[1] || "OSC", l = n.label || n.id;
        if (isNum(n)) {
          add(g, l, KRB, function (kind, name) {
            if (kind !== "ball") { return { do: "osc.value", address: n.id, per_turn: round(span(n.id)) }; }
            var o = api.own("ball." + name), b;
            if (o && o.do === "osc.xy" && o.x && !o.y && o.x !== n.id) { b = api.copy(o); b.y = n.id; return b; }
            return { do: "osc.xy", x: n.id };
          }, function (b, kind) { return kind === "ball" ? b.do === "osc.xy" && (b.x === n.id || b.y === n.id) : is(b, "osc.value", n.id); }, { sid: n.id });
          add(g, l + " to a value", B, function () { return { do: "osc.value", address: n.id, value: n.value === null || n.value === undefined ? 1 : round(n.value) }; },
            function (b) { return is(b, "osc.value", n.id); });
        } else if (isBool(n)) {
          add(g, l + " on / off", B, api.fixed({ do: "osc.value", address: n.id, mode: "toggle" }), function (b) { return is(b, "osc.value", n.id); });
        } else {
          add(g, l, B, api.fixed({ do: "osc.send", address: n.id }), function (b) { return is(b, "osc.send", n.id); });
        }
      });
      add(own, "Value", KR, api.fixed({ do: "osc.value", address: "/value" }), function (b) { return b.do === "osc.value" && !has(b.address); });
      add(own, "Two values", ["ball"], api.fixed({ do: "osc.xy", x: "/x", y: "/y" }), function (b) { return b.do === "osc.xy" && !has(b.x) && !has(b.y); });
      add(own, "Value on / off", B, api.fixed({ do: "osc.value", address: "/value", mode: "toggle" }), function (b) { return b.do === "osc.value" && !has(b.address); });
      add(own, "Message", B, api.fixed({ do: "osc.send", address: "/message" }), function (b) { return b.do === "osc.send" && !has(b.address); });
    },

    lights: function () { return st.configured ? [["OSC", [["?osc", "A value is on…"]]]] : []; },
    asks: [{ v: "?osc", match: /^osc\.(\/\S+)$/, label: "OSC address", ph: "/mix/mute",
      items: function () { return tree.map(function (n) { return { id: n.id, label: n.label || n.id, group: n.id.split("/")[1] }; }); },
      words: function (a) { return label(a) + " is on"; }, rule: function (v) { return /^\/[^\s#*,?\[\]{}"'()=!<>]+$/.test(v) ? "osc." + v : ""; } }],
    operands: "osc./path"
  });
})();
