"""Exercise a disposable Taboom desktop through public MCP tools, without page inspection.

Run against an isolated Compose project. The test stages a local form, changes its fields and
clipboard, creates a test persona file and recording, and leaves screenshots in the artifact dir.
"""

import argparse
import base64
import json
import os
from pathlib import Path
import time
import urllib.error
import urllib.request


FORM = """<!doctype html><title>Taboom desktop smoke</title>
<style>body{font:20px Arial;margin:60px;color:#151515;background:white}h1{font-size:28px}
label{display:block;margin-bottom:4px}input{display:block;width:420px;height:36px;font:20px Arial;margin-bottom:14px}
button{font:20px Arial;padding:10px;margin-right:20px}#space{height:800px}</style>
<h1>Taboom integration form</h1>
<label for=name>Name</label><input id=name>
<label for=email>Email</label><input id=email>
<label for=city>City</label><input id=city>
<p>Verify you are human</p><button type=button>Hold button</button>
<button type=button>Choice A</button><button type=button>Choice B</button>
<p>Slide control</p><input type=range min=0 max=100 value=20>
<div id=space></div><p>End of fixture</p>"""


class Client:
    def __init__(self, url, artifacts):
        self.url, self.artifacts = url.rstrip("/"), artifacts
        self.sequence = 0
        self.tools_seen = set()
        self.headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        if os.getenv("TABOOM_SMOKE_TOKEN"):
            self.headers["Authorization"] = "Bearer " + os.environ["TABOOM_SMOKE_TOKEN"]

    def rpc(self, method, params=None):
        self.sequence += 1
        data = json.dumps({"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params or {}}).encode()
        request = urllib.request.Request(self.url + "/mcp", data, self.headers)
        with urllib.request.urlopen(request, timeout=120) as response:
            payload = json.load(response)
        if "error" in payload:
            raise RuntimeError(payload["error"])
        return payload["result"]

    def tool(self, name, arguments=None, image_name=None):
        self.tools_seen.add(name)
        result = self.rpc("tools/call", {"name": name, "arguments": arguments or {}})
        blocks = result.get("content", [])
        text = next((b["text"] for b in blocks if b["type"] == "text"), "{}")
        if result.get("isError"):
            raise RuntimeError(f"{name}: {text}")
        if image_name:
            block = next(b for b in blocks if b["type"] == "image")
            (self.artifacts / image_name).write_bytes(base64.b64decode(block["data"]))
        return json.loads(text)

    def label(self, text):
        result = self.tool("find_text", {"text": text})
        matches = result
        assert matches, f"missing visible label {text}"
        match = matches[0]
        return match["x"] + 120, match["y"] + match["h"] + 18

    def dump_failure(self):
        try:
            self.tool("screenshot", image_name="failure.png")
            (self.artifacts / "failure_text.json").write_text(json.dumps(self.tool("read_text"), indent=2))
        except Exception as error:
            print("failure dump skipped:", error)


def run(client, engine="chrome"):
    initialized = client.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "taboom-smoke", "version": "1"}})
    assert initialized["serverInfo"]["name"] == "taboom"
    names = {tool["name"] for tool in client.rpc("tools/list")["tools"]}
    assert "batch_actions" in names
    for resource in client.rpc("resources/list")["resources"]:
        assert client.rpc("resources/read", {"uri": resource["uri"]})["contents"]
    for prompt in client.rpc("prompts/list")["prompts"]:
        assert client.rpc("prompts/get", {"name": prompt["name"], "arguments": {}})["messages"]
    status = client.tool("persona_status")
    assert status["route"]["ok"] and not status["mismatches"], status["mismatches"]
    assert status["declared"]["browser"]["engine"] == engine
    assert status["session"] is None, "run against a desktop without an active session"
    assert status["recording_visuals"] == "enabled"
    assert status["humanizer"]["mouse_model"] == "empirical"
    assert status["humanizer"]["calibration"]["paths"] >= 120
    client.tool("session_start")
    try:
        staged = client.tool("files_put", {"name": "taboom-smoke.html", "data_base64": base64.b64encode(FORM.encode()).decode()})
        assert base64.b64decode(client.tool("files_get", {"path": "taboom-smoke.html"})["data_base64"]) == FORM.encode()
        assert any(f["name"] == "taboom-smoke.html" for f in client.tool("files_list")["files"])
        client.tool("open_url", {"url": "file://" + staged["path"]})
        client.tool("wait", {"until_settled": True})
        frame = client.tool("computer", {"action": "screenshot"}, "before.png")
        fields = [("Name", "Alex"), ("Email", "alex@example.com"), ("City", "Seattle")]
        target = client.tool("find_text", {"text": fields[0][0]})[0]
        actions = [{"tool": "click", "x": target["x"] + target["w"] // 2, "y": target["y"] + target["h"] // 2, "describe": "Focus Name via its label"}]
        for index, (_, value) in enumerate(fields):
            if index:
                actions.append({"tool": "key", "combo": "Tab"})
            actions.append({"tool": "type", "text": value, "mode": "keys"})
        filled = client.tool("batch_actions", {"frame_id": frame["frame_id"], "actions": actions}, "filled.png")
        assert filled["completed_actions"] == len(actions)
        client.tools_seen.update(action["tool"] for action in actions)
        client.tool("wait", {"until_settled": True})
        assert client.tool("read_text"), "no text read from the screen"
        for index, (label, value) in enumerate(reversed(fields)):
            if index:
                client.tool("key", {"combo": "shift+Tab"})
            client.tool("key", {"combo": "ctrl+a ctrl+c"})
            copied = client.tool("clipboard_get")["text"]
            assert copied == value, f"{label} field holds {copied!r}, expected {value!r}"
        client.tool("clipboard_set", {"text": "fixture clipboard"})
        assert client.tool("clipboard_get")["text"] == "fixture clipboard"
        client.tool("key_down", {"key": "shift"})
        client.tool("key_up", {"key": "shift"})
        client.tool("hold_key", {"key": "shift", "duration_ms": 100})
        client.tool("move", {"x": 900, "y": 600})
        client.tool("cursor_position")
        client.tool("mouse_down", {"x": 900, "y": 600})
        client.tool("mouse_up")
        client.tool("scroll", {"direction": "down", "amount": 2})
        client.tool("scroll", {"direction": "up", "amount": 2})
        client.tool("key", {"combo": "ctrl+Home"})
        client.tool("screenshot", image_name="controls.png")
        client.tool("zoom", {"region": {"x": 20, "y": 90, "w": 500, "h": 430}}, "zoom.png")
        client.tool("detect_challenge")
        cx, cy = client.label("City")
        client.tool("challenge_click_hold", {"x": cx - 80, "y": cy + 72,
            "describe": "Exercise the fixture hold button", "hold_ms_min": 100, "hold_ms_max": 150})
        client.tool("challenge_select", {"targets": [{"x": cx + 20, "y": cy + 72}, {"x": cx + 100, "y": cy + 72}],
            "describe": "Exercise the fixture choice buttons"})
        start, end = {"x": cx - 60, "y": cy + 136}, {"x": cx + 80, "y": cy + 136}
        client.tool("drag", {"from": start, "to": end, "describe": "Move the fixture range handle"})
        client.tool("challenge_slider", {"from": end, "to": start, "describe": "Move the fixture range handle back"})
        client.tool("challenge_handoff", {"reason": "Testing manual takeover link"})
        assert "takeover_url" in client.tool("view_url")
        persona = "smoke_" + str(int(time.time()))
        client.tool("persona_generate", {"name": persona, "country": "US"})
        assert any(p["name"] == persona for p in client.tool("persona_list")["personas"])
        assert client.tool("recording_get")["steps"]
        share = client.tool("recording_share")
        with urllib.request.urlopen(share["url"], timeout=10) as response:
            assert response.status == 200
        client.tool("recording_list")
    except Exception:
        client.dump_failure()
        raise
    finally:
        client.tool("session_end")
    missing = sorted(names - client.tools_seen)
    assert not missing, f"desktop smoke did not exercise tools: {missing}"
    return {"exposed_tools": len(names), "exercised_tools": sorted(client.tools_seen), "not_exercised": missing}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://localhost:3456")
    parser.add_argument("--engine", choices=("chrome", "fortress"), default="chrome")
    parser.add_argument("--artifacts", default="/tmp/taboom-smoke")
    args = parser.parse_args()
    artifacts = Path(args.artifacts)
    artifacts.mkdir(parents=True, exist_ok=True)
    report = run(Client(args.url, artifacts), args.engine)
    (artifacts / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
