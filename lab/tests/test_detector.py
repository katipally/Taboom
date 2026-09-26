"""Tests for the human-vs-bot detector."""

import numpy as np
import pytest

from taboom_lab.datasets import MouseTrace
from taboom_lab.detector import Detector
from taboom_lab.regression import _generate_bot_trace, load_rust_traces


def _make_dataset(n_each: int = 30):
    rng = np.random.default_rng(42)
    rust_moves = [trace.data for trace in load_rust_traces() if trace.action == "move"]
    if len(rust_moves) < n_each:
        raise AssertionError(f"need {n_each} Rust movement traces; found {len(rust_moves)}")
    humans = rust_moves[:n_each]
    bots = []
    for trace in humans:
        assert isinstance(trace, MouseTrace)
        start = (trace.points[0][1], trace.points[0][2])
        end = (trace.points[-1][1], trace.points[-1][2])
        bots.append(_generate_bot_trace(rng, from_pt=start, to_pt=end))
    return humans, bots


class TestDetector:
    def test_train_and_predict(self):
        humans, bots = _make_dataset(20)
        det = Detector()
        det.train(humans, bots)
        label, conf = det.predict(humans[0])
        assert label in ("human", "bot")
        assert 0.0 <= conf <= 1.0

    def test_bot_detected_as_bot(self):
        humans, bots = _make_dataset(30)
        det = Detector()
        det.train(humans[:20], bots[:20])
        label, conf = det.predict(bots[-1])
        assert label == "bot"

    def test_evaluate_returns_metrics(self):
        humans, bots = _make_dataset(30)
        det = Detector()
        det.train(humans[:20], bots[:20])
        metrics = det.evaluate(humans[20:], bots[20:])
        assert "accuracy" in metrics
        assert "auc" in metrics
        assert "feature_importance" in metrics
        assert metrics["accuracy"] > 0.5
        # CI evaluates a held-out synthetic control: tagged Rust-humanizer movements against
        # the intentionally naive linear, constant-interval bot. This is a regression floor for
        # that pair only, not a claim about an external real-human corpus.
        assert metrics["auc"] >= 0.90

    def test_cross_validate(self):
        humans, bots = _make_dataset(30)
        det = Detector()
        traces = humans + bots
        labels = [0] * len(humans) + [1] * len(bots)
        cv = det.cross_validate(traces, labels, k=3)
        assert "accuracy_mean" in cv
        assert "auc_mean" in cv
        assert cv["accuracy_mean"] > 0.5

    def test_untrained_raises(self):
        det = Detector()
        trace = MouseTrace(points=[(0, 0, 0), (100000, 100, 100)])
        with pytest.raises(RuntimeError):
            det.predict(trace)
