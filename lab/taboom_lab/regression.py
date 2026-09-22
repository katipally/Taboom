"""Regression tests for known bot failure modes.

Each test generates synthetic traces using the humanizer's parameters
(represented in Python) and checks one specific behavioral property.
"""

from __future__ import annotations

import math
import numpy as np
from numpy.typing import NDArray

from .features import extract_mouse_features, extract_keyboard_features, extract_scroll_features
from .datasets import MouseTrace, KeystrokeTrace
from .features import ScrollTrace


def _lognormal_progress(u: float, mu: float, sigma: float) -> float:
    """Port of Rust `lognormal_progress`: lognormal CDF, scaled to reach exactly 1 at u = 1."""
    if u <= 0.0:
        return 0.0
    if u >= 1.0:
        return 1.0

    def cdf(v: float) -> float:
        return 0.5 * (1.0 + math.erf((math.log(v) - mu) / (sigma * math.sqrt(2.0))))

    return min(cdf(u) / cdf(1.0), 1.0)


def _lognormal_sample(rng: np.random.Generator, mean: float, stddev: float) -> float:
    variance = stddev * stddev
    mu = math.log(mean * mean / math.sqrt(mean * mean + variance))
    sigma = math.sqrt(math.log(1.0 + variance / (mean * mean)))
    return float(np.exp(mu + sigma * rng.standard_normal()))


LANDING_MAX_PX = 3.0


def _landing_point(rng: np.random.Generator, aim: tuple[float, float]) -> tuple[float, float]:
    """Port of Rust `landing_point`: the hand lands within a few px of the aimed point."""
    dx, dy = rng.standard_normal(), rng.standard_normal()
    r = math.hypot(dx, dy)
    k = LANDING_MAX_PX / r if r > LANDING_MAX_PX else 1.0
    return aim[0] + dx * k, aim[1] + dy * k


def _generate_mouse_trace(
    rng: np.random.Generator,
    from_pt: tuple[float, float] = (100.0, 100.0),
    to_pt: tuple[float, float] = (500.0, 400.0),
    target_size: float = 20.0,
    fitts_a: float = 200.0,
    fitts_b: float = 150.0,
    tremor_freq: float = 10.0,
    tremor_amp: float = 0.5,
    overshoot_prob: float = 0.15,
    overshoot_frac: float = 0.08,
) -> MouseTrace:
    """Port of Rust `plan_move`: a primary stroke that lands a little long or short, plus a
    corrective stroke starting before the primary stops; curved, wobbly path; tremor that fades
    at the landing; ~125 Hz polling with jitter. Points are unquantized positions and the last
    one is exactly `to_pt`."""
    dx = to_pt[0] - from_pt[0]
    dy = to_pt[1] - from_pt[1]
    distance = math.hypot(dx, dy)
    if distance < 0.5:
        return MouseTrace()
    ux, uy = dx / distance, dy / distance
    px, py = -uy, ux

    idx = math.log2(distance / max(target_size, 1.0) + 1.0)
    duration = min(max((fitts_a + fitts_b * idx) * _lognormal_sample(rng, 1.0, 0.12), 120.0), 2500.0)

    along = across = 0.0
    if distance > 40.0 and rng.random() < overshoot_prob:
        along = min(distance * overshoot_frac * rng.uniform(0.4, 1.2), 45.0)
        across = rng.uniform(-0.3, 0.3) * along
    elif distance > 25.0 and rng.random() < 0.35:
        along = -min(distance * rng.uniform(0.02, 0.07), 30.0)
        across = rng.uniform(-0.4, 0.4) * abs(along)
    aim = (dx + ux * along + px * across, dy + uy * along + py * across)

    p_mu, p_sigma = rng.uniform(-0.75, -0.35), rng.uniform(0.28, 0.42)
    c_start = duration * rng.uniform(0.72, 0.9)
    c_dur = (90.0 + math.hypot(along, across) * 4.0) * _lognormal_sample(rng, 1.0, 0.2)
    c_mu, c_sigma = rng.uniform(-0.6, -0.3), rng.uniform(0.3, 0.45)
    end = max(duration, c_start + c_dur)

    bend = rng.uniform(-0.12, 0.12) * min(distance, 600.0)
    wobble = rng.uniform(0.0, 0.025) * min(distance, 600.0)
    wobble_phase = rng.uniform(0, 2 * math.pi)
    tremor = [(tremor_freq * rng.uniform(0.8, 1.25), rng.uniform(0, 2 * math.pi), rng.uniform(0, 2 * math.pi))
              for _ in range(3)]

    def position(t: float) -> tuple[float, float]:
        s1 = _lognormal_progress(t / duration, p_mu, p_sigma)
        s2 = _lognormal_progress((t - c_start) / c_dur, c_mu, c_sigma)
        arc = math.sin(math.pi * s1)
        lateral = bend * arc + wobble * math.sin(2 * math.pi * s1 + wobble_phase) * arc
        settle = math.sqrt(min(max(1.0 - t / end, 0.0), 1.0))
        tx = sum(math.sin(2 * math.pi * f * t / 1000 + phx) for f, phx, _ in tremor)
        ty = sum(math.sin(2 * math.pi * f * t / 1000 + phy) for f, _, phy in tremor)
        amp = tremor_amp / 3.0 * settle
        return (from_pt[0] + aim[0] * s1 + (dx - aim[0]) * s2 + px * lateral + tx * amp,
                from_pt[1] + aim[1] * s1 + (dy - aim[1]) * s2 + py * lateral + ty * amp)

    trace = MouseTrace()
    trace.points.append((0, from_pt[0], from_pt[1]))
    t = rng.uniform(1.0, 8.0)
    while t < end:
        x, y = position(t)
        trace.points.append((int(t * 1000), x, y))
        t += 8.0 * rng.uniform(0.9, 1.1)
    trace.points.append((int(end * 1000), to_pt[0], to_pt[1]))
    return trace


