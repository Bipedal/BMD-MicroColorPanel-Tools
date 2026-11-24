"""The plugin's connection handling (MicroPanelInit.lua) with fake Lightroom sockets.

The fake keeps the behaviour seen live on 2026-11-23: closing a send socket that nobody has connected
to leaves its port unusable, and events of a closed socket arrive later.
Needs lupa (Lua 5.1):  python tests/test_lua_sockets.py
"""
import os
import sys

from lupa.lua51 import LuaRuntime

PLUGIN = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "lightroom", "MicroPanel.lrplugin")).replace("\\", "/")
SETUP = r"""
clock = 1000
tasks, sockets, zombie, log = {}, {}, {}, {}
local function nothing() end
local any = setmetatable({}, { __index = function() return nothing end })
local libs = {
  LrLogger = function() return { enable = nothing, info = function(_, m) log[#log + 1] = m end, warn = nothing, error = function(_, m) log[#log + 1] = 'ERROR ' .. m end } end,
  LrPathUtils = { getStandardFilePath = function() return nil end },
  LrDate = { currentTime = function() return clock end },
  LrFunctionContext = { callWithContext = function(_, f) f({}) end },
  LrTasks = {
    startAsyncTask = function(f) tasks[#tasks + 1] = coroutine.create(f) end,
    pcall = function(f, ...) return true, f(...) end,
    sleep = function() coroutine.yield() end,
  },
  LrApplicationView = { getCurrentModuleName = function() return 'library' end },
  LrApplication = { activeCatalog = function() return { getTargetPhoto = function() return nil end } end, versionString = function() return '16.0' end },
  LrSocket = { bind = function(o)
    local s = { o = o, open = true, connected = false, sent = {} }
    s.send = function(_, l) s.sent[#s.sent + 1] = l end
    s.close = function()
      if not s.connected then zombie[o.port] = true end
      s.open = false
      local was = s.connected
      s.connected = false
      if was then late[#late + 1] = function() o.onClosed(s) end end
    end
    s.reconnect = function() s.open = true; s.connected = false end
    if not zombie[o.port] then sockets[o.port] = s end
    return s
  end },
}
late = {}
function import(n) return libs[n] or any end
function step(n)
  for _ = 1, n or 1 do
    clock = clock + 0.03
    for _, t in ipairs(tasks) do
      if coroutine.status(t) == 'suspended' then local ok, e = coroutine.resume(t); if not ok then error(e) end end
    end
  end
end
function events() local l = late; late = {}; for _, f in ipairs(l) do f() end end
function connect(port)
  local s = sockets[port]
  if zombie[port] or not s or not s.open or s.connected then return false end
  s.connected = true
  if s.o.onConnected then s.o.onConnected(s) end
  return true
end
function drop(port) local s = sockets[port]; s.connected = false; s.o.onClosed(s) end
function say(line) sockets[58782].o.onMessage(sockets[58782], line) end
function heard(what) for _, l in ipairs(sockets[58781].sent) do if l:find(what, 1, true) then return true end end return false end
function load() dofile(DIR .. '/MicroPanelInit.lua') end
function shutdown() dofile(DIR .. '/MicroPanelShutdown.lua') end
"""

results = []


def check(name, ok):
    results.append(ok)
    print(("ok    " if ok else "FAIL  ") + name)


def new():
    lua = LuaRuntime(unpack_returned_tuples=True)
    g = lua.globals()
    g.DIR = PLUGIN
    lua.execute("package.path = '%s/?.lua;' .. package.path" % PLUGIN)
    lua.execute("_PLUGIN = {}")
    lua.execute(SETUP)
    # LrSocket.bind is called without parentheses: fine in plain Lua too
    lua.execute("load()")
    g.step(3)
    return g


def link(g):
    """the helper connects to both ports and says hello"""
    ok = g.connect(58781) and g.connect(58782)
    if ok:
        g.say('{"t":"hello","v":1}')
        g.step(2)
    return bool(ok) and bool(g.heard('"hello"'))


g = new()
check("first connection", link(g))

# the helper goes away and comes back, many times
for i in range(5):
    g.drop(58782)
    g.step(3)
    g.events()           # the old send socket's event comes late
    g.step(3)
    if not link(g):
        break
check("reconnects again and again", i == 4 and not g.zombie[58781])

# the receive side closes before anyone connected to the send side (start of Lightroom)
g = new()
g.sockets[58782].connected = True
g.drop(58782)
check("a half connection doesn't disturb the send socket", link(g))
g.step(20)
check("and the link stays", bool(g.sockets[58781].connected) and not g.zombie[58781])

# the loop is held up (a dialog) while the helper reconnects
g = new()
link(g)
g.drop(58782)            # flagged, but the loop doesn't run yet
g.events()
g.step(3)
g.events()
check("a held-up loop recovers", link(g))

# Reload Plug-in: the new run starts, then the shutdown script of the old one runs
g = new()
link(g)
g.clock = g.clock + 60
g.load()
g.shutdown()
g.step(60)
g.events()
g.step(60)
check("after a reload one run is alive", link(g))

print("\n%d passed, %d failed" % (sum(results), len(results) - sum(results)))
sys.exit(0 if all(results) else 1)
