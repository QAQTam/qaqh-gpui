#!/usr/bin/env python3
"""Materialize exact sibling revisions without changing existing checkouts."""
import json
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
pins = json.loads((root / "packaging/dependencies.json").read_text())
for name, pin in pins.items():
    destination = root.parent / name
    if destination.exists():
        raise SystemExit(f"Refusing to overwrite existing source: {destination}")
    subprocess.run(["git", "init", str(destination)], check=True)
    subprocess.run(["git", "-C", str(destination), "fetch", "--depth=1", pin["url"], pin["revision"]], check=True)
    subprocess.run(["git", "-C", str(destination), "checkout", "--detach", "FETCH_HEAD"], check=True)
subprocess.run(["git", "-C", str(root.parent / "qaqh-backend"), "apply", str(root / "packaging/backend-client.patch")], check=True)
