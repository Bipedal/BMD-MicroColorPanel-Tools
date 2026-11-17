"""Micro Color Panel tester: a live picture of the panel, built on panel.py.

    python panel_tester.py            # the real panel (USB if plugged in, else Bluetooth)
    python panel_tester.py --demo     # no panel: synthetic input, to see the UI
    python panel_tester.py --selftest # automatic checks of the UI logic, no window shown

What it shows: every key lights while held; knobs turn their pointer (4096 counts per
revolution) and light when pressed; each trackball's dot follows the ball and the
ring's marker follows the ring (300 steps per revolution). Status line: link (usb / bt /
down), battery, firmware. Event log on the right.
What you can change: the white backlight (slider, 0-100), and the 11 coloured key LEDs
(click a key that has an LED dot, or use All on / All off).
Close DaVinci Resolve first: both programs would write the lights.
The key layout follows the top-down product photo in the product manual.
Warning shown, not prevented: holding AUTO COLOR + STOP factory-resets the panel.
"""
import argparse
import math
import queue
import random
import sys
import threading
import time
import tkinter as tk

import panel as pm

BG, PANEL_BG = "#15171b", "#1f2228"
KEY_UP, KEY_DOWN, KEY_TEXT = "#2c3038", "#ffd54a", "#d8dbe0"
LED_OFF = "#3a3e46"
LED_ON = {"BYPASS": "#ff4040", "DISABLE": "#ff4040"}       # red keys; the rest are green
GREEN = "#43e06a"
ACCENT = "#5ab0ff"
# ----------------------------------------------------------------- layout
# Positions measured on the top-down product photo in the product manual (pixel centres
# in that 1246 x 730 image; panel body x 168-1078, y 282-668), scaled onto the canvas.
SC, OX, OY = 1.32, 168, 282
W, H = int((1078 - OX) * SC + 24), int((668 - OY) * SC + 30)


def _p(x, y):
    return (x - OX) * SC + 12, (y - OY) * SC + 12


KW, KH = 42 * SC, 28 * SC
_R = {1: 374, 2: 411, 3: 448, 4: 486, 5: 523, 6: 577, 7: 614}      # key row centres
_KEYPOS = {
    "AUTO_COLOR": (219, 1), "OFFSET": (269, 1), "COPY": (219, 2), "PASTE": (269, 2),
    "UNDO": (219, 3), "REDO": (269, 3), "DELETE": (219, 4), "RESET": (269, 4),
    "BYPASS": (219, 5), "DISABLE": (269, 5), "USER": (219, 6), "LOOP": (269, 6),
    "SHIFT_UP": (219, 7), "SHIFT_DOWN": (269, 7),
    "PLAY_STILL": (358, 1), "WIPE_STILL": (408, 1), "GRAB_STILL": (458, 1),
    "H_LITE": (547, 1), "VIEWER": (597, 1), "CURSOR": (647, 1), "SELECT": (698, 1),
    "ADD_NODE": (787, 1), "ADD_WINDOW": (837, 1), "ADD_KEYFRM": (887, 1),
    "RESET_LIFT": (573, 2), "RESET_GAMMA": (623, 2), "RESET_GAIN": (673, 2),
    "PREV_STILL": (976, 1), "NEXT_STILL": (1026, 1), "PREV_KEYFRM": (976, 2),
    "NEXT_KEYFRM": (1026, 2), "PREV_NODE": (976, 3), "NEXT_NODE": (1026, 3),
    "PREV_FRAME": (976, 4), "NEXT_FRAME": (1026, 4), "PREV_CLIP": (976, 5),
    "NEXT_CLIP": (1026, 5), "PLAY_REV": (976, 6), "PLAY_FWD": (1026, 6),
}
KEYS = {}      # name -> (x, y, w, h) on the canvas
for _n, (_x, _r) in _KEYPOS.items():
    _cx, _cy = _p(_x, _R[_r])
    KEYS[_n] = (_cx - KW / 2, _cy - KH / 2, KW, KH)
