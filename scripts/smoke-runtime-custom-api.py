#!/usr/bin/env python3
"""Explicit local native acceptance. Fake key, loopback API, isolated Homes, no project input.

Build: cargo build -p rovai-core --example custom_api_native_fixture
Run: python3 scripts/smoke-runtime-custom-api.py --codex /absolute/codex [--claude ...]
No user endpoint or key is accepted by this script. --official-roundtrip-root opts into real
official calls using an existing isolated native login; no daily credentials are copied.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import tempfile
import threading
import time
import uuid
from urllib.parse import urlsplit

FAKE_KEY = "rovai-isolated-fake-key"
ROTATED_FAKE_KEY = "rovai-isolated-rotated-key"
REQUESTS = []


class Api(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        authorization = self.headers.get("Authorization", "")
        auth_header = "bearer" if authorization else "x-api-key"
        if not authorization and self.headers.get("x-api-key"):
            authorization = "Bearer " + self.headers["x-api-key"]
        version = 1 if authorization == "Bearer " + FAKE_KEY else 2 if authorization == "Bearer " + ROTATED_FAKE_KEY else 0
        REQUESTS.append({"path": self.path, "model": body.get("model"), "keyMatches": version > 0, "keyVersion": version, "authHeader": auth_header, "nativeHeaderPreserved":self.headers.get("x-rovai-fixture") == "preserved", "proxyAuthPreserved":self.headers.get("Proxy-Authorization") == "Basic isolated-proxy-key"})
        model = body.get("model", "fixture")
        route = self.path.split("?", 1)[0]
        if route.endswith("/messages"):
            events = [("message_start", {"type": "message_start", "message": {"id": "msg_fixture", "type": "message", "role": "assistant", "model": model, "content": [], "stop_reason": None, "usage": {"input_tokens": 1, "output_tokens": 0}}}),
                      ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                      ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "OK"}}),
                      ("content_block_stop", {"type": "content_block_stop", "index": 0}),
                      ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 1}}),
                      ("message_stop", {"type": "message_stop"})]
        elif route.endswith("/chat/completions"):
            events = [(None, {"id": "chat_fixture", "object": "chat.completion.chunk", "created": 1, "model": model, "choices": [{"index": 0, "delta": {"role": "assistant", "content": "OK"}, "finish_reason": None}]}),
                      (None, {"id": "chat_fixture", "object": "chat.completion.chunk", "created": 1, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})]
        else:
            item = {"id": "msg_fixture", "type": "message", "status": "completed", "role": "assistant", "content": [{"type": "output_text", "text": "OK", "annotations": []}]}
            response = {"id": "resp_fixture", "object": "response", "created_at": 1, "status": "completed", "model": model, "output": [item], "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2, "input_tokens_details": {"cached_tokens": 0}, "output_tokens_details": {"reasoning_tokens": 0}}}
            events = [("response.created", {"type": "response.created", "response": {**response, "status": "in_progress", "output": []}}),
                      ("response.output_item.added", {"type": "response.output_item.added", "output_index": 0, "item": {**item, "status": "in_progress", "content": []}}),
                      ("response.content_part.added", {"type": "response.content_part.added", "item_id": "msg_fixture", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}),
                      ("response.output_text.delta", {"type": "response.output_text.delta", "item_id": "msg_fixture", "output_index": 0, "content_index": 0, "delta": "OK"}),
                      ("response.output_text.done", {"type": "response.output_text.done", "item_id": "msg_fixture", "output_index": 0, "content_index": 0, "text": "OK"}),
                      ("response.output_item.done", {"type": "response.output_item.done", "output_index": 0, "item": item}),
                      ("response.completed", {"type": "response.completed", "response": response})]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for sequence, (event, data) in enumerate(events):
            if event and event.startswith("response."):
                data["sequence_number"] = sequence
            self.wfile.write((("event: " + event + "\n" if event else "") + "data: " + json.dumps(data) + "\n\n").encode())
        if route.endswith("/chat/completions"):
            self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


class Native:
    def __init__(self, helper, executable, root, config, private=False):
        path = root / "connection.json"
        path.write_text(json.dumps(config))
        (root / ".rovai-custom-api-fixture").touch()
        env = {"PATH": os.environ["PATH"], "HOME": str(root), "USERPROFILE": str(root),
               "GROK_HOME": str(root / "grok"), "LANG": "en_US.UTF-8"}
        self.child = subprocess.Popen([str(helper), str(executable), str(root), str(path)], cwd=root,
                                     env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.frames = queue.Queue()
        self.errors = []
        self.rid = 0
        self.private = private
        def read():
            for line in self.child.stdout:
                try:
                    self.frames.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
            self.frames.put({"fixtureExited": True})
        def stderr():
            for line in self.child.stderr:
                if not private:
                    self.errors.append(line.replace(FAKE_KEY, "<fake-key>").replace(ROTATED_FAKE_KEY, "<rotated-fake-key>"))
        threading.Thread(target=read, daemon=True).start()
        threading.Thread(target=stderr, daemon=True).start()

    def send(self, frame):
        self.child.stdin.write(json.dumps(frame) + "\n")
        self.child.stdin.flush()

    def wait(self, predicate, timeout=35):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            frame = self.frames.get(timeout=max(.1, end - time.monotonic()))
            if frame.get("fixtureExited"):
                raise AssertionError("Native exited: " + "".join(self.errors)[-1600:])
            if predicate(frame):
                return frame
        raise AssertionError("native protocol timed out")

    def rpc(self, method, params):
        self.rid += 1
        self.send({"jsonrpc": "2.0", "id": self.rid, "method": method, "params": params})
        frame = self.wait(lambda f: f.get("id") == self.rid)
        assert "error" not in frame, "Native request failed" if self.private else str(frame).replace(FAKE_KEY, "<fake-key>").replace(ROTATED_FAKE_KEY, "<rotated-fake-key>")
        return frame.get("result", {})

    def control(self, subtype):
        self.send({"type": "control_request", "request_id": subtype, "request": {"subtype": subtype}})
        frame = self.wait(lambda f: f.get("response", {}).get("request_id") == subtype)
        assert frame["response"]["subtype"] == "success", "native control unavailable: " + subtype
        return frame["response"]["response"]

    def close(self):
        if self.child.poll() is not None:
            return
        try:
            os.killpg(self.child.pid, signal.SIGTERM)
            self.child.wait(timeout=4)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(self.child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.child.wait()


def run(kind, executable, helper, root, base):
    config = {"kind": {"claude": "claude-code-cli", "codex": "codex-cli"}[kind], "mode": "custom_api", "baseUrl": base}
    if kind == "codex":
        (root / "codex").mkdir()
        (root / "codex/config.toml").write_text('model_provider="relay.test"\n[model_providers."relay.test"]\nname="Fixture relay"\nbase_url="http://127.0.0.1:1/old"\nwire_api="responses"\nsupports_websockets=true\nrequest_max_retries=0\nstream_max_retries=0\nstream_idle_timeout_ms=15000\nwebsocket_connect_timeout_ms=2000\n[model_providers."relay.test".query_params]\napi-version="fixture-v1"\n[model_providers."relay.test".http_headers]\nProxy-Authorization="Basic isolated-proxy-key"\nx-rovai-fixture="preserved"\n[model_providers."relay.test".env_http_headers]\nx-optional="UNSET_FIXTURE_HEADER"\n')
        config.update(models=[{"rowId":"known", "id": "gpt-6.1-sol", "displayName": "Known"}, {"rowId":"unknown", "id": "rovai-unknown", "displayName": "Unknown"}], defaultModel="rovai-unknown", defaultRowId="unknown")
    elif kind == "claude":
        config["models"] = {"model": "rovai-main", "reasoningModel": "rovai-thinking", "haikuModel": "rovai-haiku", "sonnetModel": "rovai-sonnet", "opusModel": "rovai-opus"}
        (root / "claude").mkdir()
        (root / "claude/settings.json").write_text(json.dumps({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:1/old", "ANTHROPIC_AUTH_TOKEN": "old-fake-key", "ANTHROPIC_MODEL": "old-model", "ANTHROPIC_CUSTOM_HEADERS":"x-rovai-fixture: preserved\nProxy-Authorization: Basic isolated-proxy-key"}}))
    start = len(REQUESTS)
    if kind == "codex":
        # Existing native API needs neither an initial save nor a model allowlist.
        path = root / "codex/config.toml"
        original = path.read_text()
        inherited_text = original.replace('http://127.0.0.1:1/old', base).replace('old-fake-key', FAKE_KEY)
        inherited_text = 'model="gpt-6.1-sol"\n' + inherited_text.replace('wire_api="responses"', 'wire_api="responses"\nexperimental_bearer_token="' + FAKE_KEY + '"')
        path.write_text(inherited_text)
        (root / "reuse-native-fixture").touch()
        inherited = Native(helper, executable, root, config)
        try:
            inherited.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
            inherited.send({"method":"initialized","params":{}})
            provider = inherited.rpc("config/read", {"cwd":str(root),"includeLayers":False})["config"]["model_provider"]
            session = inherited.rpc("thread/start", {"cwd":str(root),"model":"rovai-unknown","approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
            inherited.rpc("turn/start", {"threadId":session["thread"]["id"],"input":[{"type":"text","text":"Reply OK."}]})
            assert inherited.wait(lambda f:f.get("method")=="turn/completed")["params"]["turn"]["status"] == "completed"
            assert path.read_text() == inherited_text, "reading/using native settings must not rewrite them"
        finally:
            inherited.close()
            (root / "reuse-native-fixture").unlink()
        path.write_text(original)
    native = Native(helper, executable, root, config)
    try:
        if kind == "claude":
            identity = native.control("initialize")["account"]
            settings = native.control("get_settings")
            assert settings["effective"]["env"]["ANTHROPIC_AUTH_TOKEN"] == FAKE_KEY
            assert settings["effective"]["env"]["ANTHROPIC_REASONING_MODEL"] == "rovai-thinking"
            assert settings["applied"]["model"] == "rovai-main"
            assert identity["tokenSource"] == "ANTHROPIC_AUTH_TOKEN" and identity["apiProvider"] == "firstParty"
            assert settings["effective"]["env"]["ANTHROPIC_BASE_URL"] == base
            native.send({"type": "user", "session_id": str(uuid.uuid4()), "message": {"role": "user", "content": "Reply OK."}, "parent_tool_use_id": None})
            result = native.wait(lambda f: f.get("type") == "result")
            assert not result.get("is_error"), "Claude native result failed: " + json.dumps(result).replace(FAKE_KEY, "<fake-key>").replace(ROTATED_FAKE_KEY, "<rotated-fake-key>")
        elif kind == "codex":
            native.rpc("initialize", {"clientInfo": {"name": "rovai_fixture", "version": "1"}, "capabilities": {"experimentalApi": True}})
            native.send({"method": "initialized", "params": {}})
            effective = native.rpc("config/read", {"cwd": str(root), "includeLayers": False})["config"]
            provider_id = effective["model_provider"]
            assert effective["model"] == "rovai-unknown", "native default must match the selected row"
            assert provider_id == "relay.test", "execution must retain the native provider"
            provider = effective["model_providers"][provider_id]
            assert provider["http_headers"]["Proxy-Authorization"] == "Basic isolated-proxy-key", "proxy credentials must remain independent from the model key"
            assert provider["base_url"] == base and provider["experimental_bearer_token"] == FAKE_KEY
            assert provider.get("env_key") is None, "native writes must not inject a second key environment"
            for field, expected_value in {"query_params":{"api-version":"fixture-v1"}, "supports_websockets":True, "request_max_retries":0, "stream_max_retries":0, "stream_idle_timeout_ms":15000, "websocket_connect_timeout_ms":2000}.items():
                assert provider[field] == expected_value, "lost native provider field: " + field
            catalog = native.rpc("model/list", {"includeHidden": True, "limit": 100})["data"]
            ids = {m["model"] for m in catalog}
            assert {"gpt-6.1-sol", "rovai-unknown"} <= ids
            sessions = []
            for model in ["gpt-6.1-sol", "rovai-unknown"]:
                session = native.rpc("thread/start", {"cwd": str(root), "model": model, "approvalPolicy": "never", "sandbox": "read-only", "ephemeral": False})
                sessions.append(session["thread"]["id"])
                assert session["modelProvider"] == provider_id and session["model"] == model
                native.rpc("turn/start", {"threadId": session["thread"]["id"], "input": [{"type": "text", "text": "Reply OK."}]})
                result = native.wait(lambda f: f.get("method") == "turn/completed")
                assert result["params"]["turn"]["status"] == "completed", "Codex turn failed"
            # A new process resumes with the new connection; an already-running
            # different thread in the old process keeps its captured credentials.
            native_config_path = root / "codex/config.toml"
            config_text = native_config_path.read_text()
            previous_catalog_path = Path(json.loads((root / "catalog-path.json").read_text()))
            previous_catalog_bytes = previous_catalog_path.read_bytes()
            existing_catalog = json.loads(previous_catalog_bytes)
            existing_entry = next(m for m in existing_catalog["models"] if m["slug"] == "rovai-unknown")
            existing_entry.update(context_window=8192, max_context_window=8192, input_modalities=["text"], supports_reasoning_summary_parameter=False, future_native_metadata={"preserved":True})
            external_catalog_path = root / "codex/external-native-catalog.json"
            external_catalog_path.write_text(json.dumps(existing_catalog))
            native_config_path.write_text(config_text.replace(str(previous_catalog_path), str(external_catalog_path)))
            (root / "rotate-fixture-key").touch()
            renamed_models = [{**row,"displayName":"Edited existing name"} if row["id"] == "rovai-unknown" else row for row in config["models"]]
            rotated = Native(helper, executable, root, {**config, "models":renamed_models, "baseUrl": base + "/rotated"})
            try:
                rotated.rpc("initialize", {"clientInfo": {"name": "rovai_fixture", "version": "1"}})
                rotated.send({"method": "initialized", "params": {}})
                rotated_provider = rotated.rpc("config/read", {"cwd":str(root),"includeLayers":False})["config"]["model_provider"]
                assert rotated_provider == provider_id, "native provider identity is stable; connection fingerprint fences reuse"
                edited_catalog_path = Path(json.loads((root / "catalog-path.json").read_text()))
                edited_entry = next(m for m in json.loads(edited_catalog_path.read_text())["models"] if m["slug"] == "rovai-unknown")
                assert edited_entry == {**existing_entry,"display_name":"Edited existing name"}, "a label edit must retain all native model metadata"
                assert previous_catalog_path.read_bytes() == previous_catalog_bytes, "old runtime catalog stays immutable"

                fresh = rotated.rpc("thread/start", {"cwd":str(root),"model":"rovai-unknown","approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
                boundary = len(REQUESTS)
                rotated.rpc("turn/start", {"threadId": fresh["thread"]["id"], "input": [{"type":"text","text":"Reply OK."}]})
                result = rotated.wait(lambda f:f.get("method") == "turn/completed")
                assert result["params"]["turn"]["status"] == "completed"
                assert REQUESTS[boundary:] and all(r["keyVersion"] == 2 and r["path"].startswith("/custom/prefix/rotated/") for r in REQUESTS[boundary:])
                boundary = len(REQUESTS)
                native.rpc("turn/start", {"threadId":sessions[0],"input":[{"type":"text","text":"Reply OK."}]})
                result = native.wait(lambda f:f.get("method") == "turn/completed")
                assert result["params"]["turn"]["status"] == "completed"
                assert REQUESTS[boundary:] and all(r["keyVersion"] == 1 and not r["path"].startswith("/custom/prefix/rotated/") for r in REQUESTS[boundary:])
                native.close()  # release the native writer before resuming its persisted thread
                resumed = rotated.rpc("thread/resume", {"threadId": sessions[-1], "cwd": str(root), "model": "rovai-unknown", "approvalPolicy": "never", "sandbox": "read-only"})
                assert resumed["modelProvider"] == rotated_provider and resumed["model"] == "rovai-unknown"
                boundary = len(REQUESTS)
                rotated.rpc("turn/start", {"threadId":sessions[-1],"input":[{"type":"text","text":"Reply OK."}]})
                result = rotated.wait(lambda f:f.get("method") == "turn/completed")
                assert result["params"]["turn"]["status"] == "completed"
                assert REQUESTS[boundary:] and all(r["keyVersion"] == 2 and r["path"].startswith("/custom/prefix/rotated/") for r in REQUESTS[boundary:])
            finally:
                rotated.close()
            # Rename onto an existing hidden ID using the same stable row identity
            # as the production editor. Native loading/execution must still work.
            source_catalog = json.loads(edited_catalog_path.read_text())
            hidden = next(entry for entry in source_catalog["models"] if entry["visibility"] != "list" and entry.get("supported_in_api"))
            collision_models = [{**row, "rowId":"sha256:" + hashlib.sha256(json.dumps(row["id"]).encode()).hexdigest(), "id":hidden["slug"] if row["id"] == "rovai-unknown" else row["id"], "displayName":"Hidden ID selected" if row["id"] == "rovai-unknown" else row["displayName"]} for row in config["models"]]
            default_row = next(row["rowId"] for row in collision_models if row["id"] == hidden["slug"])
            collision = Native(helper, executable, root, {**config, "models":collision_models, "defaultRowId":default_row, "defaultModel":hidden["slug"]})
            try:
                collision.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
                collision.send({"method":"initialized","params":{}})
                entries = json.loads(Path(json.loads((root / "catalog-path.json").read_text())).read_text())["models"]
                assert len({entry["slug"] for entry in entries}) == len(entries)
                assert next(entry for entry in entries if entry["slug"] == hidden["slug"]) == {**hidden,"visibility":"list","display_name":"Hidden ID selected"}
                session = collision.rpc("thread/start", {"cwd":str(root),"model":hidden["slug"],"approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
                collision.rpc("turn/start", {"threadId":session["thread"]["id"],"input":[{"type":"text","text":"Reply OK."}]})
                assert collision.wait(lambda f:f.get("method")=="turn/completed")["params"]["turn"]["status"] == "completed"
            finally:
                collision.close()
        native.close()
        if kind == "claude":
            # Reuse a shell-only credential without copying it into native settings.
            path = root / "claude/settings.json"
            data = json.loads(path.read_text())
            data["env"].pop("ANTHROPIC_AUTH_TOKEN")
            path.write_text(json.dumps(data))
            (root / "shell-credential-fixture").touch()
            shell = Native(helper, executable, root, config)
            try:
                identity = shell.control("initialize")["account"]
                resolved = shell.control("get_settings")
                assert "ANTHROPIC_AUTH_TOKEN" not in resolved["effective"]["env"]
                assert identity["tokenSource"] == "ANTHROPIC_AUTH_TOKEN"
                shell.send({"type":"user","session_id":str(uuid.uuid4()),"message":{"role":"user","content":"Reply OK."},"parent_tool_use_id":None})
                assert not shell.wait(lambda f:f.get("type") == "result").get("is_error")
                assert FAKE_KEY not in path.read_text(), "shell credentials must not be copied into a file"
            finally:
                shell.close()
            # Rotate an existing x-api-key connection and retain the header and top-level model source.
            (root / "shell-credential-fixture").unlink()
            data = json.loads(path.read_text())
            data["env"].pop("ANTHROPIC_AUTH_TOKEN", None)
            data["env"]["ANTHROPIC_API_KEY"] = "previous-x-api-key"
            data["env"].pop("ANTHROPIC_MODEL", None)
            data["model"] = "old-top-model"
            path.write_text(json.dumps(data))
            boundary = len(REQUESTS)
            rotated_header = Native(helper, executable, root, config)
            try:
                identity = rotated_header.control("initialize")["account"]
                resolved = rotated_header.control("get_settings")
                assert identity["apiKeySource"] == "ANTHROPIC_API_KEY"
                assert resolved["applied"]["model"] == "rovai-main"
                assert json.loads(path.read_text())["model"] == "rovai-main"
                rotated_header.send({"type":"user","session_id":str(uuid.uuid4()),"message":{"role":"user","content":"Reply OK."},"parent_tool_use_id":None})
                assert not rotated_header.wait(lambda f:f.get("type")=="result").get("is_error")
                assert REQUESTS[boundary:] and all(r["authHeader"] == "x-api-key" for r in REQUESTS[boundary:])
            finally:
                rotated_header.close()
            # Fake official OAuth input only checks native status; no official request is sent.
            (root / "official-oauth-fixture").touch()
        # Explicit Save switches native configuration; launch adds no connection overlay.
        config_path = root / ("claude/settings.json" if kind == "claude" else "codex/config.toml")
        native_before = config_path.read_bytes()
        official = Native(helper, executable, root, {**config, "mode":"official_login"})
        try:
            if kind == "codex":
                official.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
                official.send({"method":"initialized","params":{}})
                resolved = official.rpc("config/read", {"cwd":str(root),"includeLayers":False})["config"]
                assert resolved["model_provider"] == "openai"
                assert not resolved.get("openai_base_url")
                assert resolved["model"] != "rovai-unknown"
                account = official.rpc("account/read", {"refreshToken":False})
                assert account["account"] is None, "no native login in the isolated fixture"
            else:
                identity = official.control("initialize")["account"]
                settings = official.control("get_settings")
                for name in ["ANTHROPIC_BASE_URL","ANTHROPIC_AUTH_TOKEN","ANTHROPIC_API_KEY"]:
                    assert not settings["effective"].get("env", {}).get(name)
                assert identity["tokenSource"] == "CLAUDE_CODE_OAUTH_TOKEN"
            assert config_path.read_bytes() != native_before, "official Save must change the actual native selection"
            assert not (root / "derived").exists(), "normal launch must not generate temporary connection files"
        finally:
            official.close()
        requests = REQUESTS[start:]
        assert requests and all(r["keyMatches"] and r["path"].startswith("/custom/prefix/") for r in requests), requests
        if kind == "codex":
            assert all(r["authHeader"] == "bearer" for r in requests)
        else:
            assert {r["authHeader"] for r in requests} == {"bearer", "x-api-key"}
        expected = {"claude": {"rovai-main"}, "codex": {"gpt-6.1-sol", "rovai-unknown"}}[kind]
        assert expected <= {r["model"] for r in requests}, requests
        assert all(r["nativeHeaderPreserved"] for r in requests)
        if kind == "claude":
            assert all(r["proxyAuthPreserved"] for r in requests)
        # Codex's HTTP stack scopes Proxy-Authorization to proxy traffic; it need
        # not forward that credential to the origin. config/read above owns retention.
        if kind == "codex":
            assert all("api-version=fixture-v1" in r["path"] for r in requests), "native query parameters were lost"
        return {"runtime": kind, "status": "passed", "requests": requests}
    finally:
        native.close()


def run_auto_fallback(executable, helper, root, base):
    root.mkdir(parents=True, exist_ok=False)
    (root / "codex").mkdir()
    (root / "reuse-native-fixture").touch()
    (root / "codex/config.toml").write_text('cli_auth_credentials_store="auto"\nmodel="gpt-6.1-sol"\nopenai_base_url=' + json.dumps(base) + '\n')
    auth = root / "codex/auth.json"
    auth.write_text(json.dumps({"OPENAI_API_KEY":FAKE_KEY}))
    auth.chmod(0o600)
    before = auth.read_bytes()
    config = {"kind":"codex-cli","mode":"custom_api","baseUrl":base,"models":[{"rowId":"a","id":"gpt-6.1-sol","displayName":""}],"defaultModel":"gpt-6.1-sol","defaultRowId":"a"}
    for address in [base, base + "/edited"]:
        if address != base:
            (root / "reuse-native-fixture").unlink()
            (root / "edit-address-only-fixture").touch()
        boundary = len(REQUESTS)
        native = Native(helper, executable, root, {**config,"baseUrl":address})
        try:
            native.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
            native.send({"method":"initialized","params":{}})
            effective = native.rpc("config/read", {"cwd":str(root),"includeLayers":False})["config"]
            assert effective.get("model_provider") in [None, "openai"] and effective["openai_base_url"] == address
            assert native.rpc("account/read", {"refreshToken":False})["account"]["type"] == "apiKey"
            session = native.rpc("thread/start", {"cwd":str(root),"model":"gpt-6.1-sol","approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
            assert session["modelProvider"] == "openai"
            native.rpc("turn/start", {"threadId":session["thread"]["id"],"input":[{"type":"text","text":"Reply OK."}]})
            assert native.wait(lambda f:f.get("method")=="turn/completed")["params"]["turn"]["status"] == "completed"
            assert REQUESTS[boundary:] and all(r["keyMatches"] and r["path"] == urlsplit(address).path + "/responses" for r in REQUESTS[boundary:])
            assert auth.read_bytes() == before, "auto fallback/address editing must not migrate/delete file credentials"
        finally:
            native.close()
    return {"runtime":"codex","case":"auto-file-fallback-and-address-edit","status":"passed"}


def run_official_roundtrip(kind, executable, helper, root, base):
    """Explicit development acceptance only. No daily credentials are imported.

    User completes native login in root/claude or root/codex first, with HOME=root/home.
    Real official calls send only a fixed minimal prompt, and may incur usage/fees.
    The API leg always uses the same fake key and loopback server as the local smoke.
    """
    assert root.is_absolute() and (root / ".rovai-official-login-acceptance").is_file(), "isolated official-login marker missing"
    assert not any((root / marker).exists() for marker in ["official-oauth-fixture", "shell-credential-fixture", "reuse-native-fixture", "rotate-fixture-key"]), "fake-only markers are not valid official evidence"
    config = {"kind":{"claude":"claude-code-cli","codex":"codex-cli"}[kind],"baseUrl":base}
    if kind == "claude":
        config["models"] = {"model":"rovai-roundtrip","reasoningModel":"","haikuModel":"","sonnetModel":"","opusModel":""}
    else:
        config.update(models=[{"rowId":"test","id":"gpt-6.1-sol","displayName":""}],defaultModel="gpt-6.1-sol",defaultRowId="test")
    official_identity = None
    completed = []
    for mode in ["official_login", "custom_api", "official_login"]:
        boundary = len(REQUESTS)
        native = Native(helper, executable, root, {**config,"mode":mode}, private=True)
        try:
            if kind == "claude":
                account = native.control("initialize")["account"]
                native.control("get_settings")  # acceptance-only observation, not an execution gate
                if mode == "official_login":
                    assert account.get("tokenSource") in ["claude.ai", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"] or ("tokenSource" not in account and "subscriptionType" in account), "complete isolated native Claude official login first"
                    identity = {k:account.get(k) for k in ["email","organization","tokenSource","subscriptionType"]}
                    if official_identity is None:
                        official_identity = identity
                    assert official_identity == identity, "official identity changed across switching"
                else:
                    assert account.get("tokenSource") == "ANTHROPIC_AUTH_TOKEN", "API credential source mismatch"
                native.send({"type":"user","session_id":str(uuid.uuid4()),"message":{"role":"user","content":"Reply only OK. Do not use tools."},"parent_tool_use_id":None})
                assert not native.wait(lambda f:f.get("type")=="result", timeout=90).get("is_error"), "native Claude call failed"
            else:
                native.rpc("initialize", {"clientInfo":{"name":"rovai_official_acceptance","version":"1"}})
                native.send({"method":"initialized","params":{}})
                effective = native.rpc("config/read", {"cwd":str(root),"includeLayers":False})["config"]
                if mode == "official_login":
                    account = native.rpc("account/read", {"refreshToken":False}).get("account") or {}
                    assert account.get("type") == "chatgpt", "complete isolated native ChatGPT login first"
                    if official_identity is None:
                        official_identity = account
                    assert official_identity == account, "official identity changed across switching"
                    assert effective["model_provider"] == "openai" and effective["openai_base_url"] == "https://chatgpt.com/backend-api/codex"
                session = native.rpc("thread/start", {"cwd":str(root / "work"),"approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
                native.rpc("turn/start", {"threadId":session["thread"]["id"],"input":[{"type":"text","text":"Reply only OK. Do not use tools."}]})
                assert native.wait(lambda f:f.get("method")=="turn/completed", timeout=90)["params"]["turn"]["status"] == "completed", "native Codex call failed"
            calls = REQUESTS[boundary:]
            if mode == "custom_api":
                assert calls and all(r["keyMatches"] for r in calls), "API leg did not receive the isolated key"
            else:
                assert not calls, "official leg still used the API endpoint"
            completed.append(mode)
        finally:
            native.close()
    return {"runtime":kind,"case":"real-official-api-roundtrip","status":"passed","completed":completed}


def run_auth_migrations(kind, executable, helper, root, base):
    root.mkdir(parents=True, exist_ok=False)
    cases = ["command", "aws"] if kind == "codex" else ["BEDROCK", "VERTEX", "FOUNDRY"]
    for case in cases:
        directory = root / case
        native_home = directory / ("codex" if kind == "codex" else "claude")
        native_home.mkdir(parents=True)
        config = {"kind": "codex-cli" if kind == "codex" else "claude-code-cli", "mode":"custom_api", "baseUrl":base + "/" + case}
        if kind == "codex":
            config.update(models=[{"rowId":"a","id":"gpt-6.1-sol","displayName":""}], defaultModel="gpt-6.1-sol", defaultRowId="a")
            auth = 'auth={command="/bin/echo",args=["' + FAKE_KEY + '"]}' if case == "command" else 'aws={region="us-east-1"}'
            (native_home / "config.toml").write_text('model_provider="relay"\nmodel="gpt-6.1-sol"\n[model_providers.relay]\nname="Fixture"\nbase_url=' + json.dumps(config["baseUrl"]) + '\nwire_api="responses"\n' + auth + '\nquery_params={fixture="keep"}\nrequest_max_retries=0\n')
            (directory / "reuse-native-fixture").touch()
            inherited = Native(helper, executable, directory, config)
            try:
                inherited.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
                inherited.send({"method":"initialized","params":{}})
                projected = json.loads((directory / "native-read.json").read_text())
                assert projected["credential"]["status"] == "available"
                assert ("原生命令" if case == "command" else "AWS") in projected["credential"]["sourceLabel"]
                if case == "command":
                    thread = inherited.rpc("thread/start", {"cwd":str(directory),"model":"gpt-6.1-sol","approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
                    inherited.rpc("turn/start", {"threadId":thread["thread"]["id"],"input":[{"type":"text","text":"Reply OK."}]})
                    assert inherited.wait(lambda f:f.get("method")=="turn/completed")["params"]["turn"]["status"] == "completed"
            finally:
                inherited.close()
            (directory / "reuse-native-fixture").unlink()
        else:
            config["models"] = {"model":"rovai-main","reasoningModel":"","haikuModel":"","sonnetModel":"","opusModel":""}
            (native_home / "settings.json").write_text(json.dumps({"env":{"CLAUDE_CODE_USE_"+case:"1","ANTHROPIC_"+case+"_BASE_URL":"https://cloud.invalid/old","ANTHROPIC_MODEL":"old-cloud-model"}}))
        boundary = len(REQUESTS)
        replaced = Native(helper, executable, directory, config)
        try:
            if kind == "codex":
                replaced.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
                replaced.send({"method":"initialized","params":{}})
                provider = replaced.rpc("config/read", {"cwd":str(directory),"includeLayers":False})["config"]["model_providers"]["relay"]
                assert all(provider.get(name) is None for name in ["auth","aws","env_key"])
                assert provider["experimental_bearer_token"] == FAKE_KEY and provider["query_params"]["fixture"] == "keep"
                thread = replaced.rpc("thread/start", {"cwd":str(directory),"model":"gpt-6.1-sol","approvalPolicy":"never","sandbox":"read-only","ephemeral":True})
                replaced.rpc("turn/start", {"threadId":thread["thread"]["id"],"input":[{"type":"text","text":"Reply OK."}]})
                assert replaced.wait(lambda f:f.get("method")=="turn/completed")["params"]["turn"]["status"] == "completed"
            else:
                replaced.control("initialize")
                projected = json.loads((directory / "native-read.json").read_text())
                assert projected["configuration"]["baseUrl"] == "https://cloud.invalid/old"
                assert projected["credential"]["source"] == "native_cloud"
                saved = json.loads((native_home / "settings.json").read_text())
                assert saved["env"].get("CLAUDE_CODE_USE_"+case) in [None,"0",""]
                replaced.send({"type":"user","message":{"role":"user","content":"Reply OK."}})
                assert not replaced.wait(lambda f:f.get("type")=="result").get("is_error",False)
            assert REQUESTS[boundary:] and all(r["keyMatches"] and r["path"].startswith(urlsplit(config["baseUrl"]).path + "/") for r in REQUESTS[boundary:])
        finally:
            replaced.close()
    return {"runtime":kind,"case":"native-auth-source-migrations","status":"passed","sources":cases}


def run_managed_auth_switch(executable, helper, root, store):
    root.mkdir(parents=True, exist_ok=False)
    home = root / "codex"
    home.mkdir()
    (home / "config.toml").write_text('cli_auth_credentials_store=' + json.dumps(store) + '\n')
    env = {"PATH":os.environ["PATH"],"HOME":str(root),"USERPROFILE":str(root),"CODEX_HOME":str(home),"DO_NOT_TRACK":"1"}
    logged_in = False
    try:
        if store == "keyring":
            login = subprocess.run([str(executable),"login","--with-api-key"],input=FAKE_KEY + "\n",env=env,cwd=root,capture_output=True,text=True,timeout=20)
            assert login.returncode == 0, "isolated native credential-store setup unavailable (no daily keychain is used)"
            logged_in = True
        else:
            (home / "auth.json").write_text(json.dumps({"auth_mode":"apikey","OPENAI_API_KEY":FAKE_KEY}))
        config = {"kind":"codex-cli","mode":"custom_api","baseUrl":"https://api.openai.com/v1","models":[{"rowId":"a","id":"gpt-6.1-sol","displayName":""}],"defaultModel":"gpt-6.1-sol","defaultRowId":"a"}
        (root / "reuse-native-fixture").touch()
        native = Native(helper, executable, root, config)
        try:
            native.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
            native.send({"method":"initialized","params":{}})
            assert native.rpc("account/read", {"refreshToken":False})["account"]["type"] == "apiKey"
            projected = json.loads((root / "native-read.json").read_text())
            assert projected["configuration"]["mode"] == "custom_api" and projected["credential"]["status"] == "available"
            if store == "keyring":
                assert not (home / "auth.json").exists(), "keyring credential must not be copied to a file"
        finally:
            native.close()
        official = Native(helper, executable, root, {**config,"mode":"official_login"})
        try:
            official.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
            official.send({"method":"initialized","params":{}})
            response = official.rpc("account/read", {"refreshToken":False})
            assert response["account"] is None and response["requiresOpenaiAuth"], "old managed API must no longer authenticate the official path"
            projected = json.loads((root / "native-after.json").read_text())
            assert projected["configuration"]["mode"] == "official_login" and projected["observation"]["loginStatus"] == "signed_out"
            if store == "keyring":
                assert not (home / "auth.json").exists()
        finally:
            official.close()
        if store == "auto":
            # Exercise the actual native auth-type filter even with a retained
            # API object. The keyring fixture tests the same store-independent
            # path without depending on the OS's interactive secure backend.
            (home / "auth.json").write_text(json.dumps({"auth_mode":"apikey","OPENAI_API_KEY":FAKE_KEY}))
            native = Native(helper, executable, root, config)
            try:
                native.rpc("initialize", {"clientInfo":{"name":"rovai_fixture","version":"1"}})
                native.send({"method":"initialized","params":{}})
                response = native.rpc("account/read", {"refreshToken":False})
                assert response["account"] is None and response["requiresOpenaiAuth"], "native ChatGPT selection must ignore a retained API auth object"
                assert json.loads((home / "auth.json").read_text())["OPENAI_API_KEY"] == FAKE_KEY, "observation must not log out or modify the native auth object"
            finally:
                native.close()
        return {"runtime":"codex","case":"isolated-native-"+store+"-api-to-official","status":"passed","officialLogin":"required"}
    finally:
        # Only the fake credential under this newly-created CODEX_HOME is removed.
        if logged_in:
            cleanup = subprocess.run([str(executable),"logout"],env=env,cwd=root,capture_output=True,timeout=20)
            assert cleanup.returncode == 0, "isolated fake native credential cleanup failed"
        elif store != "keyring":
            (home / "auth.json").unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for kind in ["claude", "codex"]:
        parser.add_argument("--" + kind, type=Path)
    parser.add_argument("--helper", type=Path, default=Path("target/debug/examples/custom_api_native_fixture"))
    parser.add_argument("--fixture-root", type=Path, required=True, help="Explicit isolated acceptance directory")
    parser.add_argument("--official-roundtrip-root", type=Path, help="Opt-in: existing isolated native official logins; sends minimal real official requests")
    parser.add_argument("--native-keyring", action="store_true", help="Opt-in: create and clean a fake native keyring credential scoped to the new fixture CODEX_HOME")
    args = parser.parse_args()
    helper = args.helper.resolve()
    assert helper.is_file(), "build the native fixture helper first"
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Api)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    results = []
    try:
        for kind in ["claude", "codex"]:
            executable = getattr(args, kind)
            if executable is None:
                continue
            directory = args.fixture_root.resolve() / (kind + "-native")
            directory.mkdir(parents=True, exist_ok=False)
            print("Isolated native fixture: " + str(directory), flush=True)
            try:
                result = run(kind, executable.resolve(), helper, directory, "http://127.0.0.1:" + str(server.server_port) + "/custom/prefix")
            except Exception:
                import traceback
                result = {"runtime":kind,"status":"failed","error":traceback.format_exc().replace(FAKE_KEY,"<fake-key>").replace(ROTATED_FAKE_KEY,"<rotated-fake-key>")}
            results.append(result)
            print(json.dumps(result,ensure_ascii=False),flush=True)
            if kind == "codex" and result["status"] == "passed":
                auto = run_auto_fallback(executable.resolve(), helper, args.fixture_root.resolve() / "codex-auto", "http://127.0.0.1:" + str(server.server_port) + "/auto/prefix")
                results.append(auto)
                print(json.dumps(auto), flush=True)
                for store in ["auto", "file"]:
                    switched = run_managed_auth_switch(executable.resolve(), helper, args.fixture_root.resolve() / ("codex-"+store+"-official"), store)
                    results.append(switched)
                    print(json.dumps(switched), flush=True)
            if result["status"] == "passed":
                migrated = run_auth_migrations(kind, executable.resolve(), helper, args.fixture_root.resolve() / (kind + "-auth-migrations"), "http://127.0.0.1:" + str(server.server_port) + "/auth-switch")
                results.append(migrated)
                print(json.dumps(migrated), flush=True)
            if kind == "codex" and args.native_keyring:
                keyring = run_managed_auth_switch(executable.resolve(), helper, args.fixture_root.resolve() / "codex-keyring", "keyring")
                results.append(keyring)
                print(json.dumps(keyring), flush=True)
            if args.official_roundtrip_root and result["status"] == "passed":
                try:
                    official = run_official_roundtrip(kind, executable.resolve(), helper, args.official_roundtrip_root.resolve(), "http://127.0.0.1:" + str(server.server_port) + "/official-switch")
                except Exception:
                    # No account metadata, native output or actual credentials in acceptance logs.
                    official = {"runtime":kind,"case":"real-official-api-roundtrip","status":"failed","error":"Native login or roundtrip call failed; inspect the isolated native CLI."}
                results.append(official)
                print(json.dumps(official), flush=True)
    finally:
        server.shutdown()
    assert results and all(result["status"] == "passed" for result in results), "native acceptance failed"


if __name__ == "__main__":
    main()
