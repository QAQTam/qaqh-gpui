#!/usr/bin/env bash
set -euo pipefail
source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
prefix="${1:-$HOME/.local}"
if [[ "$prefix" != /* ]]; then echo 'Installation prefix must be absolute' >&2; exit 2; fi
destination="$prefix/lib/qaqh-gpui"
if [[ "$(uname -s)/$(uname -m)" != Linux/x86_64 ]]; then echo 'This package supports Linux x86_64' >&2; exit 2; fi
python3 - "$source_dir" <<'PY'
import hashlib, json, pathlib, sys
source = pathlib.Path(sys.argv[1])
manifest = json.loads((source / "manifest.json").read_text())
for name in ["qaqh-gpui", "qaqh-daemon"]:
    digest = hashlib.sha256((source / name).read_bytes()).hexdigest()
    if digest != manifest["sha256"][name]:
        raise SystemExit(f"Package checksum mismatch: {name}")
PY
for binary in qaqh-gpui qaqh-daemon; do
  missing="$(ldd "$source_dir/$binary" | awk '/not found/{print $1}')"
  if [[ -n "$missing" ]]; then echo "Missing runtime libraries for $binary: $missing" >&2; exit 1; fi
done
install -d "$destination" "$prefix/bin" "$prefix/share/applications" "$prefix/share/icons/hicolor/scalable/apps"
install -m755 "$source_dir/qaqh-gpui" "$source_dir/qaqh-daemon" "$destination/"
install -m644 "$source_dir/manifest.json" "$destination/"
ln -sfn "$destination/qaqh-gpui" "$prefix/bin/qaqh-gpui"
python3 - "$source_dir" "$prefix" <<'PY'
import pathlib, sys
source, prefix = map(pathlib.Path, sys.argv[1:])
text = (source / "io.github.QAQTam.QAQHarness.desktop").read_text()
executable = str(prefix / "bin/qaqh-gpui").replace("\\", "\\\\").replace('"', '\\"').replace("`", "\\`").replace("$", "\\$").replace("%", "%%")
text = text.replace("Exec=qaqh-gpui", f'Exec="{executable}"')
(prefix / "share/applications/io.github.QAQTam.QAQHarness.desktop").write_text(text)
PY
install -m644 "$source_dir/io.github.QAQTam.QAQHarness.svg" "$prefix/share/icons/hicolor/scalable/apps/"
echo "Installed to $destination. Open QAQHarness from your application menu."
