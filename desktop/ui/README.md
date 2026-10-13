# desktop/ui

The setup window, bundled as Tauri's `frontendDist`. It can only call the
commands granted in [`../capabilities/`](../capabilities/README.md). The main
console is not here; it is [assets/static/harness.html](../../assets/static/harness.html),
served by the sidecar.

- [`index.html`](index.html): status, retry and open-console, check-models, guidance text.
- [`setup.js`](setup.js): invokes the granted Tauri commands and refreshes status; no other API.
- [`style.css`](style.css): the single dark stylesheet.
