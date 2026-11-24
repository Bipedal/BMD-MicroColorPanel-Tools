--[[----------------------------------------------------------------------------
Init.lua - Lightroom glue for the Micro Color Panel plugin. All logic is in
Core.lua. License: CC0.
------------------------------------------------------------------------------]]

-- Startup trace: LrClassicLogs\MicroPanel.log
local LrLogger = import 'LrLogger'
local log = LrLogger('MicroPanel')
log:enable('logfile')
local function trace(msg) log:info(tostring(msg)) end
trace('Init.lua loaded')


local LrSocket = import 'LrSocket'
local LrTasks = import 'LrTasks'
local LrFunctionContext = import 'LrFunctionContext'
local LrApplicationView = import 'LrApplicationView'
local LrDevelopController = import 'LrDevelopController'
local LrApplication = import 'LrApplication'
local LrDialogs = import 'LrDialogs'
local LrDate = import 'LrDate'

local okCore, Core = pcall(require, 'Core')
if not okCore then trace('require Core FAILED: ' .. tostring(Core)) error(Core) end
trace('Core loaded')

local SEND_PORT = 58781     -- plugin -> helper
local RECEIVE_PORT = 58782  -- helper -> plugin

-- Plain global table shared with Shutdown.lua (Lightroom's plugin environment has no _G).
-- A reload starts a second run while the first is still there: tell that one to stop, and keep our
-- own table, so that whatever is done to the global one afterwards is not mistaken for ours.
local replaced = MICROPANEL ~= nil
if MICROPANEL then MICROPANEL.stop = true end
local me = { stop = false, connected = false, received = 0, lastError = nil, started = LrDate.currentTime() }
MICROPANEL = me


LrTasks.startAsyncTask(function()
  local ok, err = LrTasks.pcall(function()
  LrFunctionContext.callWithContext('micropanel', function(context)
    trace('task started')
    if replaced then LrTasks.sleep(1) end   -- let the earlier run give up its ports
    local sendConnected = false
    local sendNeedsRestart = false
    local drops = 0
    local sendSocket
    local sendGen = 0          -- events of a socket we have replaced are ignored
    local sendUsed = false     -- somebody has connected to the current one

    -- Never close a send socket nobody has connected to: Lightroom then keeps the port in a state
    -- where every later connection is accepted and dropped at once (seen live 2026-11-23).
    local function startSendSocket()
      sendGen = sendGen + 1
      local mine = sendGen
      sendUsed, sendConnected, sendNeedsRestart = false, false, false
      sendSocket = LrSocket.bind {
        functionContext = context,
        plugin = _PLUGIN,
        port = SEND_PORT,
        mode = 'send',
        onConnected = function()
          if mine ~= sendGen then return end
          sendUsed, sendConnected, sendNeedsRestart = true, true, false
          me.connected = true
          log:info('send socket connected')
          if me.core then me.core:onSendConnected() end
        end,
        onClosed = function()
          if mine ~= sendGen then return end
          sendConnected = false
          sendNeedsRestart = true -- the loop re-binds it; calling reconnect here can hang
          log:info('send socket closed')
        end,
        onError = function(socket, err)
          if mine ~= sendGen then return end
          sendConnected = false
          log:warn('send socket error: ' .. tostring(err))
          if not me.stop then socket:reconnect() end
        end,
      }
    end

    local function sendLine(line)
      if sendConnected and sendSocket then
        sendSocket:send(line .. '\n')
      else
        drops = drops + 1
      end
    end

    local sdk = {
      getModule = function() return LrApplicationView.getCurrentModuleName() end,
      switchToDevelop = function() LrApplicationView.switchToModule('develop') end,
      hasPhoto = function() return LrApplication.activeCatalog():getTargetPhoto() ~= nil end,
      getValue = function(id) return LrDevelopController.getValue(id) end,
      setValue = function(id, v) LrDevelopController.setValue(id, v) end,
      stopTracking = function() LrDevelopController.stopTracking() end,
      bezel = function(text) LrDialogs.showBezel(text, 1.5) end,
      remove = LrDevelopController.setRemovePanelPreferences and {
        get = function() return LrDevelopController.getRemovePanelPreferences() end,
        set = function(t) return LrDevelopController.setRemovePanelPreferences(t) end,
      },
      denoise = LrDevelopController.setEnhance and {
        state = function()
          local st = LrDevelopController.getEnhancePanelState()
          -- what it looks like isn't documented: once per change in the log
          local function str(x, d)
            if type(x) ~= 'table' or d > 4 then return tostring(x) end
            local parts = {}
            for k, v in pairs(x) do parts[#parts + 1] = tostring(k) .. '=' .. str(v, d + 1) end
            table.sort(parts)
            return '{' .. table.concat(parts, ', ') .. '}'
          end
          local s = str(st, 0)
          if s ~= me.enhance then me.enhance = s; log:info('enhance panel: ' .. s) end
          return st
        end,
        -- in tasks of their own: the calculation may take a while and must not hold up the rest
        set = function(on, amount)
          log:info('setEnhance denoise ' .. tostring(on) .. ' ' .. tostring(amount))
          LrTasks.startAsyncTask(function()
            local ok, e = LrTasks.pcall(function()
              if amount then LrDevelopController.setEnhance('denoise', on, amount) else LrDevelopController.setEnhance('denoise', on) end
            end)
            log:info('setEnhance done: ' .. tostring(ok) .. ' ' .. tostring(e))
            if not ok and me.core then me.core:sendError('setEnhance: ' .. tostring(e)) end
          end)
        end,
        amount = function(v)
          log:info('changeDenoiseAmount ' .. v)
          LrTasks.startAsyncTask(function()
            local ok, e = LrTasks.pcall(function() LrDevelopController.changeDenoiseAmount(v) end)
            log:info('changeDenoiseAmount done: ' .. tostring(ok) .. ' ' .. tostring(e))
            if not ok and me.core then me.core:sendError('changeDenoiseAmount: ' .. tostring(e)) end
          end)
        end,
      },
      getRange = function(id) return LrDevelopController.getRange(id) end,
      getOrientation = function()
        local ph = LrApplication.activeCatalog():getTargetPhoto()
        local ds = ph and ph:getDevelopSettings()
        return ds and ds.orientation
      end,
      resetToDefault = function(id) LrDevelopController.resetToDefault(id) end,
      getTool = function() return LrDevelopController.getSelectedTool() end,
      getFlag = function()
        local ph = LrApplication.activeCatalog():getTargetPhoto()
        local v = ph and ph:getRawMetadata('pickStatus')
        if v == 1 then return 'pick' elseif v == -1 then return 'reject' end
        return 'none'
      end,
      getRating = function()
        local ph = LrApplication.activeCatalog():getTargetPhoto()
        return ph and (ph:getRawMetadata('rating') or 0) or 0
      end,
      getLabel = function()
        local ph = LrApplication.activeCatalog():getTargetPhoto()
        local v = ph and ph:getRawMetadata('colorNameForLabel')
        if v == nil or v == '' then return 'none' end
        return v
      end,
      getCgView = function() return LrDevelopController.getActiveColorGradingView() end,
      now = function() return LrDate.currentTime() end,
      lrVersion = function() return LrApplication.versionString() end,
      sendReady = function() return sendConnected end,
      pcall = LrTasks.pcall,
      probeExtra = function()
        local fns = {}
        for k, v in pairs(LrDevelopController) do
          if type(v) == 'function' then fns[#fns + 1] = k end
        end
        table.sort(fns)
        local keys = {}
        local ph = LrApplication.activeCatalog():getTargetPhoto()
        local ds = ph and ph:getDevelopSettings() or {}
        for k, v in pairs(ds) do
          if type(v) ~= 'table' then keys[#keys + 1] = k .. '=' .. tostring(v) else keys[#keys + 1] = k .. '={...}' end
        end
        table.sort(keys)
        local function str(x, d)
          d = d or 0
          if type(x) ~= 'table' or d > 3 then return tostring(x) end
          local parts = {}
          for k, v in pairs(x) do parts[#parts + 1] = tostring(k) .. '=' .. str(v, d + 1) end
          return '{' .. table.concat(parts, ', ') .. '}'
        end
        local masks, sel = 'getAllMasks failed', 'getSelectedMask failed'
        pcall(function() masks = str(LrDevelopController.getAllMasks()) end)
        pcall(function() sel = str(LrDevelopController.getSelectedMask()) end)
        local enh = 'n/a'
        pcall(function() enh = str(LrDevelopController.getEnhancePanelState()) end)
        local wb = 'n/a'
        pcall(function() wb = tostring(LrDevelopController.getValue('WhiteBalance')) end)
        return { functions = fns, settings = keys, masks = masks, selected_mask = sel,
                 tool = tostring(LrDevelopController.getSelectedTool()), wb = wb, enhance = enh }
      end,
    }
    local core = Core.new(sdk, sendLine)
    me.core = core -- for debugging

    local receiveSocket = LrSocket.bind {
      functionContext = context,
      plugin = _PLUGIN,
      port = RECEIVE_PORT,
      mode = 'receive',
      onMessage = function(_, message)
        if type(message) == 'string' then
          me.received = me.received + 1
          core:onLine(message)
        end
      end,
      onClosed = function(socket)
        log:info('receive socket closed')
        -- The helper went away. A send-mode socket doesn't notice its peer closing, so re-bind
        -- it too; the next helper then connects to a fresh listener on both ports.
        sendConnected = false
        me.connected = false
        if sendUsed then sendNeedsRestart = true end
        if not me.stop then socket:reconnect() end
      end,
      onError = function(socket, err)
        log:warn('receive socket error: ' .. tostring(err))
        if not me.stop then socket:reconnect() end
      end,
    }

    startSendSocket()
    log:info('Micro Color Panel plugin started')
    trace('sockets bound, loop running')

    local nextContext = 0
    while not me.stop do
      local ok, err = LrTasks.pcall(core.step, core)
      if not ok then me.lastError = 'step: ' .. tostring(err); log:error(me.lastError) end
      local now = LrDate.currentTime()
      if now >= nextContext then
        nextContext = now + 0.25
        ok, err = LrTasks.pcall(core.pollContext, core)
        if not ok then log:error('pollContext: ' .. tostring(err)) end
      end
      if sendNeedsRestart then
        pcall(function() sendSocket:close() end)
        startSendSocket()
      end
      LrTasks.sleep(0.03)
    end

    log:info('stopping; dropped sends: ' .. drops)
    pcall(function() sendSocket:close() end)
    pcall(function() receiveSocket:close() end)
    trace('stopped')
  end)
  end)
  if not ok then
    trace('FAILED: ' .. tostring(err)); log:error('plugin task failed: ' .. tostring(err))
    local LrDialogs = import 'LrDialogs'
    LrDialogs.message('MicroPanel plugin task failed', tostring(err), 'critical')
  end
end)
