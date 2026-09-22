"""Run all regression tests from the regression module."""

from taboom_lab.regression import (
    test_velocity_peak_not_at_start,
    test_fitts_law_holds,
    test_no_constant_dt,
    test_no_zero_delta_moves,
    test_no_single_axis_noise,
    test_no_loopy_paths,
    test_overshoot_then_correct,
    test_no_exact_endpoint_every_time,
    test_one_continuous_motion,
    test_moves_vary,
    test_idle_hand_rests_and_stays_near,
    test_no_integer_staircase,
    test_click_hold_nonzero,
    test_no_fixed_start,
    test_typing_bigram_variation,
    test_typing_rhythm_varies_and_rolls_over,
    test_scroll_has_bursts,
)


class TestRegressionSuite:
    def test_velocity_peak(self):
        test_velocity_peak_not_at_start()

    def test_fitts(self):
        test_fitts_law_holds()

    def test_constant_dt(self):
        test_no_constant_dt()

    def test_zero_delta(self):
        test_no_zero_delta_moves()

    def test_single_axis_noise(self):
        test_no_single_axis_noise()

    def test_loopy_paths(self):
        test_no_loopy_paths()

    def test_overshoot_then_correct(self):
        test_overshoot_then_correct()

    def test_no_exact_endpoint_every_time(self):
        test_no_exact_endpoint_every_time()

    def test_continuous_motion(self):
        test_one_continuous_motion()

    def test_moves_vary(self):
        test_moves_vary()

    def test_idle_hand(self):
        test_idle_hand_rests_and_stays_near()

    def test_integer_staircase(self):
        test_no_integer_staircase()

    def test_click_hold(self):
        test_click_hold_nonzero()

    def test_fixed_start(self):
        test_no_fixed_start()

    def test_bigram_variation(self):
        test_typing_bigram_variation()

    def test_typing_rhythm(self):
        test_typing_rhythm_varies_and_rolls_over()

    def test_scroll_bursts(self):
        test_scroll_has_bursts()
