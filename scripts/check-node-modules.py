#!/usr/bin/env python3
"""Fail unless strand-agent/node_modules holds exactly the versions package-lock.json pins.

    check-node-modules.py [strand-agent dir]

Runs inside the builder image after npm install, so a build never uses packages that drifted
from the committed lock file (for example an old node_modules volume). Optional packages that
do not apply to this platform (esbuild's per-OS binaries) may be absent.
"""
import json
import os
import sys

root = sys.argv[1] if len(sys.argv) > 1 else "."
packages = json.load(open(os.path.join(root, "package-lock.json")))["packages"]
checked, wrong = 0, []
for path, meta in packages.items():
    if not path.startswith("node_modules/") or meta.get("link"):
        continue
    manifest = os.path.join(root, path, "package.json")
    if not os.path.exists(manifest):
        if not meta.get("optional"):
            wrong.append(f"{path}: missing (lock pins {meta.get('version')})")
        continue
    installed = json.load(open(manifest)).get("version")
    checked += 1
    if installed != meta.get("version"):
        wrong.append(f"{path}: installed {installed}, lock pins {meta.get('version')}")

if wrong:
    print(f"node_modules does not match package-lock.json ({len(wrong)} package(s)):")
    print("\n".join("  " + w for w in wrong[:40]))
    sys.exit(1)
print(f"node_modules matches package-lock.json ({checked} packages)")
