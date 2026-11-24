-- Json.lua: minimal JSON encode/decode for Lua 5.1 (Lightroom Classic).
-- Public domain (CC0). Usage: local Json = require 'Json'
-- Json.encode(v) -> string (errors on bad input)
-- Json.decode(s) -> value, or nil + "message at position N" (never throws)
-- Json.null is the sentinel for JSON null.
-- Nothing here may depend on the system locale: %c can match bytes of UTF-8 characters, and
-- number formatting and tonumber use the locale's decimal sign.

local Json = {}
local null = setmetatable({}, { __tostring = function() return "null" end })
Json.null = null

local floor, format, char, byte = math.floor, string.format, string.char, string.byte

---------------------------------------------------------------- encode
local escapes = { ['"'] = '\\"', ['\\'] = '\\\\', ['\n'] = '\\n', ['\r'] = '\\r',
                  ['\t'] = '\\t', ['\b'] = '\\b', ['\f'] = '\\f' }

local function encodeString(s)
  return '"' .. s:gsub('[%z\1-\31"\\]', function(c)
    return escapes[c] or format('\\u%04x', byte(c))
  end) .. '"'
end

local function encodeValue(v, out)
  local t = type(v)
  if v == null then
    out[#out + 1] = "null"
  elseif t == "nil" then
    out[#out + 1] = "null"
  elseif t == "boolean" then
    out[#out + 1] = v and "true" or "false"
  elseif t == "number" then
    if v ~= v or v == math.huge or v == -math.huge then
      error("cannot encode NaN or infinity")
    end
    if v == floor(v) and v > -1e15 and v < 1e15 then
      out[#out + 1] = format("%d", v)
    else
      out[#out + 1] = (format("%.14g", v):gsub("[^0-9eE+-]+", "."))
    end
  elseif t == "string" then
    out[#out + 1] = encodeString(v)
  elseif t == "table" then
    local n, count = #v, 0
    for _ in pairs(v) do count = count + 1 end
    if n >= 1 and count == n then
      out[#out + 1] = "["
      for i = 1, n do
        if i > 1 then out[#out + 1] = "," end
        encodeValue(v[i], out)
      end
      out[#out + 1] = "]"
    else
      out[#out + 1] = "{"
      local first = true
      for k, val in pairs(v) do
        if type(k) ~= "string" then error("object keys must be strings") end
        if not first then out[#out + 1] = "," end
        first = false
        out[#out + 1] = encodeString(k) .. ":"
        encodeValue(val, out)
      end
      out[#out + 1] = "}"
    end
  else
    error("cannot encode type " .. t)
  end
end

function Json.encode(v)
  local out = {}
  encodeValue(v, out)
  return table.concat(out)
end

---------------------------------------------------------------- decode
local function utf8Char(cp)
  if cp < 0x80 then return char(cp) end
  if cp < 0x800 then
    return char(0xC0 + floor(cp / 64), 0x80 + cp % 64)
  end
  if cp < 0x10000 then
    return char(0xE0 + floor(cp / 4096), 0x80 + floor(cp / 64) % 64, 0x80 + cp % 64)
  end
  return char(0xF0 + floor(cp / 262144), 0x80 + floor(cp / 4096) % 64,
              0x80 + floor(cp / 64) % 64, 0x80 + cp % 64)
end

local simple = { ['"'] = '"', ['\\'] = '\\', ['/'] = '/', b = '\b', f = '\f',
                 n = '\n', r = '\r', t = '\t' }

local function toNumber(s)
  return tonumber(s) or tonumber((s:gsub("%.", (format("%.1f", 0.5):match("^0(.+)5$")))))
end

local function fail(pos, msg) error({ pos = pos, msg = msg }, 0) end

local function skip(s, i) return s:find("[^ \t\r\n]", i) or #s + 1 end

local parseValue

local function parseString(s, i) -- s:sub(i,i) == '"'
  local buf, j = {}, i + 1
  while true do
    local k = s:find('["\\%z\1-\31]', j)
    if not k then fail(i, "unterminated string") end
    buf[#buf + 1] = s:sub(j, k - 1)
    local c = s:sub(k, k)
    if c == '"' then return table.concat(buf), k + 1 end
    if c ~= "\\" then fail(k, "control character in string") end
    local e = s:sub(k + 1, k + 1)
    if simple[e] then
      buf[#buf + 1] = simple[e]
      j = k + 2
    elseif e == "u" then
      local hex = s:match("^%x%x%x%x", k + 2)
      if not hex then fail(k, "bad \\u escape") end
      local cp = tonumber(hex, 16)
      j = k + 6
      if cp >= 0xD800 and cp < 0xDC00 then
        local lo = s:match("^\\u(%x%x%x%x)", j)
        lo = lo and tonumber(lo, 16)
        if not lo or lo < 0xDC00 or lo > 0xDFFF then fail(k, "bad surrogate pair") end
        cp = 0x10000 + (cp - 0xD800) * 1024 + (lo - 0xDC00)
        j = j + 6
      elseif cp >= 0xDC00 and cp <= 0xDFFF then
        fail(k, "unpaired low surrogate")
      end
      buf[#buf + 1] = utf8Char(cp)
    else
      fail(k, "bad escape")
    end
  end
end

function parseValue(s, i)
  i = skip(s, i)
  local c = s:sub(i, i)
  if c == "{" then
    local obj = {}
    i = skip(s, i + 1)
    if s:sub(i, i) == "}" then return obj, i + 1 end
    while true do
      if s:sub(i, i) ~= '"' then fail(i, "expected string key") end
      local key
      key, i = parseString(s, i)
      i = skip(s, i)
      if s:sub(i, i) ~= ":" then fail(i, "expected ':'") end
      obj[key], i = parseValue(s, i + 1)
      i = skip(s, i)
      local d = s:sub(i, i)
      if d == "}" then return obj, i + 1 end
      if d ~= "," then fail(i, "expected ',' or '}'") end
      i = skip(s, i + 1)
    end
  elseif c == "[" then
    local arr, n = {}, 0
    i = skip(s, i + 1)
    if s:sub(i, i) == "]" then return arr, i + 1 end
    while true do
      n = n + 1
      arr[n], i = parseValue(s, i)
      i = skip(s, i)
      local d = s:sub(i, i)
      if d == "]" then return arr, i + 1 end
      if d ~= "," then fail(i, "expected ',' or ']'") end
      i = i + 1
    end
  elseif c == '"' then
    return parseString(s, i)
  elseif c == "-" or c:match("%d") then
    local num = s:match("^-?%d+%.?%d*", i)
    if not num then fail(i, "bad number") end
    local e = s:match("^[eE][+-]?%d+", i + #num)
    if num:sub(-1) == "." then fail(i, "bad number") end
    if num:match("^-?0%d") then fail(i, "leading zero in number") end
    num = num .. (e or "")
    local v = toNumber(num)
    if not v then fail(i, "bad number") end
    return v, i + #num
  end
  for word, val in pairs({ ["true"] = true, ["false"] = false }) do
    if s:sub(i, i + #word - 1) == word then return val, i + #word end
  end
  if s:sub(i, i + 3) == "null" then return null, i + 4 end
  if c == "" then fail(i, "unexpected end of input") end
  fail(i, "unexpected character '" .. c .. "'")
end

function Json.decode(s)
  if type(s) ~= "string" then return nil, "input is not a string" end
  local ok, v, i = pcall(function()
    local val, pos = parseValue(s, 1)
    pos = skip(s, pos)
    if pos <= #s then fail(pos, "trailing garbage") end
    return val, pos
  end)
  if ok then return v end
  if type(v) == "table" and v.msg then
    return nil, v.msg .. " at position " .. v.pos
  end
  return nil, "decode error: " .. tostring(v)
end

return Json
