"""Build the public copy of this project in ./publish, with private identifiers removed.

Usage: python tools/make_publish.py [--out publish]

* Copies only the published set: docs, panel.py, tests/, tools/, logs/, helper/, integrations/,
  lightroom/, docs/ (minus excluded).
* Not published (yet): the Premiere part (private/, premiere/), BRIDGE_PROTOCOL.md, the Lightroom link's test
  sessions. The run fails if Premiere's name turns up in what is published.
* Excluded: legacy files from another tool, the product manual (Blackmagic's copyright),
  raw BT captures (*_PRIVATE_*: pairing keys), offline simulation logs (*_OFFLINE_*),
  aborted runs (*_ABORTED_*) and empty logs,
  private_ids.json itself, __pycache__.
* The unit serial and the Bluetooth address are read from private_ids.json (never
  published) and replaced, in every text AND binary file, with same-length placeholders
  (serial -> 32 x 'X', BT address -> 'XXXXXXXXXXXX' / zero bytes). Same length keeps
  pcapng files valid. Forms covered: ASCII upper/lower, UTF-16LE (USB string descriptor),
  spaced hex dumps, colon form, raw and byte-reversed binary (BT HCI order).
* Afterwards the whole output is scanned again for every form; any hit fails the run.
"""
import argparse
import fnmatch
import json
import os
import shutil
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

INCLUDE_TOP = ["README.md", "LICENSE", "PANEL_PROTOCOL.md", "panel.py", "panel_tester.py",
               "requirements.txt",
               ".gitignore", ".gitattributes"]
INCLUDE_DIRS = ["tests", "tools", "logs", "helper", "integrations", "lightroom", "docs", ".github"]
EXCLUDE = ["*_PRIVATE_*", "*_OFFLINE_*", "*_ABORTED_*", "private_ids.json", "__pycache__", "*.pyc", "*.pdf",
           "target", "trace.log", "bridge_*", "lr_*", "premiere*", "editor_mock.py", "STOP_CAPTURE", "dist"]
# application parts that are not public yet
NOT_PUBLIC = [b"premiere"]


def forms(ids):
    """[(needle bytes, replacement bytes)], longest first."""
    out = []
    ser = ids["serial"].upper()
    ph = "X" * len(ser)
    for s in (ser, ser.lower()):
        out.append((s.encode(), ph.encode()))
        out.append((s.encode("utf-16le"), ph.encode("utf-16le")))
        out.append((" ".join("%02x" % c for c in s.encode()).encode(),
                    " ".join("%02x" % c for c in ph.encode()).encode()))
        out.append((s.encode().hex().encode(), ph.encode().hex().encode()))
    bt = ids["bt_address"].lower().replace(":", "")
    raw = bytes.fromhex(bt)
    for txt in (bt, bt.upper()):
        out.append((txt.encode(), b"X" * 12))
        out.append((":".join(txt[i:i + 2] for i in range(0, 12, 2)).encode(), b"XX:" * 5 + b"XX"))
    for r in (raw, raw[::-1]):
        out.append((" ".join("%02x" % c for c in r).encode(), b"00 " * 5 + b"00"))
        out.append((" ".join("%02X" % c for c in r).encode(), b"00 " * 5 + b"00"))
        out.append((r, b"\0" * 6))
    for a, b in out:
        assert len(a) == len(b), (a, b)
    return sorted(out, key=lambda x: -len(x[0]))


def excluded(rel):
    parts = rel.replace("\\", "/").split("/")
    return any(fnmatch.fnmatch(p, pat) for p in parts for pat in EXCLUDE)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(ROOT, "publish"))
    a = ap.parse_args()
    ids = json.load(open(os.path.join(ROOT, "private_ids.json"), encoding="utf-8"))
    fm = forms(ids)
    files = [f for f in INCLUDE_TOP if os.path.exists(os.path.join(ROOT, f))]
    for d in INCLUDE_DIRS:
        for r, _, fs in os.walk(os.path.join(ROOT, d)):
            files += [os.path.relpath(os.path.join(r, f), ROOT) for f in fs]
    files = sorted(f for f in files if not excluded(f)
                   and os.path.getsize(os.path.join(ROOT, f)) > 0)   # empty = killed runs
    if os.path.isdir(a.out):
        for name in os.listdir(a.out):            # keep a .git folder if one exists
            if name == ".git":
                continue
            p = os.path.join(a.out, name)
            shutil.rmtree(p) if os.path.isdir(p) else os.remove(p)
    changed = {}
    for rel in files:
        data = open(os.path.join(ROOT, rel), "rb").read()
        n = 0
        for needle, rep in fm:
            c = data.count(needle)
            if c:
                data = data.replace(needle, rep)
                n += c
        dst = os.path.join(a.out, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        open(dst, "wb").write(data)
        if n:
            changed[rel] = n
    # independent re-scan of the output
    hits = []
    for r, ds, fs in os.walk(a.out):
        ds[:] = [d for d in ds if d != ".git"]
        for f in fs:
            p = os.path.join(r, f)
            data = open(p, "rb").read()
            for needle, _ in fm:
                if needle in data:
                    hits.append((os.path.relpath(p, a.out), needle[:16]))
    for top in ("helper", "integrations", "lightroom", "docs"):
        for r, ds, fs in os.walk(os.path.join(a.out, top)):
            for f in fs:
                p = os.path.join(r, f)
                low = open(p, "rb").read().lower()
                hits += [(os.path.relpath(p, a.out), n) for n in NOT_PUBLIC if n in low]
    missing = [f for f in INCLUDE_TOP if not os.path.exists(os.path.join(ROOT, f))]
    print("copied %d files to %s" % (len(files), a.out))
    print("not present (skipped): %s" % (missing or "none"))
    print("redacted:")
    for k, v in changed.items():
        print("   %-55s %d" % (k, v))
    if hits:
        print("!! identifiers still present:", hits)
        return 1
    print("re-scan: no identifiers left")
    return 0


if __name__ == "__main__":
    sys.exit(main())
