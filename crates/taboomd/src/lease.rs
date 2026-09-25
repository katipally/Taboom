use chrono::{DateTime, Utc};
use uuid::Uuid;

pub struct Lease {
    pub id: Uuid,
    pub agent_id: String,
    pub acquired_at: DateTime<Utc>,
}

/// Exclusive use of the container's one desktop.
#[derive(Default)]
pub struct LeaseManager {
    lease: Option<Lease>,
}

impl LeaseManager {
    pub fn acquire(&mut self, agent_id: &str) -> Result<&Lease, LeaseError> {
        if let Some(existing) = &self.lease {
            return Err(LeaseError::AlreadyHeld { held_by: existing.agent_id.clone() });
        }
        Ok(self.lease.insert(Lease {
            id: Uuid::new_v4(),
            agent_id: agent_id.to_string(),
            acquired_at: Utc::now(),
        }))
    }

    pub fn release(&mut self, agent_id: &str) -> Result<(), LeaseError> {
        match &self.lease {
            Some(lease) if lease.agent_id == agent_id => {
                self.lease = None;
                Ok(())
            }
            Some(lease) => Err(LeaseError::NotOwner {
                held_by: lease.agent_id.clone(),
                requested_by: agent_id.to_string(),
            }),
            None => Err(LeaseError::NotHeld),
        }
    }

    pub fn holder(&self) -> Option<&Lease> {
        self.lease.as_ref()
    }

    pub fn lease_for(&self, agent_id: &str) -> Option<&Lease> {
        self.lease.as_ref().filter(|l| l.agent_id == agent_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("the desktop is in a session held by '{held_by}'; it must call session_end first")]
    AlreadyHeld { held_by: String },

    #[error("no session is active")]
    NotHeld,

    #[error("the session is held by '{held_by}', not '{requested_by}'")]
    NotOwner { held_by: String, requested_by: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let mut mgr = LeaseManager::default();
        assert!(matches!(mgr.release("a"), Err(LeaseError::NotHeld)));
        let id = mgr.acquire("a").unwrap().id;
        assert!(matches!(mgr.acquire("b"), Err(LeaseError::AlreadyHeld { .. })));
        assert!(matches!(mgr.acquire("a"), Err(LeaseError::AlreadyHeld { .. })));
        assert!(matches!(mgr.release("b"), Err(LeaseError::NotOwner { .. })));
        assert_eq!(mgr.lease_for("a").unwrap().id, id);
        assert!(mgr.lease_for("b").is_none());
        mgr.release("a").unwrap();
        assert!(mgr.holder().is_none());
        assert_eq!(mgr.acquire("b").unwrap().agent_id, "b");
    }
}
