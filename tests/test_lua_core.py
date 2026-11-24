"""Offline tests for lightroom/MicroPanel.lrplugin/Core.lua (needs lupa)."""
import json
import math
import os
import sys

try:
    from lupa.lua51 import LuaRuntime
except ImportError:
    from lupa import LuaRuntime

PLUGIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lightroom", "MicroPanel.lrplugin")
PLUGIN = os.path.abspath(PLUGIN).replace("\\", "/")

lua = LuaRuntime(unpack_returned_tuples=True)
lua.execute("package.path = '%s/?.lua;' .. package.path" % PLUGIN)

# Lua side: fake Lightroom, and helpers that hand plain data back to Python.
lua.execute(r"""
function import(name)  -- Catalog.lua only calls this inside run functions
  return setmetatable({}, { __index = function() return function() end end })
end
Core = require 'Core'
Catalog = require 'Catalog'

fake = {}
function fake.reset()
  fake.module, fake.photo, fake.tool = 'develop', true, 'loupe'
  fake.values = { Exposure2012 = 0, Contrast2012 = 0,
    SplitToningShadowHue = 0, SplitToningShadowSaturation = 0,
    ColorGradeMidtoneHue = 0, ColorGradeMidtoneSat = 0,
    SplitToningHighlightHue = 0, SplitToningHighlightSaturation = 0,
    ColorGradeGlobalHue = 0, ColorGradeGlobalSat = 0 }
  fake.ranges = { Exposure2012 = {-5, 5}, Contrast2012 = {-100, 100} }
  fake.sets, fake.resets, fake.switches, fake.out = {}, {}, 0, {}
  fake.clock = 100
  fake.failSet = false
  fake.cmdRan = 0
end
fake.reset()

sdk = {
  getModule = function() return fake.module end,
  switchToDevelop = function() fake.switches = fake.switches + 1; fake.module = 'develop' end,
  hasPhoto = function() return fake.photo end,
  getValue = function(id) return fake.values[id] end,
  setValue = function(id, v)
    if fake.failSet then error('boom') end
    fake.values[id] = v; fake.sets[#fake.sets + 1] = id
  end,
  getRange = function(id)
    local r = fake.ranges[id]
    if r then return r[1], r[2] end
    if id:find('Hue') then return 0, 360 end
    return 0, 100
  end,
  resetToDefault = function(id) fake.resets[#fake.resets + 1] = id; fake.values[id] = 0 end,
  getTool = function() return fake.tool end,
  denoise = { state = function() return fake.enhance end, set = function(on, a) fake.enhanceSet = tostring(on) .. ' ' .. tostring(a) end, amount = function(v) fake.dn = v; fake.ndn = (fake.ndn or 0) + 1 end },
  remove = { get = function() return fake.rem end, set = function(t)
    fake.nrem = (fake.nrem or 0) + 1
    if fake.remFail then return false, 'not now' end
    for k, v in pairs(t) do fake.rem[k] = v end
    return true
  end },
  stopTracking = function() fake.stops = (fake.stops or 0) + 1; fake.setsAtStop = #fake.sets end,
  bezel = function(t) fake.bezel = t; fake.nbezel = (fake.nbezel or 0) + 1 end,
  now = function() return fake.clock end,
  lrVersion = function() return '16.0' end,
}
function newCore()
  fake.out = {}
  return Core.new(sdk, function(line) fake.out[#fake.out + 1] = line end)
end
function nsets() return #fake.sets end
function nout() return #fake.out end
function outline(i) return fake.out[i] end
function getv(id) return fake.values[id] end
function setv(id, v) fake.values[id] = v end
function setfake(k, v) fake[k] = v end
function getfake(k) return fake[k] end
function ncat() return #Catalog.params + #Catalog.cmds end
function addcmd(id, fn) Catalog.cmds[#Catalog.cmds + 1] = { id = id, label = id, group = 'T', run = fn } end
""")

G = lua.globals()
passed = failed = 0


def check(name, cond, extra=""):
    global passed, failed
    if cond:
        passed += 1
        print("ok   ", name)
    else:
        failed += 1
        print("FAIL ", name, extra)


