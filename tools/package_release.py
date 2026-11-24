"""Put a Windows release together: dist/MicroPanel-<version>-windows-x64.zip and its .sha256.

Usage: python tools/package_release.py [--exe FILE] [--tag v0.1.0] [--out dist]

The exe comes from integrations/ (see .github/workflows/release.yml for the build flags). The run fails if
the exe needs the Visual C++ runtime, contains the name of the account it was built with, or if --tag
doesn't match its version.
"""
import argparse
import getpass
import hashlib
import os
import re
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PLUGIN = os.path.join(ROOT, "lightroom", "MicroPanel.lrplugin")


def version():
    t = open(os.path.join(ROOT, "integrations", "app", "Cargo.toml"), encoding="utf-8").read()
    return re.search(r'^version = "([^"]+)"', t, re.M).group(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=os.path.join(ROOT, "integrations", "target", "release", "micropanel.exe"))
    ap.add_argument("--tag")
    ap.add_argument("--out", default=os.path.join(ROOT, "dist"))
    a = ap.parse_args()
    v = version()
    if a.tag and a.tag != "v" + v:
        return "the tag %s doesn't match version %s in integrations/app/Cargo.toml" % (a.tag, v)
    exe = open(a.exe, "rb").read()
    low = exe.lower()
    if b"vcruntime140" in low:
        return "the exe needs the Visual C++ runtime: build with -C target-feature=+crt-static"
    user = getpass.getuser().lower()
    for form in (user.encode(), user.encode("utf-16le")):
        if re.search(rb"users[\\/]+" + re.escape(form), low) or (len(user) > 4 and form in low):
            return "the exe contains the account name %r: build with --remap-path-prefix" % user
    os.makedirs(a.out, exist_ok=True)
    lic = os.path.join(a.out, "THIRD_PARTY_LICENSES.txt")
    subprocess.run([sys.executable, os.path.join(ROOT, "tools", "third_party_licenses.py"), "--out", lic], check=True)
    name = "MicroPanel-%s-windows-x64" % v
    z = os.path.join(a.out, name + ".zip")
    files = [(a.exe, "micropanel.exe"), (os.path.join(ROOT, "tools", "release_readme.txt"), "README.txt"),
             (os.path.join(ROOT, "LICENSE"), "LICENSE.txt"), (lic, "THIRD_PARTY_LICENSES.txt")]
    files += [(os.path.join(PLUGIN, f), "Lightroom plugin (only for installing by hand)/MicroPanel.lrplugin/" + f) for f in sorted(os.listdir(PLUGIN))]
    with zipfile.ZipFile(z, "w", zipfile.ZIP_DEFLATED) as out:
        for src, dst in files:
            data = open(src, "rb").read()
            if dst.endswith(".txt"):
                data = data.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
            out.writestr(name + "/" + dst, data)
    digest = hashlib.sha256(open(z, "rb").read()).hexdigest()
    with open(z + ".sha256", "w", newline="\n") as f:
        f.write("%s  %s\n" % (digest, os.path.basename(z)))
    print("%s: %d files, %d bytes\nsha256 %s" % (z, len(files), os.path.getsize(z), digest))
    return 0


if __name__ == "__main__":
    sys.exit(main())
