"""Visualization utilities for traces and detection results."""

from __future__ import annotations

from pathlib import Path

import numpy as np
from numpy.typing import NDArray
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection

from .datasets import MouseTrace


def plot_velocity_profile(trace: MouseTrace, output: str | Path = "velocity_profile.png") -> None:
    pts = np.array(trace.points, dtype=np.float64)
    t = pts[:, 0] / 1_000_000.0
    x, y = pts[:, 1], pts[:, 2]

    dt = np.diff(t)
    dt = np.where(dt > 0, dt, 1e-9)
    ds = np.sqrt(np.diff(x)**2 + np.diff(y)**2)
    velocity = ds / dt

    fig, ax = plt.subplots(figsize=(10, 4))
    t_mid = (t[:-1] + t[1:]) / 2
    ax.plot(t_mid, velocity, color="#2196F3", linewidth=1.2)
    ax.set_xlabel("Time (s)")
    ax.set_ylabel("Velocity (px/s)")
    ax.set_title("Velocity Profile")
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(output, dpi=150)
    plt.close(fig)


def plot_path(trace: MouseTrace, output: str | Path = "mouse_path.png") -> None:
    pts = np.array(trace.points, dtype=np.float64)
    t = pts[:, 0] / 1_000_000.0
    x, y = pts[:, 1], pts[:, 2]

    dt = np.diff(t)
    dt = np.where(dt > 0, dt, 1e-9)
    ds = np.sqrt(np.diff(x)**2 + np.diff(y)**2)
    velocity = ds / dt

    fig, ax = plt.subplots(figsize=(8, 6))

    points = np.array([x, y]).T.reshape(-1, 1, 2)
    segments = np.concatenate([points[:-1], points[1:]], axis=1)

    v_norm = velocity / (velocity.max() + 1e-9)
    colors = plt.cm.viridis(v_norm)

    lc = LineCollection(segments, colors=colors, linewidth=1.5)
    ax.add_collection(lc)

    ax.plot(x[0], y[0], "go", markersize=8, label="Start")
    ax.plot(x[-1], y[-1], "rs", markersize=8, label="End")
    ax.set_xlim(x.min() - 10, x.max() + 10)
    ax.set_ylim(y.min() - 10, y.max() + 10)
    ax.set_xlabel("X (px)")
    ax.set_ylabel("Y (px)")
    ax.set_title("Mouse Path (color = velocity)")
    ax.legend()
    ax.set_aspect("equal")
    fig.tight_layout()
    fig.savefig(output, dpi=150)
    plt.close(fig)


def plot_feature_distributions(
    human_features: dict[str, list[float]],
    bot_features: dict[str, list[float]],
    output: str | Path = "feature_distributions.png",
) -> None:
    feature_names = sorted(set(human_features.keys()) & set(bot_features.keys()))
    n = len(feature_names)
    if n == 0:
        return

    cols = min(n, 4)
    rows = (n + cols - 1) // cols
    fig, axes = plt.subplots(rows, cols, figsize=(4 * cols, 3 * rows))
    if n == 1:
        axes = np.array([axes])
    axes = np.atleast_2d(axes)

    for idx, name in enumerate(feature_names):
        r, c = divmod(idx, cols)
        ax = axes[r, c]
        h_vals = human_features[name]
        b_vals = bot_features[name]
        bins = 20
        ax.hist(h_vals, bins=bins, alpha=0.6, color="#4CAF50", label="Human", density=True)
        ax.hist(b_vals, bins=bins, alpha=0.6, color="#F44336", label="Bot", density=True)
        ax.set_title(name, fontsize=9)
        ax.legend(fontsize=7)

    for idx in range(n, rows * cols):
        r, c = divmod(idx, cols)
        axes[r, c].set_visible(False)

    fig.tight_layout()
    fig.savefig(output, dpi=150)
    plt.close(fig)


def plot_roc_curve(
    y_true: NDArray[np.int_],
    y_scores: NDArray[np.float64],
    output: str | Path = "roc_curve.png",
) -> None:
    from sklearn.metrics import roc_curve as sk_roc_curve, roc_auc_score

    fpr, tpr, _ = sk_roc_curve(y_true, y_scores)
    auc_val = roc_auc_score(y_true, y_scores)

    fig, ax = plt.subplots(figsize=(6, 6))
    ax.plot(fpr, tpr, color="#2196F3", linewidth=2, label=f"AUC = {auc_val:.3f}")
    ax.plot([0, 1], [0, 1], color="#999", linestyle="--", linewidth=1)
    ax.set_xlabel("False Positive Rate")
    ax.set_ylabel("True Positive Rate")
    ax.set_title("ROC Curve")
    ax.legend(loc="lower right")
    ax.set_xlim(-0.02, 1.02)
    ax.set_ylim(-0.02, 1.02)
    ax.set_aspect("equal")
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(output, dpi=150)
    plt.close(fig)
