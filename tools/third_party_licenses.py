"""Licence texts of everything that goes into micropanel.exe.

Usage: python tools/third_party_licenses.py [--out FILE]     (needs cargo; the crates must have been fetched)

Asks cargo which crates the released program (integrations/app, Windows) is built from and collects the
licence files they come with. A crate without one is named and fails the run, unless it is listed in TEXT_OF.
"""
import argparse
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TARGET = "x86_64-pc-windows-msvc"
NAME = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)", re.I)
# crates whose package leaves the text out: take it from a sibling of the same project
TEXT_OF = {}


def packages():
    run = lambda *a: subprocess.run(["cargo", *a], cwd=os.path.join(ROOT, "integrations"), capture_output=True, text=True, encoding="utf-8", check=True).stdout
    meta = {(p["name"], p["version"]): p for p in json.loads(run("metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET))["packages"]}
    tree = run("tree", "--locked", "-p", "micropanel-app", "-e", "normal", "--target", TARGET, "--prefix", "none", "--format", "{p}")
    seen = sorted({tuple(line.split()[:2]) for line in tree.splitlines() if line.strip()})
    out = []
    for name, version in seen:
        p = meta[(name, version.lstrip("v"))]
        if p["source"]:                       # not our own
            out.append(p)
    return out


def texts(folder):
    found = []
    for r, ds, fs in os.walk(folder):
        ds[:] = sorted(d for d in ds if d not in ("target", "tests", "examples", ".git"))
        for f in sorted(fs):
            if NAME.match(f) and not f.endswith((".rs", ".toml")):
                found.append(os.path.join(r, f))
    return found


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="THIRD_PARTY_LICENSES.txt")
    a = ap.parse_args()
    pkgs = packages()
    at = {p["name"]: os.path.dirname(p["manifest_path"]) for p in pkgs}
    parts = ["micropanel.exe is in the public domain (CC0 1.0). It contains the following libraries,\n"
             "each under its own licence, and the Rust standard library (MIT or Apache-2.0,\n"
             "Copyright the Rust Project contributors, https://www.rust-lang.org/policies/licenses).\n"]
    missing, same = [], {}
    for p in pkgs:
        files = texts(at[p["name"]]) or texts(at.get(TEXT_OF.get(p["name"]), "/nonexistent"))
        if not files:
            missing.append("%s %s (%s)" % (p["name"], p["version"], p["license"]))
        head = "%s\n%s %s\nLicence: %s\n%s\n%s\n" % ("=" * 78, p["name"], p["version"], p["license"], p["repository"] or "", "=" * 78)
        body = []
        for f in files:
            t = open(f, encoding="utf-8", errors="replace").read().strip()
            key = re.sub(r"\s+", " ", t)
            rel = os.path.relpath(f, at[p["name"]]).replace("\\", "/")
            if key in same:
                body.append("[%s: the same text as %s]" % (rel, same[key]))
            else:
                same[key] = "%s %s, %s" % (p["name"], p["version"], rel)
                body.append("[%s]\n%s" % (rel, t))
        parts.append(head + "\n\n".join(body) + "\n")
    if missing:
        print("no licence text in: " + "; ".join(missing))
        return 1
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(parts))
    print("%d libraries, %d different texts -> %s" % (len(pkgs), len(same), a.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
