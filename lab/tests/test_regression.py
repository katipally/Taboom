"""Run the Rust trace-backed humanizer regression suite."""

from taboom_lab import regression


class TestRegressionSuite:
    def test_rust_export_actions(self):
        regression.test_rust_export_has_all_action_types()

    def test_velocity_peak(self):
        regression.test_velocity_peak_not_at_start()

    def test_fitts(self):
        regression.test_fitts_law_holds()

    def test_constant_dt(self):
        regression.test_no_constant_dt()

    def test_zero_delta(self):
        regression.test_no_zero_delta_moves()

    def test_single_axis_noise(self):
        regression.test_no_single_axis_noise()

    def test_loopy_paths(self):
        regression.test_no_loopy_paths()

    def test_overshoot_then_correct(self):
        regression.test_overshoot_then_correct()

    def test_continuous_motion(self):
        regression.test_one_continuous_motion()

    def test_moves_vary(self):
        regression.test_moves_vary()

    def test_idle_hand(self):
        regression.test_idle_hand_rests_and_stays_near()

    def test_click_hold(self):
        regression.test_click_hold_nonzero()

    def test_fixed_start(self):
        regression.test_no_fixed_start()

    def test_bigram_variation(self):
        regression.test_typing_bigram_variation()

    def test_typing_rhythm(self):
        regression.test_typing_rhythm_varies_and_rolls_over()

    def test_scroll_bursts(self):
        regression.test_scroll_has_bursts()
