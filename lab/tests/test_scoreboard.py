import json

from taboom_lab.scoreboard import Scoreboard


def test_scoreboard_round_trip_preserves_gauntlet_runs(tmp_path):
    path = tmp_path / "scoreboard.json"
    run = {"persona": "test", "engine": "chrome", "commit": "abc1234", "pages": []}
    path.write_text(json.dumps({"results": [], "gauntlet_runs": [run]}), encoding="utf-8")

    board = Scoreboard.load(path)
    board.add_result("mouse", 0.8, 0.7, "2026-09-26")
    board.save(path)

    stored = json.loads(path.read_text(encoding="utf-8"))
    assert stored["gauntlet_runs"] == [run]
    assert stored["results"][0]["test_name"] == "mouse"