_cx, _cy = _p(1001, _R[7])                       # STOP: one wide key under the arrows
KEYS["STOP"] = (_cx - 46 * SC, _cy - KH / 2, 92 * SC, KH)
BALL_Y = _p(0, 535)[1]
BALL_X = {b: _p(x, 0)[0] for b, x in (("LEFT", 408), ("MIDDLE", 622), ("RIGHT", 836))}
RING_R, BALL_R = 90 * SC, 50 * SC
KNOB_Y, KNOB_R = _p(0, 313)[1], 18 * SC
KNOB_X = {n: _p(x, 0)[0] for n, x in zip(pm.KNOBS, (216, 288, 357, 427, 518, 588, 658, 727,
                                                     818, 888, 958, 1027))}
LABEL = {"AUTO_COLOR": "AUTO\nCOLOR", "PLAY_STILL": "PLAY\nSTILL", "WIPE_STILL": "WIPE\nSTILL",
         "GRAB_STILL": "GRAB\nSTILL", "H_LITE": "H/LITE", "ADD_NODE": "ADD\nNODE",
         "ADD_WINDOW": "ADD\nWINDOW", "ADD_KEYFRM": "ADD\nKEYFRM", "SHIFT_UP": "◤ SHIFT",
         "SHIFT_DOWN": "SHIFT ◢", "PLAY_REV": "◀", "STOP": "■", "PLAY_FWD": "▶",
         "RESET_LIFT": "RESET\nLIFT", "RESET_GAMMA": "RESET\nGAMMA", "RESET_GAIN": "RESET\nGAIN"}
for n in list(KEYS):
    if n not in LABEL:
        LABEL[n] = n.replace("_", "\n", 1).replace("_", " ")
assert set(KEYS) | set(pm.KNOBS) == set(pm.BUTTONS.values()), "layout must cover every button"
assert set(pm.LEDS) <= set(KEYS)


# ----------------------------------------------------------------- sources
class LiveSource:
    """Opens the panel in a background thread (retrying until found); events -> queue."""

    def __init__(self, q):
        self.q, self.p, self.stop = q, None, threading.Event()
        threading.Thread(target=self._connect, daemon=True).start()

    def _connect(self):
        while not self.stop.is_set():
            try:
                p = pm.Panel(transport="auto")
            except Exception as e:
                self.q.put(("status", "panel not found (%s); retrying" % type(e).__name__))
                self.stop.wait(2.0)
                continue
            self.p = p
            self.q.put(("link", p.link, True))
            self.q.put(("info", p.info()))
            self.q.put(("battery", p.battery()))
            p.start(lambda e: self.q.put(("event", e)))
            return

    def set_leds(self, names):
        return self.p.set_leds(names) if self.p else False

    def set_backlight(self, v):
        return self.p.set_backlight(v) if self.p else False

    def close(self):
        self.stop.set()
        if self.p:
            self.p.close()


class DemoSource:
    """Synthetic input for --demo and --selftest; records light changes."""

    def __init__(self, q, auto=True):
        self.q, self.leds, self.backlight, self.calls = q, set(), 100, []
        self.stop = threading.Event()
        q.put(("link", "demo", True))
        q.put(("info", {"version": "demo", "build": "-"}))
        q.put(("battery", {"charging": False, "level": 87}))
        if auto:
            threading.Thread(target=self._run, daemon=True).start()

    def _ev(self, kind, name, **d):
        self.q.put(("event", pm.Event(kind, name, time.time(), d)))

    def _run(self):
        names = list(pm.BUTTONS.items())
        while not self.stop.wait(0.05):
            r = random.random()
            if r < 0.04:
                bit, n = random.choice(names)
                self._ev("button", n, bit=bit, down=True)
                time.sleep(0.15)
                self._ev("button", n, bit=bit, down=False)
            elif r < 0.5:
                b = random.choice(list(pm.BALLS))
                t = time.time()
                self._ev("ball", b, dx=int(8 * math.cos(t)), dy=int(8 * math.sin(t * 1.3)))
            elif r < 0.7:
                self._ev("ring", random.choice(list(pm.RINGS)), delta=random.randint(-3, 6))
            else:
                self._ev("knob", random.choice(pm.KNOBS), delta=random.randint(-60, 90))

    def set_leds(self, names):
        self.leds = set(names)
        self.calls.append(("leds", sorted(self.leds)))
        return True

    def set_backlight(self, v):
        self.backlight = v
        self.calls.append(("backlight", v))
        return True

    def close(self):
        self.stop.set()


