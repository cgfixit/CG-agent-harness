# desktop/capabilities

Tauri capability files grant webviews access to registered native commands.

- [`setup.json`](setup.json): grants only the local `setup` window the four
  permissions in [`../permissions/setup.toml`](../permissions/setup.toml).
  The sidecar-served `harness` window receives no native command grants.
