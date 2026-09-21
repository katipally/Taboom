use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use taboom_proto::*;
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Instant;

pub struct ControlChannel {
    file: Arc<std::sync::Mutex<std::fs::File>>,
    buf: BytesMut,
    start_time: Instant,
}

impl ControlChannel {
    pub async fn open(path: &str) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening control device {path}"))?;

        Ok(Self {
            file: Arc::new(std::sync::Mutex::new(file)),
            buf: BytesMut::with_capacity(4096),
            start_time: Instant::now(),
        })
    }

    pub async fn handshake(&mut self) -> Result<()> {
        let hostname = tokio::process::Command::new("hostname")
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".into());

        let envelope = Envelope::new(Message::Handshake(Handshake {
            version: PROTOCOL_VERSION,
            hostname,
        }));

        self.send(&envelope).await?;

        let response = self.recv().await?;
        match response.body {
            Message::HandshakeAck(ack) if ack.accepted => {
                debug!("handshake accepted, protocol v{}", ack.version);
                Ok(())
            }
            Message::HandshakeAck(ack) => {
                bail!("handshake rejected by host (version {})", ack.version);
            }
            other => {
                bail!("unexpected response to handshake: {other:?}");
            }
        }
    }

    pub fn start_heartbeat(&self) -> JoinHandle<()> {
        let file = Arc::clone(&self.file);
        let start = self.start_time;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                let uptime = start.elapsed().as_secs();
                let envelope = Envelope::new(Message::Heartbeat(Heartbeat {
                    uptime_secs: uptime,
                }));
                match FrameWriter::encode(&envelope) {
                    Ok(data) => {
                        let file = Arc::clone(&file);
                        let result = tokio::task::spawn_blocking(move || {
                            let mut f = file.lock().unwrap();
                            f.write_all(&data)
                        }).await;
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                warn!("heartbeat send failed: {e}");
                                break;
                            }
                            Err(e) => {
                                warn!("heartbeat task panicked: {e}");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("heartbeat encode failed: {e}");
                        break;
                    }
                }
            }
        })
    }

    pub async fn send(&mut self, envelope: &Envelope) -> Result<()> {
        let data = FrameWriter::encode(envelope)?;
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || {
            let mut f = file.lock().unwrap();
            f.write_all(&data)
        })
        .await??;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Envelope> {
        loop {
            if let Some(envelope) = FrameReader::try_decode(&mut self.buf)? {
                return Ok(envelope);
            }

            let file = Arc::clone(&self.file);
            let mut tmp = vec![0u8; 4096];
            let n = tokio::task::spawn_blocking(move || {
                let mut f = file.lock().unwrap();
                f.read(&mut tmp).map(|n| (n, tmp))
            })
            .await??;
            if n.0 == 0 {
                bail!("control channel closed");
            }
            self.buf.extend_from_slice(&n.1[..n.0]);
        }
    }
}
