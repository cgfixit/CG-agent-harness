# How-To: Fine-Tune CG-Agent with MLX QLoRA (Apple Silicon)

A step-by-step guide to training a `qwen3.8:27b-mlx` LoRA adapter for CG-Agent and
serving the fused model — with no Rust changes. Reference: [`docs/FINETUNE.md`](../FINETUNE.md).
Tooling: [`finetune/`](../../finetune/).

Before training, resolve the [base-model matching warning](../FINETUNE.md#tldr):
the recipe's `malekoo/Qwen3.8-27B-MLX-4bit` weights are not verified to match an
installed `qwen3.8:27b-mlx` tag. Mock chat success does not validate the training
recipe or a fused adapter.

**Prerequisites:** a Mac on Apple Silicon (M-series, 48 GB+ recommended), Python 3.10+,
`git`, and the CG-agent-harness repo checked out. Ollama optional (GGUF path only).

---

## Step 0 — Verify the runtime path (before you train)

Confirm CG-Agent can use an OpenAI-compatible loopback endpoint with no Rust changes:

```bash
bash finetune/smoke_test.sh
```

This starts `finetune/mock_server.py`, exports `CGAGENTHARNESS_HOME` to an owned
temp home (`serve` accepts only `--host` / `--port`), points both
`models.local_llm` and `agentic.deepagent_github` at the mock, builds the harness
(`cargo build --release`), and sends one authenticated chat. The script's own
`GET /v1/models` is mock liveness only. If chat succeeds, the mock must log a
real `POST /v1/chat/completions` after harness boot. Skip this only if you have
already confirmed it.

## Step 1 — Build the dataset

```bash
python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data
```

This emits `finetune/data/train.jsonl` and `finetune/data/valid.jsonl` in MLX ChatML
format (`{"text": ...}`). It combines 20 hand-written Q&A (`curated_qa.py`) with
bounded code-reference pairs from 28 architecturally significant files. If any listed
file has drifted, the builder warns on stderr — fix the path in
`SIGNIFICANT_FILES` before training.

## Step 2 — Install the training dependency

```bash
python3 -m venv .venv && source .venv/bin/activate
pip install -r finetune/requirements.txt   # mlx-lm
```

## Step 3 — Train the LoRA adapter (QLoRA)

```bash
mlx_lm.lora --config finetune/lora_config.yaml
```

QLoRA is automatic: the 4-bit MLX base is frozen, the LoRA adapter trains at full
precision. On an M5 Pro 48 GB, 600 iterations fit comfortably (~24–28 GB peak). If you
hit an out-of-memory error, reduce `num_layers` in `lora_config.yaml`.

## Step 4 — Fuse the adapter into the base

```bash
mlx_lm.fuse --model malekoo/Qwen3.8-27B-MLX-4bit \
  --adapter-path ./adapters --save-path ./cgagent-fused
```

Ollama cannot hot-load an MLX adapter, so this step is mandatory — the fused model is a
standalone model any OpenAI-compatible server (or Ollama via GGUF) can load.

## Step 5 — Serve the fused model on loopback

```bash
mlx_lm.server --model ./cgagent-fused --port 1234
```

Leave this running in a terminal. It exposes an OpenAI-compatible API at
`http://127.0.0.1:1234/v1`.

## Step 6 — Wire CG-Agent at the fused model (MLX as PRIMARY, both paths)

Add/merge into your `config.yaml`:

```yaml
models:
  local_llm:
    provider: "lmstudio"
    base_url: "http://127.0.0.1:1234/v1"
    model: "cgagent-fused"
    reasoning_effort: "none"
agentic:
  deepagent_github:
    provider: "openai_compatible"
    base_url: "http://127.0.0.1:1234/v1"
    model: "cgagent-fused"
    allow_cloud_providers: false
    providers:
      grok:
        enabled: false
      claude:
        enabled: false
```

`load_agentic_config` rejects parent-off plus any enabled cloud-provider child.
Disable every enabled `providers.*` child when the parent is false, or the home
will fail to load against a default config.

Then restart the console. **Both** blocks must point at the server — `models.local_llm`
for chat, `agentic.deepagent_github` for the coding planner. Setting MLX as fallback is
not enough: the fallback is only used when the primary (Ollama) model is not installed.

## Step 7 — (Optional) Ollama-native path via GGUF

Prefer one Ollama process over `mlx_lm.server`? Convert the fused model to GGUF and
register it:

```bash
# Convert fused MLX -> GGUF with llama.cpp, then:
ollama create cgagent-fused -f finetune/Modelfile.cgagent   # set FROM to your GGUF file
```

Then set `models.local_llm.provider: "ollama"`,
`base_url: "http://127.0.0.1:11434/v1"`, `model: "cgagent-fused"` (and the same model id
for `agentic.deepagent_github`). Small quality loss vs the live-MLX path.

## Step 8 — Keep it current

When the repo changes (new modules, refactors), re-run Step 1 to refresh the dataset,
then retrain (Step 3) and re-fuse (Step 4). The builder warns if `SIGNIFICANT_FILES`
has drifted, so you know what to update.

---

## Troubleshooting

- **The fine-tuned model is not used** — you set it as `fallback`. Set it as primary
  (Step 6), or stop Ollama so the primary is not "installed".
- **Chat uses the fine-tune but coding suggestions don't** — you repointed
  `models.local_llm` but not `agentic.deepagent_github`. Repoint both.
- **Boot refuses the `base_url`** — it must be loopback (`127.0.0.1`/`localhost`/`::1`),
  with no credentials, query, or fragment.
- **`cargo build` fails in `smoke_test.sh`** — ensure the Rust toolchain (MSRV 1.88) is
  installed; this is a build-environment issue, not an integration gap.
