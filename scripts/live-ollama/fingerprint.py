#!/usr/bin/env python3
"""Hardware-independent fingerprint of the harness against a live Ollama.

Called by scripts/smoke-ollama.sh when FINGERPRINT_OUT is set, after its login.
It selects each model in turn and records what auto_tune derives from the
loaded window (budgets, tool support, the in-force caps), slash parse
decisions and the memory and goal routes. Speeds and the timeouts derived from
them are checked against their ranges but never recorded, so a CPU container,
a GitHub runner and an Apple-silicon Mac running the same models with the same
OLLAMA_CONTEXT_LENGTH should write the same "compare" section.
Exit 1 when any check fails; the JSON is written either way.
`--compare a.json b.json ...` exits 1 if any run failed or the sections differ.
"""
import argparse
import json
import ssl
import sys
import time
import urllib.error
import urllib.request

# Each line's expected dispatch decision. False means refused or suggest-only.
SLASH_CORPUS = [
    ("/model list", True),
    ("/model list extra", False),
    ("/model profile --dry-run", False),
    ("/model use some-model:latest", True),
    ("/memory facts", True),
    ("/memory status", True),
    ("/memory facts clear", False),
    ("/loop stop", True),
    ("/loop stop now", False),
    ("/web status", True),
    ("/web on", True),
    ("/goal clear", True),
    ("/modle list", False),
    ("/m\u200bodel list", False),
    ("/model\u2028list", False),
    ("/model list\n/agent run x", False),
    ("//model list", False),
    ("/api sett SERPAPI_API_KEY fingerprint-secret-value", False),
]
SECRET = "fingerprint-secret-value"


