# desktop/capabilities

Tauri capability files. A new window or command needs a capability entry here
or Tauri refuses the call.

- [`setup.json`](setup.json): binds the `setup` window to the four permissions in
  [`../permissions/setup.toml`](../permissions/setup.toml) and nothing else.
