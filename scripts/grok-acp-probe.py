#!/usr/bin/env python3
"""Disposable, credential-free Grok ACP launch-contract probe."""

import argparse
import asyncio
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


FIXTURE_KEY = "fixture-not-a-credential"
DENIED_METHODS = {
    "session/request_permission",
    "fs/read_text_file",
    "fs/write_text_file",
    "terminal/create",
}


class _CancelReady(Exception):
    pass


OFF_ENV = {
    "GROK_AGENT_DASHBOARD": "0",
    "GROK_CLAUDE_AGENTS_ENABLED": "0",
    "GROK_CLAUDE_HOOKS_ENABLED": "0",
    "GROK_CLAUDE_MCPS_ENABLED": "0",
    "GROK_CLAUDE_RULES_ENABLED": "0",
    "GROK_CLAUDE_SKILLS_ENABLED": "0",
    "GROK_CRASH_HANDLER": "0",
    "GROK_CURSOR_AGENTS_ENABLED": "0",
    "GROK_CURSOR_HOOKS_ENABLED": "0",
    "GROK_CURSOR_MCPS_ENABLED": "0",
    "GROK_CURSOR_RULES_ENABLED": "0",
    "GROK_CURSOR_SKILLS_ENABLED": "0",
    "GROK_LSP_TOOLS": "0",
    "GROK_MEMORY": "0",
    "GROK_SUBAGENTS": "0",
    "GROK_TOOL_SEARCH": "0",
    "GROK_WEB_FETCH": "0",
    "GROK_WRITE_FILE": "0",
}


def _profile(root, name, auth_mode):
    profile = root / name
    paths = {part: profile / part for part in ("home", "grok", "tmp", "work")}
    for path in paths.values():
        path.mkdir(parents=True)
    requirements = """[cli]
auto_update = false
[features]
web_fetch = false
write_file = false
tool_search = false
lsp_tools = false
[subagents]
enabled = false
[memory]
enabled = false
[compat.claude]
skills = false
rules = false
agents = false
mcps = false
hooks = false
[compat.cursor]
skills = false
rules = false
agents = false
mcps = false
hooks = false
"""
    if auth_mode == "subscription":
        requirements += "[grok_com_config]\ndisable_api_key_auth = true\n"
    (paths["grok"] / "requirements.toml").write_text(requirements)
    env = {
        "HOME": str(paths["home"]),
        "GROK_HOME": str(paths["grok"]),
        "PATH": "/usr/bin:/bin",
        "TMPDIR": str(paths["tmp"]),
        **OFF_ENV,
    }
    if auth_mode == "api_key":
        env["XAI_API_KEY"] = FIXTURE_KEY
    return paths, env


def _argv(runtime, paths):
    return [
        *runtime,
        "--no-auto-update",
        "--cwd", str(paths["work"]),
        "--tools", "",
        "--disallowed-tools", "Bash,Edit,Glob,Grep,MCPTool,Read,Task,WebFetch,WebSearch,Write",
        "--deny", "*",
        "--permission-mode", "dontAsk",
        "--sandbox", "strict",
        "--disable-web-search",
        "--no-subagents",
        "--no-plan",
        "agent", "stdio",
        "--leader-socket", str(paths["grok"] / "leader-probe.sock"),
    ]


def _select_auth(auth_mode, result, env):
    methods = {row.get("id") for row in result.get("authMethods", []) if isinstance(row, dict)}
    if ("XAI_API_KEY" in env) != (auth_mode == "api_key"):
        raise RuntimeError("API-key presence does not match the selected funding mode")
    if auth_mode == "api_key":
        return None
    if "grok.com" not in methods:
        raise RuntimeError("runtime did not advertise required auth method: grok.com")
    return "grok.com"


async def _send(proc, message):
    proc.stdin.write((json.dumps(message, separators=(",", ":")) + "\n").encode())
    await proc.stdin.drain()


