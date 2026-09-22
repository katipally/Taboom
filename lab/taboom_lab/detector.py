"""Human-vs-bot classifier using RandomForest on extracted features."""

from __future__ import annotations

import numpy as np
from numpy.typing import NDArray
from sklearn.ensemble import RandomForestClassifier
from sklearn.model_selection import cross_val_score, StratifiedKFold
from sklearn.metrics import roc_auc_score, accuracy_score, roc_curve

from .features import extract_all_features, FEATURE_NAMES
from .datasets import MouseTrace, KeystrokeTrace


class Detector:
    def __init__(self, n_estimators: int = 100, random_state: int = 42):
        self._clf = RandomForestClassifier(
            n_estimators=n_estimators,
            max_depth=10,
            min_samples_leaf=5,
            random_state=random_state,
            n_jobs=-1,
        )
        self._fitted = False

    def _traces_to_features(
        self, traces: list[MouseTrace | KeystrokeTrace],
    ) -> NDArray[np.float64]:
        rows = []
        for tr in traces:
            if isinstance(tr, MouseTrace):
                row = extract_all_features(mouse=tr)
            elif isinstance(tr, KeystrokeTrace):
                row = extract_all_features(keyboard=tr)
            else:
                row = extract_all_features()
            rows.append(row)
        return np.array(rows, dtype=np.float64)

    def train(
        self,
        human_traces: list[MouseTrace | KeystrokeTrace],
        bot_traces: list[MouseTrace | KeystrokeTrace],
    ) -> None:
        X_human = self._traces_to_features(human_traces)
        X_bot = self._traces_to_features(bot_traces)
        X = np.vstack([X_human, X_bot])
        y = np.array([0] * len(X_human) + [1] * len(X_bot))
        self._clf.fit(X, y)
        self._fitted = True

    def predict(self, trace: MouseTrace | KeystrokeTrace) -> tuple[str, float]:
        if not self._fitted:
            raise RuntimeError("Detector not trained")
        X = self._traces_to_features([trace])
        proba = self._clf.predict_proba(X)[0]
        label = "bot" if proba[1] > 0.5 else "human"
        confidence = float(max(proba))
        return label, confidence

    def evaluate(
        self,
        human_traces: list[MouseTrace | KeystrokeTrace],
        bot_traces: list[MouseTrace | KeystrokeTrace],
    ) -> dict:
        if not self._fitted:
            raise RuntimeError("Detector not trained")

        X_human = self._traces_to_features(human_traces)
        X_bot = self._traces_to_features(bot_traces)
        X = np.vstack([X_human, X_bot])
        y_true = np.array([0] * len(X_human) + [1] * len(X_bot))

        y_pred = self._clf.predict(X)
        y_scores = self._clf.predict_proba(X)[:, 1]

        importances = self._clf.feature_importances_
        feature_importance = dict(zip(FEATURE_NAMES, importances.tolist()))

        return {
            "accuracy": float(accuracy_score(y_true, y_pred)),
            "auc": float(roc_auc_score(y_true, y_scores)),
            "feature_importance": feature_importance,
            "n_human": len(human_traces),
            "n_bot": len(bot_traces),
        }

    def cross_validate(
        self,
        traces: list[MouseTrace | KeystrokeTrace],
        labels: list[int],
        k: int = 5,
    ) -> dict:
        X = self._traces_to_features(traces)
        y = np.array(labels)
        cv = StratifiedKFold(n_splits=k, shuffle=True, random_state=42)

        acc_scores = cross_val_score(self._clf, X, y, cv=cv, scoring="accuracy")
        auc_scores = cross_val_score(self._clf, X, y, cv=cv, scoring="roc_auc")

        return {
            "accuracy_mean": float(np.mean(acc_scores)),
            "accuracy_std": float(np.std(acc_scores)),
            "auc_mean": float(np.mean(auc_scores)),
            "auc_std": float(np.std(auc_scores)),
            "k": k,
        }
