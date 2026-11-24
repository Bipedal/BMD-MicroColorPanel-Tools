"""Offline test of lightroom/MicroPanel.lrplugin/Json.lua on a Lua 5.1 runtime (lupa)."""
import json
import math
import os
import sys

rt = None
for name in ("lupa.lua51", "lupa.luajit21", "lupa.luajit20"):
    try:
        rt = __import__(name, fromlist=["LuaRuntime"])
        print("Lua runtime:", name)
        break
    except ImportError:
        continue
if rt is None:
    sys.exit("FAIL: no Lua 5.1-compatible lupa runtime (lua51/luajit21/luajit20)")

lua = rt.LuaRuntime(unpack_returned_tuples=True)
here = os.path.dirname(os.path.abspath(__file__))
path = os.path.join(here, "..", "lightroom", "MicroPanel.lrplugin", "Json.lua")
with open(path, encoding="utf-8") as f:
    src = f.read()
J = lua.eval("function(src) return loadstring(src)() end")(src)
null = J.null
is_null = lua.eval("function(a, b) return rawequal(a, b) end")

passed = failed = 0


def check(label, cond, extra=""):
    global passed, failed
    if cond:
        passed += 1
    else:
        failed += 1
        print("FAIL:", label, extra)


def py(v):
    """Convert Lua value to Python (tables -> dict/list, null -> None)."""
    if is_null(v, null):
        return None
    if lupa_is_table(v):
        keys = list(v.keys())
        if keys and keys == list(range(1, len(keys) + 1)):
            return [py(v[k]) for k in keys]
        return {k: py(v[k]) for k in keys}
    return v


def lupa_is_table(v):
    return hasattr(v, "keys") and hasattr(v, "items")


def lt(pyval):
    """Build a Lua table from Python dict/list/None."""
    if pyval is None:
        return null
    if isinstance(pyval, dict):
        return lua.table_from({k: lt(v) for k, v in pyval.items()})
    if isinstance(pyval, list):
        return lua.table_from([lt(v) for v in pyval])
    return pyval


def enc(v):
    return J.encode(lt(v))


cases = [
    {"a": [1, 2, {"b": None, "c": [True, False]}], "d": {"e": {"f": "g"}}},
    {"s": "café", "emoji": "\U0001F600", "cjk": "日本"},
    {"esc": 'q"b\\s/\n\r\t\b\f\x01\x1f end'},
    {"n": [0.05, -3, 1e-7, 0, 123456789, 2.5, -0.5, 1e20]},
    {},
    {"x": None},
    [1, "two", None, [3], {"k": 4}],
]
for c in cases:
    s = enc(c)
    check("single line " + s, "\n" not in s and "\r" not in s, s)
    try:
        parsed = json.loads(s)
        check("python parses " + s, parsed == c, "%r != %r" % (parsed, c))
    except ValueError as e:
        check("python parses " + s, False, str(e))
    back = J.decode(s)
    check("lua roundtrip " + s, py(back) == c, "%r" % (py(back),))

# specific formatting
check("int no decimal", enc([3]) == "[3]", enc([3]))
check("negative int", enc([-3]) == "[-3]")
check("float 0.05", enc([0.05]) == "[0.05]", enc([0.05]))
check("empty table is object", enc({}) == "{}")
check("null encodes", J.encode(null) == "null")
check("control escape", J.encode("\x01") == '"\\u0001"', J.encode("\x01"))

# decoding specifics
check("surrogate pair", J.decode('"\\ud83d\\ude00"') == "\U0001F600".encode().decode("latin-1")
      or J.decode('"\\ud83d\\ude00"') == "\U0001F600")
check("\\u00e9", J.decode('"\\u00e9"') in ("é", "é".encode().decode("latin-1")))
check("exponent", J.decode("1E3") == 1000 and J.decode("-2.5e-1") == -0.25)
check("true/false", J.decode("[true,false]")[1] is True and J.decode("[true,false]")[2] is False)
check("null sentinel", is_null(J.decode("null"), null))
check("whitespace", py(J.decode(' \n{ "a" : [ 1 , 2 ] }\t ')) == {"a": [1, 2]})

# errors: must return nil + message with position, never throw
for label, bad in [("truncated obj", '{"a":1'), ("truncated str", '"abc'),
                   ("truncated arr", "[1,2"), ("trailing garbage", "{} x"),
                   ("bad escape", '"\\q"'), ("bad unicode", '"\\u12G4"'),
                   ("lone surrogate", '"\\ud83d"'), ("empty", ""),
                   ("bad literal", "tru"), ("bad number", "-"), ("leading zero", "01"),
                   ("missing colon", '{"a" 1}'), ("trailing comma", "[1,]"),
                   ("raw control", '"a\nb"')]:
    try:
        r = J.decode(bad)
        # unpack_returned_tuples gives (nil, msg) as a tuple
        v, msg = r if isinstance(r, tuple) else (r, None)
        check("error: " + label, v is None and isinstance(msg, str) and "position" in msg, repr(r))
    except Exception as e:
        check("error: " + label, False, "threw " + repr(e))

# encode errors
for label, bad in [("nan", float("nan")), ("inf", math.inf)]:
    try:
        J.encode([bad])
        check("encode error " + label, False, "no error")
    except Exception:
        check("encode error " + label, True)
try:
    J.encode(lua.table_from({1: "a", 3: "b"}))
    check("encode error sparse keys", False, "no error")
except Exception:
    check("encode error sparse keys", True)

# the system locale must not matter (byte strings, as Lightroom has them)
raw = rt.LuaRuntime(unpack_returned_tuples=True, encoding="latin-1")
R = raw.eval("function(src) return loadstring(src)() end")(src)
setlocale = raw.eval("os.setlocale")
known = 0
for loc in ("C", "English_United States.1252", "German_Germany.1252", "Japanese_Japan.932", ".UTF8", "",
            "de_DE.UTF-8", "ja_JP.UTF-8"):
    if setlocale(loc) is None:
        continue
    known += 1
    for b in range(0x80, 0x100):
        ch = chr(b)
        check("%s: encode %02x" % (loc, b), R.encode(ch) == '"%s"' % ch, repr(R.encode(ch)))
        check("%s: decode %02x" % (loc, b), R.decode('"%s"' % ch) == ch, repr(R.decode('"%s"' % ch)))
        v = R.decode(ch)
        check("%s: no value %02x" % (loc, b), isinstance(v, tuple) and v[0] is None, repr(v))
    for v in (0.05, -2.5, 1e-7, 1.5e20, 100.25):
        e = R.encode(raw.table(v))
        # old code gives a comma here, which json refuses
        try:
            check("%s: number %r" % (loc, v), json.loads(e) == [v], e)
            check("%s: number back %r" % (loc, v), R.decode(e)[1] == v, e)
        except Exception as x:
            check("%s: number %r" % (loc, v), False, "%s: %s" % (e, x))
    for c in range(0x20):
        check("%s: control %02x" % (loc, c), json.loads(R.encode(chr(c))) == chr(c))
        v = R.decode('"a%sb"' % chr(c))
        check("%s: raw control %02x" % (loc, c), isinstance(v, tuple) and v[0] is None, repr(v))
setlocale("C")
check("several locales tried", known >= 2, known)

print("%d passed, %d failed" % (passed, failed))
sys.exit(1 if failed else 0)