async def _rpc(proc, state, method, params, cancel=None):
    request_id = state["next_id"]
    state["next_id"] += 1
    await _send(proc, {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
    while True:
        raw = await asyncio.wait_for(proc.stdout.readline(), timeout=2)
        if not raw:
            raise RuntimeError("ACP runtime exited before replying")
        if len(raw) > 65536:
            raise RuntimeError("ACP frame exceeded 64 KiB")
        message = json.loads(raw)
        incoming = message.get("method")
        if incoming == "session/update":
            update = message.get("params", {}).get("update", {})
            text = update.get("content", {}).get("text")
            if text:
                state["text"].append(text)
            if cancel and text == "cancel-ready":
                await _send(proc, {"jsonrpc": "2.0", "method": "session/cancel",
                                   "params": {"sessionId": cancel["session_id"]}})
                deadline = time.monotonic() + 1
                while not cancel["marker"].exists() and time.monotonic() < deadline:
                    await asyncio.sleep(0.01)
                if not cancel["marker"].exists():
                    raise RuntimeError("runtime did not receive session/cancel")
                raise _CancelReady
            continue
        if incoming == "_x.ai/mcp/servers_updated":
            if message.get("params", {}).get("mcpServers") != []:
                raise RuntimeError("runtime discovered an MCP server")
            state["empty_mcp_updates"] += 1
            continue
        if incoming and "id" in message:
            state["denied"].add(incoming)
            await _send(proc, {"jsonrpc": "2.0", "id": message["id"],
                               "error": {"code": -32601, "message": "client capability disabled"}})
            continue
        if message.get("id") != request_id:
            raise RuntimeError("unexpected ACP response id")
        if "error" in message:
            raise RuntimeError(message["error"].get("message", "ACP request failed"))
        return message.get("result", {})


async def _terminate_group(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        await asyncio.wait_for(proc.wait(), timeout=0.5)
    except asyncio.TimeoutError:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        await proc.wait()


def _running(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


async def probe_profile(root, name, auth_mode, cancel=False):
    paths, env = _profile(root, name, auth_mode)
    runtime = [sys.executable, str(Path(__file__).resolve()), "--fake-runtime"]
    proc = await asyncio.create_subprocess_exec(
        *_argv(runtime, paths), cwd=paths["work"], env=env,
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.DEVNULL, start_new_session=True,
    )
    state = {"next_id": 1, "text": [], "denied": set(), "empty_mcp_updates": 0}
    cancelled = False
    try:
        init = await _rpc(proc, state, "initialize", {"protocolVersion": 1, "clientCapabilities": {}})
        method_id = _select_auth(auth_mode, init, env)
        if method_id:
            await _rpc(proc, state, "authenticate", {"methodId": method_id,
                                                      "_meta": {"headless": True}})
        session = await _rpc(proc, state, "session/new", {"cwd": str(paths["work"]), "mcpServers": []})
        prompt = "cancel" if cancel else "fixture"
        try:
            result = await _rpc(
                proc, state, "session/prompt",
                {"sessionId": session["sessionId"], "prompt": [{"type": "text", "text": prompt}]},
                {"session_id": session["sessionId"], "marker": paths["grok"] / "cancel-seen"}
                if cancel else None,
            )
        except _CancelReady:
            cancelled = True
            result = {}
        if not cancel and result.get("stopReason") != "end_turn":
            raise RuntimeError("fixture did not reach the ACP completion boundary")
    finally:
        if cancelled:
            await _terminate_group(proc)
        else:
            proc.stdin.close()
            try:
                await asyncio.wait_for(proc.wait(), timeout=1)
            except asyncio.TimeoutError:
                await _terminate_group(proc)
    if cancel:
        child_pid = int((paths["grok"] / "child.pid").read_text())
        deadline = time.monotonic() + 2
        while _running(child_pid) and time.monotonic() < deadline:
            await asyncio.sleep(0.01)
        if _running(child_pid):
            os.kill(child_pid, signal.SIGKILL)
            raise RuntimeError("runtime descendant survived cancellation cleanup")
        return {"cancel_notification": True, "descendant_reaped": True}
    if state["denied"] != DENIED_METHODS:
        raise RuntimeError("fixture did not exercise every disabled client capability")
    return {"auth_mode": auth_mode, "answer": "".join(state["text"]),
            "denied_methods": sorted(state["denied"]),
            "empty_mcp_updates": state["empty_mcp_updates"]}


async def run_matrix(root):
    return {
        "subscription": await probe_profile(root, "subscription", "subscription"),
        "api_key": await probe_profile(root, "api-key", "api_key"),
        "cancellation": await probe_profile(root, "cancel", "subscription", cancel=True),
    }


def _fake_error(request_id, message):
    print(json.dumps({"jsonrpc": "2.0", "id": request_id,
                      "error": {"code": -32000, "message": message}}), flush=True)


def _fake_result(request_id, result):
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)


def _fake_runtime():
    args = sys.argv[2:]
    grok_home = Path(os.environ["GROK_HOME"])
    cwd = Path(args[args.index("--cwd") + 1])
    checks = [
        "--no-auto-update" in args,
        args[args.index("--tools") + 1] == "",
        args[args.index("--permission-mode") + 1] == "dontAsk",
        args[args.index("--sandbox") + 1] == "strict",
        all(flag in args for flag in ("--disable-web-search", "--no-subagents", "--no-plan")),
        args[args.index("--leader-socket") + 1] == str(grok_home / "leader-probe.sock"),
        cwd.resolve() == Path.cwd().resolve(),
        all(os.environ.get(key) == "0" for key in OFF_ENV),
        not any(key in os.environ for key in
                ("ANTHROPIC_API_KEY", "DEEPAGENT_API_KEY", "GH_TOKEN", "GITHUB_TOKEN", "GROK_API_KEY")),
    ]
    requirements = (grok_home / "requirements.toml").read_text()
    auth_mode = "api_key" if "XAI_API_KEY" in os.environ else "subscription"
    checks.append(("disable_api_key_auth = true" in requirements) == (auth_mode == "subscription"))
    if not all(checks) or os.environ.get("XAI_API_KEY") not in (None, FIXTURE_KEY):
        raise SystemExit("unsafe fake-runtime launch contract")

    session_id = f"fixture-{grok_home.parent.name}"
    authenticated = auth_mode == "api_key"
    pending = None
    awaiting = set()
    child = None
    for line in sys.stdin:
        message = json.loads(line)
        method = message.get("method")
        request_id = message.get("id")
        params = message.get("params", {})
        if method == "initialize":
            if params.get("clientCapabilities") != {}:
                _fake_error(request_id, "client capabilities must be empty")
            else:
                _fake_result(request_id, {"protocolVersion": 1,
                                          "authMethods": [{"id": "grok.com"}]})
                print(json.dumps({"jsonrpc": "2.0", "method": "_x.ai/mcp/servers_updated",
                                  "params": {"mcpServers": []}}), flush=True)
        elif method == "authenticate":
            if auth_mode != "subscription" or params.get("methodId") != "grok.com":
                _fake_error(request_id, "wrong auth method")
            else:
                authenticated = True
                (grok_home / "state.json").write_text(json.dumps({"auth_mode": auth_mode}))
                _fake_result(request_id, {})
        elif method == "session/new":
            if not authenticated:
                _fake_error(request_id, "not authenticated")
            elif params != {"cwd": str(cwd), "mcpServers": []}:
                _fake_error(request_id, "session must use the isolated cwd and no MCP servers")
            else:
                (grok_home / "state.json").write_text(json.dumps({"auth_mode": auth_mode}))
                _fake_result(request_id, {"sessionId": session_id})
        elif method == "session/prompt":
            text = params.get("prompt", [{}])[0].get("text")
            if text == "cancel":
                child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
                (grok_home / "child.pid").write_text(str(child.pid))
                print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {
                    "sessionId": session_id, "update": {"sessionUpdate": "agent_message_chunk",
                                                         "content": {"type": "text", "text": "cancel-ready"}}}}),
                      flush=True)
                pending = request_id
            else:
                pending = request_id
                awaiting = set(range(100, 100 + len(DENIED_METHODS)))
                for denied_id, denied_method in zip(sorted(awaiting), sorted(DENIED_METHODS)):
                    print(json.dumps({"jsonrpc": "2.0", "id": denied_id, "method": denied_method,
                                      "params": {"sessionId": session_id}}), flush=True)
        elif method == "session/cancel":
            if params.get("sessionId") == session_id:
                (grok_home / "cancel-seen").write_text("yes")
        elif request_id in awaiting:
            if message.get("error", {}).get("code") != -32601:
                raise SystemExit("disabled client request was not refused")
            awaiting.remove(request_id)
            if not awaiting:
                print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {
                    "sessionId": session_id, "update": {"sessionUpdate": "agent_message_chunk",
                                                         "content": {"type": "text", "text": f"fixture:{auth_mode}"}}}}),
                      flush=True)
                _fake_result(pending, {"stopReason": "end_turn"})
    if child is not None:
        child.wait()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fake-runtime", action="store_true", help=argparse.SUPPRESS)
    args, _ = parser.parse_known_args()
    if args.fake_runtime:
        _fake_runtime()
        return
    if os.name != "posix":
        raise SystemExit("probe requires POSIX process groups")
    with tempfile.TemporaryDirectory(prefix="cgah-grok-acp-") as tmp:
        print(json.dumps(asyncio.run(run_matrix(Path(tmp))), indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
