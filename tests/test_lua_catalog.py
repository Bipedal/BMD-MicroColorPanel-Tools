"""Offline check of lightroom/MicroPanel.lrplugin/Catalog.lua using lupa (stub `import`)."""
import json
import os
import sys

try:
    import lupa  # noqa: F401
except ImportError:
    sys.exit("lupa is required: pip install lupa")

LuaRuntime = None
for mod in ("lupa.lua51", "lupa.luajit21", "lupa.luajit20"):
    try:
        LuaRuntime = __import__(mod, fromlist=["LuaRuntime"]).LuaRuntime
        print("runtime:", mod)
        break
    except ImportError:
        continue
if LuaRuntime is None:
    sys.exit("no Lua 5.1 / LuaJIT lupa runtime available")

HERE = os.path.dirname(os.path.abspath(__file__))
PATH = os.path.join(HERE, "..", "lightroom", "MicroPanel.lrplugin", "Catalog.lua")

lua = LuaRuntime(unpack_returned_tuples=True)
lua.execute(r"""
calls = {}
function reset_calls() calls = {} end
function n_calls() return #calls end
local function sdk_table(name)
  return setmetatable({}, { __index = function(t, k)
    local f = function(...)
      calls[#calls + 1] = name .. '.' .. tostring(k)
      return t
    end
    rawset(t, k, f)
    return f
  end })
end
local cache = {}
function import(name)
  cache[name] = cache[name] or sdk_table(name)
  return cache[name]
end
-- a photo with one mask of one component, for the commands that look at them
local D = import 'LrDevelopController'
local tool = { ID = 'T1', Hidden = false }
masks = { { ID = 'M1', Hidden = false, Tools = { tool } } }
local function rec(k) calls[#calls + 1] = 'LrDevelopController.' .. k end
rawset(D, 'getAllMasks', function() rec('getAllMasks'); return masks end)
rawset(D, 'getSelectedMask', function() rec('getSelectedMask'); return 'M1' end)
rawset(D, 'getSelectedMaskTool', function() rec('getSelectedMaskTool'); return 'T1' end)
rawset(D, 'toggleHideMask', function(id) rec('toggleHideMask'); if id == 'M1' then masks[1].Hidden = not masks[1].Hidden end end)
rawset(D, 'toggleHideMaskTool', function(id) rec('toggleHideMaskTool'); if id == 'T1' then tool.Hidden = not tool.Hidden end end)
""")
with open(PATH, encoding="utf-8") as f:
    cat = lua.execute(f.read())


def lst(t):
    return [t[i] for i in range(1, len(t) + 1)]


errors = []
params, cmds = lst(cat["params"]), lst(cat["cmds"])
# the copy inside the helper, for before Lightroom has connected once
BUILT_IN = os.path.join(HERE, "..", "integrations", "lightroom", "src", "catalogue.json")
now = {k: [{f: e[f] for f in ("id", "label", "group")} for e in v] for k, v in (("params", params), ("cmds", cmds))}
if "--write" in sys.argv:
    with open(BUILT_IN, "w", encoding="utf-8", newline="\n") as f:
        json.dump(now, f, ensure_ascii=False, indent=0)
        f.write("\n")
try:
    with open(BUILT_IN, encoding="utf-8") as f:
        if json.load(f) != now:
            errors.append("catalogue.json is out of date: run with --write")
except OSError as e:
    errors.append(str(e))
ids = [e["id"] for e in params + cmds]
dups = {i for i in ids if ids.count(i) > 1}
if dups:
    errors.append(f"duplicate ids: {sorted(dups)}")
for kind, items in (("param", params), ("cmd", cmds)):
    for e in items:
        for k in ("id", "label", "group"):
            if not e[k] or not isinstance(e[k], str):
                errors.append(f"{kind} {e['id']}: missing {k}")
wrapped = []
for e in params:
    is_hue = e["id"].endswith("Hue") and e["id"].startswith(("SplitToning", "ColorGrade"))
    if is_hue != (e["wrap"] is True):
        errors.append(f"param {e['id']}: wrap={e['wrap']} but hue={is_hue}")
    if e["wrap"] is True:
        wrapped.append(e["id"])
if len(wrapped) != 4:
    errors.append(f"expected 4 wrapped params, got {wrapped}")
for e in cmds:
    if e["run"] is None:
        errors.append(f"cmd {e['id']}: no run")
        continue
    lua.globals().reset_calls()
    try:
        e["run"]()
    except Exception as ex:
        errors.append(f"cmd {e['id']}: error {ex}")
        continue
    if lua.globals().n_calls() < 1:
        errors.append(f"cmd {e['id']}: no SDK call recorded")

print(f"params: {len(params)} (wrap: {len(wrapped)}), cmds: {len(cmds)}, ids: {len(ids)}")
if errors:
    print("FAIL")
    for x in errors:
        print(" -", x)
    sys.exit(1)
print("PASS")