def _generate_idle_trace(
    rng: np.random.Generator,
    seconds: float = 60.0,
    hand_on_keyboard: bool = False,
    step_ms: float = 10.0,
    radius: float = 40.0,
) -> MouseTrace:
    """Port of Rust `IdleMotion`: rests, then slow minimum-jerk glides near the anchor (0, 0)."""
    offset = (0.0, 0.0)
    rest = _lognormal_sample(rng, 2500.0, 1000.0) if hand_on_keyboard else _lognormal_sample(rng, 450.0, 200.0)
    glide = None
    trace = MouseTrace()
    t = 0.0
    while t < seconds * 1000:
        t += step_ms
        if glide is None:
            rest -= step_ms
            if rest > 0:
                continue
            roll = rng.random()
            if hand_on_keyboard:
                reach = rng.uniform(2.0, 8.0)
            elif roll < 0.6:
                reach = rng.uniform(2.0, 12.0)
            elif roll < 0.9:
                reach = rng.uniform(12.0, 30.0)
            else:
                reach = rng.uniform(30.0, radius * 1.5)
            angle = rng.uniform(0, 2 * math.pi)
            to = (offset[0] + reach * math.cos(angle), offset[1] + reach * math.sin(angle))
            r = math.hypot(*to)
            if r > radius:
                k = radius * rng.uniform(0.3, 0.8) / r
                to = (to[0] * k, to[1] * k)
            dist = math.hypot(to[0] - offset[0], to[1] - offset[1])
            glide = [offset, to, rng.uniform(-0.2, 0.2) * dist, 0.0,
                     (250.0 + dist * 18.0) * _lognormal_sample(rng, 1.0, 0.25)]
        frm, to, bend, elapsed, dur = glide
        elapsed = min(elapsed + step_ms, dur)
        glide[3] = elapsed
        u = elapsed / dur
        s = u ** 3 * (10 - 15 * u + 6 * u * u)
        vx, vy = to[0] - frm[0], to[1] - frm[1]
        length = max(math.hypot(vx, vy), 1e-9)
        lateral = bend * math.sin(math.pi * s)
        offset = (frm[0] + vx * s - vy / length * lateral, frm[1] + vy * s + vx / length * lateral)
        trace.points.append((int(t * 1000), offset[0], offset[1]))
        if u >= 1.0:
            glide = None
            rest = _lognormal_sample(rng, 3500.0, 1500.0) if hand_on_keyboard else _lognormal_sample(rng, 650.0, 400.0)
    return trace


def _generate_bot_trace(
    rng: np.random.Generator,
    from_pt: tuple[float, float] = (100.0, 100.0),
    to_pt: tuple[float, float] = (500.0, 400.0),
    steps: int = 50,
) -> MouseTrace:
    """Generate a naive bot trace: linear interpolation, constant dt."""
    trace = MouseTrace()
    for i in range(steps + 1):
        frac = i / steps
        x = from_pt[0] + (to_pt[0] - from_pt[0]) * frac
        y = from_pt[1] + (to_pt[1] - from_pt[1]) * frac
        t_us = i * 8000
        trace.points.append((t_us, x, y))
    return trace


