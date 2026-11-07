#!/usr/bin/env python3
"""One isolated native smoke run: install, failure recovery, restart, seats, long UI.

No real model calls: a loopback fixture serves deterministic Chat Completions.
The acceptance feature drives the same SessionStore handlers as the native UI.
"""
import argparse
import http.server
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import signal
import struct
import zlib
import threading
import time
import urllib.request
import urllib.error
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--client", required=True, type=pathlib.Path)
parser.add_argument("--daemon", required=True, type=pathlib.Path)
args = parser.parse_args()
client_binary, daemon_binary = args.client.resolve(), args.daemon.resolve()
root = pathlib.Path(tempfile.mkdtemp(prefix="qaqh-native-acceptance-"))
(root / "workspace").mkdir()
env = dict({key: value for key, value in os.environ.items() if not key.startswith("QAQH_")}, XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"), QAQH_ALLOW_TEST_DATA_ROOT="1", QAQH_DATA_DIR=str(root / "config/qaqh"))
(root / "config/qaqh").mkdir(parents=True)
(root / "config/qaqh/config.toml").write_text("[mcp]\nenabled = false\n")
processes = []
report = {"root": str(root), "scenarios": {}}
stage = root / "package"
stage.mkdir()
for name, source in [("qaqh-gpui", client_binary), ("qaqh-daemon", daemon_binary)]:
    shutil.copy2(source, stage / name)
packaging = pathlib.Path(__file__).resolve().parents[1] / "packaging"
for name in ["install.sh", "io.github.QAQTam.QAQHarness.desktop", "io.github.QAQTam.QAQHarness.svg"]:
    shutil.copy2(packaging / name, stage / name)
(stage / "manifest.json").write_text(json.dumps({"sha256": {name: hashlib.sha256((stage / name).read_bytes()).hexdigest() for name in ["qaqh-gpui", "qaqh-daemon"]}}))
prefix = root / "installed"
subprocess.run(["bash", str(stage / "install.sh"), str(prefix)], env=env, check=True)
client_binary, daemon_binary = prefix / "lib/qaqh-gpui/qaqh-gpui", prefix / "lib/qaqh-gpui/qaqh-daemon"

class Model(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        choice = {"index": 0, "message": {"role": "assistant", "content": "Native acceptance response.\n\n```rust\nfn main() {}\n```"}, "finish_reason": "stop"}
        if request.get("stream"):
            delta = {"id": "fixture", "object": "chat.completion.chunk", "model": "fixture", "choices": [{"index": 0, "delta": choice["message"], "finish_reason": None}]}
            terminal = {"id": "fixture", "object": "chat.completion.chunk", "model": "fixture", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
            data = ("data: " + json.dumps(delta) + "\n\ndata: " + json.dumps(terminal) + "\n\ndata: [DONE]\n\n").encode()
            media = "text/event-stream"
        else:
            data = json.dumps({"id": "fixture", "object": "chat.completion", "created": 1, "model": "fixture", "choices": [choice], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}).encode()
            media = "application/json"
        self.send_response(200)
        self.send_header("Content-Type", media)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model)
threading.Thread(target=model.serve_forever, daemon=True).start()

def start(binary, *arguments):
    log = open(root / f"{binary.name}-{len(processes)}.log", "wb")
    process = subprocess.Popen([str(binary), *map(str, arguments)], env=env, stdout=log, stderr=log)
    processes.append(process)
    return process

def wait(predicate, description, timeout=70):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.2)
    raise AssertionError(f"Timed out: {description}; evidence: {root}")

def discovery():
    path = root / "config/qaqh/daemon.json"
    try:
        data = json.loads(path.read_text())
        os.kill(data["pid"], 0)
        return data
    except (FileNotFoundError, ProcessLookupError, json.JSONDecodeError):
        return None

def api(path, body=None, session=None):
    service = discovery()
    headers = {"Authorization": "Bearer " + service["token"], "Content-Type": "application/json"}
    if session:
        headers["X-QAQH-Client-Session-Id"] = session
    base = service["endpoint"].split("/ringing")[0]
    request = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise AssertionError({"http_status": error.code, "body": json.load(error)}) from error

caps = json.loads((pathlib.Path(__file__).resolve().parents[1] / "docs/capabilities.json").read_text())
def open_client():
    return api("/ringing/v2/clients/open", {"schema": "qaqh.Ringing", "version": 2, "client_instance_id": "acceptance-" + uuid.uuid4().hex, "command_contract_revision": caps["command_revision"], "session_contract_revision": caps["session_revision"]})["client_session_id"]

def command(client, payload, target=None, revision=None, command_id=None):
    result = api("/ringing/v2/commands", {"schema": "qaqh.Ringing", "version": 2, "contract_revision": caps["command_revision"], "command_id": command_id or uuid.uuid4().hex, "client_instance_id": "native-acceptance", "client_session_id": client, "target": target or {"kind": "global"}, "preconditions": [] if revision is None else [{"kind": "config_revision", "revision": revision}], "command": payload}, client)
    assert result["status"] == "accepted", result
    return result

sequence = 0
def observe():
    try:
        return json.loads((root / "observed.json").read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return {}

def control(action, **fields):
    global sequence
    sequence += 1
    temporary = root / "control.tmp"
    temporary.write_text(json.dumps(dict(sequence=sequence, action=action, **fields)))
    temporary.replace(root / "control.json")
    state = wait(lambda: (state if (state := observe()).get("sequence") == sequence else None), action)
    assert not state.get("error"), state
    return state

def current():
    return observe().get("current") or {}

try:
    daemon = start(daemon_binary, "run")
    wait(discovery, "isolated daemon")
    seed_client = open_client()
    catalog = command(seed_client, {"type": "provider.list"})["result"]
    revision = command(seed_client, {"type": "provider.upsert", "id": "native-fixture", "provider": {"name": "Native fixture", "base_url": f"http://127.0.0.1:{model.server_port}/v1", "wire": "chat", "credential": "api_key", "headers": {}, "compat": None, "api_key": {"operation": "set", "value": "local-fixture-only"}}}, revision=catalog["revision"])["revision"]
    command(seed_client, {"type": "provider.model.upsert", "provider_id": "native-fixture", "model_id": "fixture", "model": {"capabilities": {"image_input": True}}}, revision=revision)
    seed = command(seed_client, {"type": "session.create", "cwd": str(root / "workspace"), "title": "Native acceptance", "model_selection": {"provider_id": "native-fixture", "model_id": "fixture", "options": {}}})["result"]["session_id"]
    target = {"kind": "session", "session_id": seed}
    daemon.kill()
    daemon.wait(timeout=10)
    app = start(client_binary, "--acceptance-root", root)
    wait(lambda: current().get("stream_open") and not current().get("snapshot_pending"), "native window and bootstrap")
    assert app.poll() is None
    attachment = root / "attachment.txt"
    attachment.write_text("Native UTF-8 文本附件")
    control("attach", path=str(attachment))
    wait(lambda: current().get("attachments") == 1 and not current().get("attachment_pending"), "text attachment upload")
    def png_chunk(kind, content):
        return struct.pack(">I", len(content)) + kind + content + struct.pack(">I", zlib.crc32(kind + content))
    image = root / "image.png"
    image.write_bytes(b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)) + png_chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00\xff")) + png_chunk(b"IEND", b""))
    control("attach", path=str(image))
    wait(lambda: current().get("images") == 1 and not current().get("attachment_pending"), "image staging")
    text = "Native first message — 中文输入与发送验收"
    control("draft", text=text)
    control("fail_send")
    pending = current()["outbox"]
    assert pending["state"] == "Uncertain" and current()["draft"] == text
    saved = json.loads((root / "config/qaqh-gpui/workbench.json").read_text())
    assert saved["drafts"][seed]["outbox"]["command_id"] == pending["command_id"]
    control("retry")
    wait(lambda: current().get("outbox", {}).get("state") == "Succeeded", "same-command retry")
    assert current()["outbox"]["command_id"] == pending["command_id"]
    wait(lambda: current().get("entries", 0) >= 2, "first native response")
    assert current()["draft"] == ""
    replay_client = current()["client_session_id"]
    replay = command(replay_client, dict(type="conversation.send_message", **pending["message"]), target, command_id=pending["command_id"])
    assert replay["existing"]["state"] == "succeeded", replay
    assert current()["entries"] == 2, current()["entries"]
    previous_lease = current()["client_session_id"]
    app.terminate()
    app.wait(timeout=10)
    # Simulate a crash after server acceptance but before the ACK reached durable UI state.
    (root / "config/qaqh-gpui/workbench.json").write_text(json.dumps(saved))
    (root / "control.json").unlink()
    (root / "observed.json").unlink()
    app = start(client_binary, "--acceptance-root", root)
    wait(lambda: current().get("stream_open") and current().get("client_session_id") != previous_lease, "new lease with durable logical identity")
    assert current()["outbox"]["state"] == "Uncertain"
    control("receipt")
    wait(lambda: current().get("outbox", {}).get("state") == "Succeeded", "receipt recovered after app restart")
    assert current()["entries"] == 2
    report["receipt-after-app-restart"] = True
    report["scenarios"]["send-failure-recovery"] = {"passed": True, "command_id": pending["command_id"]}
    report["scenarios"]["install-first-message"] = {"passed": True, "binary": str(client_binary)}
    report["attachments"] = True
    control("driver", release=False, stale_holder=None)
    own = wait(lambda: current().get("client_session_id") if (current().get("driver") or {}).get("holder") == current().get("client_session_id") else None, "first driver")
    second = open_client()
    command(second, {"type": "session.attach"}, target)
    control("driver", release=True, stale_holder=None)
    wait(lambda: not (current().get("driver") or {}).get("holder"), "driver released")
    command(second, {"type": "driver.claim", "client_session_id": second}, target)
    wait(lambda: (current().get("driver") or {}).get("holder") == second, "second driver observed")
    control("driver", release=False, stale_holder=second)
    wait(lambda: (current().get("driver") or {}).get("holder") == own, "explicit takeover")
    report["scenarios"]["driver-handoff"] = {"passed": True}
    metadata_client = open_client()
    (root / "other-workspace").mkdir()
    command(metadata_client, {"type": "workspace.create", "path": str(root / "other-workspace")})
    control("workspaces")
    assert app.poll() is None
    command(metadata_client, {"type": "session.archive"}, target)
    entries = api("/ringing/v2/service/session.list", {"include_archived": True}, metadata_client)
    assert any(entry["session_id"] == seed and entry["archived"] for entry in entries)
    command(metadata_client, {"type": "session.unarchive"}, target)
    report["archive-query"] = True
    before = current()["entries"]
    previous_client = current()["client_session_id"]
    killed = discovery()["pid"]
    os.kill(killed, signal.SIGKILL)
    wait(lambda: (data if (data := discovery()) and data["pid"] != killed else None), "automatic daemon restart discovery", timeout=100)
    wait(lambda: current().get("client_session_id") != previous_client and current().get("stream_open") and not current().get("snapshot_pending") and current().get("entries", 0) >= before, "automatic restart recovery", timeout=100)
    control("refill")
    wait(lambda: not current().get("snapshot_pending"), "post-restart snapshot")
    report["scenarios"]["daemon-restart"] = {"passed": True, "entries": current()["entries"]}
    control("long_session", entries=10000)
    wait(lambda: current().get("entries", 0) >= 10000, "long native timeline")
    time.sleep(4)
    assert app.poll() is None
    status = pathlib.Path(f"/proc/{app.pid}/status").read_text()
    rss = next(int(line.split()[1]) for line in status.splitlines() if line.startswith("VmRSS:"))
    assert rss < 600 * 1024, f"Native client RSS over budget: {rss} KiB"
    started = time.monotonic()
    control("draft", text="long timeline remains interactive")
    latency = time.monotonic() - started
    assert latency < 4, latency
    report["scenarios"]["long-session"] = {"passed": True, "entries": current()["entries"], "rss_kib": rss, "control_roundtrip_seconds": round(latency, 3)}
    control("refill")
    wait(lambda: not current().get("snapshot_pending") and current().get("entries", 10000) < 10000, "fixture removed")
    control("draft", text="restored native draft")
    time.sleep(2)
    app.terminate()
    app.wait(timeout=10)
    (root / "control.json").unlink()
    (root / "observed.json").unlink()
    app = start(client_binary, "--acceptance-root", root)
    wait(lambda: current().get("draft") == "restored native draft" and current().get("stream_open"), "draft and recent session restored")
    report["workbench-restored"] = True
    report["passed"] = True
finally:
    for process in reversed(processes):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
    model.shutdown()
    if discovery():
        subprocess.run([str(daemon_binary), "stop"], env=env, timeout=20, check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["bash", str(packaging / "uninstall.sh"), str(prefix)], env=env, check=True)
    shutil.rmtree(stage)
    destination = pathlib.Path(__file__).resolve().parents[1] / "dist/acceptance.json"
    destination.parent.mkdir(exist_ok=True)
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(destination)
