--[[----------------------------------------------------------------------------
Core.lua - all logic of the Micro Color Panel plugin, free of Lightroom imports.
Everything Lightroom-specific is injected through the `sdk` table (see Core.new).
License: CC0. Lua 5.1.
------------------------------------------------------------------------------]]

local Json = require 'Json'
local Catalog = require 'Catalog'

local Core = {}
Core.__index = Core

Core.PLUGIN_VERSION = '0.3.7'

Core.defaults = { show_values = false, denoise_default = 20, switch_to_develop = true, wheel_dir = 1, wheel_offset = 0 }

local ZONES = {
  shadow    = { hue = 'SplitToningShadowHue',    sat = 'SplitToningShadowSaturation' },
  midtone   = { hue = 'ColorGradeMidtoneHue',    sat = 'ColorGradeMidtoneSat' },
  highlight = { hue = 'SplitToningHighlightHue', sat = 'SplitToningHighlightSaturation' },
  global    = { hue = 'ColorGradeGlobalHue',     sat = 'ColorGradeGlobalSat' },
}
Core.zones = ZONES

-- sdk: getModule, switchToDevelop, hasPhoto, getValue, setValue, getRange,
--      resetToDefault, getTool, now, lrVersion (and optionally send)
function Core.new(sdk, send)
  local self = setmetatable({}, Core)
  self.sdk = sdk
  self.sendLine = send or sdk.send
  self.settings = {}
  for k, v in pairs(Core.defaults) do self.settings[k] = v end
  self.pending = {}        -- id -> summed delta
  self.pendingOrder = {}   -- ids in first-seen order
  self.wheels = {}         -- zone -> {dx=, dy=}
  self.ops = {}            -- ordered set/reset operations
  self.lastContext = nil
  self.errTimes = {}       -- key -> time of last error sent
  self.wrap = {}
  self.labels = {}
  self.range = {}          -- of settings getRange doesn't know
  self.cmdById = {}
  for _, p in ipairs(Catalog.params) do
    self.labels[p.id] = p.label
    if p.lo then self.range[p.id] = { p.lo, p.hi } end
  end
  for _, p in ipairs(Catalog.params) do if p.wrap then self.wrap[p.id] = true end end
  for _, c in ipairs(Catalog.cmds) do self.cmdById[c.id] = c end
  return self
end

function Core:send(tbl)
  local ok, line = pcall(Json.encode, tbl)
  if ok then self.sendLine(line) end
end

function Core:sendError(msg)
  self:send({ t = 'error', msg = tostring(msg) })
end

-- one error per key per second at most
function Core:limitedError(key, msg)
  local now = self.sdk.now()
  local last = self.errTimes[key]
  if last and now - last < 1 then return end
  self.errTimes[key] = now
  self:sendError(msg)
end

function Core:sendCatalogue()
  local n = 0
  for _, p in ipairs(Catalog.params) do
    self:send({ t = 'catalog', kind = 'param', id = p.id, label = p.label, group = p.group })
    n = n + 1
  end
  for _, c in ipairs(Catalog.cmds) do
    self:send({ t = 'catalog', kind = 'cmd', id = c.id, label = c.label, group = c.group })
    n = n + 1
  end
  self:send({ t = 'catalog_end', n = n })
end

function Core:greet()
  self.greetPending = false
  self:send({ t = 'hello', v = 1, plugin = Core.PLUGIN_VERSION, lr = self.sdk.lrVersion() })
  self:pollContext(true)
  self:sendCatalogue()
end

-- Called by the glue when the send socket (plugin -> helper) connects.
function Core:onSendConnected()
  -- greeted from the plugin's task (step), like commands: asking for the photo can wait
end

----------------------------------------------------------------- input

function Core:onLine(line)
  local ok, err = pcall(self.handleLine, self, line)
  if not ok then self:sendError('internal: ' .. tostring(err)) end
end