def new():
    G.fake.reset()
    c = G.newCore()
    c.settings.switch_to_develop = True
    return c


def out_all():
    return [json.loads(G.outline(i)) for i in range(1, G.nout() + 1)]


def out():
    """Everything except the LED state messages (tested separately)."""
    return [m for m in out_all() if m.get("t") != "state"]


def feed(c, obj):
    c.onLine(c, obj if isinstance(obj, str) else json.dumps(obj))


def step(c):
    c.step(c)


def v(id_):
    return G.getv(id_)


# hello
c = new()
feed(c, {"t": "hello", "v": 1, "helper": "0.1.0"})
check("no greeting from the socket callback", len(out()) == 0)
step(c)
o = out()
check("hello reply first", o[0] == {"t": "hello", "v": 1, "plugin": "0.3.5", "lr": "16.0"}, o[0])
check("context after hello", o[1]["t"] == "context" and o[1]["module"] == "develop" and o[1]["photo"] is True, o[1])
cat = [m for m in o if m["t"] == "catalog"]
end = o[-1]
check("catalog_end last, n matches", end["t"] == "catalog_end" and end["n"] == len(cat) == G.ncat(), end)
check("catalog has both kinds", {m["kind"] for m in cat} == {"param", "cmd"})
check("catalog item fields", all(set(m) == {"t", "kind", "id", "label", "group"} for m in cat))
check("hello + context + catalogue line count", len(o) == 2 + len(cat) + 1)

c = new()
feed(c, {"t": "catalog?"})
o = out()
check("catalog? resends", o[-1]["t"] == "catalog_end" and len(o) == o[-1]["n"] + 1)

# ping
c = new()
feed(c, {"t": "ping"})
check("ping -> pong", out() == [{"t": "pong"}])

# params
c = new()
feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.5})
feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.25})
check("param not applied before step", v("Exposure2012") == 0)
step(c)
check("param deltas summed", abs(v("Exposure2012") - 0.75) < 1e-9)
check("applied with one setValue", G.nsets() == 1)
step(c)
check("nothing re-applied on next step", G.nsets() == 1)

# letting go of Alt ends the adjustment at once, after what is still to be applied
c = new(); G.fake.stops = 0
feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.1}); feed(c, {"t": "done"})
step(c)
check("not ended before it is applied", G.fake.stops == 0)
step(c); step(c)
check("ended once, after the change", G.fake.stops == 1 and G.fake.setsAtStop == G.nsets())

# a reset wins over movement just before it
c = new()
feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.5}); feed(c, {"t": "reset", "id": "Exposure2012"}); step(c)
check("movement before a reset is dropped", v("Exposure2012") == 0, v("Exposure2012"))
feed(c, {"t": "reset", "id": "Exposure2012"}); feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.5}); step(c)
check("movement after a reset counts", v("Exposure2012") == 0.5)

# ranges of settings getRange doesn't know
c = new()
G.setv("GlowDirection", 100)
feed(c, {"t": "param", "id": "GlowDirection", "delta": 60}); step(c)
check("own range", v("GlowDirection") == 160, v("GlowDirection"))
feed(c, {"t": "param", "id": "GlowDirection", "delta": 60}); step(c)
check("and its end", v("GlowDirection") == 180)

# text from the helper
c = new()
feed(c, {"t": "show", "text": "Brush size  12.3"}); step(c)
check("not shown when values are off", G.fake.bezel != "Brush size  12.3")
feed(c, {"t": "options", "show_values": True}); feed(c, {"t": "show", "text": "Brush size  12.4"}); G.fake.clock += 1; step(c)
check("text from the helper", G.fake.bezel == "Brush size  12.4", G.fake.bezel)

# the Remove tool's brush
lua.execute("fake.rem = { brushSize = 30, brushFeather = 10 }; fake.nrem = 0")
c = new()
feed(c, {"t": "options", "show_values": True})
for _ in range(10):
    feed(c, {"t": "param", "id": "remove.size", "delta": 0.25}); step(c)
