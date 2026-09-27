# Fine-Tuning CG-Agent with MLX QLoRA

How to train a LoRA adapter for `qwen3.8:27b-mlx` on Apple Silicon and serve the
fused model to CG-Agent — with **no Rust changes**. The adapter bakes CG-Agent's
architecture, security model and module layout into the model, so chat **and** the
coding planner give repo-aware answers. The tooling is vendored in
[`finetune/`](../finetune/).

> **Unverified:** this recipe did not tune the right local model and needs to be
> revisited and verified locally. It targets `malekoo/Qwen3.8-27B-MLX-4bit`
> (Hugging Face 4-bit MLX); an installed Ollama tag does not establish that these
> are the same weights. Base-model matching and a successful adapter training/fusion
> run remain unverified; resolve the matching before training. The mock smoke below
> proves chat protocol compatibility only, not the training recipe or a fused adapter.

**Prerequisites:** a Mac on Apple Silicon with 48 GB+ unified memory (the recipe
targets a MacBook Pro M5 Pro, 48 GB), Python 3.10+, `git`, and this repo checked
out. Training runs `mlx_lm.lora` QLoRA on Metal — no CUDA. Ollama is optional (GGUF
path only).

## The runtime path

CG-Agent already speaks any OpenAI-compatible loopback model server, so the fused
model is served by `mlx_lm.server` on `127.0.0.1:1234` and wired in through config —
no new Rust model backend is required. Two independent local-model configurations
both accept that endpoint, and the inventory probe already parses the standard
`{"data":[{"id":...}]}` shape that `mlx_lm.server` emits:

- **`models.local_llm.*`** — chat, structured memory, compaction, web research. Provider
  accepts `ollama` | `lmstudio`. Resolved in `src/llm/backend.rs::resolve_local_backend`;
  the `ChatClient` is built from the resolved backend in `src/server/mod.rs`.
- **`agentic.deepagent_github.*`** — the coding planner (`LocalProposerClient`). Provider
  accepts `ollama` | `openai_compatible` (`src/agentic/config.rs`,
  `VALID_DEEPAGENT_PROVIDERS`). Its `provider()` audit label hardcodes `"ollama"` even
  for `openai_compatible` — cosmetic (the endpoint is correct; a Rust cleanup is optional).

Loopback-only enforcement (`is_loopback_url`) is preserved on both — the security
invariant holds. Port 1234 is the LM Studio convention that [MODELS.md](MODELS.md)
uses for its `lmstudio` fallback example; `mlx_lm.server` itself defaults to port 8080
when no `--port` is given. Cloud chat (`models.cloud_chat.grok` / `.claude`) is a
third, separate path and is not part of local fine-tuning.

## Two things you must get right

1. **MLX must be PRIMARY, not fallback.** `resolve_local_backend` probes the primary
   first and only falls back if the primary model is **not** installed. If Ollama is
   running and `qwen3.8:27b-mlx` is present (the normal case), a fine-tuned model
   configured as `models.local_llm.fallback` is **never used** — fallback is a backup
   path, not a default-selection path. Set it as primary, import the fused model into
   Ollama via GGUF (so it is the primary tag), or stop Ollama so the primary is not
   "installed".
2. **Repoint BOTH configs.** `models.local_llm` controls chat;
   `agentic.deepagent_github` controls the coding planner. Repoint only the first and
   chat uses the fine-tune but the **coding planner stays on stock Ollama** —
   defeating the main value (repo-aware code suggestions).

## Verification (do this before training)

Run `bash finetune/smoke_test.sh` (skip it only if you have already confirmed the
runtime path). It starts `finetune/mock_server.py` (a standard-library-only
OpenAI-compatible mock of `/v1/models` and `/v1/chat/completions`), exports
`CGAGENTHARNESS_HOME` to an owned temp home (`serve` accepts only `--host` /
`--port`; there is no global `--config`), points both configs at the mock, builds
the harness if needed (`cargo build --release`), and sends one authenticated chat.
The smoke's own `GET /v1/models` is mock liveness only, not resolver proof. If chat
succeeds, the mock must log a real `POST /v1/chat/completions` after harness boot;
if chat fails, the script prints that clearly. Planner `/api/agent/run` coverage is
out of scope for this smoke. If `cargo build` fails there, ensure the Rust toolchain
(MSRV 1.88) is installed; that is a build-environment issue, not an integration gap.

## Workflow

Run every step from the repo root.

