import json

from taboom_lab.datasets import KeystrokeTrace, MouseTrace
from taboom_lab.regression import ButtonTrace, load_rust_traces


def test_loader_accepts_legacy_and_jsonl_records_in_the_same_file(tmp_path):
    path = tmp_path / "mixed-trace.txt"
    records = [
        "10.000 pos 4 5",
        "10.008 pos 5 5",
        json.dumps({"timestamp_us": 20_000, "event": "key", "a": 30, "b": 1}),
    ]
    path.write_text("\n".join(records) + "\n", encoding="utf-8")

    traces = load_rust_traces(path)
    move = next(trace for trace in traces if trace.action == "move")
    typing = next(trace for trace in traces if trace.action == "type")

    assert isinstance(move.data, MouseTrace)
    assert move.data.points == [(10_000, 4.0, 5.0), (10_008, 5.0, 5.0)]
    assert isinstance(typing.data, KeystrokeTrace)
    assert typing.data.events == [(20_000, 30, True)]


def test_live_vinput_positions_are_a_stream_not_recovered_actions(tmp_path):
    path = tmp_path / "live-trace.jsonl"
    records = [
        {"timestamp_us": 1_000, "event": "pos", "a": 10, "b": 20},
        {"timestamp_us": 2_000, "event": "btn", "a": 1, "b": 1},
        {"timestamp_us": 2_100, "event": "btn", "a": 1, "b": 0},
        # This could be idle drift or movement. Live records do not mark action boundaries.
        {"timestamp_us": 3_000, "event": "pos", "a": 11, "b": 20},
    ]
    path.write_text("\n".join(json.dumps(record) for record in records) + "\n", encoding="utf-8")

    traces = load_rust_traces(path)
    move = next(trace for trace in traces if trace.action == "move")
    click = next(trace for trace in traces if trace.action == "click")

    assert move.trace_id == "live:move"
    assert isinstance(move.data, MouseTrace)
    assert move.data.points == [(1_000, 10.0, 20.0), (3_000, 11.0, 20.0)]
    assert isinstance(click.data, ButtonTrace)
    assert click.trace_id == "live:click"