check("brush size follows slow turns, in whole numbers", G.fake.rem.brushSize == 33, G.fake.rem.brushSize)
check("only sent when it changes", G.fake.nrem <= 4, G.fake.nrem)
G.fake.clock += 1; step(c)
check("and is shown", G.fake.bezel == "Remove brush size  33", G.fake.bezel)
G.fake.clock += 5; G.fake.rem.brushSize = 50
feed(c, {"t": "param", "id": "remove.size", "delta": 2}); step(c)
check("goes on from what Lightroom has", G.fake.rem.brushSize == 52)
feed(c, {"t": "param", "id": "remove.size", "delta": -500}); step(c)
check("size stops at 1", G.fake.rem.brushSize == 1)
feed(c, {"t": "param", "id": "remove.feather", "delta": -500}); step(c)
check("feather stops at 0", G.fake.rem.brushFeather == 0)
feed(c, {"t": "set", "id": "remove.size", "value": 40}); step(c)
check("set to a value", G.fake.rem.brushSize == 40)
G.fake.remFail = True
feed(c, {"t": "param", "id": "remove.feather", "delta": 20}); step(c)
check("a refusal is reported", any(m["t"] == "error" and "not now" in m["msg"] for m in out()))
G.fake.remFail = False
check("no plain setValue for them", not any(str(x).startswith("remove.") for x in list(G.fake.sets.values())))

# AI Denoise (state as Lightroom 16 reports it)
def enhance(on, amount, enabled=True):
    lua.execute("fake.enhance = { denoiseAmount = %s, denoiseEnabled = %s, denoiseState = %s, rawDetailsEnabled = true, rawDetailsState = false, enhanceNeedsUpdate = false, denoiseInfoText = 'x' }"
                % (amount, str(enabled).lower(), str(on).lower()))
enhance(True, 40)
c = new(); G.fake.ndn = 0
feed(c, {"t": "options", "show_values": True, "denoise_default": 25})
feed(c, {"t": "param", "id": "denoise.amount", "delta": 5}); step(c)
feed(c, {"t": "param", "id": "denoise.amount", "delta": 5.4}); step(c); step(c)
check("denoise amount is shown while turning", str(G.fake.bezel).startswith("Denoise amount  "), G.fake.bezel)
check("but not sent yet", G.fake.ndn == 0)
G.fake.clock += 0.5; step(c)
check("sent once, as a whole number", G.fake.ndn == 1 and G.fake.dn == 50, G.fake.dn)
feed(c, {"t": "param", "id": "denoise.amount", "delta": 900}); step(c); G.fake.clock += 0.5; step(c)
check("at most 100", G.fake.dn == 100)
feed(c, {"t": "reset", "id": "denoise.amount"}); step(c); G.fake.clock += 0.5; step(c)
check("reset = the default", G.fake.dn == 25)
feed(c, {"t": "cmd", "id": "denoiseToggle"}); step(c)
check("toggle off", G.fake.enhanceSet == "false nil" and G.fake.bezel == "Denoise off", G.fake.enhanceSet)
enhance(False, 50)
feed(c, {"t": "cmd", "id": "denoiseToggle"}); step(c)
check("toggle on, at the default amount", G.fake.enhanceSet == "true 25" and G.fake.bezel == "Denoise on  25", G.fake.enhanceSet)
n1 = G.fake.ndn
feed(c, {"t": "param", "id": "denoise.amount", "delta": 5}); step(c); G.fake.clock += 0.5; step(c)
check("no amount while it is off", G.fake.ndn == n1 and G.fake.bezel == "Denoise is off")
enhance(False, 50, False); G.fake.enhanceSet = None
feed(c, {"t": "cmd", "id": "denoiseToggle"}); step(c)
check("not available: says so", G.fake.enhanceSet is None and "not available" in G.fake.bezel)
lua.execute("fake.enhance = nil")
feed(c, {"t": "cmd", "id": "denoiseToggle"}); step(c)
check("an unknown answer is reported", G.fake.enhanceSet is None and any(m["t"] == "error" and "denoiseState" in m["msg"] for m in out()))
enhance(True, 30)
c = new()
feed(c, {"t": "watch", "ids": ["denoise"]}); c.pollContext(c, True)
s = [m for m in out_all() if m["t"] == "state"]
check("its state for lights", s and s[-1]["values"].get("denoise") == 1, s)

