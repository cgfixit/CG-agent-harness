# desktop/icons

App icon sources. `icon.png` and `icon.icns` are the `bundle.icon` entries in [`desktop/tauri.conf.json`](../tauri.conf.json).

- [`icon.icns`](icon.icns): the shipped icon set, copied into the `.app` by `package-desktop.sh`. Regenerate on macOS:
  `swift scripts/desktop-icon.swift /tmp/AppIcon.iconset && iconutil -c icns /tmp/AppIcon.iconset -o desktop/icons/icon.icns`
  ([the script](../../scripts/desktop-icon.swift) draws 16–1024px PNGs; `iconutil` packs them).
- [`icon.png`](icon.png): separate 2048px artwork, not script output (over 1 MiB, listed in the guard's `LARGE_FILES`).
- [`lol-im-a-fool-swift-vectorizes-the-image--OG-icon.png`](lol-im-a-fool-swift-vectorizes-the-image--OG-icon.png):
  the 512px original raster the Swift script was drawn from; referenced nowhere, not shipped.
