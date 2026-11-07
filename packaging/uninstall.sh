#!/usr/bin/env bash
set -euo pipefail
prefix="${1:-$HOME/.local}"
if [[ "$prefix" != /* || "$prefix" == / ]]; then echo 'Use an absolute installation prefix' >&2; exit 2; fi
rm -f -- "$prefix/bin/qaqh-gpui" "$prefix/share/applications/io.github.QAQTam.QAQHarness.desktop" "$prefix/share/icons/hicolor/scalable/apps/io.github.QAQTam.QAQHarness.svg"
rm -f -- "$prefix/lib/qaqh-gpui/qaqh-gpui" "$prefix/lib/qaqh-gpui/qaqh-daemon" "$prefix/lib/qaqh-gpui/manifest.json"
rmdir -- "$prefix/lib/qaqh-gpui"
echo 'Application removed. Sessions, settings and drafts are preserved.'
