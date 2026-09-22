"""Feature extraction from mouse, keyboard, and scroll traces.

Features from arXiv 2607.26935 and standard HCI metrics.
"""

from __future__ import annotations

import numpy as np
from numpy.typing import NDArray
from scipy import stats as sp_stats
from dataclasses import dataclass

from .datasets import MouseTrace, KeystrokeTrace


@dataclass
class ScrollTrace:
    """Sequence of (timestamp_us, delta) scroll events."""
    events: list[tuple[int, int]]


def extract_mouse_features(trace: MouseTrace) -> dict[str, float]:
    if len(trace) < 3:
        return _empty_mouse_features()

    pts = np.array(trace.points, dtype=np.float64)
    t = pts[:, 0] / 1_000_000.0
    x, y = pts[:, 1], pts[:, 2]

    dt = np.diff(t)
    dt_nonzero = np.where(dt > 0, dt, 1e-9)

    dx = np.diff(x)
    dy = np.diff(y)
    ds = np.sqrt(dx**2 + dy**2)
    velocity = ds / dt_nonzero

    duration = t[-1] - t[0]
    n_events = len(trace)
    move_rate = n_events / duration if duration > 0 else 0.0

    peak_idx = np.argmax(velocity)
    peak_position = peak_idx / max(len(velocity) - 1, 1)

    total_path = float(np.sum(ds))
    direct_dist = np.sqrt((x[-1] - x[0])**2 + (y[-1] - y[0])**2)
    straightness = direct_dist / total_path if total_path > 0 else 1.0

    perp_dev = _perpendicular_deviation(x, y)
    noise_spectrum = _noise_spectrum_energy(perp_dev)

    jerk = _compute_jerk(x, y, t)

    dt_var = float(np.var(dt)) if len(dt) > 1 else 0.0

    return {
        "move_rate": move_rate,
        "velocity_peak_position": peak_position,
        "velocity_mean": float(np.mean(velocity)),
        "velocity_std": float(np.std(velocity)),
        "straightness": straightness,
        "noise_spectrum_energy": noise_spectrum,
        "jerk_mean": jerk,
        "dt_variance": dt_var,
        "duration": duration,
        "path_length": total_path,
    }


def extract_keyboard_features(trace: KeystrokeTrace) -> dict[str, float]:
    if len(trace) < 4:
        return _empty_keyboard_features()

    presses: dict[int, int] = {}
    hold_times: list[float] = []
    flight_times: list[float] = []
    last_release_us: int | None = None

    for t_us, key, is_press in trace.events:
        if is_press:
            presses[key] = t_us
            if last_release_us is not None:
                flight = (t_us - last_release_us) / 1000.0
                if 0 < flight < 2000:
                    flight_times.append(flight)
        else:
            if key in presses:
                hold = (t_us - presses[key]) / 1000.0
                if 0 < hold < 1000:
                    hold_times.append(hold)
                del presses[key]
                last_release_us = t_us

    hold_arr = np.array(hold_times) if hold_times else np.array([0.0])
    flight_arr = np.array(flight_times) if flight_times else np.array([0.0])

    return {
        "hold_mean_ms": float(np.mean(hold_arr)),
        "hold_std_ms": float(np.std(hold_arr)),
        "flight_mean_ms": float(np.mean(flight_arr)),
        "flight_std_ms": float(np.std(flight_arr)),
        "hold_skew": float(sp_stats.skew(hold_arr)) if len(hold_arr) > 2 else 0.0,
        "flight_skew": float(sp_stats.skew(flight_arr)) if len(flight_arr) > 2 else 0.0,
        "n_keystrokes": float(len(hold_times)),
    }


def extract_scroll_features(trace: ScrollTrace) -> dict[str, float]:
    if len(trace.events) < 2:
        return _empty_scroll_features()

    times = np.array([e[0] for e in trace.events], dtype=np.float64)
    deltas = np.array([e[1] for e in trace.events], dtype=np.float64)

    dt = np.diff(times) / 1_000_000.0
    dt_nonzero = np.where(dt > 0, dt, 1e-9)

    burst_gaps = _detect_burst_gaps(dt)

    return {
        "delta_mean": float(np.mean(np.abs(deltas))),
        "delta_std": float(np.std(deltas)),
        "interval_mean_s": float(np.mean(dt)),
        "interval_std_s": float(np.std(dt)),
        "interval_cv": float(np.std(dt) / np.mean(dt_nonzero)) if np.mean(dt_nonzero) > 0 else 0.0,
        "burst_count": float(burst_gaps),
        "n_events": float(len(trace.events)),
    }


