# A11y-CUA shape fixture

## Attribution and source

Derived from the SU (sighted user) desktop interaction logs in the [Reduced A11y-CUA dataset](https://huggingface.co/datasets/berkeley-hci/Reduced-A11y-CUA), provided by UC Berkeley HCI. Dataset authors: Ananya Gubbi Mohanbabu, Rosiana Natalie, Brandon Kim, Anhong Guo, and Amy Pavel, [“A11y-CUA Dataset: Characterizing the Accessibility Gap in Computer Use Agents”](https://arxiv.org/abs/2602.09310). The dataset card lists the source as [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

This fixture is an adapted, feature-only extraction of that source. It preserves the dataset credit and license link. No source event logs, screen content, task IDs, application names, raw cursor coordinates, or timestamps are included here.

## Pinned input and integrity

- Dataset repository: `berkeley-hci/Reduced-A11y-CUA`
- Revision: `26ee1103f74436cb7051543ba65d0c57902c0eb6`
- Input files: all 796 `*.exe.json` event logs under the `SU/` group at that revision
- SHA-256 over sorted source-relative log paths and their individual SHA-256 digests: `a76b5babcd61729821dd28261110a98905b061aa9b7a644b42f2db7a08fa0f93`

`shape_eval.py --human-root` checks this digest before extracting. This catches a changed or incomplete local snapshot rather than labeling it as the pinned revision.

## Extraction protocol

The primary population includes every eligible movement run in each SU app-task event log; it does not filter by click, UI control, or task type.

1. Read only `*.exe.json` logs under `SU/`. Within each log, form maximal consecutive rows whose event type is `mouse_move` and whose timestamp and `cursor_position` are finite. Split a block whenever consecutive movement timestamps differ by more than 2.0 seconds. Use only positions recorded on the movement rows; do not infer a start point from a prior event.
2. Require at least four movement rows in the split segment. Remove consecutive duplicate positions, then require at least four remaining positions and a straight-line start-to-end distance from 50 through 800 pixels, inclusive.
3. Translate the first point to the origin, rotate the chord to the positive x axis, and divide coordinates by the chord length. Resample to 16 equally spaced positions by cumulative geometric path length. Derive the 11 spatial shape features in the fixture. Timestamps, speed, and the source sample count are not classifier features.
4. Assign lexicographically ordered source participant groups pseudonyms `p01`–`p08`. For each group, select at most 64 eligible runs by sorting SHA-256 keys of source-relative log path, first source row index, and temporal segment index. Store only pseudonym, rounded endpoint distance, and rounded shape features.

`click_adjacent_counts` in the fixture metadata is a separate selector diagnostic over direct preceding event rows. It counts a consecutive move block immediately followed by `mouse_up` with `Button.left`, then stages the six allowed target families (`ButtonControl`, `MenuItemControl`, `TreeItemControl`, `ListItemControl`, `HyperlinkControl`, `EditControl`), valid `bounding_rect`, a nonzero start-to-release direction, and a 24–128 px projected effective width. The count of start-to-release distances from 50–800 px is reported but is not a click-selector requirement. It reports intersections with at least three and at least four move rows. This subset is too sparse to serve as the primary comparison.

## Rust comparison protocol

Generate traces with `cargo run -p taboom-humanizer --example trace_export -- lab/traces.jsonl`. For each Rust `move` trace, use its own JSONL timestamps to keep the first point, then each next point at least 200, 250, or 330 ms after the last retained point, and always retain the final endpoint. Human timestamps are never assigned to or used to warp a Rust trace. Apply the same geometric normalization and distance filters to both sources.

For each cadence, pair human and Rust examples one-to-one by minimum absolute endpoint-distance difference, accepting only matches within `max(40 px, 20% of the human distance)`. Train a fixed random forest on human groups outside the held-out participant and Rust seeds outside the held-out `seed % 5` fold; score the held-out participant against the held-out Rust seeds. A fold is reported only with at least 20 training pairs and 10 test pairs. The report includes fold counts and AUC values at all three Rust sampling cadences.

## CI and acceptance status

CI runs this pipeline against the derived fixture and requires all 40 participant/seed folds to meet the documented minimum pair counts. It does not gate on AUC. No P5 human-shape threshold is specified yet, so the reported score is diagnostic evidence rather than a successful real-human acceptance result. The corpus combines task-wide movement runs with the humanizer's simple seeded move plans; the AUC can still reflect unmodeled task, device, capture, or path-shape differences.