# ----------------------------------------------------------------- UI
class Tester:
    def __init__(self, root, source_factory):
        self.root = root
        root.title("Micro Color Panel tester")
        root.configure(bg=BG)
        self.q = queue.Queue()
        self.held, self.leds = set(), set()
        self.knob = {n: 0 for n in pm.KNOBS}
        self.knob_down = set()
        self.ball = {b: [0.0, 0.0] for b in pm.BALLS}
        self.ball_tot = {b: [0, 0] for b in pm.BALLS}
        self.ring = {b: 0 for b in pm.RINGS}
        self.counts = {"button": 0, "knob": 0, "ball": 0, "ring": 0}
        self.link, self.bat, self.fw = "connecting", None, None

        top = tk.Frame(root, bg=BG)
        top.pack(fill="both", expand=True)
        self.cv = tk.Canvas(top, width=W, height=H, bg=BG, highlightthickness=0)
        self.cv.pack(side="left", padx=8, pady=8)
        right = tk.Frame(top, bg=BG)
        right.pack(side="left", fill="y", padx=(0, 8), pady=8)
        tk.Label(right, text="Events", bg=BG, fg=KEY_TEXT, anchor="w").pack(fill="x")
        self.log = tk.Listbox(right, width=34, height=30, bg=PANEL_BG, fg=KEY_TEXT,
                              borderwidth=0, highlightthickness=0, font=("Consolas", 9))
        self.log.pack(fill="y", expand=True)

        bar = tk.Frame(root, bg=BG)
        bar.pack(fill="x", padx=8, pady=(0, 8))
        tk.Label(bar, text="Backlight", bg=BG, fg=KEY_TEXT).pack(side="left")
        self.bl = tk.Scale(bar, from_=0, to=100, orient="horizontal", length=260,
                           bg=BG, fg=KEY_TEXT, troughcolor=PANEL_BG, highlightthickness=0,
                           command=self._bl_changed)
        self.bl.set(100)
        self.bl.pack(side="left", padx=(6, 18))
        for txt, cmd in (("All LEDs on", lambda: self._set_leds(set(pm.LEDS))),
                         ("All LEDs off", lambda: self._set_leds(set())),
                         ("Reset counters", self._reset)):
            tk.Button(bar, text=txt, command=cmd, bg=PANEL_BG, fg=KEY_TEXT,
                      activebackground=KEY_UP, relief="flat", padx=10).pack(side="left", padx=4)
        self.status = tk.Label(bar, text="", bg=BG, fg=KEY_TEXT, anchor="e")
        self.status.pack(side="right")

        self._draw_static()
        self._bl_job = None
        self.src = source_factory(self.q)
        root.protocol("WM_DELETE_WINDOW", self.close)
        self._tick()

    # ---- drawing
    def _draw_static(self):
        c = self.cv
        c.create_rectangle(4, 4, W - 4, H - 4, fill=PANEL_BG, outline="#2a2e35", width=2)
        self.key_items, self.led_items, self.key_text = {}, {}, {}
        for n, (x, y, w, h) in KEYS.items():
            r = c.create_rectangle(x, y, x + w, y + h, fill=KEY_UP, outline="#3b404a",
                                   tags=("key", "k_" + n))
            big = n in ("PLAY_REV", "STOP", "PLAY_FWD")
            self.key_text[n] = c.create_text(x + w / 2, y + h / 2 + (2 if n in pm.LEDS else 0), text=LABEL[n],
                          fill=KEY_TEXT, font=("Segoe UI", 13 if big else 7, "bold"), justify="center",
                          tags=("key", "k_" + n))
            self.key_items[n] = r
            if n in pm.LEDS:
                self.led_items[n] = c.create_oval(x + w - 11, y + 4, x + w - 4, y + 11,
                                                  fill=LED_OFF, outline="", tags=("k_" + n,))
                c.tag_bind("k_" + n, "<Button-1>", lambda e, n=n: self._toggle_led(n))
                c.tag_bind("k_" + n, "<Enter>", lambda e: c.config(cursor="hand2"))
                c.tag_bind("k_" + n, "<Leave>", lambda e: c.config(cursor=""))
        self.knob_items = {}
        for n, x in KNOB_X.items():
            o = c.create_oval(x - KNOB_R, KNOB_Y - KNOB_R, x + KNOB_R, KNOB_Y + KNOB_R,
                              fill=KEY_UP, outline="#4a505b", width=2)
            ln = c.create_line(x, KNOB_Y, x, KNOB_Y - KNOB_R + 4, fill=ACCENT, width=3)
            c.create_text(x, KNOB_Y + KNOB_R + 10, text=n.replace("_", " "), fill=KEY_TEXT,
                          font=("Segoe UI", 7, "bold"))
            v = c.create_text(x, KNOB_Y + KNOB_R + 22, text="0", fill="#8a909a",
                              font=("Consolas", 8))
            self.knob_items[n] = (o, ln, v)
        self.ball_items = {}
        for b, x in BALL_X.items():
            c.create_oval(x - RING_R, BALL_Y - RING_R, x + RING_R, BALL_Y + RING_R,
                          fill="#2a2d33", outline="#4a505b", width=2)
            c.create_oval(x - BALL_R, BALL_Y - BALL_R, x + BALL_R, BALL_Y + BALL_R,
                          fill="#101215", outline="#3b404a", width=2)
            c.create_line(x - 6, BALL_Y, x + 6, BALL_Y, fill="#3b404a")
            c.create_line(x, BALL_Y - 6, x, BALL_Y + 6, fill="#3b404a")
            dot = c.create_oval(x - 9, BALL_Y - 9, x + 9, BALL_Y + 9, fill=ACCENT, outline="")
            mark = c.create_oval(0, 0, 0, 0, fill=KEY_DOWN, outline="")
            txt = c.create_text(x, BALL_Y + RING_R + 14, text="", fill="#8a909a",
                                font=("Consolas", 8))
            self.ball_items[b] = (dot, mark, txt)
        self.warn = c.create_text(W / 2, H - 18, text="", fill="#ff5050",
                                  font=("Segoe UI", 11, "bold"))
        self._redraw_all()

    def _redraw_all(self):
        for n in KEYS:
            self._draw_key(n)
        for n in pm.KNOBS:
            self._draw_knob(n)
        for b in pm.BALLS:
            self._draw_ball(b)

    def _draw_key(self, n):
        down = n in self.held
        self.cv.itemconfig(self.key_items[n], fill=KEY_DOWN if down else KEY_UP)
        self.cv.itemconfig(self.key_text[n], fill="#15171b" if down else KEY_TEXT)
        if n in self.led_items:
            col = (LED_ON.get(n, GREEN)) if n in self.leds else LED_OFF
            self.cv.itemconfig(self.led_items[n], fill=col)

    def _draw_knob(self, n):
        o, ln, v = self.knob_items[n]
        x = KNOB_X[n]
        a = 2 * math.pi * self.knob[n] / 4096.0
        self.cv.coords(ln, x, KNOB_Y, x + (KNOB_R - 4) * math.sin(a), KNOB_Y - (KNOB_R - 4) * math.cos(a))
        self.cv.itemconfig(o, fill=KEY_DOWN if n in self.knob_down else KEY_UP)
        self.cv.itemconfig(v, text=str(self.knob[n]))

    def _draw_ball(self, b):
        dot, mark, txt = self.ball_items[b]
        x = BALL_X[b]
        dx, dy = self.ball[b]
        self.cv.coords(dot, x + dx - 9, BALL_Y - dy - 9, x + dx + 9, BALL_Y - dy + 9)
        a = 2 * math.pi * self.ring[b] / 300.0
        mx, my = x + (RING_R - 18) * math.sin(a), BALL_Y - (RING_R - 18) * math.cos(a)
        self.cv.coords(mark, mx - 6, my - 6, mx + 6, my + 6)
        tx, ty = self.ball_tot[b]
        self.cv.itemconfig(txt, text="%s   x %d  y %d  ring %d" % (b, tx, ty, self.ring[b]))

    # ---- events
    def handle(self, item):
        kind = item[0]
        if kind == "event":
            e = item[1]
            k = e.kind
            if k in self.counts:
                self.counts[k] += 1
            if k == "button":
                down = e.data["down"]
                tgt = self.knob_down if e.name in pm.KNOBS else self.held
                (tgt.add if down else tgt.discard)(e.name)
                if e.name in pm.KNOBS:
                    self._draw_knob(e.name)
                elif e.name in KEYS:
                    self._draw_key(e.name)
                self._log("%s %s%s" % (e.name, "down" if down else "up",
                                       " (resync)" if e.data.get("synthetic") else ""))
            elif k == "knob":
                self.knob[e.name] += e.data["delta"]
                self._draw_knob(e.name)
                self._log("knob %s %+d" % (e.name, e.data["delta"]), merge=True)
            elif k == "ball":
                v = self.ball[e.name]
                v[0] = max(-BALL_R + 12, min(BALL_R - 12, v[0] + e.data["dx"] * 0.6))
                v[1] = max(-BALL_R + 12, min(BALL_R - 12, v[1] + e.data["dy"] * 0.6))
                self.ball_tot[e.name][0] += e.data["dx"]
                self.ball_tot[e.name][1] += e.data["dy"]
                self._draw_ball(e.name)
                self._log("ball %s %+d %+d" % (e.name, e.data["dx"], e.data["dy"]), merge=True)
            elif k == "ring":
                self.ring[e.name] += e.data["delta"]
                self._draw_ball(e.name)
                self._log("ring %s %+d" % (e.name, e.data["delta"]), merge=True)
            elif k == "battery":
                self.bat = e.data
                self._log("battery %d%%%s" % (e.data["level"], " charging" if e.data["charging"] else ""))
            elif k == "link":
                self.link = e.name if e.data["up"] else "down (reconnecting)"
                if not e.data["up"]:
                    self.held.clear()
                    self.knob_down.clear()
                    self._redraw_all()
                self._log("link %s %s" % (e.name, "up" if e.data["up"] else "lost"))
            warn = {"AUTO_COLOR", "STOP"} <= self.held
            self.cv.itemconfig(self.warn, text="AUTO COLOR + STOP held: this combination "
                               "factory-resets the panel. Release!" if warn else "")
        elif kind == "link":
            self.link = item[1]
        elif kind == "info":
            self.fw = (item[1] or {}).get("version")
        elif kind == "battery":
            self.bat = item[1]
        elif kind == "status":
            self.link = item[1]

    def _log(self, s, merge=False):
        s = time.strftime("%H:%M:%S ") + s
        if merge and self.log.size():
            first = self.log.get(0)
            if first[9:].split(" ")[:2] == s[9:].split(" ")[:2]:
                self.log.delete(0)
        self.log.insert(0, s)
        if self.log.size() > 300:
            self.log.delete(300, "end")

    def _tick(self):
        n = 0
        while n < 500:
            try:
                item = self.q.get_nowait()
            except queue.Empty:
                break
            self.handle(item)
            n += 1
        for b in pm.BALLS:                      # the dot drifts back to the centre
            v = self.ball[b]
            if abs(v[0]) > 0.5 or abs(v[1]) > 0.5:
                v[0] *= 0.92
                v[1] *= 0.92
                self._draw_ball(b)
        bat = "-" if not self.bat else "%d %%%s" % (self.bat["level"], " charging" if self.bat["charging"] else "")
        self.status.config(text="link: %s   battery: %s   firmware: %s   events: %d" % (
            self.link, bat, self.fw or "-", sum(self.counts.values())))
        self._job = self.root.after(16, self._tick)

    # ---- actions
    def _toggle_led(self, n):
        self._set_leds(self.leds ^ {n})

    def _set_leds(self, names):
        self.src.set_leds(names)    # if the link is down, panel.py keeps it and restores it
        self.leds = set(names)
        for n in pm.LEDS:
            self._draw_key(n)

    def _bl_changed(self, v):
        if self._bl_job:
            self.root.after_cancel(self._bl_job)
        self._bl_job = self.root.after(60, lambda: self.src.set_backlight(int(float(v))))

    def _reset(self):
        for n in pm.KNOBS:
            self.knob[n] = 0
        for b in pm.BALLS:
            self.ball[b] = [0.0, 0.0]
            self.ball_tot[b] = [0, 0]
            self.ring[b] = 0
        self._redraw_all()

    def close(self):
        try:
            self.src.close()
        finally:
            self.root.destroy()