1. **Build the dataset.**

   ```bash
   python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data
   ```

   This emits `finetune/data/train.jsonl` and `finetune/data/valid.jsonl` in MLX
   ChatML format (`{"text": ...}`). The dataset is intentionally small: 20
   hand-written Q&A in `curated_qa.py` — the reasoning core (architecture, gates,
   sanitize_handoff, loopback, structured memory, the two-config reality, MLX path) —
   plus bounded code-reference pairs from the 28 architecturally significant files
   in `SIGNIFICANT_FILES` (not every file). If a listed file has drifted, the builder
   warns on stderr — fix the path in `SIGNIFICANT_FILES` before training.

2. **Install the training dependency** in a venv on the Mac. Only training needs
   `mlx-lm`; the builder and mock server use only the Python standard library.

   ```bash
   python3 -m venv .venv && source .venv/bin/activate
   pip install -r finetune/requirements.txt   # mlx-lm
   ```

3. **Train the LoRA adapter (QLoRA).**

   ```bash
   mlx_lm.lora --config finetune/lora_config.yaml
   ```

   `lora_config.yaml` sets the 4-bit base, batch 1, grad_checkpoint, rank 16 and 600
   iterations. QLoRA is automatic: the 4-bit MLX base is frozen and the LoRA adapter
   trains at full precision. On 48 GB unified, 27B QLoRA peak is ~24–28 GB (4-bit
   base ~16–18 GB + adapter/Adam/activations) — it fits with OS headroom, not
   guaranteed under heavy memory pressure. If you hit an out-of-memory error, reduce
   `num_layers` in `lora_config.yaml`.

4. **Fuse the adapter into the base.** Ollama cannot hot-load an MLX adapter, so this
   step is mandatory — the fused model is a standalone model any OpenAI-compatible
   server (or Ollama via GGUF) can load.

   ```bash
   mlx_lm.fuse --model malekoo/Qwen3.8-27B-MLX-4bit \
     --adapter-path ./adapters --save-path ./cgagent-fused
   ```

5. **Serve the fused model on loopback.** Leave it running in a terminal.

   ```bash
   mlx_lm.server --model ./cgagent-fused --port 1234
   ```

6. **Wire both configs** (MLX as primary, both paths): merge this block into your
   `config.yaml`, then restart the console.

   ```yaml
   models:
     local_llm:
       provider: "lmstudio"                              # chat path
       base_url: "http://127.0.0.1:1234/v1"              # mlx_lm.server loopback
       model: "cgagent-fused"                            # exact id mlx_lm.server reports
       reasoning_effort: "none"
   agentic:
     deepagent_github:
       provider: "openai_compatible"                     # coding planner path
       base_url: "http://127.0.0.1:1234/v1"              # same server
       model: "cgagent-fused"
       allow_cloud_providers: false
       providers:
         grok:
           enabled: false
         claude:
           enabled: false
   ```

When the repo changes (new modules, refactors), re-run step 1, then retrain (step 3)
and re-fuse (step 4). Expand `curated_qa.py` as the repo grows.

## Config constraints

`load_agentic_config` rejects parent-off plus any enabled cloud-provider child.
When setting `allow_cloud_providers: false`, also disable every enabled
`providers.*` child (today `grok` and `claude`) or the home will fail to load.

Both `base_url`s must be loopback (`127.0.0.1`/`localhost`/`::1`): console boot
refuses a non-loopback chat URL, and coding commands refuse a non-loopback planner
URL. Keep credentials, query and fragment out too; desktop chat and the model
inventory check refuse them. Restart the console to re-evaluate after changing
services.

The compatible-backend chat path reserves twice its output ceiling for prompt
safety, even with `reasoning_effort: "none"` in YAML (that field is sent only
to Ollama). Keep chat and `/loop` ceilings at most 12952; the defaults 4096 and
2048 already fit. Summary tuning and usage calibration apply here too; see
[local history compaction](CONSOLE.md#local-history-compaction).

## GGUF / Ollama-native alternative

If you want one process (Ollama) serving both paths instead of running `mlx_lm.server`:
fuse, convert the fused model to GGUF (llama.cpp), then
`ollama create cgagent-fused -f finetune/Modelfile.cgagent` (after setting `FROM` to your
GGUF file; the Modelfile ships no SYSTEM prompt — persona lives in the harness), and set
`models.local_llm.provider: "ollama"`, `base_url: "http://127.0.0.1:11434/v1"`,
`model: "cgagent-fused"` (and the same model id for `agentic.deepagent_github`). Small
quality loss vs the live-MLX path; avoids a second process.
