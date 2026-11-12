# Select an installed model and check Ollama

Index: [README.md](../README.md). The harness sends no `num_ctx`; Ollama's own window decides.

## 4. Select an installed model and check Ollama

Inspect inventory:

```bash
ollama --version
ollama list
curl --fail --silent --show-error http://127.0.0.1:11434/v1/models
```

If the endpoint is unavailable, start the Ollama app or `ollama serve`, never
a second daemon on an occupied port.

Seeded `web.total_tokens` 28000 and `web.evidence_tokens` 6000 require a
**32768-token** window. Every request inherits the window Ollama started
with; changing it needs a full relaunch.

```bash
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
```

For the Ollama.app GUI, run `launchctl setenv OLLAMA_CONTEXT_LENGTH 32768`,
then quit and relaunch it.

After choosing a tag (below), send a short generate without `num_ctx` and
confirm the loaded window:

```bash
curl --fail --silent --show-error http://127.0.0.1:11434/api/generate \
  -d '{"model":"REPLACE-WITH-EXACT-TAG","prompt":"hi","stream":false,"options":{"num_predict":1}}'
curl --fail --silent --show-error http://127.0.0.1:11434/api/ps
```

`/api/ps` must report `context_length` ≥ **32768**. Otherwise keep `web.total_tokens: 16000` and
`web.evidence_tokens: 3000` in the home `config.yaml`.

### Pin 32768 with a local derived tag

A Modelfile `PARAMETER num_ctx` wins over `OLLAMA_CONTEXT_LENGTH`. The shipped
chat and planner default is the derived tag below. It exists only after `ollama
create`; until then inventory reports it `tag_missing`, so `/model use` an
installed tag to chat.

```bash
ollama create qwen3.8:27b-mlx-cg -f scripts/qwen3.8-27b-mlx-cg.Modelfile
```

That file is `FROM qwen3.8:27b-mlx` with `PARAMETER num_ctx 32768`. Select it
with `/model use qwen3.8:27b-mlx-cg`; it is local-only, so do not pull it.
`/model profile` reports the declared and loaded window; chat uses the loaded one.
The alternative `gemma4-26b-a4b:ud-q4_k_xl-cg` is built the same way from
`scripts/bakeoff/modelfiles/gemma4-26b-a4b-ud-q4_k_xl-cg.Modelfile`.

Thinking is off through `models.local_llm.reasoning_effort: none`, not the
recipe. For a 65536 window on a 48 GB Mac, see the
[bake-off report](bakeoff/local-model-bakeoff-2026-10-10.md#windows-above-32768-on-a-48-gb-mac).

When no model is installed, `/model list` shows an example `ollama pull`
command to copy into a terminal; the console downloads nothing, and
`/api/ollama/pull` serves CLI/API clients. Check the execution backend
independently of the `-mlx` suffix:

```bash
ollama show qwen3.8:27b
```

Set `models.local_llm.model` (chat) and `agentic.deepagent_github.model`
(planner) to your exact tag in [first run](INSTALL.md#6-first-run); each has
its own `base_url`, and both require loopback. `/model use <tag>` changes chat
selection only; check `/status`.

### Exact-model diagnostics and optional fallback

Open **Setup and recovery** (Cmd and +), then **Check installed chat
and planner models**. Both use bounded `/models` inventories:

| State | Meaning / next step |
|---|---|
| `installed` | The exact ID is listed. Send a short chat to test inference; inventory does not run it. |
| `tag_missing` | Inventory answered without that exact tag. Correct the selection or install the model. |
| `unavailable` | Inventory failed, timed out, was oversized or malformed. Check the service and endpoint. |
| `not_probed` | The endpoint is not an eligible loopback URL, or the planner is a cloud provider. |

`/model` reports selection and `/model profile` the loaded window, proposed
limits and any tuning; `/model use <tag>` persists the selection without
downloading. With `models.local_llm.auto_tune: true`, it also loads the model,
times one short generate and applies per-model limits. Token budgets are
lowered only to fit a window below 32768; at or above it the configured values
stand within their ranges. Chat and same-model planner timeouts and
`web.synthesis_seconds` follow measured speed, up to one hour; a model
declaring no tools gets none. `web.research_seconds` only rises, to a cold
load plus `web.rounds` planning calls plus one `web.run_seconds` crawl.
Above 32768, `/model profile` proposes larger budgets to set in `config.yaml`
yourself; caps follow the measured window, up to 131072. Startup warms and
tunes the local selection. A changed config model clears a saved selection;
`/model use local` restores the configured local backend.

Cloud aliases are case-insensitive: `grok|xai`, `claude|anthropic` (configured
model IDs also work). A missing active key or disabled provider refuses before
saving. Save keys in Settings → API Keys and restart; never paste credentials
into chat. The first selection per account, provider and server run requires
confirmation naming `api.x.ai` or `api.anthropic.com`; restart resets to local.
Only the new message leaves; history, memory, attachments and tools stay local.
`/loop` refuses cloud, and the coding planner stays separate.

`/model status` reports provider, key presence and profile sources. Optional
`models.cloud_chat.<provider>.profile` sets context, output, temperature,
timeouts, tool-support metadata and RPM ([config](../assets/config.default.yaml)).
Output and timeouts only tighten the shared limits, spend gates are unchanged,
and profiles need a restart.
A reply must finish normally (Claude `end_turn`, Grok `completed`); truncation
fails with 502 and is not saved.

Grok Build ACP is not shipped; `scripts/grok-acp-probe.py` checks only a fake
runtime's launch contract.

Optional chat fallback ships disabled. To use another compatible local server,
merge its endpoint and exact inventory ID into these fields, then restart:

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

At startup the resolver keeps the primary only when inventory lists its
configured model; otherwise it tries the fallback. If neither is ready, startup
keeps a degraded primary so the console stays available. There is no
per-message retry or automatic download; restart to reevaluate
([resolver](../src/llm/backend.rs), [inventory](../src/llm/inventory.rs)).
