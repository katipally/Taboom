"""Tests for the human-vs-bot detector."""

import numpy as np
import pytest

from taboom_lab.datasets import MouseTrace
from taboom_lab.detector import Detector
from taboom_lab.regression import _generate_mouse_trace, _generate_bot_trace


def _make_dataset(n_each: int = 30):
    rng = np.random.default_rng(42)
    humans = []
    bots = []
    for _ in range(n_each):
        to_x = rng.uniform(200, 800)
        to_y = rng.uniform(200, 600)
        humans.append(_generate_mouse_trace(rng, to_pt=(to_x, to_y)))
        bots.append(_generate_bot_trace(rng, to_pt=(to_x, to_y)))
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
        assert 0.0 <= metrics["auc"] <= 1.0

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