def compare_runs(paths):
    """Exit 1 if any file recorded failures or its "compare" section differs."""
    runs = [(path, json.load(open(path))) for path in paths]
    status = 0
    for path, run in runs:
        for failure in run.get("failures", []):
            print(f"{path}: FAILED {failure}")
            status = 1
    base_path, base = runs[0]

    def flat(value, prefix=""):
        if isinstance(value, dict):
            out = {}
            for key, inner in value.items():
                out.update(flat(inner, f"{prefix}{key}."))
            return out
        return {prefix.rstrip("."): value}

    want = flat(base["compare"])
    for path, run in runs[1:]:
        got = flat(run["compare"])
        for key in sorted(set(want) | set(got)):
            if want.get(key) != got.get(key):
                print(f"DIFF {key}: {base_path}={want.get(key)!r} {path}={got.get(key)!r}")
                status = 1
    print("fingerprints agree" if status == 0 else "fingerprints differ or failed")
    return status


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--compare", nargs="+", metavar="FINGERPRINT_JSON")
    ap.add_argument("--base")
    ap.add_argument("--cacert")
    ap.add_argument("--session-file")
    ap.add_argument("--csrf")
    ap.add_argument("--model-url")
    ap.add_argument("--models", help="comma-separated, installed tags")
    ap.add_argument("--out")
    ap.add_argument("--tune-timeout", type=float, default=900)
    args = ap.parse_args()
    if args.compare:
        return compare_runs(args.compare)
    missing = [k for k in ("base", "cacert", "session_file", "csrf", "model_url", "models", "out") if not getattr(args, k)]
    if missing:
        ap.error("missing --" + ", --".join(m.replace("_", "-") for m in missing))

    cookie = json.load(open(args.session_file))["cookie"]
    tls = ssl.create_default_context(cafile=args.cacert)
    # Loopback only: never route these calls through an environment proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=tls))

    def call(method, url, body=None, harness=True, timeout=900):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        if harness:
            req.add_header("Cookie", cookie)
            req.add_header("X-CyClaw-CSRF", args.csrf)
        try:
            with opener.open(req, timeout=timeout) as resp:
                raw = resp.read()
                status = resp.status
        except urllib.error.HTTPError as err:
            raw, status = err.read(), err.code
        try:
            return status, json.loads(raw or b"null")
        except ValueError:
            return status, None

    def harness(method, path, body=None, timeout=900):
        return call(method, args.base + path, body, timeout=timeout)

    def ollama(method, path, body=None):
        return call(method, args.model_url.rstrip("/") + path, body, harness=False, timeout=60)

    compare = {"schema": 1, "models": {}, "slash": {}, "routes": {}}
    info = {"models": {}}
    failures = []

    def check(name, ok, detail=""):
        if not ok:
            failures.append(f"{name}: {detail}")
        return bool(ok)

    status, inv = harness("GET", "/api/ollama/inventory")
    names = [m.get("name") for m in (inv or {}).get("models", [])]
    compare["routes"]["inventory_listed"] = check("inventory", status == 200 and inv.get("tags_state") == "listed", inv)

    models = [m.strip() for m in args.models.split(",") if m.strip()]
    for model in models:
        rec = {}
        rec["installed"] = check(f"{model}: installed", model in names, names)
        status, sel = harness("POST", "/api/model", {"model": model})
        rec["select_ok"] = check(f"{model}: select", status == 200 and sel.get("provider") == "ollama", sel)
        deadline = time.monotonic() + args.tune_timeout
        prof = {}
        while time.monotonic() < deadline:
            status, prof = harness("GET", "/api/ollama/profile")
            if status == 200 and (prof.get("tuning") or {}).get("model") == model:
                break
            time.sleep(2)
        tuning = prof.get("tuning") or {}
        rec["tuned"] = check(f"{model}: tuned", tuning.get("model") == model, prof)
        _, ps = ollama("GET", "/api/ps")
        loaded = next((m for m in (ps or {}).get("models", []) if m.get("name") in (model, model + ":latest")), {})
        _, show = ollama("POST", "/api/show", {"model": model})
        declared_tools = "tools" in ((show or {}).get("capabilities") or [])
        window = tuning.get("window")
        rec["window_matches_api_ps"] = check(f"{model}: window", window is not None and window == loaded.get("context_length"), (window, loaded))
        rec["profile_loaded_matches"] = check(f"{model}: profile loaded", (prof.get("loaded") or {}).get("context_length") == window, prof.get("loaded"))
        rec["declared_tools"] = declared_tools
        rec["tools_off_consistent"] = check(f"{model}: tools_off", tuning.get("tools_off") == (not declared_tools or tuning.get("state") == "not_viable"), tuning)
        for key in ("window", "state", "tools_off", "max_tokens", "compact_prompt_tokens", "web"):
            rec[key] = tuning.get(key)
        rec["in_force"] = prof.get("in_force")
        w = window or 0
        rec["budgets_fit_window"] = check(
            f"{model}: budgets",
            0 < (tuning.get("max_tokens") or 0) <= w
            and 0 < (tuning.get("compact_prompt_tokens") or 0) < w
            and 0 < ((prof.get("in_force") or {}).get("prompt_cap") or 0) <= w,
            (tuning, prof.get("in_force")),
        )
        timeout = tuning.get("timeout_sec")
        rec["timeout_in_range"] = check(f"{model}: timeout range", timeout is None or 120 <= timeout <= 3600, timeout)
        planner = prof.get("planner_timeout_sec")
        rec["planner_timeout_in_range"] = check(f"{model}: planner range", planner is None or planner <= 3180, planner)
        speed = tuning.get("speed") or {}
        rec["speed_measured"] = check(f"{model}: speed", (speed.get("decode_tps") or 0) > 0, speed)
        for _ in range(300):
            status, chat = harness("POST", "/api/chat", {"message": "Reply with the single word pong."}, timeout=1800)
            # A tune that is still sampling answers 409 CHAT_BUSY; retry only that.
            if not (status == 409 and ((chat or {}).get("detail") or {}).get("code") == "CHAT_BUSY"):
                break
            time.sleep(2)
        reply = (chat or {}).get("reply") or ""
        rec["chat_ok"] = check(f"{model}: chat", status == 200 and reply.strip() != "" and chat.get("model") == model, chat)
        harness("POST", "/api/web", {"enabled": True})
        _, st = harness("GET", "/api/status")
        rec["chat_tools_available"] = (st or {}).get("chat_tools_available")
        harness("POST", "/api/web", {"enabled": False})
        compare["models"][model] = rec
        info["models"][model] = {
            "reply": reply[:80],
            "timeout_sec": timeout,
            "synthesis_seconds": tuning.get("synthesis_seconds"),
            "planner_timeout_sec": planner,
            "speed": speed,
        }

    # One tuning slot: switching back re-measures. The window-derived values
    # must come out the same; only speed-derived ones may move.
    if len(models) > 1:
        first = compare["models"][models[0]]
        harness("POST", "/api/model", {"model": models[0]})
        deadline = time.monotonic() + args.tune_timeout
        t = {}
        while time.monotonic() < deadline:
            _, prof = harness("GET", "/api/ollama/profile")
            t = (prof or {}).get("tuning") or {}
            if t.get("model") == models[0]:
                break
            time.sleep(2)
        keys = ("window", "state", "tools_off", "max_tokens", "compact_prompt_tokens", "web")
        compare["routes"]["swap_back_retunes_same"] = check(
            "swap back",
            t.get("model") == models[0] and all(t.get(k) == first[k] for k in keys),
            t,
        )

    for line, expected in SLASH_CORPUS:
        status, parsed = harness("POST", "/api/slash/parse", {"line": line})
        dispatched = bool((parsed or {}).get("dispatch"))
        compare["slash"][line] = dispatched
        check(f"slash {line!r}", status == 200 and dispatched == expected, parsed)
        if "SERPAPI" in line:
            check("slash secret not echoed", SECRET not in json.dumps(parsed), parsed)

    status, _ = harness("GET", "/api/memory")
    compare["routes"]["memory"] = check("memory", status == 200, status)
    status, _ = harness("GET", "/api/structured-memory")
    compare["routes"]["structured_memory"] = check("structured memory", status == 200, status)
    status, session = harness("POST", "/api/sessions", {"title": "fingerprint"})
    sid = (session or {}).get("id") or (session or {}).get("session_id")
    status, goal = harness("POST", f"/api/sessions/{sid}/goal", {"goal": "fingerprint goal"})
    compare["routes"]["goal_set"] = check("goal", status == 200 and (goal or {}).get("goal") == "fingerprint goal", goal)

    out = {"compare": compare, "info": info, "failures": failures}
    with open(args.out, "w") as handle:
        json.dump(out, handle, indent=2, sort_keys=True)
    print(json.dumps({"failures": failures}, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
