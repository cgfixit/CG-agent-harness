#!/usr/bin/env python3
"""Native Windows Job Object acceptance fixture; no production entrypoint."""
import ctypes
from ctypes import wintypes
import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


class BasicLimits(ctypes.Structure):
    _fields_ = [("process_time", ctypes.c_int64), ("job_time", ctypes.c_int64),
                ("flags", wintypes.DWORD), ("minimum", ctypes.c_size_t),
                ("maximum", ctypes.c_size_t), ("processes", wintypes.DWORD),
                ("affinity", ctypes.c_size_t), ("priority", wintypes.DWORD),
                ("scheduling", wintypes.DWORD)]


class Limits(ctypes.Structure):
    _fields_ = [("basic", BasicLimits), ("io", ctypes.c_uint64 * 6),
                ("process_memory", ctypes.c_size_t), ("job_memory", ctypes.c_size_t),
                ("peak_process", ctypes.c_size_t), ("peak_job", ctypes.c_size_t)]


def launch(mode, directory, flags=0):
    return subprocess.Popen([sys.executable, __file__, mode, str(directory)],
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, close_fds=True,
                            creationflags=flags)


def atomic(path, value):
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value), encoding="utf-8")
    os.replace(temp, path)


def fixture_call(name, args):
    if name == "windows_start":
        directory = Path(args["directory"])
        limits = Limits()
        query = ctypes.WinDLL("kernel32", use_last_error=True).QueryInformationJobObject
        query.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p]
        query.restype = wintypes.BOOL
        if not query(None, 9, ctypes.byref(limits), ctypes.sizeof(limits), None):
            raise ctypes.WinError(ctypes.get_last_error())
        child = launch("child", directory, subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP)
        deadline = time.monotonic() + 10
        while not (directory / "descendants").exists():
            if time.monotonic() > deadline:
                raise RuntimeError("grandchild never started")
            time.sleep(0.02)
        try:
            escaped = launch("sleep", directory, subprocess.CREATE_BREAKAWAY_FROM_JOB)
        except OSError as error:
            if error.winerror != 5:
                raise
            breakaway_denied = True
        else:
            escaped.kill()
            escaped.wait()
            breakaway_denied = False
        descendants = json.loads((directory / "descendants").read_text(encoding="utf-8"))
        return {"pids": [os.getpid(), child.pid, descendants["grandchild"]],
                "flags": limits.basic.flags, "processes": limits.basic.processes,
                "memory": limits.job_memory, "breakaway_denied": breakaway_denied}
    if name == "windows_process_limit":
        children = []
        for _ in range(32):
            try:
                children.append(launch("sleep", Path(args["directory"])))
            except OSError:
                break
        # Keep all successful children alive until the job is closed.
        return {"created": len(children), "limit_enforced": 0 < len(children) < 32}
    if name == "windows_memory_limit":
        positive = bytearray(4 * 1024 * 1024)
        positive[0] = 1
        try:
            excessive = bytearray(512 * 1024 * 1024)
        except MemoryError:
            return {"positive": positive[0], "limit_enforced": True}
        return {"positive": positive[0], "limit_enforced": len(excessive) == 0}
    if name == "windows_hang":
        time.sleep(60)
        return {}
    raise ValueError(name)


def main():
    if os.name != "nt":
        raise SystemExit("native Windows required")
    mode = sys.argv[1]
    if mode == "child":
        directory = Path(sys.argv[2])
        grandchild = launch("grandchild", directory, subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP)
        atomic(directory / "descendants", {"grandchild": grandchild.pid})
        time.sleep(60)
    elif mode == "grandchild":
        directory = Path(sys.argv[2])
        while True:
            with (directory / "heartbeat").open("ab") as stream:
                stream.write(str(time.monotonic_ns()).encode() + b"\n")
            time.sleep(0.05)
    elif mode == "sleep":
        time.sleep(60)
    elif mode == "stdio":
        base = importlib.import_module("mcp-fixture")
        while True:
            message = base._read_stdio()
            if message is None:
                return
            params = message.get("params") or {}
            name = params.get("name", "")
            if message.get("method") == "tools/call" and name.startswith("windows_"):
                payload = fixture_call(name, params.get("arguments") or {})
                reply = {"jsonrpc": "2.0", "id": message["id"], "result": {
                    "content": [{"type": "text", "text": json.dumps(payload)}]}}
            else:
                reply = base._handle(message)
            if reply is not None:
                base._write_stdio(reply)


if __name__ == "__main__":
    main()
