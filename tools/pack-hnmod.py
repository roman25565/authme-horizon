#!/usr/bin/env python3
"""Packs authme_horizon/ into dist/authme-<version>.hnmod (docs/api/mod-files.md)."""
import json, os, re, sys, zipfile
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
bundle = os.path.join(root, "authme_horizon")
manifest = open(os.path.join(bundle, "horizon-mixin.toml"), encoding="utf-8").read()
version = re.search(r'^version = "([^"]+)"', manifest, re.M).group(1)
horizon = re.search(r'^horizon_revision = "([0-9a-f]{40})"', manifest, re.M).group(1)
MIXINS = "ddd9700b21997c95d95ac6ee5ecafe3830cf622a"  # the horizon-mixins that Horizon commit pins
os.makedirs(os.path.join(root, "dist"), exist_ok=True)
out = os.path.join(root, "dist", f"authme-{version}.hnmod")
native = {"schema": 1, "id": "authme", "version": version, "game": "26.3", "horizon": horizon, "mixins": MIXINS}
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("horizon-native.json", json.dumps(native, indent=2) + "\n")
    for dirpath, dirs, files in os.walk(bundle):
        dirs[:] = sorted(d for d in dirs if d != "target")
        for f in sorted(files):
            if not (f.endswith((".rs", ".toml")) or f.startswith(("README", "LICENSE"))):
                continue
            full = os.path.join(dirpath, f)
            rel = os.path.relpath(full, bundle).replace(os.sep, "/")
            z.write(full, f"mixins/authme/{rel}")
print(out)
