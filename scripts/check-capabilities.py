#!/usr/bin/env python3
"""Cheap protocol release gate: classify every canonical command, pin generations."""
import json
import pathlib
import re

root = pathlib.Path(__file__).resolve().parents[1]
backend = root.parent / "qaqh-backend/crates/qaqh-ringing/src"
capabilities = json.loads((root / "docs/capabilities.json").read_text())
source = (backend / "command.rs").read_text()
names = set(re.findall(r'#\[serde\(rename = "([^"]+)"\)\]', source))
assert names == set(capabilities["commands"]), "Command vocabulary changed: update the capability review"
for command, entry in capabilities["commands"].items():
    assert entry["status"] in {"supported", "deferred", "excluded"}, command
    if entry["status"] != "supported":
        assert entry.get("reason"), command
frontend = "\n".join(path.read_text() for path in (root / "src").glob("*.rs"))
for command, variant in re.findall(r'#\[serde\(rename = "([^"]+)"\)\]\s*(\w+)', source):
    if capabilities["commands"][command]["status"] == "supported" and command != "session.resume":
        assert re.search(rf'RingingCommand::{variant}\b', frontend), f"{command} is classified as supported without a command call site"
revisions = (backend / "v2/types.rs").read_text()
for field, constant in [("command_revision", "COMMAND_CONTRACT_REVISION"), ("session_revision", "SESSION_CONTRACT_REVISION")]:
    actual = re.search(rf'pub const {constant}: &str = "([^"]+)"', revisions).group(1)
    assert capabilities[field] == actual, f"{field} changed: review GPUI and run the five release scenarios"
print(f"Capability review: {len(names)} commands, {capabilities['command_revision']} / {capabilities['session_revision']}")
