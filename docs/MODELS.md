# Select an installed model and check Ollama

Index: [setup-guide.md](../setup-guide.md). Seeded web token budgets assume `OLLAMA_CONTEXT_LENGTH=32768` is set before the Ollama process starts. The harness sends no `num_ctx`.

## 4. Select an installed model and check Ollama

Inspect the full local inventory before choosing a model:

```bash
ollama --version
ollama list
curl --fail --silent --show-error http://127.0.0.1:11434/v1/models
```

If the endpoint is unavailable, start the existing Ollama app or `ollama serve`.
Do not start a second daemon on an occupied port. Leave a terminal-started daemon
running in its terminal while you use another terminal for the harness.

Seeded `web.total_tokens` 28000 and `web.evidence_tokens` 6000 require a
**32768-token** Ollama window. The harness sends no `num_ctx`; it inherits
whatever window the Ollama process started with. Console `POST /api/ollama/pull`
and startup `keep_alive` warmup use the same rule. Set the env **before** that
process starts. Changing it later needs a full quit and relaunch, not a second
daemon on an occupied port.

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

`/api/ps` must report `context_length` **32768**. If you cannot raise the
window, keep `web.total_tokens: 16000` and `web.evidence_tokens: 3000` in the
home `config.yaml` instead of the seed values.

Choose the **exact installed identifier**. On the acceptance Mac it was
`qwen3.8:27b`; the separately installed `qwen3.8:27b-mlx` had a different digest.
Neither is assumed to exist on another machine. On a fresh machine with an empty
inventory, deliberately choose and download a model appropriate to your hardware
using Ollama before continuing; that is a separate network download and disk
allocation. Preserve working installed models. An `-mlx` suffix alone proves no
execution backend. Inspect the chosen model, using its actual inventory spelling:

```bash
ollama show qwen3.8:27b
```

Configure both `models.local_llm.model` (chat) and
`agentic.deepagent_github.model` (planner) with your chosen tag in [first run](INSTALL.md#6-first-run).
`/model use <tag>` changes chat selection only. Existing persisted chat selection
can override the chat config, so inspect `/status` after restart.

The shipped tag is `qwen3.8:27b-mlx`; it is a default string, not an installation
check. A 27B model is not required just to use the app. Do not copy an example tag
unless your inventory contains it. Chat uses `models.local_llm.base_url`; the
planner uses `agentic.deepagent_github.base_url`. Both local paths require a
loopback OpenAI-compatible service — which is also how a fine-tuned MLX model is
served; see [FINETUNE.md](FINETUNE.md) for the QLoRA workflow.

### Exact-model diagnostics and optional fallback

In the app, open **Harness → Setup and recovery** (Cmd-,), then **Check installed
chat and planner models**. Both checks use bounded `/models` inventories, with
these results:

| State | Meaning / next step |
|---|---|
| `installed` | The exact requested ID is listed. Send a short chat to test inference; inventory alone does not run it. |
| `tag_missing` | Inventory responded but omitted that exact tag. Correct the selection or deliberately install the intended model. |
| `unavailable` | Inventory failed, timed out, was oversized or malformed. Check the service and endpoint. |
| `not_probed` | The endpoint is not an eligible loopback URL, or the planner is a cloud provider. Setup does not probe cloud planners. |

`/model` reports selection; `/model use <tag>` persists a name without checking or
downloading it. The selection is shared across sessions and can override the tag
chosen from configuration, including the fallback tag. For local chat, check the
selected name against the endpoint actually in use. For explicit cloud chat, use
`/model use grok` (`grok-4.6`) or `/model use claude` (`claude-sonnet-5`) only
after an administrator saves the matching key and restarts. Cloud selection sends
only the newly typed message and does not change the coding planner.

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

Desktop inventory defaults to two seconds and 262,144 response bytes, configured
under `models.local_llm.inventory`; fallback uses `probe_timeout_sec` with the same
byte limit. Probes use no proxies or redirects. See the
[resolver](../src/llm/backend.rs) and [inventory checks](../src/llm/inventory.rs).

Keep historical model measurements separate from current settings. Native CLI
acceptance recorded context 8192 as a conservative fixture; desktop acceptance
recorded 32768. Neither is a copy-paste default. The seeded 28000/6000 web
budgets require a verified `OLLAMA_CONTEXT_LENGTH=32768` as above, not those
records. Refer to [historical native CLI record](MAC_ACCEPTANCE.md) and
[desktop details](DESKTOP.md). Historical native matrix: [DESKTOP_ACCEPTANCE.md](DESKTOP_ACCEPTANCE.md). Use those files only for each run's dated evidence.
Do not infer an execution backend from a tag suffix or change the running model
service just to match a historical measurement.
