# profiles.toml

The mapping editor writes this file; you can also edit it by hand. The helper reloads it when it changes.
If it doesn't load, the previous settings stay in force and the editor shows what is wrong. It is in the
config folder (see [README](README.md)).

## Places and bindings

```toml
[default.knob]                       # every app
HUE = { do = "mouse.scroll" }

[photoshop.button]                   # one app
COPY = { do = "key", keys = "ctrl+shift+c" }
PASTE = "none"                       # does nothing here

[photoshop.layer.more.button]        # the same app while layer "more" is open
COPY = { do = "key", keys = "ctrl+c" }

[photoshop.dialog.ball]              # while one of its dialog boxes is in front
RIGHT = { do = "mouse.move" }

[photoshop.leds]
USER = "layer.more"
```

A table's name is a **place** and the kind of control: `button`, `knob`, `ring` or `ball`. Control names are
the ones on the panel with `_` for spaces; rings and balls are `LEFT`, `MIDDLE`, `RIGHT`. A knob's press is
the button with the knob's name.

A place is `default`, an app, or a part of an app (`app.part`, `app.part.tool`). The app's name is its
program file without `.exe`, lower-case. `[apps]` gives it another name:

```toml
[apps]
photoshop = ["photoshop.exe"]
```

**Which binding counts:** open layers first (the latest first), then no layer. In each, the most specific
place first: `app/part/tool`, `app/part`, `app`, `default`. Each control is looked up on its own. `"none"`
counts as found. A key's release goes to the binding that got its press, even if the place changed meanwhile.

A control an open layer doesn't set does nothing, except a key that opens or closes layers, unless the layer inherits:

```toml
[layers]
fine = { inherit = true }    # unset controls keep their normal job while "fine" is open
```

## Actions

| `do` | Controls | Fields |
|---|---|---|
| `none` | any | |
| `key` | button | `keys`: one combination, or several in order (`"shift+3, ctrl+z"`). `hold = true`: down while the key is held |
| `key` | knob, ring | `cw`, `ccw`: combinations. `per`: counts per tap (default 1/48 turn; a knob has 4096 counts per turn, a ring 300) |
| `mouse.move` | ball | `speed` (1), `accel` (1 = even, more = faster when rolled fast), `lock` (0 to 0.5: ignores the weaker axis) |
| `mouse.scroll` | ball, ring, knob | `axis` `"v"` / `"h"` (a ball: both), `speed` (1; negative = reversed), `mods`: keys held meanwhile, `h_mods`: the same for a ball's sideways axis, `notch = true`: whole notches only |
| `mouse.button` | button | `button`: left, right, middle. `keys`: a combination held meanwhile (`"space"` to pan) |
| `layer` | button | `name`, `mode`: `hold`, `toggle`, or `auto` (tap = stays open, hold = while held) |
| `sheet` | button | the cheat sheet. `mode`: `toggle` (default), `hold` |
| `restart` | button | held for a second: the helper quits and starts again; works even when the rest of it hangs |
| `preset` | button | `scope`: `app` (default), `global`. `name`: `next` (default) or a preset |
| `midi.cc` | knob, ring | `cc`, `channel` (1), `mode`: `relative` (two's complement, default), `offset`, `signbit`, `absolute`; `per_turn` (48 ticks, or 127 values) |
| `midi.cc` | button | `cc`, `channel`, `on` (127), `off` (0), `toggle` |
| `midi.note` | button | `note`, `channel`, `velocity` |
| `midi.xy` | ball | `x`, `y` (CC numbers), `channel`, `mode`, `per_unit` (0.5) |
| `mcu.fader` | knob, ring | `strip` (1 to 8, 9 = master), `per_turn` (0.5 of the travel), `touch_ms` (400) |
| `mcu.jog` | knob, ring | `per_turn` (48) |
| `mcu.button` | button | `button`: play, stop, record, …; `strip` for mute, solo, arm, select |
| `osc.value` | knob, ring | `address`, `per_turn` (default: the whole range), `min`, `max` (default: from OSCQuery, else 0 and 1), `mode`: `absolute` (default), `relative`; `type`: `float`, `int`, `bool` (default: from OSCQuery, else float) |
| `osc.value` | button | `address`, `value` (1), `mode`: `set` (default), `hold`, `toggle`; `off` (0), `type` |
| `osc.xy` | ball | `x`, `y` (addresses), `per_unit` (default: a thousandth of the range), `min`, `max`, `mode`, `type` |
| `osc.send` | button | `address`, `args` as text: `"1 0.5 true play 'two words'"` |

Any binding can also have `label = "…"`: the name shown for it in the editor and the cheat sheet.

Key names: letters, digits, `f1`–`f24`, `space enter tab esc backspace delete insert home end pageup pagedown
left up right down`, `comma period minus plus slash backslash semicolon quote bracketleft bracketright
backquote`, `numpad0`–`numpad9`, `printscreen pause capslock menu`, the volume and media keys, and the
modifiers `ctrl shift alt win`.

An action of an integration this build doesn't have is kept and does nothing; `--check` and the editor
mention it.

## Lights

Eleven keys have one: BYPASS and DISABLE red; USER, OFFSET, SHIFT_UP, SHIFT_DOWN, PLAY_STILL, WIPE_STILL,
H_LITE, VIEWER and CURSOR green. A key that opens a layer or the cheat sheet lights by itself while it is open.

```toml
[default.leds]
VIEWER = "midi.cc.20 > 64 and not layer.more"
CURSOR = "osc./mix/mute"
```

`layer.<name>`, `sheet`, `midi.cc.<n>`, `midi.note.<n>`, `midi.ch<c>.cc.<n>`, `osc./address`, numbers, words,
`true`, `false`; `== != > >= < <=`, `and`, `or`, `not`, `( )`. On its own a value counts as on when it is not
0, empty or `none`. The most specific place's rule wins, also over a layer key's own light.

## Settings

```toml
[settings]
backlight = 60                # 0 to 100
backlight_off_after_s = 0     # dark after this long without a touch; 0 = never
pointer_smoothing_ms = 40     # spreads the panel's bursts of ball movement; 0 = off
start_deg = 2                 # knobs and rings: a first movement up to this (degrees) does nothing; 0 = off
start_ball = 4                # balls: the same, in steps (not for the mouse pointer)
restart_keys = "USER+LOOP"    # held together for restart_hold_s seconds (2): the helper starts again; the keys keep their jobs. "" = off
sleep_after_min = 15          # on battery: the panel may switch itself off when neither it nor the PC was used this long; 0 = never
press_ms = 600                # after pressing a knob, small turns do nothing for this long; 0 = off
resolve = "yield"             # give the panel to DaVinci Resolve while it runs; or "share"
midi_out = "auto"             # "auto", "off", or part of a port's name
midi_in = "auto"
osc_target = "127.0.0.1:9000"
osc_listen = 0                # port for values that come back; 0 = wherever the messages came from
oscquery = "http://127.0.0.1:5678"
```
