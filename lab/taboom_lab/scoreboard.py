"""Track detection scores over time."""

from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict
from datetime import date
from pathlib import Path


@dataclass
class Result:
    test_name: str
    human_score: float
    taboom_score: float
    date: str


@dataclass
class Scoreboard:
    results: list[Result] = field(default_factory=list)

    def add_result(
        self,
        test_name: str,
        human_score: float,
        taboom_score: float,
        result_date: str | date | None = None,
    ) -> None:
        if result_date is None:
            result_date = date.today().isoformat()
        elif isinstance(result_date, date):
            result_date = result_date.isoformat()
        self.results.append(Result(
            test_name=test_name,
            human_score=human_score,
            taboom_score=taboom_score,
            date=result_date,
        ))

    def report(self) -> str:
        if not self.results:
            return "No results recorded."

        name_w = max(len(r.test_name) for r in self.results)
        name_w = max(name_w, 4)

        header = f"{'Test':<{name_w}}  {'Human':>7}  {'Taboom':>7}  {'Gap':>7}  {'Date':>10}"
        sep = "-" * len(header)
        lines = [header, sep]

        for r in self.results:
            gap = r.taboom_score - r.human_score
            lines.append(
                f"{r.test_name:<{name_w}}  {r.human_score:>7.3f}  "
                f"{r.taboom_score:>7.3f}  {gap:>+7.3f}  {r.date:>10}"
            )

        return "\n".join(lines)

    def save(self, path: str | Path) -> None:
        with open(path, "w") as f:
            json.dump([asdict(r) for r in self.results], f, indent=2)

    @classmethod
    def load(cls, path: str | Path) -> Scoreboard:
        with open(path) as f:
            data = json.load(f)
        board = cls()
        board.results = [Result(**r) for r in data]
        return board
