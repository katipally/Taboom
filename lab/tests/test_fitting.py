import pytest

from taboom_lab.fitting import fit_typing_params


def test_flight_times_and_rollover_use_matching_strokes_for_repeated_keys():
    params = fit_typing_params([
        (0, 30, True), (80_000, 31, True), (100_000, 30, False),
        (160_000, 31, False), (200_000, 30, True), (260_000, 30, False),
    ])
    assert params.hold_mean_ms == pytest.approx(80)
    assert params.bigram_flight_ms["30_31"][0] == -20
    assert params.bigram_flight_ms["31_30"][0] == 40
    assert params.rollover_probability == 0.5


def test_key_repeat_and_incomplete_strokes_do_not_corrupt_hold_times():
    params = fit_typing_params([
        (0, 30, True), (40_000, 30, True), (100_000, 30, False),
        (120_000, 31, True),
    ])
    assert params.hold_mean_ms == 100
    assert params.bigram_flight_ms == {}
