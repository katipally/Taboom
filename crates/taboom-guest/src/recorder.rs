use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecorderState {
    Idle,
    Recording,
    Stopping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recorder {
    pub session_id: Uuid,
    pub output_path: PathBuf,
    pub retention_days: u32,
    state: RecorderState,
}

impl Recorder {
    pub fn new(output_dir: PathBuf, retention_days: u32) -> Self {
        let session_id = Uuid::new_v4();
        let filename = format!("{session_id}.mp4");
        Self {
            session_id,
            output_path: output_dir.join(filename),
            retention_days,
            state: RecorderState::Idle,
        }
    }

    pub fn state(&self) -> &RecorderState {
        &self.state
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        match self.state {
            RecorderState::Idle => {
                self.state = RecorderState::Recording;
                Ok(())
            }
            RecorderState::Recording => {
                anyhow::bail!("already recording")
            }
            RecorderState::Stopping => {
                anyhow::bail!("recorder is stopping")
            }
        }
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        match self.state {
            RecorderState::Recording => {
                self.state = RecorderState::Stopping;
                self.state = RecorderState::Idle;
                Ok(())
            }
            RecorderState::Idle => {
                anyhow::bail!("not recording")
            }
            RecorderState::Stopping => {
                anyhow::bail!("already stopping")
            }
        }
    }
}

pub fn apply_retention(recordings_dir: &std::path::Path, retention_days: u32) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(retention_days as u64 * 86400);

    let Ok(entries) = std::fs::read_dir(recordings_dir) else {
        return removed;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "mp4").unwrap_or(false) {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff && std::fs::remove_file(&path).is_ok() {
                        removed.push(path);
                    }
                }
            }
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_lifecycle() {
        let mut r = Recorder::new(PathBuf::from("/tmp"), 7);
        assert_eq!(r.state(), &RecorderState::Idle);
        r.start().unwrap();
        assert_eq!(r.state(), &RecorderState::Recording);
        assert!(r.start().is_err());
        r.stop().unwrap();
        assert_eq!(r.state(), &RecorderState::Idle);
        assert!(r.stop().is_err());
    }
}
