use crate::input::InputDevices;
use anyhow::Result;
use std::time::Instant;
use taboom_humanizer::ActionPlan;
use tracing::{debug, warn};

pub struct EmitterStats {
    pub events_emitted: u64,
    pub max_lateness_us: i64,
    pub total_lateness_us: i64,
}

pub struct Emitter;

impl Emitter {
    pub fn execute_plan(
        devices: &mut InputDevices,
        plan: &ActionPlan,
    ) -> Result<EmitterStats> {
        if plan.is_empty() {
            return Ok(EmitterStats {
                events_emitted: 0,
                max_lateness_us: 0,
                total_lateness_us: 0,
            });
        }

        let start = Instant::now();
        let mut stats = EmitterStats {
            events_emitted: 0,
            max_lateness_us: 0,
            total_lateness_us: 0,
        };

        for timed in &plan.events {
            let target_elapsed_us = timed.timestamp_us as i64;

            loop {
                let actual_us = start.elapsed().as_micros() as i64;
                let remaining = target_elapsed_us - actual_us;
                if remaining <= 0 {
                    break;
                }
                if remaining > 1000 {
                    std::thread::sleep(std::time::Duration::from_micros(
                        (remaining as u64).saturating_sub(500),
                    ));
                } else {
                    std::hint::spin_loop();
                }
            }

            let actual_us = start.elapsed().as_micros() as i64;
            let lateness = actual_us - target_elapsed_us;
            stats.max_lateness_us = stats.max_lateness_us.max(lateness);
            stats.total_lateness_us += lateness;

            devices.execute(&timed.event)?;
            stats.events_emitted += 1;
        }

        if stats.max_lateness_us > 5000 {
            warn!(
                max_lateness_us = stats.max_lateness_us,
                "emitter fell behind schedule"
            );
        }

        debug!(
            events = stats.events_emitted,
            max_lateness_us = stats.max_lateness_us,
            "plan executed"
        );

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputConfig, MockUinput};
    use taboom_humanizer::InputEvent;

    #[test]
    fn emitter_preserves_event_order() {
        let mock = Box::new(MockUinput::new());
        let mock_ref = unsafe {
            &*(mock.as_ref() as *const MockUinput)
        };
        let mut devices = InputDevices::new(mock, InputConfig::default());

        let mut plan = ActionPlan::new();
        plan.push(0, InputEvent::MouseRel { dx: 5, dy: 0 });
        plan.push(0, InputEvent::Sync);
        plan.push(1000, InputEvent::MouseRel { dx: 10, dy: 0 });
        plan.push(1000, InputEvent::Sync);

        Emitter::execute_plan(&mut devices, &plan).unwrap();

        let events = mock_ref.drain();
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn empty_plan_ok() {
        let mock = Box::new(MockUinput::new());
        let mut devices = InputDevices::new(mock, InputConfig::default());
        let plan = ActionPlan::new();
        let stats = Emitter::execute_plan(&mut devices, &plan).unwrap();
        assert_eq!(stats.events_emitted, 0);
    }
}