def _generate_typing_trace(
    rng: np.random.Generator,
    text: str = "the quick brown fox",
    mean_flight_ms: float = 80.0,
    stddev_flight_ms: float = 25.0,
    hold_mean_ms: float = 70.0,
    rollover_rate: float = 0.2,
) -> KeystrokeTrace:
    """Port of Rust `plan_type` (without typos): press-to-press gaps from a per-call tempo that
    drifts across the text, word and punctuation pauses, slower digits and symbols, and hold
    times drawn separately so fast pairs overlap (rollover). Events are time-sorted."""
    COMMON_BIGRAMS = {
        "th", "he", "in", "er", "an", "re", "on", "at", "en", "nd",
        "ti", "es", "or", "te", "of", "ed", "is", "it", "al", "ar",
        "st", "to", "nt", "ng", "se", "ha", "as", "ou", "io", "le",
    }
    base_iki = mean_flight_ms + hold_mean_ms
    tempo = _lognormal_sample(rng, 1.0, 0.15)
    t = _lognormal_sample(rng, 120.0, 50.0)
    prev = " "
    last_hold = 0.0
    released: dict[int, float] = {}
    events: list[tuple[float, int, bool]] = []

    for i, ch in enumerate(text):
        tempo = min(max(0.92 * tempo + 0.08 * _lognormal_sample(rng, 1.0, 0.25), 0.6), 1.8)
        bigram = (prev + ch).lower()
        factor = 0.7 if bigram in COMMON_BIGRAMS else (1.2 if " " in (prev, ch) else 1.0)
        gap = base_iki * factor * tempo * _lognormal_sample(rng, 1.0, 0.22)
        if prev == " ":
            gap += _lognormal_sample(rng, 60.0, 30.0)
            if rng.random() < 0.035:
                gap += _lognormal_sample(rng, 700.0, 300.0)
        if prev in ".,!?;:\n":
            gap += _lognormal_sample(rng, 220.0, 110.0)
        if ch.isdigit() or (not ch.isalnum() and ch != " "):
            gap *= rng.uniform(1.2, 1.6)
        if prev.islower() and ch.islower() and prev != ch and rng.random() < rollover_rate:
            gap = min(gap, last_hold * rng.uniform(0.45, 0.85))
        if i > 0:
            t += gap
        keycode = ord(ch)
        down = max(t, released.get(keycode, -1e9) + 8.0)
        last_hold = min(max(_lognormal_sample(rng, hold_mean_ms, 18.0), 25.0), 250.0) * (1.15 if ch == " " else 1.0)
        events.append((down, keycode, True))
        events.append((down + last_hold, keycode, False))
        released[keycode] = down + last_hold
        prev = ch

    events.sort(key=lambda e: (e[0], not e[2]))
    trace = KeystrokeTrace()
    trace.events.extend((int(e[0] * 1000), e[1], e[2]) for e in events)
    return trace


def _generate_scroll_trace(
    rng: np.random.Generator,
    total_notches: int = 20,
    burst_min: int = 2,
    burst_max: int = 5,
    pause_mean_ms: float = 300.0,
) -> ScrollTrace:
    """Generate synthetic scroll trace matching the Rust humanizer."""
    events: list[tuple[int, int]] = []
    t_us = 0
    remaining = total_notches

    while remaining > 0:
        burst = min(rng.integers(burst_min, burst_max + 1), remaining)
        for _ in range(burst):
            events.append((t_us, 1))
            inter_ms = _lognormal_sample(rng, 30.0, 8.0)
            t_us += int(inter_ms * 1000)
        remaining -= burst
        if remaining > 0:
            pause_ms = _lognormal_sample(rng, pause_mean_ms, 80.0)
            t_us += int(pause_ms * 1000)

    return ScrollTrace(events=events)


def test_velocity_peak_not_at_start() -> None:
    """Peak velocity should be 30-60% through the movement, not at t=0."""
    rng = np.random.default_rng(42)
    for _ in range(20):
        to_x = rng.uniform(200, 1000)
        to_y = rng.uniform(200, 800)
        trace = _generate_mouse_trace(rng, to_pt=(to_x, to_y), overshoot_prob=0.0)
        feat = extract_mouse_features(trace)
        pos = feat["velocity_peak_position"]
        assert 0.15 <= pos <= 0.75, f"velocity peak at {pos:.2f}, expected 0.15-0.75"


