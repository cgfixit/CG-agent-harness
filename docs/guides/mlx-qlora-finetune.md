# How-To: Fine-Tune CG-Agent with MLX-vLM QLoRA (Apple Silicon, multimodal)

A step-by-step guide to training a `qwen3.8:27b-mlx`-family LoRA adapter for CG-Agent and
serving the fine-tuned model — with no Rust changes. Reference: [`docs/FINETUNE.md`](../FINETUNE.md).
Tooling: [`finetune/`](../../finetune/).

**Why mlx-vlm, not mlx-lm:** the `qwen3.8:27b-mlx` target is multimodal (Text+Image).
`mlx-vlm` fine-tunes the VLM with the vision encoder frozen (`--train-vision` off), so the
fine-tuned model keeps vision. `mlx-lm` is text-only and would strip the vision encoder.

**Prerequisites:** a Mac on Apple Silicon (M-series, 48 GB+ recommended), Python 3.10+,
`git`, and the CG-agent-harness repo checked out. Ollama optional (GGUF path only).

---

## Step 0 — Verify the runtime path (before you train)

Confirm CG-Agent can use an OpenAI-compatible loopback endpoint with no Rust changes:

```bash
bash finetune/smoke_test.sh
```

This starts `finetune/mock_server.py`, points both `models.local_llm` and
`agentic.deepagent_github` at it, builds the harness (`cargo build --release`), and
sends a chat. If the mock logs a `GET /v1/models` and a `POST /v1/chat/completions`,
the path works. Skip this only if you have already confirmed it.

## Step 1 — Build the dataset

```bash
python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data
```

This emits `finetune/data/train.jsonl` and `finetune/data/valid.jsonl` in mlx-vlm
format (`{"messages": [...]}`). It combines 20 hand-written Q&A (`curated_qa.py`) with
bounded code-reference pairs from 28 architecturally significant files. If any listed
file has drifted, the builder warns on stderr — fix the path in `SIGNIFICANT_FILES`
before training.

## Step 2 — Install the training dependency

```bash
python3 -m venv .venv && source .venv/bin/activate
pip install -r finetune/requirements.txt   # mlx-vlm[train]
```

## Step 3 — Train the LoRA adapter (QLoRA)

```bash
bash finetune/train.sh
```

This runs `finetune/train.py` (a thin shim around `mlx_vlm.lora`) on the multimodal 4-bit base
`mlx-community/Qwen3.8-27B-4bit`. The shim lets `--dataset` point at a local `.jsonl` (which
`mlx_vlm.lora` cannot load directly). QLoRA is automatic on the quantized base; the vision
encoder is frozen (`--train-vision` off), so LoRA only adapts the language model. On an M5
Pro 48 GB, 600 iterations fit (~28–34 GB peak). If you hit an out-of-memory error, reduce
`--max-seq-length` in `finetune/train.sh`.

## Step 4 — Serve the fine-tuned model on loopback (no fuse step)

```bash
mlx_vlm.server --model mlx-community/Qwen3.8-27B-4bit \
  --adapter-path ./adapters --port 1234
```

Leave this running in a terminal. `mlx_vlm.server` applies the adapter live — there is no
separate fuse step — and exposes an OpenAI-compatible API at `http://127.0.0.1:1234/v1`.

## Step 5 — Wire CG-Agent at the fine-tuned model (MLX as PRIMARY, both paths)

Add/merge into your `config.yaml`:

```yaml
models:
  local_llm:
    provider: "lmstudio"
    base_url: "http://127.0.0.1:1234/v1"
    model: "mlx-community/Qwen3.8-27B-4bit"
    reasoning_effort: "none"
agentic:
  deepagent_github:
    provider: "openai_compatible"
    base_url: "http://127.0.0.1:1234/v1"
    model: "mlx-community/Qwen3.8-27B-4bit"
    allow_cloud_providers: false
```

Verify the exact `model` id with `curl http://127.0.0.1:1234/v1/models`. Then restart the
console. **Both** blocks must point at the server — `models.local_llm` for chat,
`agentic.deepagent_github` for the coding planner. Setting MLX as fallback is not enough:
the fallback is only used when the primary (Ollama) model is not installed.

## Step 6 — (Optional) Ollama-native path via GGUF

Prefer one Ollama process over `mlx_vlm.server`? mlx-vlm has no fuse command, so fuse the
adapter into the base weights manually (llama.cpp or a safetensors merge), convert to GGUF,
then register it:

```bash
# Fuse + convert fused MLX -> GGUF with llama.cpp, then:
ollama create cgagent-fused -f finetune/Modelfile.cgagent   # set FROM to your GGUF file
```

Then set `models.local_llm.provider: "ollama"`,
`base_url: "http://127.0.0.1:11434/v1"`, `model: "cgagent-fused"` (and the same model id
for `agentic.deepagent_github`). Small quality loss vs the live-MLX path.

## Step 7 — Keep it current

When the repo changes (new modules, refactors), re-run Step 1 to refresh the dataset,
then retrain (Step 3). The builder warns if `SIGNIFICANT_FILES` has drifted, so you know
what to update.

---

## Troubleshooting

- **The fine-tuned model is not used** — you set it as `fallback`. Set it as primary
  (Step 5), or stop Ollama so the primary is not "installed".
- **Chat uses the fine-tune but coding suggestions don't** — you repointed
  `models.local_llm` but not `agentic.deepagent_github`. Repoint both.
- **Boot refuses the `base_url`** — it must be loopback (`127.0.0.1`/`localhost`/`::1`),
  with no credentials, query, or fragment.
- **`cargo build` fails in `smoke_test.sh`** — ensure the Rust toolchain (MSRV 1.88) is
  installed; this is a build-environment issue, not an integration gap.
- **You see vision/image errors at train time** — do not add `--train-vision`; our
  text-only dataset is correct for language-model LoRA with the vision tower frozen.
