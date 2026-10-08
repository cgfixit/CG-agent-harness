# Select an installed model and check Ollama

Index: [README.md](../README.md). Set `OLLAMA_CONTEXT_LENGTH=32768` before starting Ollama for seeded web budgets. The harness sends no `num_ctx`.

## 4. Select an installed model and check Ollama

Inspect the full local inventory before choosing a model:

```bash
ollama --version
ollama list
curl --fail --silent --show-error http://127.0.0.1:11434/v1/models
```

If the endpoint is unavailable, start the existing Ollama app or `ollama serve`.
Keep a terminal-started daemon running; use another terminal for the harness.
Never start a second daemon on an occupied port.

Seeded `web.total_tokens` 28000 and `web.evidence_tokens` 6000 require a
**32768-token** window. Chat, console `POST /api/ollama/pull` and startup
`keep_alive` warmup inherit the window Ollama started with. Changing it requires
a full quit and relaunch.

```bash
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
```

For the Ollama.app GUI, set the same variable on the app's process (for example
`launchctl setenv OLLAMA_CONTEXT_LENGTH 32768`), then fully quit and relaunch
the app. Do not import a CyClaw Ollama env file into this repository.

After you choose a tag (below) and send a short generate, confirm the loaded
window. Do not send `num_ctx` in the generate body:

```bash
curl --fail --silent --show-error http://127.0.0.1:11434/api/generate \
  -d '{"model":"REPLACE-WITH-EXACT-TAG","prompt":"hi","stream":false,"options":{"num_predict":1}}'
curl --fail --silent --show-error http://127.0.0.1:11434/api/ps
```

`/api/ps` must report `context_length` **32768**; chat warns when a turn
exceeds the loaded window. Otherwise keep `web.total_tokens: 16000` and
`web.evidence_tokens: 3000` in the home `config.yaml`.

Choose the **exact installed identifier**. The acceptance Mac had `qwen3.8:27b`
and a separate `qwen3.8:27b-mlx` with a different digest. Neither is assumed
installed elsewhere. With an empty inventory, deliberately download a model
suited to your hardware before continuing; allow for network and disk use.
Preserve working models. An `-mlx` suffix proves no execution backend. Inspect
the exact inventory tag:

```bash
ollama show qwen3.8:27b
```

Configure both `models.local_llm.model` (chat) and
`agentic.deepagent_github.model` (planner) with your chosen tag in [first run](INSTALL.md#6-first-run).
`/model use <tag>` changes chat selection only; inspect `/status` after restart.

The shipped `qwen3.8:27b-mlx` default does not verify installation. The app
does not require a 27B model. Chat uses `models.local_llm.base_url`; the
planner uses `agentic.deepagent_github.base_url`. Both local paths require a
loopback OpenAI-compatible service — which is also how a fine-tuned MLX model is
served; see [FINETUNE.md](FINETUNE.md) for the QLoRA workflow.

### Exact-model diagnostics and optional fallback

Open **Harness → Setup and recovery** (Cmd-,), then **Check installed chat
and planner models**. Both use bounded `/models` inventories:

| State | Meaning / next step |
|---|---|
| `installed` | The exact requested ID is listed. Send a short chat to test inference; inventory alone does not run it. |
| `tag_missing` | Inventory responded but omitted that exact tag. Correct the selection or deliberately install the intended model. |
| `unavailable` | Inventory failed, timed out, was oversized or malformed. Check the service and endpoint. |
| `not_probed` | The endpoint is not an eligible loopback URL, or the planner is a cloud provider. Setup does not probe cloud planners. |

`/model` reports selection and `/model profile` its loaded window, proposed
limits and any tuning; `/model use <tag>` persists it without downloading. With
`models.local_llm.auto_tune: true` it also loads the model, times one short
generate and applies per-model limits (budgets only tighten; the timeout follows
measured speed; a model without tools gets none).
This shared selection can override configuration, including fallback.
Check it against the active local endpoint. For explicit cloud chat, use
`/model use grok` (`grok-4.6`) or `/model use claude` (`claude-sonnet-5`) only
after an administrator saves the matching key and restarts. Cloud selection sends
only the newly typed message and does not change the coding planner. A cloud reply
counts only when the provider reports a normal finish (Claude `stop_reason: end_turn`,
Grok `status: completed`); a truncated one fails with HTTP 502 and is not saved.
Raise `models.cloud_chat.max_tokens` (default 4096) if that recurs.

**Grok Build ACP is not shipped.** `scripts/grok-acp-probe.py` and its CI test
check only a fake runtime's launch contract: isolated profile, answer-only ACP, no
MCP servers, no API-key fallback. Before any connection UI or subscription-billing
claim, test sign-in, entitlement and overage with a chosen account, prove two real
profiles stay isolated, and rerun `grok inspect --json` before each live run.
Runtime 1.0.34 advertised only the `grok.com` auth method, unlike xAI's
`cached_token`/`xai.api_key` example; recheck it for every runtime version.

Optional chat fallback ships disabled. If you already run another compatible
local server, merge its real endpoint and exact inventory ID into these existing
fields, then restart:

```yaml
models:
  local_llm:
    fallback:
      enabled: true
      provider: "lmstudio"
      base_url: "http://127.0.0.1:1234/v1"
      model: "replace-with-exact-installed-id"
      probe_timeout_sec: 1.5
```

At backend startup, the resolver keeps the primary only when its configured model
appears in inventory; otherwise it tries the configured fallback model. If neither
is ready, startup keeps a degraded primary so the console remains available.
With fallback off, startup selects the primary without this probe. There is no
per-message retry or automatic model download, and the coding planner retains its
separate configuration. Restart to reevaluate fallback after changing services.

Inventory and fallback probes are bounded by `models.local_llm.inventory` and
`probe_timeout_sec`, without proxies or redirects. See the
[resolver](../src/llm/backend.rs) and [inventory checks](../src/llm/inventory.rs).

Verify the loaded window with `/model profile`. See
[desktop details](DESKTOP.md) and [the native matrix](DESKTOP_ACCEPTANCE.md).