# ----------------------------------------------------------------- self-test
def selftest():
    root = tk.Tk()
    root.withdraw()
    holder = {}
    app = Tester(root, lambda q: holder.setdefault("s", DemoSource(q, auto=False)))
    src = holder["s"]
    ok = True

    def chk(c, m):
        nonlocal ok
        ok &= bool(c)
        print("  %s  %s" % ("PASS" if c else "FAIL", m))

    def feed(kind, name, **d):
        app.handle(("event", pm.Event(kind, name, time.time(), d)))
    while not app.q.empty():
        app.handle(app.q.get())
    chk(app.link == "demo" and app.fw == "demo" and app.bat["level"] == 87, "status from source")
    feed("button", "OFFSET", bit=21, down=True)
    chk("OFFSET" in app.held and app.cv.itemcget(app.key_items["OFFSET"], "fill") == KEY_DOWN
        and app.cv.itemcget(app.key_text["OFFSET"], "fill") == "#15171b", "key lights while held, label dark")
    feed("button", "OFFSET", bit=21, down=False)
    chk(app.cv.itemcget(app.key_items["OFFSET"], "fill") == KEY_UP, "key dark after release")
    feed("button", "Y_LIFT", bit=8, down=True)
    chk("Y_LIFT" in app.knob_down and "Y_LIFT" not in app.held, "knob press lights the knob")
    feed("knob", "Y_LIFT", index=0, delta=1024)
    o, ln, v = app.knob_items["Y_LIFT"]
    x0, y0, x1, y1 = app.cv.coords(ln)
    chk(abs(x1 - x0 - (KNOB_R - 4)) < 0.5 and abs(y1 - y0) < 0.5, "quarter turn points right")
    feed("ball", "LEFT", dx=10, dy=0, raw_x=-160, raw_y=0)
    chk(app.ball["LEFT"][0] > 0 and app.ball_tot["LEFT"] == [10, 0], "ball dot moves right")
    feed("ring", "RIGHT", delta=75, raw=1200)
    dot, mark, txt = app.ball_items["RIGHT"]
    mx = sum(app.cv.coords(mark)[0::2]) / 2
    chk(abs(mx - (BALL_X["RIGHT"] + RING_R - 18)) < 0.5, "ring quarter turn: marker at 3 o'clock")
    app._toggle_led("OFFSET")
    app._toggle_led("BYPASS")
    app._toggle_led("OFFSET")
    chk(src.calls[-1] == ("leds", ["BYPASS"]) and app.cv.itemcget(app.led_items["BYPASS"], "fill") == "#ff4040",
        "LED toggles: %s" % src.calls)
    app._set_leds(set(pm.LEDS))
    chk(src.leds == set(pm.LEDS), "all LEDs on")
    app._bl_changed("40")
    root.update()
    time.sleep(0.1)
    root.update()
    chk(("backlight", 40) in src.calls, "backlight slider -> set_backlight(40)")
    feed("button", "AUTO_COLOR", bit=20, down=True)
    feed("button", "STOP", bit=59, down=True)
    chk("factory-resets" in app.cv.itemcget(app.warn, "text"), "reset-combo warning shown")
    feed("button", "STOP", bit=59, down=False)
    chk(app.cv.itemcget(app.warn, "text") == "", "warning cleared")
    feed("link", "usb", up=False, error="x")
    chk(app.link.startswith("down") and not app.held, "link loss clears held keys")
    app.close()
    print("selftest:", "ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--demo", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    root = tk.Tk()
    Tester(root, DemoSource if a.demo else LiveSource)
    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