# value overlay
c = new(); G.fake.bezel = None; G.fake.nbezel = 0
feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.5}); step(c); step(c)
check("no overlay unless asked for", G.fake.bezel is None)
feed(c, {"t": "options", "show_values": True})
feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.25}); step(c); step(c)
check("overlay shows label and value", G.fake.bezel == "Exposure  +0.75", G.fake.bezel)
for _ in range(5):
    feed(c, {"t": "param", "id": "Exposure2012", "delta": 0.01}); step(c)
check("not more often than every 0.1 s", G.fake.nbezel == 1, G.fake.nbezel)
G.fake.clock += 0.2; step(c)
check("the last value is always shown", G.fake.bezel == "Exposure  +0.80", G.fake.bezel)
feed(c, {"t": "reset", "id": "Exposure2012"}); G.fake.clock += 0.2; step(c); step(c)
check("reset shows the new value", G.fake.bezel == "Exposure  0.00", G.fake.bezel)
n1 = G.fake.nbezel; G.fake.clock += 5; step(c)
check("nothing without a change", G.fake.nbezel == n1)

c = new()
feed(c, {"t": "param", "id": "Exposure2012", "delta": 99})
step(c)
check("clamp at range max", v("Exposure2012") == 5)
feed(c, {"t": "param", "id": "Exposure2012", "delta": 1})
n0 = G.nsets()
step(c)
check("no setValue when unchanged at limit", G.nsets() == n0)
feed(c, {"t": "param", "id": "Exposure2012", "delta": -99})
step(c)
check("clamp at range min", v("Exposure2012") == -5)

# hue wrap
c = new()
G.setv("SplitToningShadowHue", 350)
feed(c, {"t": "param", "id": "SplitToningShadowHue", "delta": 20})
step(c)
check("hue wraps 350+20 -> 10", abs(v("SplitToningShadowHue") - 10) < 1e-9, v("SplitToningShadowHue"))
feed(c, {"t": "param", "id": "SplitToningShadowHue", "delta": -30})
step(c)
check("hue wraps 10-30 -> 340", abs(v("SplitToningShadowHue") - 340) < 1e-9, v("SplitToningShadowHue"))

# set / reset
c = new()
feed(c, {"t": "set", "id": "Contrast2012", "value": 12})
feed(c, {"t": "reset", "id": "Exposure2012"})
G.setv("Exposure2012", 3)
step(c)
check("set applied", v("Contrast2012") == 12)
check("reset applied", v("Exposure2012") == 0)

# module switching
c = new()
G.setfake("module", "library")
feed(c, {"t": "param", "id": "Exposure2012", "delta": 1})
step(c)
check("first step switches to develop", G.getfake("switches") == 1 and v("Exposure2012") == 0)
step(c)
check("next step applies", v("Exposure2012") == 1)

c = new()
c.settings.switch_to_develop = False
G.setfake("module", "library")
feed(c, {"t": "param", "id": "Exposure2012", "delta": 1})
step(c)
G.setfake("module", "develop")
step(c)
check("switch_to_develop off drops pending", G.getfake("switches") == 0 and v("Exposure2012") == 0)

c = new()
G.setfake("photo", False)
feed(c, {"t": "param", "id": "Exposure2012", "delta": 1})
step(c)
G.setfake("photo", True)
step(c)
check("no photo drops pending", v("Exposure2012") == 0)

# wheel
c = new()
feed(c, {"t": "wheel", "zone": "shadow", "dx": 10, "dy": 0})
step(c)
check("wheel: sat 0 + dx10 -> sat 10, hue 0",
      abs(v("SplitToningShadowSaturation") - 10) < 1e-9 and v("SplitToningShadowHue") == 0)
feed(c, {"t": "wheel", "zone": "shadow", "dx": 0, "dy": 10})
step(c)
check("wheel: dy10 from (10,0) -> hue 45 sat 14.14",
      abs(v("SplitToningShadowHue") - 45) < 1e-6 and abs(v("SplitToningShadowSaturation") - 10 * math.sqrt(2)) < 1e-6,
      (v("SplitToningShadowHue"), v("SplitToningShadowSaturation")))
