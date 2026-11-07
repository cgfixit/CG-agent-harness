# desktop/permissions

Tauri permission definitions for the setup window; [`../capabilities/setup.json`](../capabilities/setup.json)
grants them.

- [`setup.toml`](setup.toml): `desktop_status`, `retry_backend`, `prepare_cargo`,
  `check_models`, and `export_browser_certificate`, each mapping one identifier to exactly the command it allows.
