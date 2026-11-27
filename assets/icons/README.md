# Colored Brand & Document Icons

Curated offline SVG assets vendored from [Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme) `v5.40.0` (MIT License, see `LICENSE-MIT-MATERIAL-ICON-THEME`).

## Normalization

Each SVG keeps its upstream `viewBox` and colors unmodified, with explicit `width="32" height="32"` attributes normalized on the root `<svg>` element so GPUI's `ImageAssetLoader` (`SMOOTH_SVG_SCALE_FACTOR = 2.0`) rasterizes every icon uniformly to `64×64` RGBA pixels regardless of source `viewBox` scale (e.g., `16×16` vs `960×960`).

Embedded at compile time via `crates/qaqh-gpui-kit/src/icons.rs` (`AppAssets`).