feed(c, {"t": "wheel", "zone": "shadow", "dx": 5, "dy": 0})
feed(c, {"t": "wheel", "zone": "shadow", "dx": 5, "dy": 0})
n0 = G.nsets()
step(c)
check("wheel deltas summed in one step", G.nsets() - n0 <= 2)
feed(c, {"t": "wheel", "zone": "shadow", "dx": 1000, "dy": 0})
step(c)
check("wheel sat clamps at 100", v("SplitToningShadowSaturation") == 100)

c = new()
G.setv("ColorGradeMidtoneHue", 90)
G.setv("ColorGradeMidtoneSat", 10)
feed(c, {"t": "wheel", "zone": "midtone", "dx": 0, "dy": -10})
step(c)
check("wheel |p|=0 keeps hue, sat 0",
      v("ColorGradeMidtoneSat") < 1e-9 and v("ColorGradeMidtoneHue") == 90,
      (v("ColorGradeMidtoneHue"), v("ColorGradeMidtoneSat")))

c = new()
feed(c, {"t": "wheel", "zone": "highlight", "dx": 0, "dy": 10})
step(c)
check("wheel highlight ids", abs(v("SplitToningHighlightHue") - 90) < 1e-6
      and abs(v("SplitToningHighlightSaturation") - 10) < 1e-9)
feed(c, {"t": "wheel", "zone": "global", "dx": -10, "dy": 0})
step(c)
check("wheel global ids", abs(v("ColorGradeGlobalHue") - 180) < 1e-6
      and abs(v("ColorGradeGlobalSat") - 10) < 1e-9)

c = new()
c.settings.wheel_dir = -1
feed(c, {"t": "wheel", "zone": "shadow", "dx": 0, "dy": 10})
step(c)
check("wheel_dir -1 mirrors hue (+y -> 270)", abs(v("SplitToningShadowHue") - 270) < 1e-6,
      v("SplitToningShadowHue"))
c = new()
c.settings.wheel_offset = 30
feed(c, {"t": "wheel", "zone": "shadow", "dx": 10, "dy": 0})
step(c)
check("wheel_offset 30: +x -> hue 330", abs(v("SplitToningShadowHue") - 330) < 1e-6,
      v("SplitToningShadowHue"))

c = new()
feed(c, {"t": "wheel", "zone": "bogus", "dx": 1, "dy": 1})
check("wheel unknown zone -> error", out()[-1]["t"] == "error")

# commands
c = new()
feed(c, {"t": "cmd", "id": "nope"})
o = out()
check("unknown cmd -> error", len(o) == 1 and o[0]["t"] == "error" and "nope" in o[0]["msg"], o)

G.addcmd("testCmd", lua.eval("function() fake.cmdRan = fake.cmdRan + 1 end"))
c = new()
feed(c, {"t": "cmd", "id": "testCmd"})
check("cmd waits for the task loop", G.getfake("cmdRan") == 0)
step(c)
check("cmd runs stub", G.getfake("cmdRan") == 1 and G.nout() == 0)
G.addcmd("badCmd", lua.eval("function() error('kaput') end"))
c = new()
feed(c, {"t": "cmd", "id": "badCmd"})
step(c)
o = out()
check("failing cmd -> error, no throw", len(o) == 1 and o[0]["t"] == "error" and "badCmd" in o[0]["msg"], o)

# bad input
c = new()
ok = True
for bad in ["{not json", "", "[1,2]", '"str"', "5", '{"t":5}', '{"x":1}']:
    try:
        feed(c, bad)
    except Exception as e:  # noqa
        ok = False
check("bad JSON never throws", ok)
o = out()
check("bad JSON -> error messages", len(o) == 7 and all(m["t"] == "error" for m in o), o)
c = new()
feed(c, {"t": "wat"})
check("unknown type -> error", out()[0]["t"] == "error")
c = new()
feed(c, {"t": "param", "id": "Exposure2012"})
check("param without delta -> error", out()[0]["t"] == "error")

