"""Geometry-only comparison of A11y-CUA sighted-user runs and Rust move traces.

This is an exploratory two-sample evaluation, not a detector-evasion claim. Human timestamps
only define the source's bounded movement groups; they are never used to time-warp Rust points
or enter the shape feature vector. Rust traces are sampled at declared elapsed-time intervals to
approximate the event cadence of the A11y-CUA recorder before both trajectories are normalized
and resampled by arc length.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

import numpy as np
from scipy.optimize import linear_sum_assignment
from sklearn.ensemble import RandomForestClassifier
from sklearn.metrics import roc_auc_score

from .datasets import MouseTrace
from .regression import LoadedTrace, load_rust_traces


DATASET_REPOSITORY = "berkeley-hci/Reduced-A11y-CUA"
DATASET_REVISION = "26ee1103f74436cb7051543ba65d0c57902c0eb6"
DATASET_LICENSE = "CC BY 4.0"
EXPECTED_SU_EVENT_LOGS_SHA256 = "a76b5babcd61729821dd28261110a98905b061aa9b7a644b42f2db7a08fa0f93"
MAX_INTERNAL_GAP_S = 2.0
MIN_MOVE_ROWS = 4
MIN_POINTS = 4
MIN_DISTANCE_PX = 50.0
MAX_DISTANCE_PX = 800.0
SHAPE_SAMPLE_COUNT = 16
FIXTURE_PER_PARTICIPANT = 64
THROTTLE_INTERVALS_MS = (200, 250, 330)
TARGET_FAMILIES = frozenset(
    {
        "ButtonControl",
        "MenuItemControl",
        "TreeItemControl",
        "ListItemControl",
        "HyperlinkControl",
        "EditControl",
    }
)
SHAPE_FEATURES = (
    "path_length_ratio",
    "max_cross_track_ratio",
    "mean_abs_cross_track_ratio",
    "rms_cross_track_ratio",
    "cross_track_variation_ratio",
    "signed_area_ratio",
    "max_overshoot_ratio",
    "start_undershoot_ratio",
    "along_reversal_fraction",
    "mean_abs_turn_radians",
    "total_turn_radians",
)


@dataclass(frozen=True)
class HumanShape:
    participant: str
    distance_px: float
    features: dict[str, float]
    sample_key: str


@dataclass(frozen=True)
class RustShape:
    seed: int
    distance_px: float
    features: dict[str, float]


def _position(row: Any) -> tuple[float, float] | None:
    if not isinstance(row, dict):
        return None
    position = row.get("cursor_position")
    if not isinstance(position, (list, tuple)) or len(position) < 2:
        return None
    try:
        x, y = float(position[0]), float(position[1])
    except (TypeError, ValueError):
        return None
    if not (math.isfinite(x) and math.isfinite(y)):
        return None
    return x, y


def _event(row: Any) -> dict[str, Any] | None:
    if not isinstance(row, dict):
        return None
    event = row.get("event")
    return event if isinstance(event, dict) else None


def _move_point(row: Any) -> tuple[float, tuple[float, float]] | None:
    event = _event(row)
    position = _position(row)
    if event is None or event.get("type") != "mouse_move" or position is None:
        return None
    try:
        timestamp = float(row["timestamp"])
    except (KeyError, TypeError, ValueError):
        return None
    if not math.isfinite(timestamp):
        return None
    return timestamp, position


def _dedupe_consecutive(points: Iterable[tuple[float, float]]) -> list[tuple[float, float]]:
    cleaned: list[tuple[float, float]] = []
    for point in points:
        xy = (float(point[0]), float(point[1]))
        if not cleaned or xy != cleaned[-1]:
            cleaned.append(xy)
    return cleaned


def _resample_shape(points: list[tuple[float, float]]) -> tuple[float, np.ndarray] | None:
    """Return chord length and fixed-size, translation/rotation/scale-normalized geometry."""
    cleaned = _dedupe_consecutive(points)
    if len(cleaned) < MIN_POINTS:
        return None
    xy = np.asarray(cleaned, dtype=np.float64)
    chord = xy[-1] - xy[0]
    distance = float(np.hypot(chord[0], chord[1]))
    if not math.isfinite(distance) or distance < MIN_DISTANCE_PX or distance > MAX_DISTANCE_PX:
        return None

    unit = chord / distance
    normal = np.asarray((-unit[1], unit[0]))
    relative = xy - xy[0]
    normalized = np.column_stack((relative @ unit / distance, relative @ normal / distance))

    steps = np.diff(xy, axis=0)
    step_lengths = np.hypot(steps[:, 0], steps[:, 1])
    cumulative = np.concatenate(([0.0], np.cumsum(step_lengths)))
    total_length = float(cumulative[-1])
    if not math.isfinite(total_length) or total_length <= 0.0:
        return None
    # Parameterize by geometric path length, not elapsed time or event index.
    arc_fraction = cumulative / total_length
    keep = np.concatenate(([True], np.diff(arc_fraction) > 1e-12))
    arc_fraction = arc_fraction[keep]
    normalized = normalized[keep]
    samples_at = np.linspace(0.0, 1.0, SHAPE_SAMPLE_COUNT)
    sampled = np.column_stack(
        (
            np.interp(samples_at, arc_fraction, normalized[:, 0]),
            np.interp(samples_at, arc_fraction, normalized[:, 1]),
        )
    )
    return distance, sampled


def shape_features(points: Iterable[tuple[float, float]]) -> tuple[float, dict[str, float]] | None:
    """Compute spatial shape features only; timestamps and source point count are discarded."""
    materialized = list(points)
    resampled = _resample_shape(materialized)
    if resampled is None:
        return None
    distance, path = resampled
    x, y = path[:, 0], path[:, 1]
    delta = np.diff(path, axis=0)
    lengths = np.hypot(delta[:, 0], delta[:, 1])
    sampled_path_length = float(np.sum(lengths))
    headings = np.arctan2(delta[:, 1], delta[:, 0])
    turns = np.arctan2(
        np.sin(np.diff(headings)),
        np.cos(np.diff(headings)),
    )
    along = delta[:, 0]
    signed_area = float(np.sum((y[:-1] + y[1:]) * 0.5 * delta[:, 0]))
    features = {
        "path_length_ratio": sampled_path_length,
        "max_cross_track_ratio": float(np.max(np.abs(y))),
        "mean_abs_cross_track_ratio": float(np.mean(np.abs(y))),
        "rms_cross_track_ratio": float(np.sqrt(np.mean(y * y))),
        "cross_track_variation_ratio": float(np.sum(np.abs(np.diff(y)))),
        "signed_area_ratio": signed_area,
        "max_overshoot_ratio": max(0.0, float(np.max(x)) - 1.0),
        "start_undershoot_ratio": max(0.0, -float(np.min(x))),
        "along_reversal_fraction": float(np.mean(along < -1e-5)),
        "mean_abs_turn_radians": float(np.mean(np.abs(turns))) if len(turns) else 0.0,
        "total_turn_radians": float(np.sum(np.abs(turns))),
    }
    if not all(math.isfinite(value) for value in features.values()):
        return None
    return distance, features


def _relative_file_digest(paths: list[Path], root: Path) -> str:
    digest = hashlib.sha256()
    for path in paths:
        relative = path.relative_to(root).as_posix()
        contents = path.read_bytes()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(hashlib.sha256(contents).digest())
        digest.update(b"\n")
    return digest.hexdigest()


def _participant_groups(names: list[str]) -> dict[str, str]:
    return {name: f"p{index + 1:02d}" for index, name in enumerate(sorted(names))}


def _flush_movement_run(
    run: list[tuple[float, tuple[float, float], int]],
    relative_path: str,
    participant: str,
    selected: list[HumanShape],
    click_counts: Counter[str],
    click_row: dict[str, Any] | None,
) -> None:
    if not run:
        return

    # A maximal consecutive mouse_move block is split only at a >2s recorded gap. Segments use
    # only their logged mouse_move positions; no prior non-move event is treated as a path point.
    segments: list[tuple[int, list[tuple[float, tuple[float, float], int]]]] = []
    current: list[tuple[float, tuple[float, float], int]] = []
    previous_time: float | None = None
    for item in run:
        timestamp, _, _ = item
        if current and previous_time is not None and timestamp - previous_time > MAX_INTERNAL_GAP_S:
            segments.append((current[0][2], current))
            current = []
        current.append(item)
        previous_time = timestamp
    if current:
        segments.append((current[0][2], current))

    for segment_index, (row_start, segment) in enumerate(segments):
        if len(segment) < MIN_MOVE_ROWS:
            continue
        points = [item[1] for item in segment]
        cleaned = _dedupe_consecutive(points)
        if len(cleaned) < MIN_POINTS:
            continue
        shaped = shape_features(cleaned)
        if shaped is None:
            continue
        distance, features = shaped
        sample_key = hashlib.sha256(
            f"{relative_path}\0{row_start}\0{segment_index}".encode("utf-8")
        ).hexdigest()
        selected.append(
            HumanShape(
                participant=participant,
                distance_px=distance,
                features=features,
                sample_key=sample_key,
            )
        )

    if click_row is not None:
        _count_click_adjacent(run, click_row, click_counts)


def _count_click_adjacent(
    run: list[tuple[float, tuple[float, float], int]],
    click_row: dict[str, Any],
    counts: Counter[str],
) -> None:
    counts["move_block_before_non_move"] += 1
    event = _event(click_row)
    if event is None or event.get("type") != "mouse_up" or event.get("button") != "Button.left":
        return
    counts["left_mouse_up_adjacent"] += 1
    if len(run) >= 3:
        counts["left_mouse_up_with_3_move_rows"] += 1
    if len(run) >= 4:
        counts["left_mouse_up_with_4_move_rows"] += 1
    target = click_row.get("element_under_cursor")
    if not isinstance(target, dict) or target.get("control_type") not in TARGET_FAMILIES:
        return
    counts["allowed_target_family"] += 1
    rect = target.get("bounding_rect")
    if not isinstance(rect, dict):
        return
    try:
        left, top, right, bottom = (float(rect[key]) for key in ("left", "top", "right", "bottom"))
    except (KeyError, TypeError, ValueError):
        return
    if not all(math.isfinite(value) for value in (left, top, right, bottom)) or right <= left or bottom <= top:
        return
    counts["valid_target_rect"] += 1
    position = _position(click_row)
    if position is None:
        return
    points = [item[1] for item in run]
    cleaned = _dedupe_consecutive(points)
    if len(run) >= 3:
        counts["at_least_3_move_rows"] += 1
    if len(run) >= 4:
        counts["at_least_4_move_rows"] += 1
    if len(cleaned) < 2:
        return
    dx, dy = position[0] - cleaned[0][0], position[1] - cleaned[0][1]
    distance = math.hypot(dx, dy)
    if not math.isfinite(distance) or distance <= 0.0:
        return
    if MIN_DISTANCE_PX <= distance <= MAX_DISTANCE_PX:
        counts["distance_50_800"] += 1
    ux, uy = dx / distance, dy / distance
    projected_width = abs(ux) * (right - left) + abs(uy) * (bottom - top)
    if 24.0 <= projected_width <= 128.0:
        counts["effective_width_24_128"] += 1
        if len(run) >= 3:
            counts["strict_eligible_with_3_move_rows"] += 1
        if len(run) >= 4:
            counts["strict_eligible_with_4_move_rows"] += 1


def extract_human_shapes(root: str | Path) -> tuple[list[HumanShape], dict[str, Any]]:
    """Extract all qualifying SU movement-run shape features from a local pinned snapshot."""
    source_root = Path(root).expanduser().resolve()
    if not source_root.is_dir():
        raise FileNotFoundError(f"A11y-CUA SU directory not found: {source_root}")
    files = sorted(source_root.rglob("*.exe.json"))
    if not files:
        raise FileNotFoundError(f"no *.exe.json event logs under {source_root}")

    participant_names = sorted(
        {
            path.relative_to(source_root).parts[0]
            for path in files
            if path.relative_to(source_root).parts
        }
    )
    participants = _participant_groups(participant_names)
    selected: list[HumanShape] = []
    click_counts: Counter[str] = Counter()
    raw_move_rows = 0
    source_event_logs_sha256 = _relative_file_digest(files, source_root)
    if source_event_logs_sha256 != EXPECTED_SU_EVENT_LOGS_SHA256:
        raise ValueError(
            "SU event logs do not match the pinned Reduced-A11y-CUA revision "
            f"{DATASET_REVISION} (got SHA-256 {source_event_logs_sha256})"
        )

    for path in files:
        rel = path.relative_to(source_root).as_posix()
        participant_name = path.relative_to(source_root).parts[0]
        participant = participants[participant_name]
        try:
            rows = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ValueError(f"could not parse source log {rel}: {error}") from error
        if not isinstance(rows, list):
            raise ValueError(f"expected a JSON event list in {rel}")

        run: list[tuple[float, tuple[float, float], int]] = []
        for row_index, row in enumerate(rows):
            point = _move_point(row)
            if point is not None:
                raw_move_rows += 1
                run.append((point[0], point[1], row_index))
                continue
            if run:
                _flush_movement_run(
                    run,
                    rel,
                    participant,
                    selected,
                    click_counts,
                    row if isinstance(row, dict) else None,
                )
                run = []

        if run:
            _flush_movement_run(
                run,
                rel,
                participant,
                selected,
                click_counts,
                None,
            )

    selected.sort(key=lambda sample: (sample.participant, sample.sample_key))
    metadata = {
        "dataset_repository": DATASET_REPOSITORY,
        "source_revision": DATASET_REVISION,
        "license": DATASET_LICENSE,
        "source_file_count": len(files),
        "source_event_logs_sha256": source_event_logs_sha256,
        "raw_mouse_move_rows": raw_move_rows,
        "eligible_shape_runs": len(selected),
        "eligible_by_participant": dict(Counter(sample.participant for sample in selected)),
        "click_adjacent_counts": dict(click_counts),
        "protocol": _protocol_metadata(),
    }
    return selected, metadata


def _protocol_metadata() -> dict[str, Any]:
    return {
        "primary_population": "all SU desktop logs under the pinned SU snapshot",
        "segmentation": (
            "maximal consecutive mouse_move rows within each app-task log; split a segment "
            "when successive mouse_move timestamps differ by more than 2.0 seconds; use only "
            "the recorded mouse_move cursor positions"
        ),
        "minimum_consecutive_mouse_move_rows": MIN_MOVE_ROWS,
        "distance_px_inclusive": [MIN_DISTANCE_PX, MAX_DISTANCE_PX],
        "features": "translation-, rotation-, scale-normalized spatial shape only; no timestamps, event count, or speed",
        "shape_resampling": f"{SHAPE_SAMPLE_COUNT} equally spaced points by cumulative path length",
        "matching": "deterministic one-to-one minimum absolute endpoint-distance assignment; tolerance=max(40 px, 20% of human distance)",
        "human_groups": "leave one pseudonymous SU participant group out",
        "rust_groups": "five deterministic seed folds, seed % 5; evaluate only held-out seeds",
        "rust_capture_emulation_ms": list(THROTTLE_INTERVALS_MS),
        "click_diagnostic_target_families": sorted(TARGET_FAMILIES),
        "click_diagnostic_event": "a consecutive move block directly followed by mouse_up with Button.left; require an allowed target family, valid rect, nonzero movement direction, and 24–128 px projected width",
        "click_diagnostic_width": "project bounding rectangle onto start-to-release direction; keep 24–128 px; report but do not require the primary 50–800 px path-distance filter",
    }


def write_human_fixture(
    samples: list[HumanShape],
    metadata: dict[str, Any],
    output: str | Path,
    per_participant: int = FIXTURE_PER_PARTICIPANT,
) -> None:
    if per_participant <= 0:
        raise ValueError("per-participant fixture count must be positive")
    grouped: dict[str, list[HumanShape]] = {}
    for sample in samples:
        grouped.setdefault(sample.participant, []).append(sample)
    fixture_samples: list[dict[str, Any]] = []
    for participant in sorted(grouped):
        chosen = sorted(grouped[participant], key=lambda sample: sample.sample_key)[:per_participant]
        for index, sample in enumerate(chosen, start=1):
            fixture_samples.append(
                {
                    "participant": participant,
                    "sample": index,
                    "distance_px": round(sample.distance_px, 4),
                    "shape_features": {name: round(sample.features[name], 8) for name in SHAPE_FEATURES},
                }
            )
    fixture = {
        "schema_version": 1,
        "metadata": {
            **metadata,
            "fixture_sampling": f"first {per_participant} eligible runs per participant after sorting by SHA-256 of source-relative log path, source row start, and temporal segment index",
            "fixture_sample_count": len(fixture_samples),
            "raw_paths_or_timestamps_included": False,
        },
        "samples": fixture_samples,
    }
    path = Path(output)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(fixture, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def load_human_fixture(path: str | Path) -> tuple[list[HumanShape], dict[str, Any]]:
    fixture_path = Path(path)
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    if not isinstance(fixture, dict) or fixture.get("schema_version") != 1:
        raise ValueError(f"unsupported human shape fixture schema in {fixture_path}")
    metadata = fixture.get("metadata")
    rows = fixture.get("samples")
    if not isinstance(metadata, dict) or not isinstance(rows, list):
        raise ValueError(f"malformed human shape fixture: {fixture_path}")
    if metadata.get("dataset_repository") != DATASET_REPOSITORY or metadata.get("source_revision") != DATASET_REVISION:
        raise ValueError("fixture dataset/revision does not match the pinned A11y-CUA source")
    if metadata.get("source_event_logs_sha256") != EXPECTED_SU_EVENT_LOGS_SHA256:
        raise ValueError("fixture event-log digest does not match the pinned SU input")
    if metadata.get("license") != DATASET_LICENSE or metadata.get("source_file_count") != 796:
        raise ValueError("fixture attribution or pinned source-file count is inconsistent")
    if metadata.get("protocol") != _protocol_metadata():
        raise ValueError("fixture extraction protocol is stale; regenerate it from the pinned source")
    if metadata.get("raw_paths_or_timestamps_included") is not False:
        raise ValueError("fixture must explicitly attest that raw paths and timestamps are excluded")
    if metadata.get("fixture_sample_count") != len(rows):
        raise ValueError("fixture sample count does not match its metadata")
    samples: list[HumanShape] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"fixture sample {index} is not an object")
        if set(row) != {"participant", "sample", "distance_px", "shape_features"}:
            raise ValueError(f"fixture sample {index} contains unexpected or raw fields")
        participant = row.get("participant")
        distance = row.get("distance_px")
        features = row.get("shape_features")
        if not isinstance(participant, str) or not isinstance(features, dict):
            raise ValueError(f"fixture sample {index} has invalid fields")
        try:
            distance_value = float(distance)
            feature_values = {name: float(features[name]) for name in SHAPE_FEATURES}
        except (KeyError, TypeError, ValueError) as error:
            raise ValueError(f"fixture sample {index} has invalid numeric data") from error
        if not math.isfinite(distance_value) or not all(math.isfinite(value) for value in feature_values.values()):
            raise ValueError(f"fixture sample {index} contains a non-finite value")
        samples.append(
            HumanShape(
                participant=participant,
                distance_px=distance_value,
                features=feature_values,
                sample_key=f"fixture:{participant}:{index:06d}",
            )
        )
    if not samples:
        raise ValueError("human shape fixture contains no samples")
    return samples, metadata


def _thin_rust(points: list[tuple[int, float, float]], interval_ms: int) -> list[tuple[float, float]]:
    if not points:
        return []
    interval_us = interval_ms * 1000
    kept = [points[0]]
    last_kept_time = points[0][0]
    for point in points[1:]:
        if point[0] - last_kept_time >= interval_us:
            kept.append(point)
            last_kept_time = point[0]
    if points[-1][0] > kept[-1][0] or (points[-1][1], points[-1][2]) != (kept[-1][1], kept[-1][2]):
        kept.append(points[-1])
    return _dedupe_consecutive((point[1], point[2]) for point in kept)


def _rust_shapes(rust_traces: list[LoadedTrace], interval_ms: int) -> list[RustShape]:
    samples: list[RustShape] = []
    for loaded in rust_traces:
        if loaded.action != "move" or loaded.seed is None or not isinstance(loaded.data, MouseTrace):
            continue
        points = loaded.data.points
        thin = _thin_rust(points, interval_ms)
        shaped = shape_features(thin)
        if shaped is None:
            continue
        distance, features = shaped
        samples.append(RustShape(seed=loaded.seed, distance_px=distance, features=features))
    return samples


def _match_by_distance(
    humans: list[HumanShape],
    rust: list[RustShape],
) -> list[tuple[HumanShape, RustShape]]:
    if not humans or not rust:
        return []
    # Hungarian assignment is deterministic for sorted inputs and prevents one Rust trace from
    # appearing in more than one comparison. Distances outside the documented tolerance are
    # excluded after the minimum-cost assignment.
    human_order = sorted(humans, key=lambda sample: (sample.distance_px, sample.sample_key))
    rust_order = sorted(rust, key=lambda sample: (sample.distance_px, sample.seed))
    hd = np.asarray([sample.distance_px for sample in human_order], dtype=np.float64)
    rd = np.asarray([sample.distance_px for sample in rust_order], dtype=np.float64)
    errors = np.abs(hd[:, None] - rd[None, :])
    tolerance = np.maximum(40.0, 0.20 * hd[:, None])
    costs = errors.copy()
    costs[errors > tolerance] = 1e9
    row_indices, col_indices = linear_sum_assignment(costs)
    pairs = []
    for human_index, rust_index in zip(row_indices.tolist(), col_indices.tolist()):
        if costs[human_index, rust_index] >= 1e9:
            continue
        pairs.append((human_order[human_index], rust_order[rust_index]))
    return pairs


def _feature_row(features: dict[str, float]) -> list[float]:
    return [features[name] for name in SHAPE_FEATURES]


def evaluate_shapes(
    humans: list[HumanShape],
    rust_traces: list[LoadedTrace],
    throttle_intervals_ms: Iterable[int] = THROTTLE_INTERVALS_MS,
) -> dict[str, Any]:
    groups = sorted({sample.participant for sample in humans})
    if len(groups) < 2:
        raise ValueError("shape evaluation requires at least two participant groups")
    if not rust_traces:
        raise ValueError("shape evaluation requires Rust trace JSONL")

    results: dict[str, Any] = {}
    for interval_ms in throttle_intervals_ms:
        if interval_ms <= 0:
            raise ValueError("Rust throttle intervals must be positive")
        rust = _rust_shapes(rust_traces, interval_ms)
        if not rust:
            raise ValueError(f"no eligible Rust move traces at {interval_ms} ms sampling")
        folds: list[dict[str, Any]] = []
        for participant in groups:
            held_humans = [sample for sample in humans if sample.participant == participant]
            train_humans = [sample for sample in humans if sample.participant != participant]
            for seed_fold in range(5):
                test_rust = [sample for sample in rust if sample.seed % 5 == seed_fold]
                train_rust = [sample for sample in rust if sample.seed % 5 != seed_fold]
                train_pairs = _match_by_distance(train_humans, train_rust)
                test_pairs = _match_by_distance(held_humans, test_rust)
                if len(train_pairs) < 20 or len(test_pairs) < 10:
                    continue
                train_x: list[list[float]] = []
                train_y: list[int] = []
                test_x: list[list[float]] = []
                test_y: list[int] = []
                for human, generated in train_pairs:
                    train_x.extend((_feature_row(human.features), _feature_row(generated.features)))
                    train_y.extend((0, 1))
                for human, generated in test_pairs:
                    test_x.extend((_feature_row(human.features), _feature_row(generated.features)))
                    test_y.extend((0, 1))
                model = RandomForestClassifier(
                    n_estimators=200,
                    max_depth=8,
                    min_samples_leaf=5,
                    class_weight="balanced",
                    random_state=42,
                    n_jobs=1,
                ).fit(np.asarray(train_x), np.asarray(train_y))
                probabilities = model.predict_proba(np.asarray(test_x))[:, 1]
                folds.append(
                    {
                        "participant": participant,
                        "rust_seed_fold": seed_fold,
                        "n_train_pairs": len(train_pairs),
                        "n_test_pairs": len(test_pairs),
                        "auc": float(roc_auc_score(test_y, probabilities)),
                    }
                )
        expected_folds = len(groups) * 5
        if len(folds) != expected_folds:
            raise ValueError(
                f"incomplete participant/seed folds at {interval_ms} ms: "
                f"{len(folds)}/{expected_folds}; each fold needs >=20 train and >=10 test matched pairs"
            )
        values = np.asarray([fold["auc"] for fold in folds], dtype=np.float64)
        results[str(interval_ms)] = {
            "human_trace_count": len(humans),
            "rust_trace_count": len(rust),
            "fold_count": len(folds),
            "mean_auc": float(np.mean(values)),
            "std_auc_across_folds": float(np.std(values)),
            "folds": folds,
        }
    return results


def _round_report(value: Any, digits: int = 4) -> Any:
    if isinstance(value, float):
        return round(value, digits)
    if isinstance(value, dict):
        return {key: _round_report(item, digits) for key, item in value.items()}
    if isinstance(value, list):
        return [_round_report(item, digits) for item in value]
    return value


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--human-root", help="local directory containing the pinned SU/**/*.exe.json logs")
    source.add_argument("--human-fixture", help="derived feature fixture; contains no raw paths/timestamps")
    parser.add_argument("--rust-traces", required=True, help="Rust exporter JSONL from trace_export")
    parser.add_argument("--write-fixture", help="write a sanitized, deterministic feature fixture from --human-root")
    parser.add_argument("--max-mean-auc", type=float, default=None, help="fail when any cadence's mean AUC exceeds this; CI ratchets it down over time")
    args = parser.parse_args(argv)

    if args.write_fixture and not args.human_root:
        parser.error("--write-fixture requires --human-root")

    if args.human_root:
        humans, metadata = extract_human_shapes(args.human_root)
        if args.write_fixture:
            write_human_fixture(humans, metadata, args.write_fixture)
        # Raw-source evaluation intentionally uses the same bounded, per-participant sample
        # as the checked-in CI fixture.
        fixture_like = []
        for participant in sorted({sample.participant for sample in humans}):
            fixture_like.extend(
                sorted(
                    (sample for sample in humans if sample.participant == participant),
                    key=lambda sample: sample.sample_key,
                )[:FIXTURE_PER_PARTICIPANT]
            )
        humans = fixture_like
    else:
        humans, metadata = load_human_fixture(args.human_fixture)

    rust_traces = load_rust_traces(args.rust_traces)
    report = evaluate_shapes(humans, rust_traces)
    if args.max_mean_auc is not None:
        if not 0.0 <= args.max_mean_auc <= 1.0:
            parser.error("--max-mean-auc must be between 0 and 1")
        failed = {
            interval: result["mean_auc"]
            for interval, result in report.items()
            if result["mean_auc"] > args.max_mean_auc
        }
        if failed:
            print(json.dumps(_round_report({"failed_thresholds": failed, "results": report}), indent=2))
            return 1

    result = {
        "status": "diagnostic_threshold_unset" if args.max_mean_auc is None else "threshold_passed",
        "acceptance_threshold": args.max_mean_auc,
        "source_revision": metadata.get("source_revision"),
        "dataset_repository": metadata.get("dataset_repository"),
        "license": metadata.get("license"),
        "source_event_logs_sha256": metadata.get("source_event_logs_sha256"),
        "human_sample_count": len(humans),
        "human_participant_groups": sorted({sample.participant for sample in humans}),
        "click_adjacent_counts": metadata.get("click_adjacent_counts", {}),
        "protocol": _protocol_metadata(),
        "results_by_rust_throttle_ms": report,
    }
    print(json.dumps(_round_report(result), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
