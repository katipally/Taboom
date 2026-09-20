use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::rngs::StdRng;
use rand::Rng;

const HI_RES_PER_NOTCH: i32 = 120;

pub fn plan_scroll(
    direction: ScrollDirection,
    amount: i32,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let mut plan = ActionPlan::new();
    let mut t_us: u64 = 0;
    let mut remaining = amount.abs();
    let sign = if amount >= 0 { 1 } else { -1 };

    while remaining > 0 {
        let burst_size = rng.gen_range(style.scroll_burst_min..=style.scroll_burst_max)
            .min(remaining as u32) as i32;

        for _ in 0..burst_size {
            let (delta, hi_res) = match direction {
                ScrollDirection::Vertical => (0, sign * HI_RES_PER_NOTCH),
                ScrollDirection::Horizontal => (sign * HI_RES_PER_NOTCH, 0),
            };

            let notch_delta = match direction {
                ScrollDirection::Vertical => sign,
                ScrollDirection::Horizontal => sign,
            };

            plan.push(t_us, InputEvent::Wheel {
                delta: notch_delta,
                hi_res_delta: match direction {
                    ScrollDirection::Vertical => hi_res,
                    ScrollDirection::Horizontal => delta,
                },
            });
            plan.push(t_us, InputEvent::Sync);

            let inter_notch_ms = lognormal_sample(rng, 30.0, 8.0);
            t_us += ms_to_us(inter_notch_ms);
        }

        remaining -= burst_size;

        if remaining > 0 {
            let pause_ms = lognormal_sample(rng, style.scroll_pause_mean_ms, 80.0);
            t_us += ms_to_us(pause_ms);
        }
    }

    plan
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Vertical,
    Horizontal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn scroll_produces_events() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_scroll(ScrollDirection::Vertical, 5, &style, &mut rng);
        let wheel_count = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::Wheel { .. }))
            .count();
        assert_eq!(wheel_count, 5);
    }

    #[test]
    fn scroll_negative_direction() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_scroll(ScrollDirection::Vertical, -3, &style, &mut rng);
        for ev in &plan.events {
            if let InputEvent::Wheel { delta, .. } = &ev.event {
                assert!(*delta < 0);
            }
        }
    }
}
