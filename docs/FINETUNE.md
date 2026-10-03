# Fine-Tuning CG-Agent with MLX QLoRA

Train a LoRA adapter on Apple Silicon and serve the fused model to CG-Agent with
no Rust changes. The tooling is in
[`finetune/`](../finetune/).

> **Unverified:** this recipe did not tune the right local model and needs to be
> verified locally. It targets `malekoo/Qwen3.8-27B-MLX-4bit`
> (Hugging Face 4-bit MLX); an installed Ollama tag does not establish that these
> are the same weights. Base-model matching and a successful adapter training/fusion
> run remain unverified. Resolve the match before training. The mock smoke below
> proves chat protocol compatibility only, not the training recipe or a fused adapter.

**Prerequisites:** an Apple Silicon Mac with at least 48 GB unified memory,
Python 3.10+, `git`, and this repository. Training uses Metal. Ollama is optional
for the GGUF path.

## The runtime path

Serve the fused model through `mlx_lm.server` on `127.0.0.1:1234`. Both local-model
configurations accept that OpenAI-compatible endpoint, and inventory parses the
`{"data":[{"id":...}]}` shape that `mlx_lm.server` emits:

- **`models.local_llm.*`** controls chat, structured memory, compaction, and research. It
  accepts `ollama` | `lmstudio`. Resolved in `src/llm/backend.rs::resolve_local_backend`;
  the `ChatClient` is built from the resolved backend in `src/server/mod.rs`.
- **`agentic.deepagent_github.*`** controls the coding planner (`LocalProposerClient`). It
  accepts `ollama` | `openai_compatible` (`src/agentic/config.rs`,
  `VALID_DEEPAGENT_PROVIDERS`). Its audit label remains `"ollama"` for
  `openai_compatible`; the endpoint is still correct.

Both paths enforce `is_loopback_url`. Port 1234 is the convention that [MODELS.md](MODELS.md)
uses for its `lmstudio` fallback example; `mlx_lm.server` itself defaults to port 8080
when no `--port` is given. Cloud chat (`models.cloud_chat.grok` / `.claude`) is a
third, separate path and is not part of local fine-tuning.

## Two things you must get right

1. **Set MLX as the primary, not the fallback.** `resolve_local_backend` probes the primary
   first and only falls back if the primary model is **not** installed. If Ollama is
   running and `qwen3.8:27b-mlx` is present (the normal case), a fine-tuned model
   configured as `models.local_llm.fallback` is **never used**. Fallback is a backup
   path, not a default-selection path. Set it as primary, import the fused model into
   Ollama via GGUF (so it is the primary tag), or stop Ollama so the primary is not
   installed.
2. **Repoint both configs.** `models.local_llm` controls chat;
   `agentic.deepagent_github` controls the coding planner. Repoint only the first and
   chat uses the fine-tune while the coding planner stays on stock Ollama.

## Verification (do this before training)

Run `bash finetune/smoke_test.sh`. It starts `finetune/mock_server.py`, a standard-library
OpenAI-compatible mock of `/v1/models` and `/v1/chat/completions`, exports
`CGAGENTHARNESS_HOME` to an owned temp home (`serve` accepts only `--host` /
`--port`; there is no global `--config`), points both configs at the mock, builds
the harness if needed (`cargo build --release`), and sends one authenticated chat.
Its `GET /v1/models` proves only mock liveness. A passing chat must log a real
`POST /v1/chat/completions` after harness boot. This smoke does not cover planner
`/api/agent/run`. A build requires Rust 1.88.

## Workflow

Run every step from the repo root.

1. **Build the dataset.**

   ```bash
   python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data
   ```

   This writes MLX ChatML `train.jsonl` and `valid.jsonl`. The dataset contains 20
   hand-written Q&A entries plus bounded code-reference pairs from the 28 files in
   `SIGNIFICANT_FILES`. Fix any drift warning before training.

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

   `lora_config.yaml` sets the 4-bit base, batch 1, `grad_checkpoint`, rank 16, and
   600 iterations. The frozen base uses about 16–18 GB; total peak is estimated at
   24–28 GB. Under memory pressure, reduce
   `num_layers` in `lora_config.yaml`.

4. **Fuse the adapter into the base.** Ollama cannot hot-load an MLX adapter.

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

After relevant repository changes, rebuild the dataset, retrain, and fuse again.

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

To use Ollama for both paths, fuse, convert the model to GGUF with llama.cpp, then
`ollama create cgagent-fused -f finetune/Modelfile.cgagent` (after setting `FROM` to your
GGUF file; the Modelfile has no SYSTEM prompt because persona remains in the
harness), and set
`models.local_llm.provider: "ollama"`, `base_url: "http://127.0.0.1:11434/v1"`,
`model: "cgagent-fused"` for both configurations. This avoids a second server.
Conversion can reduce quality relative to serving the fused MLX model directly;
measure both paths on the same evaluation set before choosing one.
Keep the exact base and adapter provenance with those results.
