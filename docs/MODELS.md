# Select an installed model and check Ollama

## 4. Select an installed model and check Ollama

Run `ollama list` and check `http://127.0.0.1:11434/v1/models`. Start Ollama only
if the listener is down. The harness sends no `num_ctx`. For the shipped web
budgets, set `OLLAMA_CONTEXT_LENGTH=32768` before starting Ollama, or use a
derived tag. For Ollama.app, use `launchctl setenv OLLAMA_CONTEXT_LENGTH 32768`,
then quit and relaunch. After a short generate on the exact tag, check
`context_length` in `/api/ps`. Inventory and tag names alone prove neither
loaded context nor inference backend. For a smaller manually managed
window, reduce the web budgets and inspect `/model profile` before using
long research turns.

```bash
curl --fail --silent --show-error http://127.0.0.1:11434/api/generate \
  -d '{"model":"REPLACE-WITH-EXACT-TAG","prompt":"hi","stream":false,"options":{"num_predict":1}}'
curl --fail --silent --show-error http://127.0.0.1:11434/api/ps
```

### Pin 32768 with a local derived tag

```bash
ollama create qwen3.8:27b-mlx-cg -f scripts/qwen3.8-27b-mlx-cg.Modelfile
```

Install the recipe's base tag first. The recipe pins `num_ctx 32768`, which
takes precedence over the environment. The derived tag is the shipped chat
and planner default. `models.local_llm.reasoning_effort: none` controls
thinking separately. Choose the exact chat tag with `/model use <tag>`.
Browser tasks follow that local selection. The planner owns its own
`agentic.deepagent_github` model, provider and endpoint. Local endpoints are
loopback-only. Ollama cloud tags (`:cloud`, `-cloud`) are refused by local
inference and pull paths.

### Exact-model diagnostics and optional fallback

**Setup and recovery → Check installed chat and planner models** probes
`/models`. `installed` means the exact tag was listed. `tag_missing` means
inventory answered without it. `unavailable` means the probe failed.
`not_probed` means the endpoint or provider is ineligible. The console offers
terminal pull commands. `POST /api/ollama/pull` uses an idle deadline.

`/model profile` reports declared and loaded context and effective limits.
`auto_tune` measures a short local generate at startup and on selection.
It tightens token budgets below 32768, tunes eligible timeouts and suppresses
tools for models declaring no support. Larger windows allow configured
increases. The short sample measures latency and reads the loaded window.
Chat and eligible synthesis deadlines can scale up to one hour. Discovery
planning accounts for cold load, planning rounds and one bounded crawl.
`/model use local` restores the configured backend. Optional
`models.local_llm.fallback` selects an exact installed loopback model at startup
if the primary is unavailable. If neither qualifies, startup keeps the degraded
primary. Re-evaluation requires restart.

Cloud chat (`grok|xai`, `claude|anthropic`) needs an enabled provider, active
key and per-account confirmation each server run. It sends only the new
message, resets to local at restart and is unavailable in `/loop`. Optional
cloud profiles tighten shared limits. A truncated response fails without
saving the turn. `/model status` reports provider and key presence.
The model service performs inference under the exact resolved tag.
Cloud profiles and the optional local fallback are restart settings.
See [config](../assets/config.default.yaml) and
[spend](SPEND_AND_NOTIFICATIONS.md).
