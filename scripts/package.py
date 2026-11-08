#!/usr/bin/env python3
"""Assemble an installable local candidate; no network publication."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import tarfile
import io
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument("--profile", choices=["debug", "release"], default="release")
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
target = root / "target" / args.profile
destination = root / "dist/qaqh-gpui-linux-x86_64"
destination.mkdir(parents=True, exist_ok=True)
version = subprocess.check_output([str(target / "qaqh-gpui"), "--version"], text=True).strip()
if "acceptance" in version:
    raise SystemExit("Build the client without the acceptance feature before packaging")
manifest = {"version": tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"], "client_build": version, "profile": args.profile, "platform": "linux-x86_64-wayland", "sources": {}, "sha256": {}}
for name, source in [("qaqh-gpui", root), ("qaqh-backend", root.parent / "qaqh-backend"), ("mutil-ai", root.parent / "mutil-ai")]:
    revision = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    patch = subprocess.check_output(["git", "-C", str(source), "diff", "--binary", "HEAD"])
    patch_name = f"{name}.patch"
    (destination / patch_name).write_bytes(patch)
    manifest["sources"][name] = {"revision": revision, "patch_sha256": hashlib.sha256(patch).hexdigest()}
    # Include exact source bytes, including new files, so a dirty candidate is reproducible.
    paths = subprocess.check_output(["git", "-C", str(source), "ls-files", "--cached", "--others", "--exclude-standard", "-z"]).decode().split("\0")
    tree = hashlib.sha256()
    with tarfile.open(destination / f"{name}-source.tar.gz", "w:gz") as bundle:
        for relative in sorted(set(filter(None, paths))):
            path = source / relative
            if not path.is_file():
                continue
            data = path.read_bytes()
            tree.update(relative.encode() + b"\0" + hashlib.sha256(data).digest())
            info = tarfile.TarInfo(f"{name}/{relative}")
            info.size = len(data)
            info.mode = path.stat().st_mode & 0o777
            bundle.addfile(info, io.BytesIO(data))
    manifest["sources"][name]["source_tree_sha256"] = tree.hexdigest()
for name in ["qaqh-gpui", "qaqh-daemon"]:
    shutil.copy2(target / name, destination / name)
for name in ["install.sh", "uninstall.sh", "io.github.QAQTam.QAQHarness.desktop", "io.github.QAQTam.QAQHarness.svg"]:
    shutil.copy2(root / "packaging" / name, destination / name)
for name in ["LICENSE-MIT", "LICENSE-APACHE"]:
    shutil.copy2(root / name, destination / name)
shutil.copy2(root / "assets/queries/README.md", destination / "THIRD-PARTY.md")
shutil.copy2(root / "packaging/dependencies.json", destination / "dependencies.json")
for path in sorted(destination.iterdir()):
    if path.name != "manifest.json":
        manifest["sha256"][path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
(destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
archive = root / f"dist/{destination.name}-{args.profile}.tar.gz"
with tarfile.open(archive, "w:gz") as tar:
    tar.add(destination, arcname=destination.name)
(archive.with_suffix(archive.suffix + ".sha256")).write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n")
print(archive)
