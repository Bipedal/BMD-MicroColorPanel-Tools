--[[----------------------------------------------------------------------------
Catalog.lua - the develop settings and commands the Micro Color Panel bridge
can drive in Lightroom Classic.

License: CC0 (public domain dedication).

Param ids are Camera Raw / Lightroom develop setting names, as used by
LrDevelopController.getValue / setValue. Command ids are our own names.

`import` is only called inside `run` functions, so this file can be loaded
outside Lightroom (tests stub `import`).
------------------------------------------------------------------------------]]

local params = {}
local cmds = {}

local function P(id, label, group, wrap, lo, hi)
  params[#params + 1] = { id = id, label = label, group = group, wrap = wrap and true or false, lo = lo, hi = hi }
end

local function C(id, label, group, run)
  cmds[#cmds + 1] = { id = id, label = label, group = group, run = run }
end

-- Switch to Develop when needed, then give Lightroom a moment to catch up.
local function ensureDevelop()
  local LrApplicationView = import 'LrApplicationView'
  local LrTasks = import 'LrTasks'
  if LrApplicationView.getCurrentModuleName() ~= 'develop' then
    LrApplicationView.switchToModule('develop')
    LrTasks.yield()
  end
end

-- Wrap a function of the Develop controller as a command run function.
local function develop(fn)
  return function(core)
    ensureDevelop()
    local LrDevelopController = import 'LrDevelopController'
    fn(LrDevelopController, core)
  end
end

local function sel(fn)
  return function()
    local LrSelection = import 'LrSelection'
    fn(LrSelection)
  end
end

local function view(fn)
  return function()
    local LrApplicationView = import 'LrApplicationView'
    fn(LrApplicationView)
  end
end

----------------------------------------------------------------------------
-- Params
----------------------------------------------------------------------------

-- Basic
P('Temperature', 'Temperature', 'Basic')
P('Tint', 'Tint', 'Basic')
P('Exposure2012', 'Exposure', 'Basic')
P('Contrast2012', 'Contrast', 'Basic')
P('Highlights2012', 'Highlights', 'Basic')
P('Shadows2012', 'Shadows', 'Basic')
P('Whites2012', 'Whites', 'Basic')
P('Blacks2012', 'Blacks', 'Basic')
P('Texture', 'Texture', 'Basic')
P('Clarity2012', 'Clarity', 'Basic')
P('Dehaze', 'Dehaze', 'Basic')
P('Vibrance', 'Vibrance', 'Basic')
P('Saturation', 'Saturation', 'Basic')

-- Tone curve (parametric)
P('ParametricShadows', 'Curve shadows', 'Tone Curve')
P('ParametricDarks', 'Curve darks', 'Tone Curve')
P('ParametricLights', 'Curve lights', 'Tone Curve')
P('ParametricHighlights', 'Curve highlights', 'Tone Curve')
P('ParametricShadowSplit', 'Curve split shadows/darks', 'Tone Curve')
P('ParametricMidtoneSplit', 'Curve split darks/lights', 'Tone Curve')
P('ParametricHighlightSplit', 'Curve split lights/highlights', 'Tone Curve')

-- HSL
for _, colour in ipairs({ 'Red', 'Orange', 'Yellow', 'Green', 'Aqua', 'Blue', 'Purple', 'Magenta' }) do
  P('HueAdjustment' .. colour, colour .. ' hue', 'HSL')
  P('SaturationAdjustment' .. colour, colour .. ' saturation', 'HSL')
  P('LuminanceAdjustment' .. colour, colour .. ' luminance', 'HSL')
end

-- Color Grading (hue settings wrap around 0-360)
P('SplitToningShadowHue', 'Shadows hue', 'Color Grading', true)
P('SplitToningShadowSaturation', 'Shadows saturation', 'Color Grading')
P('ColorGradeShadowLum', 'Shadows luminance', 'Color Grading')
P('ColorGradeMidtoneHue', 'Midtones hue', 'Color Grading', true)
P('ColorGradeMidtoneSat', 'Midtones saturation', 'Color Grading')
P('ColorGradeMidtoneLum', 'Midtones luminance', 'Color Grading')
P('SplitToningHighlightHue', 'Highlights hue', 'Color Grading', true)
P('SplitToningHighlightSaturation', 'Highlights saturation', 'Color Grading')
P('ColorGradeHighlightLum', 'Highlights luminance', 'Color Grading')
P('ColorGradeGlobalHue', 'Global hue', 'Color Grading', true)
P('ColorGradeGlobalSat', 'Global saturation', 'Color Grading')
P('ColorGradeGlobalLum', 'Global luminance', 'Color Grading')
P('ColorGradeBlending', 'Blending', 'Color Grading')
P('SplitToningBalance', 'Balance', 'Color Grading')

-- Detail
P('Sharpness', 'Sharpening amount', 'Detail')
P('SharpenRadius', 'Sharpening radius', 'Detail')
P('SharpenDetail', 'Sharpening detail', 'Detail')
P('SharpenEdgeMasking', 'Sharpening masking', 'Detail')
P('LuminanceSmoothing', 'Noise luminance', 'Detail')
P('LuminanceNoiseReductionDetail', 'Noise luminance detail', 'Detail')
P('LuminanceNoiseReductionContrast', 'Noise luminance contrast', 'Detail')
P('ColorNoiseReduction', 'Noise colour', 'Detail')
P('ColorNoiseReductionDetail', 'Noise colour detail', 'Detail')
P('ColorNoiseReductionSmoothness', 'Noise colour smoothness', 'Detail')

-- Effects
P('PostCropVignetteAmount', 'Post-crop vignette amount', 'Effects')
P('PostCropVignetteMidpoint', 'Post-crop vignette midpoint', 'Effects')
P('PostCropVignetteRoundness', 'Post-crop vignette roundness', 'Effects')
P('PostCropVignetteFeather', 'Post-crop vignette feather', 'Effects')
P('PostCropVignetteHighlightContrast', 'Post-crop vignette highlights', 'Effects')
P('GrainAmount', 'Grain amount', 'Effects')
P('GrainSize', 'Grain size', 'Effects')
P('GrainFrequency', 'Grain roughness', 'Effects')

-- Calibration
P('ShadowTint', 'Calibration shadow tint', 'Calibration')
P('RedHue', 'Calibration red hue', 'Calibration')
P('RedSaturation', 'Calibration red saturation', 'Calibration')
P('GreenHue', 'Calibration green hue', 'Calibration')
P('GreenSaturation', 'Calibration green saturation', 'Calibration')
P('BlueHue', 'Calibration blue hue', 'Calibration')
P('BlueSaturation', 'Calibration blue saturation', 'Calibration')

-- Transform
P('PerspectiveVertical', 'Vertical', 'Transform')
P('PerspectiveHorizontal', 'Horizontal', 'Transform')
P('PerspectiveRotate', 'Rotate', 'Transform')
P('PerspectiveScale', 'Scale', 'Transform')
P('PerspectiveAspect', 'Aspect', 'Transform')
P('PerspectiveX', 'Offset X', 'Transform')
P('PerspectiveY', 'Offset Y', 'Transform')

-- Lens corrections
P('LensManualDistortionAmount', 'Distortion', 'Lens')
P('DefringePurpleAmount', 'Defringe purple', 'Lens')
P('DefringeGreenAmount', 'Defringe green', 'Lens')
P('VignetteAmount', 'Lens vignette amount', 'Lens')
P('VignetteMidpoint', 'Lens vignette midpoint', 'Lens')
P('LensProfileDistortionScale', 'Profile distortion', 'Lens')
P('LensProfileVignettingScale', 'Profile vignetting', 'Lens')
P('LensProfileChromaticAberrationScale', 'Profile chromatic aberration', 'Lens')

-- Glow (Lightroom 16): names as in the photo's develop settings; getRange doesn't know them.
P('Glow', 'Glow amount', 'Effects', false, 0, 100)
P('GlowRange', 'Glow range', 'Effects', false, 0, 100)
P('GlowSpread', 'Glow spread', 'Effects', false, 0, 100)
P('GlowWarmth', 'Glow warmth', 'Effects', false, -100, 100)
P('GlowDirection', 'Glow direction', 'Effects', false, -180, 180)
P('GlowDirectionalSoftness', 'Glow softness', 'Effects', false, 0, 100)

-- AI Denoise has its own calls, not a develop setting.
P('denoise.amount', 'Denoise amount', 'Detail')

-- The Remove tool's brush: preferences of its panel, not develop settings.
P('remove.size', 'Remove brush size', 'Remove')
P('remove.feather', 'Remove brush feather', 'Remove')

-- Crop. crop.x / crop.y / crop.size are the plugin's own: they move and scale the crop.
P('straightenAngle', 'Crop angle', 'Crop')
P('crop.x', 'Crop move left/right', 'Crop')
P('crop.y', 'Crop move up/down', 'Crop')
P('crop.size', 'Crop size', 'Crop')

-- Masking: these change the selected mask.
for _, m in ipairs({
  { 'local_Exposure', 'Exposure' }, { 'local_Contrast', 'Contrast' }, { 'local_Highlights', 'Highlights' },
  { 'local_Shadows', 'Shadows' }, { 'local_Whites', 'Whites' }, { 'local_Blacks', 'Blacks' },
  { 'local_Temperature', 'Temperature' }, { 'local_Tint', 'Tint' }, { 'local_Texture', 'Texture' },
  { 'local_Clarity', 'Clarity' }, { 'local_Dehaze', 'Dehaze' }, { 'local_Hue', 'Hue' },
  { 'local_Saturation', 'Saturation' }, { 'local_Sharpness', 'Sharpness' },
  { 'local_LuminanceNoise', 'Noise' }, { 'local_Moire', 'Moire' }, { 'local_Defringe', 'Defringe' },
  { 'local_Grain', 'Grain' }, { 'local_Amount', 'Amount' },
}) do
  P(m[1], 'Mask ' .. m[2]:lower(), 'Mask')
end

----------------------------------------------------------------------------
-- Commands
----------------------------------------------------------------------------

-- Navigation
C('nextPhoto', 'Next photo', 'Navigation', sel(function(S) S.nextPhoto() end))
C('previousPhoto', 'Previous photo', 'Navigation', sel(function(S) S.previousPhoto() end))

-- Flags
C('pick', 'Flag as pick', 'Flags', sel(function(S) S.flagAsPick() end))
C('reject', 'Flag as reject', 'Flags', sel(function(S) S.flagAsReject() end))
C('unflag', 'Remove flag', 'Flags', sel(function(S) S.removeFlag() end))

-- Ratings
for n = 0, 5 do
  C('rating' .. n, 'Rating ' .. n, 'Rating', sel(function(S) S.setRating(n) end))
end
C('increaseRating', 'Increase rating', 'Rating', sel(function(S) S.increaseRating() end))
C('decreaseRating', 'Decrease rating', 'Rating', sel(function(S) S.decreaseRating() end))

-- Colour labels
C('toggleRedLabel', 'Toggle red label', 'Labels', sel(function(S) S.toggleRedLabel() end))
C('toggleYellowLabel', 'Toggle yellow label', 'Labels', sel(function(S) S.toggleYellowLabel() end))
C('toggleGreenLabel', 'Toggle green label', 'Labels', sel(function(S) S.toggleGreenLabel() end))
C('toggleBlueLabel', 'Toggle blue label', 'Labels', sel(function(S) S.toggleBlueLabel() end))
C('togglePurpleLabel', 'Toggle purple label', 'Labels', sel(function(S) S.togglePurpleLabel() end))

-- Undo / redo
C('undo', 'Undo', 'Edit', function()
  local LrUndo = import 'LrUndo'
  if LrUndo.canUndo() then LrUndo.undo() end
end)
C('redo', 'Redo', 'Edit', function()
  local LrUndo = import 'LrUndo'
  if LrUndo.canRedo() then LrUndo.redo() end
end)

-- Modules
for _, m in ipairs({
  { 'moduleLibrary', 'Library', 'library' },
  { 'moduleDevelop', 'Develop', 'develop' },
  { 'moduleMap', 'Map', 'map' },
  { 'moduleBook', 'Book', 'book' },
  { 'moduleSlideshow', 'Slideshow', 'slideshow' },
  { 'modulePrint', 'Print', 'print' },
  { 'moduleWeb', 'Web', 'web' },
}) do
  C(m[1], 'Go to ' .. m[2], 'Modules', view(function(V) V.switchToModule(m[3]) end))
end

-- Views
C('viewLoupe', 'Loupe view', 'View', view(function(V) V.showView('develop_loupe') end))
C('viewBeforeAfterHoriz', 'Before/after side by side', 'View',
  view(function(V) V.showView('develop_before_after_horiz') end))
C('viewBeforeAfterVert', 'Before/after top and bottom', 'View',
  view(function(V) V.showView('develop_before_after_vert') end))
C('viewBefore', 'Before only', 'View', view(function(V) V.showView('develop_before') end))
C('toggleLoupe', 'Toggle loupe', 'View', view(function(V) V.toggleLoupe() end))
C('zoom100', 'Zoom 1:1', 'View', view(function(V) V.zoomToOneToOne() end))
C('zoomIn', 'Zoom in', 'View', view(function(V) V.zoomIn() end))
C('zoomOut', 'Zoom out', 'View', view(function(V) V.zoomOut() end))

-- Resets and photo operations
C('resetAll', 'Reset all adjustments', 'Develop', develop(function(D) D.resetAllDevelopAdjustments() end))
C('resetCrop', 'Reset crop', 'Develop', develop(function(D) D.resetCrop() end))
C('resetTransforms', 'Reset transforms', 'Develop', develop(function(D) D.resetTransforms() end))
C('resetMasking', 'Reset masking', 'Develop', develop(function(D) D.resetMasking() end))
C('virtualCopy', 'Create virtual copy', 'Photo', function()
  local LrApplication = import 'LrApplication'
  LrApplication.activeCatalog():createVirtualCopies()
end)

-- Color grading views
for _, v in ipairs({
  { 'cgView3way', '3-way', '3-way' },
  { 'cgViewShadow', 'Shadows', 'shadow' },
  { 'cgViewMidtone', 'Midtones', 'midtone' },
  { 'cgViewHighlight', 'Highlights', 'highlight' },
  { 'cgViewGlobal', 'Global', 'global' },
}) do
  C(v[1], 'Color grading: ' .. v[2], 'Color Grading', develop(function(D)
    D.setActiveColorGradingView(v[3])
  end))
end
C('revealColorGrading', 'Show Color Grading panel', 'Color Grading', develop(function(D)
  D.revealPanel('colorGradingPanel')
end))

-- Tools
for _, t in ipairs({
  { 'toolLoupe', 'Loupe tool', 'loupe' },
  { 'toolCrop', 'Crop tool', 'crop' },
  { 'toolSpot', 'Spot removal tool', 'dust' },
  { 'toolRedeye', 'Red eye tool', 'redeye' },
  { 'toolMasking', 'Masking tool', 'masking' },
}) do
  C(t[1], t[2], 'Tools', develop(function(D) D.selectTool(t[3]) end))
  if t[3] ~= 'loupe' then
    -- toggle: open the tool, or close it (back to no tool) if it is already open
    C(t[1] .. 'Toggle', t[2] .. ' on/off', 'Tools', develop(function(D)
      if D.getSelectedTool() == t[3] then D.selectTool('loupe') else D.selectTool(t[3]) end
    end))
  end
end

-- White balance presets, cycled
local WB = { 'As Shot', 'Auto', 'Daylight', 'Cloudy', 'Shade', 'Tungsten', 'Fluorescent', 'Flash' }
local function wbStep(dir)
  return develop(function(D)
    local cur = D.getValue('WhiteBalance')
    local i = 0
    for k, v in ipairs(WB) do if v == cur then i = k end end
    if i == 0 then i = (dir > 0) and 0 or 1 end -- Custom: start at the ends
    i = (i - 1 + dir) % #WB + 1
    D.setValue('WhiteBalance', WB[i])
  end)
end
C('wbNext', 'White balance: next preset', 'White Balance', wbStep(1))
C('wbPrev', 'White balance: previous preset', 'White Balance', wbStep(-1))
for _, v in ipairs(WB) do
  local id = 'wb' .. v:gsub(' ', '')
  C(id, 'White balance: ' .. v, 'White Balance', develop(function(D) D.setValue('WhiteBalance', v) end))
end

-- Masks (LrDevelopController mask calls; type / subtype as in the SDK)
local function inMasking(D)
  if D.getSelectedTool() ~= 'masking' then D.selectTool('masking') end
end
for _, m in ipairs({
  { 'Brush', 'brush' }, { 'Linear', 'gradient' }, { 'Radial', 'radialGradient' },
  { 'Subject', 'aiSelection', 'subject' }, { 'Sky', 'aiSelection', 'sky' },
  { 'People', 'aiSelection', 'people' }, { 'Objects', 'aiSelection', 'objects' },
  { 'Background', 'aiSelection', 'background' },
  { 'Color', 'rangeMask', 'color' }, { 'Luminance', 'rangeMask', 'luminance' },
}) do
  local name, typ, sub = m[1], m[2], m[3]
  C('maskNew' .. name, 'New mask: ' .. name:lower(), 'Masks', develop(function(D) D.createNewMask(typ, sub) end))
  C('maskAdd' .. name, 'Add to mask: ' .. name:lower(), 'Masks', develop(function(D) inMasking(D); D.addToCurrentMask(typ, sub) end))
  C('maskSub' .. name, 'Subtract from mask: ' .. name:lower(), 'Masks', develop(function(D) inMasking(D); D.subtractFromCurrentMask(typ, sub) end))
end
C('maskInvert', 'Invert mask', 'Masks', develop(function(D) (D.invertMask or D.toggleInvertMask)() end))
C('maskOverlay', 'Mask overlay on/off', 'Masks', develop(function(D) D.toggleOverlay() end))
-- toggleHideMask without an id does nothing: name the selected mask (or its selected component).
local function findMask(D, id)
  for _, m in ipairs(D.getAllMasks() or {}) do
    if m.ID == id then return m end
    for _, t in ipairs(m.Tools or {}) do if t.ID == id then return t end end
  end
end
local function hideSelected(getId, toggle)
  return develop(function(D)
    local id = D[getId]()
    local before = id and findMask(D, id)
    if not before then error('no mask is selected') end
    local was = before.Hidden
    D[toggle](id)
    local LrTasks = import 'LrTasks'
    LrTasks.sleep(0.2)
    local after = findMask(D, id)
    if after and after.Hidden == was then error(toggle .. ' did not change the mask') end
  end)
end
C('maskHide', 'Selected mask on/off', 'Masks', hideSelected('getSelectedMask', 'toggleHideMask'))
C('maskHideTool', 'Selected mask component on/off', 'Masks', hideSelected('getSelectedMaskTool', 'toggleHideMaskTool'))
C('masksEnable', 'All masks on/off', 'Masks', develop(function(D)
  D.setValue('EnableMaskGroupBasedCorrections', not D.getValue('EnableMaskGroupBasedCorrections'))
end))
C('maskDelete', 'Delete mask', 'Masks', develop(function(D) D.deleteMask() end))
local function maskStep(dir)
  return develop(function(D)
    inMasking(D)
    local list = D.getAllMasks() or {}
    if #list == 0 then return end
    local cur = D.getSelectedMask()
    local i = 0
    for k, m in ipairs(list) do
      local id = type(m) == 'table' and (m.ID or m.id) or m
      if id == cur then i = k end
    end
    i = (i - 1 + dir) % #list + 1
    local m = list[i]
    D.selectMask(type(m) == 'table' and (m.ID or m.id) or m)
  end)
end
C('maskNext', 'Next mask', 'Masks', maskStep(1))
C('maskPrev', 'Previous mask', 'Masks', maskStep(-1))

C('denoiseToggle', 'Denoise on/off', 'Detail', develop(function(_, core) if core then core:denoiseToggle() end end))

-- Misc develop
C('showClipping', 'Show clipping', 'Develop', develop(function(D) D.showClipping() end))
C('autoTone', 'Auto tone', 'Develop', develop(function(D) D.setAutoTone() end))
C('autoWhiteBalance', 'Auto white balance', 'Develop', develop(function(D) D.setAutoWhiteBalance() end))

return { params = params, cmds = cmds }
