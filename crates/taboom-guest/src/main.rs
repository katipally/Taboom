mod browser;
mod channel;
mod dispatcher;
mod emitter;
mod input;
mod leak_guard;
mod network;
mod ocr;
mod recorder;
mod screen;
mod settle;
mod watchdog;

use anyhow::Result;
use tracing::info;

const CONTROL_DEVICE: &str = "/dev/virtio-ports/org.taboom.ctl";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("taboom-guest starting");

    watchdog::notify_ready();

    let net_setup = network::NetworkSetup::direct();
    info!(dns = %net_setup.resolv_conf_content().trim(), "network configured");

    let mut channel = channel::ControlChannel::open(CONTROL_DEVICE).await?;
    info!("control channel connected");

    channel.handshake().await?;
    info!("handshake complete");

    let heartbeat_handle = channel.start_heartbeat();

    dispatcher::run(&mut channel).await?;

    heartbeat_handle.abort();
    info!("taboom-guest stopped");
    Ok(())
}
