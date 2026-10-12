"""Build the runtime's normalized mouse-path library from pinned human event logs.

Only p01-p06 contribute calibration paths. p07-p08 remain reserved for evaluation.
Source screen content, coordinates, timestamps and identifiers are not exported.
Timing remains heuristic because these logs capture movement at a coarse cadence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from collections import Counter, defaultdict
from pathlib import Path

import numpy as np

from .shape_eval import (
    DATASET_LICENSE, DATASET_REPOSITORY, DATASET_REVISION,
    EXPECTED_SU_EVENT_LOGS_SHA256, MAX_INTERNAL_GAP_S, MIN_MOVE_ROWS,
    _move_point, _participant_groups, _resample_shape, extract_human_shapes,
)

TRAIN_GROUPS = tuple(f"p{i:02d}" for i in range(1, 7))
EVALUATION_GROUPS = ("p07", "p08")


def build_calibration(root: str | Path) -> dict:
    root = Path(root).resolve()
    # Verify the complete pinned snapshot before using any of it.
    _, metadata = extract_human_shapes(root)
    files = sorted(root.rglob("*.exe.json"))
    groups = _participant_groups(sorted({p.relative_to(root).parts[0] for p in files}))
    candidates = defaultdict(list)
    rejected = Counter()

    def flush(run, relative, group):
        if len(run) < MIN_MOVE_ROWS:
            return
        shaped = _resample_shape([p[1] for p in run])
        if shaped is None:
            return
        distance, points = shaped
        # Keep direct aimed movements (path/chord <=1.2), excluding task-wide detours and loops.
        length = float(np.hypot(*np.diff(points, axis=0).T).sum())
        if length > 1.2 or np.any(points[:, 0] < -0.15) or np.any(points[:, 0] > 1.15) or np.any(np.abs(points[:, 1]) > 0.5):
            rejected[group] += 1
            return
        points[0], points[-1] = (0, 0), (1, 0)
        key = hashlib.sha256(f"{relative}\0{run[0][2]}".encode()).hexdigest()
        candidates[group].append((key, {
            "distance_px": round(distance, 4),
            "points": np.round(points, 6).tolist(),
        }))

    for path in files:
        relative = path.relative_to(root).as_posix()
        group = groups[path.relative_to(root).parts[0]]
        if group not in TRAIN_GROUPS:
            continue
        run = []
        for index, row in enumerate(json.loads(path.read_text())):
            point = _move_point(row)
            if point is None or (run and not 0 <= point[0] - run[-1][0] <= MAX_INTERNAL_GAP_S):
                flush(run, relative, group)
                run = []
            if point is not None:
                run.append((*point, index))
        flush(run, relative, group)

    paths, counts = [], {}
    for group in TRAIN_GROUPS:
        selected = sorted(candidates[group], key=lambda item: item[0])[:64]
        if len(selected) < 20:
            raise ValueError(f"insufficient calibration paths for {group}")
        counts[group] = len(selected)
        paths.extend(sample for _, sample in selected)
    return {
        "schema_version": 1,
        "provenance": {
            "dataset_repository": DATASET_REPOSITORY,
            "source_revision": DATASET_REVISION,
            "source_event_logs_sha256": EXPECTED_SU_EVENT_LOGS_SHA256,
            "license": DATASET_LICENSE,
            "license_url": "https://creativecommons.org/licenses/by/4.0/",
            "attribution": "Ananya Gubbi Mohanbabu, Rosiana Natalie, Brandon Kim, Anhong Guo, and Amy Pavel; UC Berkeley HCI; A11y-CUA Dataset",
            "source_url": "https://huggingface.co/datasets/berkeley-hci/Reduced-A11y-CUA",
            "paper_url": "https://arxiv.org/abs/2602.09310",
            "training_participant_groups": list(TRAIN_GROUPS),
            "reserved_evaluation_participant_groups": list(EVALUATION_GROUPS),
            "samples_by_training_group": counts,
            "excluded_unbounded_shapes_by_group": dict(rejected),
            "source_file_count": metadata["source_file_count"],
            "timing": "not exported; runtime duration and velocity remain heuristic",
            "selection": "up to 64 bounded shapes per training participant, ordered by hashed source path and segment start",
        },
        "paths": paths,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--human-root", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    artifact = build_calibration(args.human_root)
    Path(args.output).parent.mkdir(parents=True, exist_ok=True)
    # Keep one line per derived shape so changes to this generated artifact are reviewable.
    header = json.dumps({k: v for k, v in artifact.items() if k != "paths"}, indent=2, allow_nan=False)
    rows = ",\n".join("    " + json.dumps(path, separators=(",", ":"), allow_nan=False) for path in artifact["paths"])
    Path(args.output).write_text(header.rsplit("\n}", 1)[0] + ',\n  "paths": [\n' + rows + "\n  ]\n}\n")
    print(json.dumps({"paths": len(artifact["paths"]), "training_groups": TRAIN_GROUPS, "reserved_groups": EVALUATION_GROUPS}))


if __name__ == "__main__":
    main()