# setValue failure: error rate limiting
c = new()
G.setfake("failSet", True)
for _ in range(3):
    feed(c, {"t": "param", "id": "Exposure2012", "delta": 1})
    step(c)
o = out()
check("setValue failure -> one error per id per second", len(o) == 1 and o[0]["t"] == "error", o)
G.setfake("clock", 102)
feed(c, {"t": "param", "id": "Exposure2012", "delta": 1})
step(c)
check("error again after 1 s", len(out()) == 2)

# context
c = new()
c.pollContext(c)
c.pollContext(c)
o = out()
check("context sent once while unchanged", len(o) == 1 and o[0] == {"t": "context", "module": "develop", "tool": "loupe", "photo": True}, o)
G.setfake("tool", "masking")
c.pollContext(c)
check("context re-sent on tool change", len(out()) == 2 and out()[1]["tool"] == "masking")
G.setfake("module", "library")
c.pollContext(c)
o = out()
check("context outside develop has no tool", len(o) == 3 and o[2]["module"] == "library" and "tool" not in o[2], o[-1])
G.setfake("photo", False)
c.pollContext(c)
check("context re-sent on photo change", len(out()) == 4 and out()[3]["photo"] is False)

# hello before the send socket is connected: greeting waits for onSendConnected
c = new()
ready = {"v": False}
c.sdk.sendReady = lambda: ready["v"]
feed(c, {"t": "hello", "v": 1})
check("hello while send socket down: no output yet", len(out()) == 0, out())
ready["v"] = True
c.onSendConnected(c)
step(c)
o = out()
check("greeting sent on send connect", len(o) > 3 and o[0]["t"] == "hello" and o[-1]["t"] == "catalog_end", o[:2])
c.onSendConnected(c)
step(c)
check("no second greeting without a new hello", len(out()) == len(o))

# LED state: flag / rating / label / colour-grading view / watched values, sent on change only
c = new()
st = {"flag": "pick", "rating": 3, "label": "red", "cg": "3-way"}
c.sdk.getFlag = lambda: st["flag"]
c.sdk.getRating = lambda: st["rating"]
c.sdk.getLabel = lambda: st["label"]
c.sdk.getCgView = lambda: st["cg"]
feed(c, {"t": "watch", "ids": ["Exposure2012"]})
c.pollContext(c)
s = [m for m in out_all() if m["t"] == "state"]
check("state sent with flag/rating/label", len(s) == 1 and s[0]["flag"] == "pick" and s[0]["rating"] == 3 and s[0]["label"] == "red", s)
check("state has cg view and watched value in develop", s and s[0].get("cg_view") == "3-way" and "Exposure2012" in s[0].get("values", {}), s)
c.pollContext(c)
check("state not re-sent while unchanged", len([m for m in out_all() if m["t"] == "state"]) == 1)
st["flag"] = "reject"
c.pollContext(c)
s = [m for m in out_all() if m["t"] == "state"]
check("state re-sent on flag change", len(s) == 2 and s[-1]["flag"] == "reject", s)
G.setv("Exposure2012", 1.23456)
c.pollContext(c)
s = [m for m in out_all() if m["t"] == "state"]
check("watched value change re-sends, rounded", len(s) == 3 and s[-1]["values"]["Exposure2012"] == 1.235, s[-1:])

# virtual crop params: move, clamp to the photo, scale keeping aspect
c = new()
for k, val in (("CropLeft", 0.2), ("CropRight", 0.6), ("CropTop", 0.2), ("CropBottom", 0.5)):
    G.setv(k, val)
feed(c, {"t": "param", "id": "crop.x", "delta": 0.1})
feed(c, {"t": "param", "id": "crop.y", "delta": 0.05})   # +y = up
step(c)
check("crop moves right and up", abs(v("CropLeft") - 0.3) < 1e-9 and abs(v("CropRight") - 0.7) < 1e-9
      and abs(v("CropTop") - 0.15) < 1e-9 and abs(v("CropBottom") - 0.45) < 1e-9,
      [v(k) for k in ("CropLeft", "CropRight", "CropTop", "CropBottom")])
