"""Verified control maps. Source of truth until PANEL_PROTOCOL.md exists.

Button bits: report 0x02, bit number = byte_pos*8 + bit (byte_pos 1..8, LSB first).
Verified 2026-11-15 by tests/scan_buttons.py (logs/scan_buttons_20261115_201513.jsonl,
recheck of 44-46 and 57-59 in logs/scan_buttons_20261115_203109.jsonl);
labels from the DaVinci Resolve Micro Color Panel manual, p.10 drawing.
"""

BUTTONS = {
    # knob presses, left -> right
    8: "Y_LIFT", 9: "Y_GAMMA", 10: "Y_GAIN", 11: "CONTRAST",
    12: "PIVOT", 13: "MID_DETAIL", 14: "COLOR_BOOST", 15: "SHADOWS",
    16: "HIGHLIGHTS", 17: "SATURATION", 18: "HUE", 19: "LUM_MIX",
    # left columns: even = left column, odd = right column, one row per pair
    20: "AUTO_COLOR", 21: "OFFSET",
    22: "COPY", 23: "PASTE",
    24: "UNDO", 25: "REDO",
    26: "DELETE", 27: "RESET",
    28: "BYPASS", 29: "DISABLE",
    30: "USER", 31: "LOOP",
    32: "SHIFT_UP", 33: "SHIFT_DOWN",
    # above the balls
    34: "PLAY_STILL", 35: "WIPE_STILL", 36: "GRAB_STILL",
    37: "H_LITE", 38: "VIEWER", 39: "CURSOR", 40: "SELECT",
    41: "RESET_LIFT", 42: "RESET_GAMMA", 43: "RESET_GAIN",
    44: "ADD_NODE", 45: "ADD_WINDOW", 46: "ADD_KEYFRM",
    # right columns: odd = PREV column, even = NEXT column
    47: "PREV_STILL", 48: "NEXT_STILL",
    49: "PREV_KEYFRM", 50: "NEXT_KEYFRM",
    51: "PREV_NODE", 52: "NEXT_NODE",
    53: "PREV_FRAME", 54: "NEXT_FRAME",
    55: "PREV_CLIP", 56: "NEXT_CLIP",
    # transport
    57: "PLAY_REV", 58: "PLAY_FWD", 59: "STOP",
}

# Knob rotation: report 0x06, s32 LE at 1+4k, k = 0..11 left -> right.
# raw is always a multiple of 360 (raw_check 2026-11-15). Slot k = knob k left->right,
# CW = positive on all 12: verified 2026-11-16 (logs/axis_knobs_20261116_175340.jsonl).
# One full CW turn of Y LIFT (hand-measured): +1469160 raw = 4081 x 360.
# ASSUMED (accepted by user 2026-11-16, not measured exactly): 12-bit encoder,
# 4096 counts/rev, each count reported as 360 raw -> 1474560 raw/rev.
# Speed dependence untested.
# Slots 12-15 never move.
KNOBS = ["Y_LIFT", "Y_GAMMA", "Y_GAIN", "CONTRAST", "PIVOT", "MID_DETAIL",
         "COLOR_BOOST", "SHADOWS", "HIGHLIGHTS", "SATURATION", "HUE", "LUM_MIX"]

# Balls: report 0x05, s24 LE at 2+4k (byte 1+4k always 0 so far).
# Verified 2026-11-16, tests/axis_test.py --part balls (logs/axis_balls_20261116_174154.jsonl):
# identical sign convention on all three balls.
#   +X = top of ball rolled LEFT,  -X = RIGHT
#   +Y = top of ball rolled TOWARD you (down), -Y = AWAY (up)
# Raw values are multiples of 16.
BALL_SLOTS = {"LEFT": (0, 1), "MIDDLE": (3, 4), "RIGHT": (6, 7)}  # name -> (x_slot, y_slot)
# Rings: verified 2026-11-16 (logs/axis_rings_20261116_174515.jsonl): CW = positive on
# all three. Raw per report 16..64 (multiples of 16), reports every ~4-8 ms while turning.
# Counts per revolution: ~4800 raw (= ~300 steps of 16, ~1200 per quarter turn), NOT
# speed-dependent: middle ring 4816/4640/4816/4784 slow/fast/slow/fast, spread 4%
# (logs/axis_ringturns_20261116_174831.jsonl). Earlier 3104..5232 was hand error.
# Measured on the MIDDLE ring only; left/right assumed equal (same part), unverified.
RING_SLOTS = {"LEFT": 2, "MIDDLE": 5, "RIGHT": 8}

# Never hold together: factory reset, clears Bluetooth pairing (manual p.8).
FACTORY_RESET_COMBO = (20, 59)  # AUTO_COLOR + STOP