function Core:handleLine(line)
  local msg, derr = Json.decode(line)
  if type(msg) ~= 'table' or type(msg.t) ~= 'string' then
    self:sendError('bad message: ' .. tostring(derr or 'not an object with "t"'))
    return
  end
  local t = msg.t
  if t == 'hello' then
    self.greetPending = true          -- answered in step, once the send socket is connected
  elseif t == 'ping' then
    self:send({ t = 'pong' })
  elseif t == 'watch' then
    -- develop settings whose values the helper's LED rules use
    self.watch = {}
    if type(msg.ids) == 'table' then
      for _, id in ipairs(msg.ids) do
        if type(id) == 'string' then self.watch[#self.watch + 1] = id end
      end
    end
    self.lastState = nil
  elseif t == 'options' then
    if type(msg.show_values) == 'boolean' then self.settings.show_values = msg.show_values end
    if type(msg.denoise_default) == 'number' then self.settings.denoise_default = msg.denoise_default end
  elseif t == 'show' then
    -- something the helper changed itself
    if type(msg.text) == 'string' then self.sayNext = msg.text end
  elseif t == 'done' then
    self.finish = true        -- end the adjustment now (Alt was let go) instead of after Lightroom's delay
  elseif t == 'catalog?' then
    self:sendCatalogue()
  elseif t == 'param' then
    if type(msg.id) ~= 'string' or type(msg.delta) ~= 'number' then
      self:sendError('param needs id and numeric delta'); return
    end
    if self.pending[msg.id] == nil then
      self.pending[msg.id] = 0
      self.pendingOrder[#self.pendingOrder + 1] = msg.id
    end
    self.pending[msg.id] = self.pending[msg.id] + msg.delta
  elseif t == 'set' then
    if type(msg.id) ~= 'string' or type(msg.value) ~= 'number' then
      self:sendError('set needs id and numeric value'); return
    end
    self:forget(msg.id)
    self.ops[#self.ops + 1] = { kind = 'set', id = msg.id, value = msg.value }
  elseif t == 'reset' then
    if type(msg.id) ~= 'string' then self:sendError('reset needs id'); return end
    self:forget(msg.id)
    self.ops[#self.ops + 1] = { kind = 'reset', id = msg.id }
  elseif t == 'wheel' then
    local z = msg.zone
    if type(z) ~= 'string' or not ZONES[z] then
      self:sendError('unknown wheel zone: ' .. tostring(z)); return
    end
    local w = self.wheels[z]
    if not w then w = { dx = 0, dy = 0 }; self.wheels[z] = w end
    w.dx = w.dx + (tonumber(msg.dx) or 0)
    w.dy = w.dy + (tonumber(msg.dy) or 0)
  elseif t == 'probe' then
    self.probeIds = type(msg.ids) == 'table' and msg.ids or {}
  elseif t == 'cmd' then
    local c = type(msg.id) == 'string' and self.cmdById[msg.id] or nil
    if not c then
      self:sendError('unknown command: ' .. tostring(msg.id)); return
    end
    -- Run from the plugin's task (step): SDK calls that wait fail in the socket callback.
    self.cmdQueue = self.cmdQueue or {}
    self.cmdQueue[#self.cmdQueue + 1] = c
  else
    self:sendError('unknown message type: ' .. t)
  end
end

----------------------------------------------------------------- apply

function Core:hasPending()
  return #self.ops > 0 or #self.pendingOrder > 0 or next(self.wheels) ~= nil
end

-- Movement that came before a reset must not be applied after it.
function Core:forget(id)
  if self.pending[id] ~= nil then self.pending[id] = 0 end
end

function Core:dropPending()
  self.pending, self.pendingOrder, self.wheels, self.ops = {}, {}, {}, {}
end

function Core:getRange(id)
  local r = self.range[id]
  if r then return true, r[1], r[2] end
  return pcall(self.sdk.getRange, id)
end

function Core:get(id)
  local ok, v = pcall(self.sdk.getValue, id)
  if not ok then self:limitedError(id, 'getValue ' .. id .. ': ' .. tostring(v)); return nil end
  if type(v) ~= 'number' then
    self:limitedError(id, 'getValue ' .. id .. ': not a number'); return nil
  end
  return v
end

function Core:put(id, v)
  local ok, e = pcall(self.sdk.setValue, id, v)
  if ok then self:note(id, v) end
  if not ok then self:limitedError(id, 'setValue ' .. id .. ': ' .. tostring(e)) end
  return ok
end

----------------------------------------------------------------- Remove tool's brush

Core.removePrefs = {
  ['remove.size'] = { field = 'brushSize', min = 1, max = 100 },
  ['remove.feather'] = { field = 'brushFeather', min = 0, max = 100 },
}

function Core:removeMove(id, delta)
  local r = self.sdk.remove
  if not r then self:limitedError(id, 'this Lightroom has no Remove panel functions'); return end
  -- while it moves, go on from our own value: it keeps the fractions, and reading may lag behind
  local c = self.removeCache and self.removeCache[id]
  local v
  if c and self.sdk.now() - c.time < 1 then
    v = c.v
  else
    local ok, prefs = (self.sdk.pcall or pcall)(r.get)
    v = ok and type(prefs) == 'table' and tonumber(prefs[Core.removePrefs[id].field]) or nil
    if not v then self:limitedError(id, 'getRemovePanelPreferences: ' .. tostring(ok and 'no ' .. Core.removePrefs[id].field or prefs)); return end
  end
  self:removeTo(id, v + delta)
end

function Core:removeTo(id, v)
  local r, p = self.sdk.remove, Core.removePrefs[id]
  if not r then self:limitedError(id, 'this Lightroom has no Remove panel functions'); return end
  if v < p.min then v = p.min elseif v > p.max then v = p.max end
  self.removeCache = self.removeCache or {}
  local c = self.removeCache[id]
  self.removeCache[id] = { v = v, time = self.sdk.now() }
  local n = math.floor(v + 0.5)
  if c and c.sent == n then self.removeCache[id].sent = n; return end
  -- answers true, or false and why
  local ok, done, why = (self.sdk.pcall or pcall)(r.set, { [p.field] = n })
  if ok and done ~= false then
    self.removeCache[id].sent = n
    self:note(id, n)
  else
    self:limitedError(id, 'setRemovePanelPreferences: ' .. tostring(ok and why or done))
  end
end

----------------------------------------------------------------- AI Denoise

local DENOISE_WAIT = 0.4          -- the new amount goes out once the knob has rested this long

-- The Detail panel's state, as Lightroom Classic 16 reports it (seen live 2026-11-23):
-- denoiseState = on, denoiseEnabled = can be switched on for this photo, denoiseAmount,
-- enhanceNeedsUpdate; the same three for rawDetails and superRes.
function Core:denoiseState()
  local d = self.sdk.denoise
  if not d then error('this Lightroom has no Denoise functions') end
  local ok, st = (self.sdk.pcall or pcall)(d.state)
  if not ok then error('getEnhancePanelState: ' .. tostring(st)) end
  if type(st) ~= 'table' or type(st.denoiseState) ~= 'boolean' then error('getEnhancePanelState: no denoiseState') end
  return st
end

function Core:say(text)
  if self.sdk.bezel and self.settings.show_values then pcall(self.sdk.bezel, text) end
end

function Core:denoiseSay(text)
  if self.sdk.bezel and self.settings.show_values then pcall(self.sdk.bezel, text) end
end

function Core:denoiseToggle()
  local st = self:denoiseState()
  if st.denoiseState then
    self.sdk.denoise.set(false)
    self:denoiseSay('Denoise off')
  elseif st.denoiseEnabled == false then
    self:denoiseSay('Denoise is not available for this photo')
  else
    local v = self.settings.denoise_default
    self.sdk.denoise.set(true, v)
    self:denoiseSay('Denoise on  ' .. v)
  end
  self.denoiseAt, self.denoiseTarget = nil, nil
end

function Core:denoiseMove(delta)
  if not self.denoiseTarget then
    local ok, st = (self.sdk.pcall or pcall)(self.denoiseState, self)
    if not ok then self:limitedError('denoise', 'denoise: ' .. tostring(st)); return end
    if not st.denoiseState then self:denoiseSay('Denoise is off'); return end
    self.denoiseTarget = tonumber(st.denoiseAmount) or self.settings.denoise_default
  end
  self:denoiseTo(self.denoiseTarget + delta)
end

function Core:denoiseTo(v)
  if v < 1 then v = 1 elseif v > 100 then v = 100 end
  self.denoiseTarget, self.denoiseAt = v, self.sdk.now()
  self:note('denoise.amount', v)
end

-- Every change may mean a new calculation: send the amount only when the knob has come to rest.
function Core:denoiseSend()
  if not self.denoiseAt or self.sdk.now() - self.denoiseAt < DENOISE_WAIT then return end
  local v = math.floor(self.denoiseTarget + 0.5)
  self.denoiseAt, self.denoiseTarget = nil, nil
  local d = self.sdk.denoise
  if not d then return end
  local ok, e = (self.sdk.pcall or pcall)(d.amount, v)
  if not ok then self:sendError('changeDenoiseAmount ' .. v .. ': ' .. tostring(e)) end
end

local function wrap360(v) return v % 360 end

function Core:applyParam(id, delta)
  local cur = self:get(id)
  if cur == nil then return end
  local v = cur + delta
  if self.wrap[id] then
    v = wrap360(v)
  else
    local rok, lo, hi = self:getRange(id)
    if rok then
      if type(lo) == 'number' and v < lo then v = lo end
      if type(hi) == 'number' and v > hi then v = hi end
    else
      self:limitedError(id, 'getRange ' .. id .. ': ' .. tostring(lo))
    end
  end
  if v ~= cur then self:put(id, v) end
end

-- Virtual crop settings: crop.x / crop.y move the crop rectangle as seen on screen (fractions of
-- the photo, +y = up), crop.size scales it about its centre (delta 1 = x e), keeping size and aspect.
-- Lightroom keeps the crop in the camera's orientation, so screen moves go through the photo's
-- `orientation` (two letters: which original corner shows at the top left / top right; A B C D =
-- original top-left, top-right, bottom-right, bottom-left).
local CROP = { 'CropLeft', 'CropRight', 'CropTop', 'CropBottom' }
local CORNER = { A = { 0, 0 }, B = { 1, 0 }, C = { 1, 1 }, D = { 0, 1 } }

-- screen right / screen down as (du, dv) in the crop's coordinates
local function screenAxes(o)
  local tl, tr = CORNER[o and o:sub(1, 1) or ''], CORNER[o and o:sub(2, 2) or '']
  if not tl or not tr then return { 1, 0 }, { 0, 1 } end
  local right = { tr[1] - tl[1], tr[2] - tl[2] }
  if math.abs(right[1]) + math.abs(right[2]) ~= 1 then return { 1, 0 }, { 0, 1 } end
  -- the corner below TL is the neighbour of TL that isn't TR
  local down
  for _, c in pairs(CORNER) do
    local d = { c[1] - tl[1], c[2] - tl[2] }
    if math.abs(d[1]) + math.abs(d[2]) == 1 and not (d[1] == right[1] and d[2] == right[2]) then down = d end
  end
  return right, down
end
Core.screenAxes = screenAxes

function Core:applyCrop(dx, dy, ds)
  local now = self.sdk.now()
  -- While the crop is moving, work from our own copy: getValue lags behind setValue.
  local c = self.cropCache
  if not c or now - c.time > 0.6 then
    local l, r, tp, b = self:get('CropLeft'), self:get('CropRight'), self:get('CropTop'), self:get('CropBottom')
    if not (l and r and tp and b) then return end
    local o
    if self.sdk.getOrientation then
      local pc = self.sdk.pcall or pcall
      local ok, v = pc(self.sdk.getOrientation)
      if ok then o = v end
    end
    c = { l = l, r = r, top = tp, b = b, o = o }
  end
  local right, down = screenAxes(c.o)
  local du = dx * right[1] - dy * down[1]
  local dv = dx * right[2] - dy * down[2]
  local w, h = c.r - c.l, c.b - c.top
  local cx, cy = (c.l + c.r) / 2 + du, (c.top + c.b) / 2 + dv
  if ds ~= 0 then
    local f = math.exp(ds)
    local fmax = math.min(1 / w, 1 / h)
    local fmin = math.max(0.05 / w, 0.05 / h)
    if f > fmax then f = fmax end
    if f < fmin then f = fmin end
    w, h = w * f, h * f
  end
  if cx - w / 2 < 0 then cx = w / 2 end
  if cx + w / 2 > 1 then cx = 1 - w / 2 end
  if cy - h / 2 < 0 then cy = h / 2 end
  if cy + h / 2 > 1 then cy = 1 - h / 2 end
  local new = { l = cx - w / 2, r = cx + w / 2, top = cy - h / 2, b = cy + h / 2, o = c.o, time = now }
  -- Write the leading edge first so the rectangle never shrinks on the way (Lightroom's aspect
  -- lock would push back).
  local order = {}
  if new.l > c.l then order[1], order[2] = 'r', 'l' else order[1], order[2] = 'l', 'r' end
  if new.top > c.top then order[3], order[4] = 'b', 'top' else order[3], order[4] = 'top', 'b' end
  local ids = { l = 'CropLeft', r = 'CropRight', top = 'CropTop', b = 'CropBottom' }
  for _, k in ipairs(order) do
    if math.abs(new[k] - c[k]) > 1e-6 then self:put(ids[k], new[k]) end
  end
  self.cropCache = new
end

-- Report which develop settings and controller functions exist (for checking names live).
function Core:probe(ids)
  local res = {}
  for _, id in ipairs(type(ids) == 'table' and ids or {}) do
    if type(id) == 'string' then
      local r = {}
      local ok, v = pcall(self.sdk.getValue, id)
      if ok then r.value = v else r.err = tostring(v) end
      local rok, lo, hi = self:getRange(id)
      if rok then r.lo, r.hi = lo, hi else r.range_err = tostring(lo) end
      res[id] = r
    end
  end
  local out = { t = 'probe', results = res }
  if self.sdk.probeExtra then
    local pc = self.sdk.pcall or pcall
    local ok, extra = pc(self.sdk.probeExtra)
    out.extra = ok and extra or tostring(extra)
  end
  local ok, line = pcall(Json.encode, out)
  if not ok then line = Json.encode({ t = 'probe', error = tostring(line) }) end
  self.sendLine(line)
end

function Core:applyWheel(zone, dx, dy)
  local z = ZONES[zone]
  local h = self:get(z.hue)
  local s = self:get(z.sat)
  if h == nil or s == nil then return end
  local dir = self.settings.wheel_dir
  local offDeg = self.settings.wheel_offset
  local a = math.rad(h * dir + offDeg)
  local px = s * math.cos(a) + dx
  local py = s * math.sin(a) + dy
  local mag = math.sqrt(px * px + py * py)
  if mag < 1e-9 then mag = 0 end -- float noise counts as the centre
  local newS = math.min(mag, 100)
  local newH = h
  if mag > 0 then
    newH = wrap360((math.deg(math.atan2(py, px)) - offDeg) * dir)
  end
  if newH ~= h then self:put(z.hue, newH) end
  if newS ~= s then self:put(z.sat, newS) end
end

function Core:runQueued()
  local q = self.cmdQueue
  if q and #q > 0 then
    self.cmdQueue = {}
    local pc = self.sdk.pcall or pcall
    for _, c in ipairs(q) do
      local rok, rerr = pc(c.run, self)
      if not rok then self:sendError('command ' .. c.id .. ' failed: ' .. tostring(rerr)) end
    end
  end
  if self.probeIds then
    local ids = self.probeIds
    self.probeIds = nil
    self:probe(ids)
  end
end

-- Show what is being adjusted, and its value, on Lightroom's own overlay.
local REMOVE = Core.removePrefs
local function rounded(v) local r = math.floor(v + 0.5); if r == 0 then r = 0 end; return string.format('%d', r) end
local DEG = string.char(0xC2, 0xB0)
function Core:valueText(id, v)
  if id == 'denoise.amount' or REMOVE[id] then return rounded(v) end
  if id == 'Exposure2012' or id == 'local_Exposure' then
    if math.abs(v) < 0.005 then return '0.00' end
    return string.format('%+.2f', v)
  end
  if id == 'Temperature' and v > 1000 then return rounded(v) .. ' K' end
  if id == 'straightenAngle' then return string.format('%.2f', v) .. DEG end
  if self.wrap[id] then return rounded(v) .. DEG end
  if id == 'SharpenRadius' then return string.format('%.1f', v) end
  local ok, lo = self:getRange(id)
  local r = rounded(v)
  if ok and type(lo) == 'number' and lo < 0 and v >= 0.5 then return '+' .. r end
  return r
end

function Core:note(id, v)
  if not (self.settings.show_values and self.sdk.bezel and self.labels[id]) then return end
  self.shown = self.shown or { order = {}, value = {} }
  local s = self.shown
  if s.value[id] == nil then s.order[#s.order + 1] = id end
  s.value[id] = v or false             -- false: ask for it when it is shown
end

function Core:showValues()
  local s, now = self.shown, self.sdk.now()
  local say = self.sayNext
  if say and not (self.shownAt and now - self.shownAt < 0.1) then
    self.sayNext, self.shownAt = nil, now
    self:denoiseSay(say)
    return
  end
  if not s or (self.shownAt and now - self.shownAt < 0.1) then return end
  self.shown, self.shownAt = nil, now
  local parts = {}
  for _, id in ipairs(s.order) do
    local v = s.value[id] or self:get(id)
    if type(v) == 'number' then parts[#parts + 1] = self.labels[id] .. '  ' .. self:valueText(id, v) end
  end
  if #parts == 0 then return end
  local ok, e = (self.sdk.pcall or pcall)(self.sdk.bezel, table.concat(parts, '\n'))
  if not ok then self:limitedError('bezel', 'showing the value: ' .. tostring(e)) end
end

function Core:step()
  if self.greetPending and not (self.sdk.sendReady and not self.sdk.sendReady()) then self:greet() end
  self:showValues()
  self:denoiseSend()
  self:runQueued()
  if not self:hasPending() then
    if self.finish then
      self.finish = nil
      if self.sdk.stopTracking then
        local ok, e = (self.sdk.pcall or pcall)(self.sdk.stopTracking)
        if not ok then self:limitedError('stopTracking', 'stopTracking: ' .. tostring(e)) end
      end
    end
    return
  end
  local mok, module = pcall(self.sdk.getModule)
  if not mok then
    self:limitedError('module', 'getModule: ' .. tostring(module))
    self:dropPending(); return
  end
  if module ~= 'develop' then
    if self.settings.switch_to_develop then
      local sok, serr = pcall(self.sdk.switchToDevelop)
      if not sok then
        self:limitedError('switch', 'switchToDevelop: ' .. tostring(serr))
        self:dropPending()
      end
    else
      self:dropPending()
    end
    return
  end
  local pok, photo = pcall(self.sdk.hasPhoto)
  if not pok or not photo then self:dropPending(); return end

  local ops, order, pending, wheels = self.ops, self.pendingOrder, self.pending, self.wheels
  self:dropPending()

  for _, op in ipairs(ops) do
    if op.id == 'denoise.amount' then
      self:denoiseTo(op.kind == 'set' and op.value or self.settings.denoise_default)
    elseif Core.removePrefs[op.id] then
      if op.kind == 'set' then self:removeTo(op.id, op.value)
      else self:limitedError(op.id, op.id .. ' has no default to reset to') end
    elseif op.kind == 'set' then
      self:put(op.id, op.value)
    else
      local rok, rerr = pcall(self.sdk.resetToDefault, op.id)
      if rok then self:note(op.id) end
      if not rok then self:limitedError(op.id, 'reset ' .. op.id .. ': ' .. tostring(rerr)) end
    end
  end
  local cdx, cdy, cds = pending['crop.x'] or 0, pending['crop.y'] or 0, pending['crop.size'] or 0
  for _, id in ipairs(order) do
    if id == 'denoise.amount' then
      if pending[id] ~= 0 then self:denoiseMove(pending[id]) end
    elseif Core.removePrefs[id] then
      if pending[id] ~= 0 then self:removeMove(id, pending[id]) end
    elseif id:sub(1, 5) ~= 'crop.' then self:applyParam(id, pending[id]) end
  end
  if cdx ~= 0 or cdy ~= 0 or cds ~= 0 then self:applyCrop(cdx, cdy, cds) end
  for zone, w in pairs(wheels) do self:applyWheel(zone, w.dx, w.dy) end
end

----------------------------------------------------------------- context

function Core:pollContext(force)
  local mok, module = pcall(self.sdk.getModule)
  if not mok then module = nil end
  local tool
  if module == 'develop' then
    local tok, tv = pcall(self.sdk.getTool)
    if tok then tool = tv end
  end
  local pok, photo = pcall(self.sdk.hasPhoto)
  photo = (pok and photo) and true or false
  local key = tostring(module) .. '|' .. tostring(tool) .. '|' .. tostring(photo)
  if force or key ~= self.lastContext then
    self.lastContext = key
    self:send({ t = 'context', module = module, tool = tool, photo = photo })
  end
  self:pollState(force, module, photo)
end

-- Photo and develop state for the helper's LED rules: flag, rating, colour label,
-- colour-grading view and the watched develop settings. Sent only when something changes.
local function round3(v)
  if type(v) ~= 'number' then return v end
  return math.floor(v * 1000 + 0.5) / 1000
end

function Core:pollState(force, module, photo)
  local s = { t = 'state', values = {} }
  -- Lightroom's metadata calls can yield; a plain pcall can't be yielded across, so the
  -- glue passes LrTasks.pcall as sdk.pcall.
  local pc = self.sdk.pcall or pcall
  local function get(f)
    if not f then return nil end
    local ok, v = pc(f)
    if ok then return v end
    return nil
  end
  if photo then
    s.flag = get(self.sdk.getFlag)       -- 'pick' | 'reject' | 'none'
    s.rating = get(self.sdk.getRating)   -- 0..5
    s.label = get(self.sdk.getLabel)     -- 'red' | ... | 'none'
  end
  if module == 'develop' and photo then
    s.cg_view = get(self.sdk.getCgView)
    for _, id in ipairs(self.watch or {}) do
      if id == 'denoise' then          -- not a setting: is AI Denoise on
        local ok, st = pc(self.denoiseState, self)
        if ok then s.values.denoise = st.denoiseState and 1 or 0 end
      else
        local ok, v = pc(self.sdk.getValue, id)
        if ok then s.values[id] = round3(v) end
      end
    end
  end
  local okj, key = pcall(Json.encode, s)
  if not okj then return end
  if force or key ~= self.lastState then
    self.lastState = key
    self.sendLine(key)
  end
end

return Core
