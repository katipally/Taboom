"""Tests for feature extraction."""

import math
import numpy as np
import pytest

from taboom_lab.datasets import MouseTrace, KeystrokeTrace
from taboom_lab.features import (
    extract_mouse_features,
    extract_keyboard_features,
    extract_scroll_features,
    extract_all_features,
    ScrollTrace,
    FEATURE_NAMES,
)


def _make_line_trace(n: int = 100, duration_s: float = 1.0) -> MouseTrace:
    trace = MouseTrace()
    for i in range(n):
        t_us = int(i * duration_s / (n - 1) * 1_000_000)
        x = float(i * 5)
        y = float(i * 3)
        trace.points.append((t_us, x, y))
    return trace


def _make_keystroke_trace(n: int = 20) -> KeystrokeTrace:
    trace = KeystrokeTrace()
    t = 0
    for i in range(n):
        key = 30 + (i % 26)
        trace.events.append((t, key, True))
        t += 70_000
        trace.events.append((t, key, False))
        t += 80_000
    return trace


class TestMouseFeatures:
    def test_straight_line_has_high_straightness(self):
        trace = _make_line_trace()
        feat = extract_mouse_features(trace)
        assert feat["straightness"] > 0.99

    def test_move_rate_positive(self):
        trace = _make_line_trace()
        feat = extract_mouse_features(trace)
        assert feat["move_rate"] > 0

    def test_empty_trace_returns_zeros(self):
        trace = MouseTrace()
        feat = extract_mouse_features(trace)
        assert feat["move_rate"] == 0.0
        assert feat["straightness"] == 0.0

    def test_velocity_has_mean_and_std(self):
        trace = _make_line_trace()
        feat = extract_mouse_features(trace)
        assert feat["velocity_mean"] > 0
        assert isinstance(feat["velocity_std"], float)

    def test_path_length_matches_distance(self):
        trace = _make_line_trace(n=50)
        feat = extract_mouse_features(trace)
        expected = math.sqrt((49 * 5)**2 + (49 * 3)**2)
        assert abs(feat["path_length"] - expected) < 1.0


class TestKeyboardFeatures:
    def test_hold_time_positive(self):
        trace = _make_keystroke_trace()
        feat = extract_keyboard_features(trace)
        assert feat["hold_mean_ms"] > 0

    def test_flight_time_positive(self):
        trace = _make_keystroke_trace()
        feat = extract_keyboard_features(trace)
        assert feat["flight_mean_ms"] > 0

    def test_keystroke_count(self):
        trace = _make_keystroke_trace(n=15)
        feat = extract_keyboard_features(trace)
        assert feat["n_keystrokes"] == 15.0

    def test_empty_trace(self):
        trace = KeystrokeTrace()
        feat = extract_keyboard_features(trace)
        assert feat["hold_mean_ms"] == 0.0


class TestScrollFeatures:
    def test_burst_detection(self):
        events = []
        t = 0
        for burst in range(3):
            for _ in range(4):
                events.append((t, 1))
                t += 30_000
            t += 500_000
        trace = ScrollTrace(events=events)
        feat = extract_scroll_features(trace)
        assert feat["burst_count"] >= 2

    def test_empty_scroll(self):
        trace = ScrollTrace(events=[])
        feat = extract_scroll_features(trace)
        assert feat["n_events"] == 0.0


class TestAllFeatures:
    def test_output_length_matches_names(self):
        mouse = _make_line_trace()
        keyboard = _make_keystroke_trace()
        scroll = ScrollTrace(events=[(0, 1), (30_000, 1)])
        arr = extract_all_features(mouse=mouse, keyboard=keyboard, scroll=scroll)
        assert len(arr) == len(FEATURE_NAMES)

    def test_none_inputs(self):
        arr = extract_all_features()
        assert len(arr) == len(FEATURE_NAMES)
        assert np.all(arr == 0.0)