def test_fitts_law_holds() -> None:
    """Movement time should correlate with log2(D/W + 1)."""
    rng = np.random.default_rng(42)
    distances = [50, 100, 200, 400, 800]
    target_w = 20.0
    ids = []
    mts = []

    for d in distances:
        for _ in range(5):
            trace = _generate_mouse_trace(rng, from_pt=(0, 0), to_pt=(d, 0), target_size=target_w)
            feat = extract_mouse_features(trace)
            ids.append(math.log2(d / target_w + 1))
            mts.append(feat["duration"])

    ids_arr = np.array(ids)
    mts_arr = np.array(mts)
    correlation = np.corrcoef(ids_arr, mts_arr)[0, 1]
    assert correlation > 0.8, f"Fitts correlation {correlation:.3f}, expected > 0.8"


def test_no_constant_dt() -> None:
    """Inter-event intervals should vary, not be constant."""
    rng = np.random.default_rng(42)
    trace = _generate_mouse_trace(rng, to_pt=(600.0, 400.0))
    feat = extract_mouse_features(trace)
    assert feat["dt_variance"] > 0, "dt variance is zero (constant intervals)"


def test_no_zero_delta_moves() -> None:
    """No (0,0) relative moves in the trace."""
    rng = np.random.default_rng(42)
    trace = _generate_mouse_trace(rng, to_pt=(300.0, 200.0))
    for i in range(1, len(trace)):
        dx = trace.points[i][1] - trace.points[i - 1][1]
        dy = trace.points[i][2] - trace.points[i - 1][2]
        has_movement = abs(dx) > 0.001 or abs(dy) > 0.001
        assert has_movement, f"zero-delta move at index {i}"


def test_no_single_axis_noise() -> None:
    """Noise (tremor) should appear on both X and Y axes."""
    rng = np.random.default_rng(42)
    trace = _generate_mouse_trace(rng, to_pt=(400.0, 0.1), tremor_amp=1.0)
    pts = np.array(trace.points)
    if len(pts) < 4:
        return
    y_vals = pts[:, 2]
    y_range = float(np.max(y_vals) - np.min(y_vals))
    assert y_range > 0.1, f"Y axis range {y_range}, no perpendicular noise"


def test_no_loopy_paths() -> None:
    """Path straightness ratio should be > 0.85 for direct movements."""
    rng = np.random.default_rng(42)
    for _ in range(20):
        to_x = rng.uniform(200, 800)
        to_y = rng.uniform(200, 800)
        trace = _generate_mouse_trace(rng, to_pt=(to_x, to_y), overshoot_prob=0.0)
        feat = extract_mouse_features(trace)
        assert feat["straightness"] > 0.85, f"straightness {feat['straightness']:.3f}"


def test_overshoot_then_correct() -> None:
    """Some moves carry past the target and come back; every move still lands exactly."""
    rng = np.random.default_rng(42)
    target = (500.0, 400.0)
    start = (100.0, 100.0)
    ux, uy = (target[0] - start[0]) / 500.0, (target[1] - start[1]) / 500.0
    overshot = 0
    for _ in range(50):
        trace = _generate_mouse_trace(rng, from_pt=start, to_pt=target, overshoot_prob=0.5)
        final = trace.points[-1]
        assert (final[1], final[2]) == target, f"landed at {final[1:]}, not on target"
        furthest = max((p[1] - start[0]) * ux + (p[2] - start[1]) * uy for p in trace.points)
        if furthest > 500.0 + 2.0:
            overshot += 1
    assert overshot > 0, "no overshoot observed in any trace"


def test_no_exact_endpoint_every_time() -> None:
    """Aiming at the same point repeatedly lands in slightly different places, never far off."""
    rng = np.random.default_rng(11)
    aim = (500.0, 400.0)
    finals = []
    for _ in range(200):
        land = _landing_point(rng, aim)
        trace = _generate_mouse_trace(rng, to_pt=land)
        finals.append((round(trace.points[-1][1]), round(trace.points[-1][2])))
    assert len(set(finals)) > 5, f"only {len(set(finals))} distinct landing pixels"
    assert all(math.hypot(x - aim[0], y - aim[1]) <= LANDING_MAX_PX + 1 for x, y in finals)


def test_one_continuous_motion() -> None:
    """No stalls mid-move and no teleports: every report is a small step within ~8 ms."""
    rng = np.random.default_rng(7)
    for _ in range(50):
        trace = _generate_mouse_trace(rng, to_pt=(rng.uniform(300, 1500), rng.uniform(200, 900)))
        pts = trace.points
        for a, b in zip(pts, pts[1:]):
            assert b[0] - a[0] < 60_000, f"stalled {b[0] - a[0]} us"
            assert math.hypot(b[1] - a[1], b[2] - a[2]) < 60.0, "teleport-sized step"


