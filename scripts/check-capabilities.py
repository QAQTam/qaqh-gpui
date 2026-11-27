#!/usr/bin/env python3
"""RingingV3 frontend contract & anti-corruption gate for qaqh-gpui."""
import pathlib
import re

root = pathlib.Path(__file__).resolve().parents[1]
v3_dir = root.parent / "qaqh-backend/crates/qaqh-ringing-v3"

rev = int((v3_dir / "protocol_rev.txt").read_text(encoding="utf-8").strip())
assert rev >= 1, f"invalid RingingV3 protocol_rev: {rev}"

control_src = (v3_dir / "src/control.rs").read_text(encoding="utf-8")
match = re.search(r"pub enum ControlRequest \{(.*?)\n\}", control_src, re.S)
assert match, "ControlRequest enum not found in qaqh-ringing-v3/src/control.rs"
domains = re.findall(r"^\s+([A-Z]\w+)\(", match.group(1), re.M)
assert domains, "no domains found in ControlRequest"

forbidden = re.compile(r"qaqh-ringing-contract|qaqh_ringing_contract|qaqh-types|qaqh_types|QAQH_USE_V2")
scan_files = list((root / "crates").rglob("*.rs")) + [
    root / "Cargo.toml",
    root / "scripts/ringing-probe.sh",
]
for path in scan_files:
    text = path.read_text(encoding="utf-8")
    hit = forbidden.search(text)
    assert hit is None, f"forbidden V2 reference `{hit.group(0)}` in {path}"

bridge_lines = len((root / "crates/qaqh-gpui-wire/src/bridge.rs").read_text(encoding="utf-8").splitlines())
assert bridge_lines <= 650, f"bridge.rs exceeds 650 lines: {bridge_lines}"

print(
    f"V3 capability gate passed: protocol_rev={rev}, "
    f"domains={len(domains)} ({', '.join(domains)}), bridge.rs={bridge_lines} lines"
)
