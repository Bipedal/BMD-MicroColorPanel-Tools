-- MicroPanelStatus.lua: File > Plug-in Extras > Micro Color Panel status. CC0.
local LrDialogs = import 'LrDialogs'
local s = MICROPANEL
local text
if not s then
  text = 'The plugin task is not running (see LrClassicLogs\\MicroPanel.log).'
else
  text = 'Helper link: ' .. (s.connected and 'connected' or 'not connected (is the Micro Color Panel helper running?)')
    .. '\nMessages received: ' .. tostring(s.received or 0)
    .. '\nLast error: ' .. tostring(s.lastError or 'none')
end
LrDialogs.message('Micro Color Panel', text, 'info')
