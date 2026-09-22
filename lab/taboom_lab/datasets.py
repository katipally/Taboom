"""Loaders for public mouse/keyboard datasets.

Each loader returns traces in a normalized format. Users must download
datasets separately; these loaders parse the files.

License notices:
  Balabit mouse dynamics  -- CC BY-NC-SA 4.0
  SapiMouse               -- CC BY 4.0
  BOUN mouse dataset      -- for research use (see dataset README)
  Aalto 136M keystrokes   -- CC BY 4.0
"""

from __future__ import annotations

import csv
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator

import pandas as pd


@dataclass
class MouseTrace:
    """Sequence of (timestamp_us, x, y) points."""
    points: list[tuple[int, float, float]] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.points)

    def timestamps(self) -> list[int]:
        return [p[0] for p in self.points]

    def xs(self) -> list[float]:
        return [p[1] for p in self.points]

    def ys(self) -> list[float]:
        return [p[2] for p in self.points]


@dataclass
class KeystrokeTrace:
    """Sequence of (timestamp_us, key_code, is_press) events."""
    events: list[tuple[int, int, bool]] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.events)

    def press_times(self) -> list[int]:
        return [e[0] for e in self.events if e[2]]

    def release_times(self) -> list[int]:
        return [e[0] for e in self.events if not e[2]]


def load_balabit(path: str | Path) -> list[MouseTrace]:
    """Load Balabit mouse dynamics dataset.

    Expects directory with session files. Each file has columns:
    timestamp, button, state, x, y
    """
    path = Path(path)
    traces = []

    for session_file in sorted(path.glob("session_*")):
        trace = MouseTrace()
        with open(session_file) as f:
            reader = csv.DictReader(f)
            for row in reader:
                t_us = int(float(row["timestamp"]) * 1_000_000)
                x = float(row["x"])
                y = float(row["y"])
                trace.points.append((t_us, x, y))
        if trace.points:
            traces.append(trace)

    return traces


def load_sapimouse(path: str | Path) -> list[MouseTrace]:
    """Load SapiMouse dataset.

    Expects CSV files with columns: timestamp, x, y, button, state
    """
    path = Path(path)
    traces = []

    for csv_file in sorted(path.glob("*.csv")):
        trace = MouseTrace()
        df = pd.read_csv(csv_file)
        col_map = _detect_sapimouse_columns(df)

        for _, row in df.iterrows():
            t_us = int(float(row[col_map["t"]]) * 1_000_000)
            x = float(row[col_map["x"]])
            y = float(row[col_map["y"]])
            trace.points.append((t_us, x, y))

        if trace.points:
            traces.append(trace)

    return traces


def _detect_sapimouse_columns(df: pd.DataFrame) -> dict[str, str]:
    cols = [c.lower().strip() for c in df.columns]
    mapping: dict[str, str] = {}
    for c, orig in zip(cols, df.columns):
        if "time" in c or c == "t":
            mapping["t"] = orig
        elif c == "x":
            mapping["x"] = orig
        elif c == "y":
            mapping["y"] = orig

    if len(mapping) < 3:
        mapping.setdefault("t", df.columns[0])
        mapping.setdefault("x", df.columns[1])
        mapping.setdefault("y", df.columns[2])
    return mapping


def load_boun(path: str | Path) -> list[MouseTrace]:
    """Load BOUN mouse dynamics dataset.

    Expects per-user directories each containing session CSVs.
    Columns: client timestamp, button, state, x, y
    """
    path = Path(path)
    traces = []

    for user_dir in sorted(path.iterdir()):
        if not user_dir.is_dir():
            continue
        for session_file in sorted(user_dir.glob("*.csv")):
            trace = MouseTrace()
            try:
                df = pd.read_csv(session_file)
            except Exception:
                continue
            if df.shape[1] < 4:
                continue
            t_col = df.columns[0]
            x_col = df.columns[3] if df.shape[1] >= 5 else df.columns[1]
            y_col = df.columns[4] if df.shape[1] >= 5 else df.columns[2]
            for _, row in df.iterrows():
                t_us = int(float(row[t_col]) * 1_000_000)
                x = float(row[x_col])
                y = float(row[y_col])
                trace.points.append((t_us, x, y))
            if trace.points:
                traces.append(trace)

    return traces


def load_aalto(path: str | Path) -> list[KeystrokeTrace]:
    """Load Aalto 136M keystrokes dataset.

    Expects TSV or CSV with columns: PARTICIPANT_ID, TEST_SECTION_ID,
    PRESS_TIME, RELEASE_TIME, KEYCODE, LETTER
    """
    path = Path(path)
    traces = []
    current_trace = KeystrokeTrace()
    current_section: str | None = None

    if path.is_file():
        files = [path]
    else:
        files = sorted(path.glob("*.{tsv,csv,txt}"))

    for f in files:
        sep = "\t" if f.suffix == ".tsv" else ","
        try:
            df = pd.read_csv(f, sep=sep, low_memory=False)
        except Exception:
            continue

        press_col = _find_col(df, ["PRESS_TIME", "press_time", "pressTime"])
        release_col = _find_col(df, ["RELEASE_TIME", "release_time", "releaseTime"])
        key_col = _find_col(df, ["KEYCODE", "keycode", "key_code"])
        section_col = _find_col(df, ["TEST_SECTION_ID", "section", "test_section_id"])

        if press_col is None or release_col is None:
            continue

        for _, row in df.iterrows():
            section = str(row[section_col]) if section_col else "0"
            if section != current_section:
                if current_trace.events:
                    traces.append(current_trace)
                current_trace = KeystrokeTrace()
                current_section = section

            try:
                press_us = int(float(row[press_col]) * 1000)
                release_us = int(float(row[release_col]) * 1000)
            except (ValueError, TypeError):
                continue

            keycode = int(row[key_col]) if key_col and pd.notna(row[key_col]) else 0
            current_trace.events.append((press_us, keycode, True))
            current_trace.events.append((release_us, keycode, False))

        if current_trace.events:
            current_trace.events.sort(key=lambda e: e[0])

    if current_trace.events:
        traces.append(current_trace)

    return traces


def _find_col(df: pd.DataFrame, candidates: list[str]) -> str | None:
    for c in candidates:
        if c in df.columns:
            return c
    return None