def test_moves_vary() -> None:
    """The same move twice never has the same duration or path."""
    rng = np.random.default_rng(3)
    durations = {_generate_mouse_trace(rng).points[-1][0] // 1000 for _ in range(20)}
    assert len(durations) > 10, f"durations barely vary: {sorted(durations)}"


def test_idle_hand_rests_and_stays_near() -> None:
    """Between actions the hand both rests and drifts, and stays within the idle radius."""
    rng = np.random.default_rng(3)
    trace = _generate_idle_trace(rng, seconds=60.0, radius=40.0)
    pts = trace.points
    assert len(pts) > 50, "hand never moved"
    assert all(math.hypot(p[1], p[2]) <= 41.0 for p in pts), "wandered off the anchor"
    gaps = [b[0] - a[0] for a, b in zip(pts, pts[1:])]
    assert max(gaps) > 300_000, "never rested"
    kb = _generate_idle_trace(np.random.default_rng(9), seconds=30.0, hand_on_keyboard=True)
    travelled = sum(math.hypot(b[1] - a[1], b[2] - a[2]) for a, b in zip(kb.points, kb.points[1:]))
    assert travelled < 60.0, f"hand on keyboard moved {travelled:.0f} px"


def test_no_integer_staircase() -> None:
    """Path should not show a staircase pattern (integer-only coordinates)."""
    rng = np.random.default_rng(42)
    trace = _generate_mouse_trace(rng, to_pt=(300.0, 250.0))
    pts = np.array(trace.points)
    x_fractional = pts[:, 1] % 1.0
    y_fractional = pts[:, 2] % 1.0
    has_fractional = np.any(np.abs(x_fractional) > 1e-6) or np.any(np.abs(y_fractional) > 1e-6)
    assert has_fractional, "all coordinates are integers (staircase pattern)"


def test_click_hold_nonzero() -> None:
    """Click hold times should be > 0."""
    rng = np.random.default_rng(42)
    for _ in range(20):
        hold_ms = _lognormal_sample(rng, 90.0, 15.0)
        assert hold_ms > 0, f"click hold time {hold_ms} <= 0"


def test_no_fixed_start() -> None:
    """Different starting positions should produce different traces."""
    rng = np.random.default_rng(42)
    starts = [(0, 0), (100, 200), (500, 300), (800, 600)]
    first_points = []
    for sx, sy in starts:
        trace = _generate_mouse_trace(rng, from_pt=(sx, sy), to_pt=(sx + 200, sy + 150))
        first_points.append(trace.points[0])
    xs = [p[1] for p in first_points]
    assert len(set(xs)) > 1, "all traces start at same X"


def test_typing_bigram_variation() -> None:
    """Common bigrams like 'th' should be typed faster than rare ones like 'zx'."""
    rng = np.random.default_rng(42)
    common_times = []
    rare_times = []
    for _ in range(30):
        common = _generate_typing_trace(rng, text="the")
        rare = _generate_typing_trace(rng, text="zxq")
        common_times.append(common.events[-1][0] - common.events[0][0])
        rare_times.append(rare.events[-1][0] - rare.events[0][0])
    assert np.mean(common_times) < np.mean(rare_times), \
        f"common bigram avg {np.mean(common_times):.0f} >= rare {np.mean(rare_times):.0f}"


def test_typing_rhythm_varies_and_rolls_over() -> None:
    """Speed differs between calls, and fast pairs overlap (two keys down at once)."""
    rng = np.random.default_rng(5)
    text = "the quick brown fox jumps over the lazy dog"
    spans = [(tr := _generate_typing_trace(rng, text)).events[-1][0] - tr.events[0][0] for _ in range(10)]
    assert max(spans) > min(spans) * 1.15, f"same speed every time: {spans}"
    held = peak = 0
    for _, _, down in _generate_typing_trace(rng, text).events:
        held += 1 if down else -1
        peak = max(peak, held)
    assert held == 0 and peak >= 2, "no rollover or a stuck key"


def test_scroll_has_bursts() -> None:
    """Scroll events should come in bursts, not evenly spaced."""
    rng = np.random.default_rng(42)
    trace = _generate_scroll_trace(rng, total_notches=20)
    feat = extract_scroll_features(trace)
    assert feat["burst_count"] >= 1, "no burst gaps detected in scroll trace"
    assert feat["interval_cv"] > 0.3, f"interval CV {feat['interval_cv']:.3f} too low (too regular)"