feed(c, {"t": "param", "id": "crop.x", "delta": 5})
step(c)
check("crop stops at the edge, size kept", abs(v("CropRight") - 1) < 1e-9 and abs(v("CropLeft") - 0.6) < 1e-9)
feed(c, {"t": "param", "id": "crop.size", "delta": math.log(2)})
step(c)
w, h = v("CropRight") - v("CropLeft"), v("CropBottom") - v("CropTop")
check("crop scales x2 keeping aspect, inside the photo", abs(w - 0.8) < 1e-9 and abs(h - 0.6) < 1e-9
      and v("CropLeft") >= 0 and v("CropRight") <= 1, (w, h))
feed(c, {"t": "param", "id": "crop.size", "delta": 10})
step(c)
w, h = v("CropRight") - v("CropLeft"), v("CropBottom") - v("CropTop")
check("crop size capped at the photo", abs(w - 1) < 1e-9 and abs(h - 0.75) < 1e-9, (w, h))
check("no crop.* setValue leaked", all(not str(s).startswith("crop.") for s in list(G.fake.sets.values())))

# crop on a photo shown rotated (orientation DA = 90 deg clockwise): screen right = up in the crop
lua.execute("sdk.getOrientation = function() return fake.orientation end")
G.setfake("orientation", "DA")
c = new()
for k, val in (("CropLeft", 0.2), ("CropRight", 0.6), ("CropTop", 0.3), ("CropBottom", 0.6)):
    G.setv(k, val)
feed(c, {"t": "param", "id": "crop.x", "delta": 0.1})
step(c)
check("rotated photo: screen right moves the crop's top/bottom", abs(v("CropLeft") - 0.2) < 1e-9
      and abs(v("CropTop") - 0.2) < 1e-9 and abs(v("CropBottom") - 0.5) < 1e-9,
      [v(k) for k in ("CropLeft", "CropRight", "CropTop", "CropBottom")])
feed(c, {"t": "param", "id": "crop.y", "delta": 0.1})   # screen up
step(c)
check("rotated photo: screen up moves the crop's left/right", abs(v("CropLeft") - 0.1) < 1e-9
      and abs(v("CropRight") - 0.5) < 1e-9,
      [v(k) for k in ("CropLeft", "CropRight", "CropTop", "CropBottom")])
G.setv("CropLeft", 0.0)   # Lightroom's value lags: the next move uses the plugin's own copy
feed(c, {"t": "param", "id": "crop.y", "delta": 0.05})
step(c)
check("moving crop works from its own copy, not lagging reads", abs(v("CropLeft") - 0.05) < 1e-9, v("CropLeft"))
check("screenAxes for normal orientation", list(G.Core.screenAxes("AB")[0].values()) == [1, 0])
lua.execute("sdk.getOrientation = nil")

# probe
c = new()
feed(c, {"t": "probe", "ids": ["Exposure2012", "Nope"]})
step(c)
p = [m for m in out() if m["t"] == "probe"]
check("probe reports values and missing ids", p and p[0]["results"]["Exposure2012"]["value"] == 0
      and "value" not in p[0]["results"].get("Nope", {}), p)

# syntax checks of the glue files
for name in ("MicroPanelInit.lua", "MicroPanelShutdown.lua", "MicroPanelStatus.lua", "Info.lua", "Core.lua"):
    path = PLUGIN + "/" + name
    with open(path, "rb") as f:
        src = f.read().decode("utf-8")
    fn, err = lua.eval("function(s, n) local f, e = loadstring(s, n); return f ~= nil, e end")(src, name)
    check("compiles: " + name, fn, err)

# Info.lua content
info = lua.eval("function(s) return loadstring(s)() end")(open(PLUGIN + "/Info.lua").read())
check("Info.lua fields", info.LrToolkitIdentifier == "org.micropanel.lightroom"
      and info.LrForceInitPlugin is True and info.LrInitPlugin == "MicroPanelInit.lua"
      and info.LrShutdownPlugin == "MicroPanelShutdown.lua"
      and info.LrExportMenuItems is not None)  # LR runs LrInitPlugin only for plugins that provide something [A]

print("\n%d passed, %d failed" % (passed, failed))
sys.exit(1 if failed else 0)