def extract_all_features(
    mouse: MouseTrace | None = None,
    keyboard: KeystrokeTrace | None = None,
    scroll: ScrollTrace | None = None,
) -> NDArray[np.float64]:
    parts: list[float] = []

    mf = extract_mouse_features(mouse) if mouse else _empty_mouse_features()
    parts.extend(mf.values())

    kf = extract_keyboard_features(keyboard) if keyboard else _empty_keyboard_features()
    parts.extend(kf.values())

    sf = extract_scroll_features(scroll) if scroll else _empty_scroll_features()
    parts.extend(sf.values())

    return np.array(parts, dtype=np.float64)


def _empty_mouse_features() -> dict[str, float]:
    return {
        "move_rate": 0.0, "velocity_peak_position": 0.0,
        "velocity_mean": 0.0, "velocity_std": 0.0,
        "straightness": 0.0, "noise_spectrum_energy": 0.0,
        "jerk_mean": 0.0, "dt_variance": 0.0,
        "duration": 0.0, "path_length": 0.0,
    }


def _empty_keyboard_features() -> dict[str, float]:
    return {
        "hold_mean_ms": 0.0, "hold_std_ms": 0.0,
        "flight_mean_ms": 0.0, "flight_std_ms": 0.0,
        "hold_skew": 0.0, "flight_skew": 0.0,
        "n_keystrokes": 0.0,
    }


def _empty_scroll_features() -> dict[str, float]:
    return {
        "delta_mean": 0.0, "delta_std": 0.0,
        "interval_mean_s": 0.0, "interval_std_s": 0.0,
        "interval_cv": 0.0, "burst_count": 0.0,
        "n_events": 0.0,
    }


def _perpendicular_deviation(x: NDArray, y: NDArray) -> NDArray:
    """Signed perpendicular distance from each point to the start-end line."""
    x0, y0 = x[0], y[0]
    x1, y1 = x[-1], y[-1]
    dx, dy = x1 - x0, y1 - y0
    length = np.sqrt(dx**2 + dy**2)
    if length < 1e-9:
        return np.zeros(len(x))
    return ((x - x0) * dy - (y - y0) * dx) / length


def _noise_spectrum_energy(deviation: NDArray) -> float:
    """High-frequency energy in the perpendicular deviation signal."""
    if len(deviation) < 8:
        return 0.0
    fft = np.fft.rfft(deviation)
    power = np.abs(fft)**2
    n = len(power)
    high_freq = power[n // 2:]
    total = np.sum(power[1:])
    return float(np.sum(high_freq) / total) if total > 0 else 0.0


def _compute_jerk(x: NDArray, y: NDArray, t: NDArray) -> float:
    """Mean absolute jerk (3rd derivative of position)."""
    if len(x) < 4:
        return 0.0
    dt = np.diff(t)
    dt = np.where(dt > 0, dt, 1e-9)
    vx = np.diff(x) / dt
    vy = np.diff(y) / dt
    if len(vx) < 2:
        return 0.0
    dt2 = dt[:-1]
    ax = np.diff(vx) / dt2
    ay = np.diff(vy) / dt2
    if len(ax) < 2:
        return 0.0
    dt3 = dt2[:-1]
    jx = np.diff(ax) / dt3
    jy = np.diff(ay) / dt3
    jerk_mag = np.sqrt(jx**2 + jy**2)
    return float(np.mean(jerk_mag))


def _detect_burst_gaps(dt: NDArray) -> int:
    """Count pauses that separate scroll bursts (gaps > 5x median interval)."""
    if len(dt) < 2:
        return 0
    median = float(np.median(dt))
    if median <= 0:
        return 0
    threshold = median * 5
    return int(np.sum(dt > threshold))


FEATURE_NAMES = (
    list(_empty_mouse_features().keys())
    + list(_empty_keyboard_features().keys())
    + list(_empty_scroll_features().keys())
)
