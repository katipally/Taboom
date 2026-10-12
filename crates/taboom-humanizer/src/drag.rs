use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, mouse, ms_to_us};
use rand::Rng;
use rand::rngs::StdRng;

pub fn plan_drag(
    from: (f64, f64),
    to: (f64, f64),
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let mut plan = ActionPlan::new();
    let mut t_us: u64 = 0;

    let hold_before_ms = lognormal_sample(rng, 120.0, 30.0);
    t_us += ms_to_us(hold_before_ms);

    plan.push(t_us, InputEvent::MouseButton { button: 0, pressed: true });
    plan.push(t_us, InputEvent::Sync);

    let pre_move_ms = lognormal_sample(rng, 80.0, 20.0);
    t_us += ms_to_us(pre_move_ms);

    let mut drag_style = style.clone();
    drag_style.fitts_a_ms *= 1.5;
    drag_style.fitts_b_ms *= 1.3;
    drag_style.overshoot_probability *= 0.3;

    let mut move_plan = mouse::plan_move(from, to, 10.0, &drag_style, rng);
    move_plan.offset(t_us);
    let move_dur = move_plan.duration_us();
    plan.extend(move_plan);

    t_us = move_dur + ms_to_us(50.0);

    let settle_ms = lognormal_sample(rng, 100.0, 25.0);
    t_us += ms_to_us(settle_ms);

    plan.push(t_us, InputEvent::MouseButton { button: 0, pressed: false });
    plan.push(t_us, InputEvent::Sync);

    plan
}

/// A slider drag: primarily horizontal with intentional overshoot, a pause, then correction.
/// Simulates the two-phase motion humans use when solving slider puzzles.
pub fn plan_slider_drag(
    from: (f64, f64),
    to: (f64, f64),
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let mut plan = ActionPlan::new();
    let mut t_us: u64 = 0;

    let hold_before_ms = lognormal_sample(rng, 150.0, 40.0);
    t_us += ms_to_us(hold_before_ms);

    plan.push(t_us, InputEvent::MouseButton { button: 0, pressed: true });
    plan.push(t_us, InputEvent::Sync);

    let pre_move_ms = lognormal_sample(rng, 100.0, 30.0);
    t_us += ms_to_us(pre_move_ms);

    let dx = to.0 - from.0;
    let _dy = to.1 - from.1;
    let overshoot_px = lognormal_sample(rng, 8.0, 4.0) * dx.signum();
    let overshoot_target = (to.0 + overshoot_px, to.1 + rng.gen_range(-2.0..2.0));

    let mut slider_style = style.clone();
    slider_style.fitts_a_ms *= 1.3;
    slider_style.fitts_b_ms *= 1.1;
    slider_style.overshoot_probability = 0.0;
    slider_style.tremor_amplitude_px *= 1.5;

    let target_w = if dx.abs() > 100.0 { 15.0 } else { 10.0 };
    let mut overshoot_plan = mouse::plan_move(from, overshoot_target, target_w, &slider_style, rng);
    overshoot_plan.offset(t_us);
    let overshoot_dur = overshoot_plan.duration_us();
    plan.extend(overshoot_plan);
    t_us = overshoot_dur;

    let pause_ms = lognormal_sample(rng, 120.0, 40.0);
    t_us += ms_to_us(pause_ms);

    let mut correct_style = style.clone();
    correct_style.fitts_a_ms *= 0.6;
    correct_style.fitts_b_ms *= 0.5;
    correct_style.overshoot_probability = 0.0;
    correct_style.tremor_amplitude_px *= 0.5;

    let mut correct_plan = mouse::plan_move(overshoot_target, to, 5.0, &correct_style, rng);
    correct_plan.offset(t_us);
    let correct_dur = correct_plan.duration_us();
    plan.extend(correct_plan);
    t_us = correct_dur;

    let settle_ms = lognormal_sample(rng, 80.0, 20.0);
    t_us += ms_to_us(settle_ms);

    plan.push(t_us, InputEvent::MouseButton { button: 0, pressed: false });
    plan.push(t_us, InputEvent::Sync);

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn drag_has_press_and_release() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_drag((100.0, 100.0), (300.0, 200.0), &style, &mut rng);

        let presses: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: true, .. }))
            .collect();
        let releases: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: false, .. }))
            .collect();

        assert_eq!(presses.len(), 1);
        assert_eq!(releases.len(), 1);
        assert!(releases[0].timestamp_us > presses[0].timestamp_us);
    }

    #[test]
    fn slider_drag_has_press_and_release() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_slider_drag((100.0, 300.0), (400.0, 300.0), &style, &mut rng);

        let presses: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: true, .. }))
            .collect();
        let releases: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: false, .. }))
            .collect();

        assert_eq!(presses.len(), 1);
        assert_eq!(releases.len(), 1);
        assert!(releases[0].timestamp_us > presses[0].timestamp_us);
    }

    #[test]
    fn slider_drag_has_mouse_movement() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_slider_drag((50.0, 200.0), (350.0, 200.0), &style, &mut rng);

        let rel_count = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseRel { .. }))
            .count();
        assert!(rel_count >= 5, "slider drag should have mouse movement events");
    }
}
