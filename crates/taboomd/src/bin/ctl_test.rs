use anyhow::Result;
use bytes::BytesMut;
use taboom_proto::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use uuid::Uuid;

async fn send_msg(stream: &mut UnixStream, msg: Message) -> Result<()> {
    let envelope = Envelope::new(msg);
    let data = FrameWriter::encode(&envelope)?;
    stream.write_all(&data).await?;
    Ok(())
}

async fn send_screenshot_req(stream: &mut UnixStream) -> Result<()> {
    send_msg(
        stream,
        Message::Screenshot(ScreenshotReq {
            format: ImageFormat::Png,
            max_edge: Some(1280),
            region: None,
            frame_id: Some(Uuid::new_v4()),
        }),
    )
    .await
}

#[tokio::main]
async fn main() -> Result<()> {
    let socket_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("{}/.taboom/run/fulltest.ctl", std::env::var("HOME").unwrap()));

    let mode = std::env::args().nth(2).unwrap_or_default();
    let out_dir = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "/tmp/taboom-recording".into());

    if mode == "record" {
        std::fs::create_dir_all(&out_dir)?;
    }

    eprintln!("[host] connecting to {socket_path}");
    let mut stream = UnixStream::connect(&socket_path).await?;
    eprintln!("[host] connected, waiting for handshake...");

    let mut buf = BytesMut::with_capacity(256 * 1024);
    let mut handshake_done = false;
    let mut screenshot_sent = false;
    let mut frame_count: u32 = 0;
    let max_frames: u32 = if mode == "record" { 20 } else { 1 };

    loop {
        let n = stream.read_buf(&mut buf).await?;
        if n == 0 {
            eprintln!("[host] connection closed");
            break;
        }

        while let Some(envelope) = FrameReader::try_decode(&mut buf)? {
            match &envelope.body {
                Message::Handshake(hs) => {
                    eprintln!(
                        "[host] handshake from '{}' (protocol v{})",
                        hs.hostname, hs.version
                    );
                    send_msg(
                        &mut stream,
                        Message::HandshakeAck(HandshakeAck {
                            version: PROTOCOL_VERSION,
                            accepted: true,
                        }),
                    )
                    .await?;
                    eprintln!("[host] sent handshake ack");
                    handshake_done = true;

                    if mode == "screenshot" || mode == "record" {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        send_screenshot_req(&mut stream).await?;
                        screenshot_sent = true;
                        eprintln!("[host] first screenshot request sent");
                    }
                }
                Message::Heartbeat(hb) => {
                    eprintln!("[host] heartbeat: uptime={}s", hb.uptime_secs);

                    if handshake_done && !screenshot_sent && mode.is_empty() {
                        eprintln!("[host] channel verified, exiting");
                        return Ok(());
                    }
                }
                Message::ScreenshotReply(reply) => {
                    frame_count += 1;
                    let path = if mode == "record" {
                        format!("{}/frame_{:04}.png", out_dir, frame_count)
                    } else {
                        format!("/tmp/taboom-screenshot-{}.png", reply.frame_id)
                    };
                    std::fs::write(&path, &reply.data)?;
                    eprintln!(
                        "[host] frame {}/{}: {}x{} ({} bytes) -> {}",
                        frame_count, max_frames,
                        reply.returned_width, reply.returned_height,
                        reply.data.len(), path
                    );

                    if frame_count >= max_frames {
                        eprintln!("[host] recording complete: {} frames in {}", frame_count, out_dir);
                        return Ok(());
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    send_screenshot_req(&mut stream).await?;
                }
                other => {
                    eprintln!("[host] received: {other:?}");
                }
            }
        }
    }

    Ok(())
}
