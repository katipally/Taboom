"""Local event recorder using evdev (Linux) and task-page generator."""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Optional


@dataclass
class Event:
    timestamp_us: int
    type: str
    code: int
    value: int


def load_recording(path: str | Path) -> list[Event]:
    with open(path) as f:
        raw = json.load(f)
    return [Event(**e) for e in raw]


def save_recording(events: list[Event], path: str | Path) -> None:
    with open(path, "w") as f:
        json.dump([asdict(e) for e in events], f)


class EvdevRecorder:
    """Captures REL_X/Y and KEY events from /dev/input/event* devices via evdev."""

    EV_REL = 2
    EV_KEY = 1
    REL_X = 0
    REL_Y = 1
    REL_WHEEL = 8
    REL_WHEEL_HI_RES = 11

    def __init__(self, device_paths: Optional[list[str]] = None):
        try:
            import evdev
        except ImportError as exc:
            raise ImportError(
                "evdev is required for recording. Install with: pip install taboom-lab[recorder]"
            ) from exc
        self._evdev = evdev
        if device_paths is None:
            device_paths = self._find_devices()
        self._device_paths = device_paths

    def _find_devices(self) -> list[str]:
        evdev = self._evdev
        devices = [evdev.InputDevice(p) for p in evdev.list_devices()]
        paths = []
        for dev in devices:
            caps = dev.capabilities()
            has_rel = self.EV_REL in caps
            has_key = self.EV_KEY in caps
            if has_rel or has_key:
                paths.append(dev.path)
            dev.close()
        return paths

    def record_session(self, output_path: str | Path, duration_s: float = 30.0) -> list[Event]:
        evdev = self._evdev
        devices = [evdev.InputDevice(p) for p in self._device_paths]
        events: list[Event] = []
        start = time.monotonic()

        try:
            import select as _select

            while (time.monotonic() - start) < duration_s:
                r, _, _ = _select.select(devices, [], [], 0.01)
                for dev in r:
                    for ev in dev.read():
                        if ev.type in (self.EV_REL, self.EV_KEY):
                            events.append(Event(
                                timestamp_us=int(ev.timestamp() * 1_000_000),
                                type="REL" if ev.type == self.EV_REL else "KEY",
                                code=ev.code,
                                value=ev.value,
                            ))
        finally:
            for dev in devices:
                dev.close()

        if events:
            t0 = events[0].timestamp_us
            for e in events:
                e.timestamp_us -= t0

        save_recording(events, output_path)
        return events


def generate_task_page(
    task: str = "pointing",
    num_targets: int = 10,
    target_size_px: int = 40,
) -> str:
    """Generate an HTML page for standard recording tasks.

    Tasks:
      pointing  -- click randomly placed circular targets in sequence
      typing    -- type prompted sentences into a text field
      scrolling -- scroll through a long page of content
      form      -- fill out a registration-style form
    """
    if task == "pointing":
        return _pointing_page(num_targets, target_size_px)
    elif task == "typing":
        return _typing_page()
    elif task == "scrolling":
        return _scrolling_page()
    elif task == "form":
        return _form_page()
    raise ValueError(f"Unknown task: {task}")


def _pointing_page(num_targets: int, size: int) -> str:
    import random

    targets = []
    for i in range(num_targets):
        x = random.randint(50, 1200)
        y = random.randint(50, 800)
        targets.append({"id": i, "x": x, "y": y})

    targets_json = json.dumps(targets)
    return f"""<!doctype html>
<html><head><title>Pointing Task</title></head>
<body style="margin:0;overflow:hidden;background:#111;cursor:crosshair">
<script>
const targets = {targets_json};
const SIZE = {size};
let idx = 0;
const canvas = document.createElement('canvas');
canvas.width = 1280; canvas.height = 900;
document.body.appendChild(canvas);
const ctx = canvas.getContext('2d');

function draw() {{
  ctx.clearRect(0, 0, 1280, 900);
  if (idx < targets.length) {{
    const t = targets[idx];
    ctx.fillStyle = '#e44';
    ctx.beginPath();
    ctx.arc(t.x, t.y, SIZE/2, 0, Math.PI*2);
    ctx.fill();
    ctx.fillStyle = '#fff';
    ctx.font = '14px sans-serif';
    ctx.fillText((idx+1) + '/' + targets.length, 10, 20);
  }} else {{
    ctx.fillStyle = '#4e4';
    ctx.font = '32px sans-serif';
    ctx.fillText('Done', 600, 450);
  }}
}}

canvas.addEventListener('click', (e) => {{
  if (idx >= targets.length) return;
  const t = targets[idx];
  const dx = e.offsetX - t.x, dy = e.offsetY - t.y;
  if (dx*dx + dy*dy < (SIZE/2)*(SIZE/2)) {{ idx++; draw(); }}
}});
draw();
</script></body></html>"""


def _typing_page() -> str:
    return """<!doctype html>
<html><head><title>Typing Task</title></head>
<body style="font:18px sans-serif;max-width:700px;margin:40px auto">
<p id="prompt">The quick brown fox jumps over the lazy dog.</p>
<textarea id="input" rows="4" cols="60" autofocus
  style="font:18px monospace;width:100%"></textarea>
<p id="status"></p>
<script>
const prompt = document.getElementById('prompt').textContent;
const input = document.getElementById('input');
input.addEventListener('input', () => {
  const typed = input.value;
  if (typed === prompt) document.getElementById('status').textContent = 'Correct!';
});
</script></body></html>"""


def _scrolling_page() -> str:
    paragraphs = "\n".join(
        f"<p>{'Lorem ipsum dolor sit amet. ' * 8}</p>" for _ in range(50)
    )
    return f"""<!doctype html>
<html><head><title>Scrolling Task</title></head>
<body style="font:16px serif;max-width:700px;margin:40px auto">
<h1>Scroll to the bottom</h1>
{paragraphs}
<p id="end" style="color:green;font-size:24px">You reached the end.</p>
</body></html>"""


def _form_page() -> str:
    return """<!doctype html>
<html><head><title>Form Task</title></head>
<body style="font:16px sans-serif;max-width:500px;margin:40px auto">
<h2>Registration</h2>
<form onsubmit="event.preventDefault();document.getElementById('done').hidden=false">
<label>Name<br><input name="name" style="width:100%"></label><br><br>
<label>Email<br><input name="email" type="email" style="width:100%"></label><br><br>
<label>Password<br><input name="pw" type="password" style="width:100%"></label><br><br>
<label>City<br><input name="city" style="width:100%"></label><br><br>
<label>Bio<br><textarea name="bio" rows="3" style="width:100%"></textarea></label><br><br>
<button type="submit">Submit</button>
</form>
<p id="done" hidden style="color:green">Form submitted.</p>
</body></html>"""
