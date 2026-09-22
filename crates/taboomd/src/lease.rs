use std::collections::HashMap;
use uuid::Uuid;
use chrono::{DateTime, Utc};

pub struct Lease {
    pub id: Uuid,
    pub persona: String,
    pub agent_id: String,
    pub acquired_at: DateTime<Utc>,
}

pub struct LeaseManager {
    leases: HashMap<String, Lease>,
}

impl LeaseManager {
    pub fn new() -> Self {
        Self {
            leases: HashMap::new(),
        }
    }

    pub fn acquire(&mut self, persona: &str, agent_id: &str) -> Result<&Lease, LeaseError> {
        if let Some(existing) = self.leases.get(persona) {
            return Err(LeaseError::AlreadyHeld {
                persona: persona.to_string(),
                held_by: existing.agent_id.clone(),
            });
        }

        let lease = Lease {
            id: Uuid::new_v4(),
            persona: persona.to_string(),
            agent_id: agent_id.to_string(),
            acquired_at: Utc::now(),
        };

        self.leases.insert(persona.to_string(), lease);
        Ok(self.leases.get(persona).unwrap())
    }

    pub fn release(&mut self, persona: &str, agent_id: &str) -> Result<(), LeaseError> {
        match self.leases.get(persona) {
            Some(lease) if lease.agent_id == agent_id => {
                self.leases.remove(persona);
                Ok(())
            }
            Some(lease) => Err(LeaseError::NotOwner {
                persona: persona.to_string(),
                held_by: lease.agent_id.clone(),
                requested_by: agent_id.to_string(),
            }),
            None => Err(LeaseError::NotHeld {
                persona: persona.to_string(),
            }),
        }
    }

    pub fn is_held(&self, persona: &str) -> bool {
        self.leases.contains_key(persona)
    }

    pub fn holder_is(&self, agent_id: &str) -> bool {
        self.leases.values().any(|l| l.agent_id == agent_id)
    }

    pub fn persona_for(&self, agent_id: &str) -> Option<&str> {
        self.leases
            .values()
            .find(|l| l.agent_id == agent_id)
            .map(|l| l.persona.as_str())
    }

    pub fn any(&self) -> Option<&Lease> {
        self.leases.values().next()
    }

    pub fn lease_for(&self, agent_id: &str) -> Option<&Lease> {
        self.leases.values().find(|l| l.agent_id == agent_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("persona '{persona}' already held by agent '{held_by}'")]
    AlreadyHeld { persona: String, held_by: String },

    #[error("persona '{persona}' not held by any agent")]
    NotHeld { persona: String },

    #[error("persona '{persona}' held by '{held_by}', not '{requested_by}'")]
    NotOwner {
        persona: String,
        held_by: String,
        requested_by: String,
    },
}
