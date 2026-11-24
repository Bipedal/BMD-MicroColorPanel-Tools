-- Shutdown.lua: tells the Init.lua loop to stop. On "Reload Plug-in" this can run after the new
-- Init.lua; a run that has only just started is not the one to stop. CC0.
local LrDate = import 'LrDate'
if MICROPANEL and LrDate.currentTime() - (MICROPANEL.started or 0) > 5 then MICROPANEL.stop = true end
