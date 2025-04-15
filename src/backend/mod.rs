use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chirpstack_api::gw;
use log::{debug, info};
use tokio::sync::OnceCell;

use crate::config::Configuration;

#[cfg(feature = "concentratord")]
pub mod concentratord;
#[cfg(feature = "semtech_udp")]
pub mod semtech_udp;

static BACKEND: OnceCell<Box<dyn Backend + Sync + Send>> = OnceCell::const_new();

#[async_trait]
pub trait Backend {
    async fn get_gateway_id(&self) -> Result<String>;
    async fn send_downlink_frame(&self, pl: &gw::DownlinkFrame) -> Result<()>;
    async fn send_configuration_command(&self, pl: &gw::GatewayConfiguration) -> Result<()>;
}

pub async fn setup(conf: &Configuration) -> Result<()> {
    match conf.backend.enabled.as_ref() {
        #[cfg(feature = "semtech_udp")]
        "semtech_udp" => {
            info!("Setting up Semtech UDP Packet Forwarder backend");
            let b = semtech_udp::Backend::setup(conf).await?;
            BACKEND
                .set(Box::new(b))
                .map_err(|e| anyhow!("OnceCell error: {}", e))?;
        }
        #[cfg(feature = "concentratord")]
        "concentratord" => {
            info!("Setting up ChirpStack Concentratord backend");
            let b = concentratord::Backend::setup(conf).await?;
            BACKEND
                .set(Box::new(b))
                .map_err(|e| anyhow!("OnceCell error: {}", e))?;
        }
        _ => {
            return Err(anyhow!("Unexpected backend: {}", conf.backend.enabled));
        }
    }

    debug!("Retrieving Gateway ID from backend");

    loop {
        match get_gateway_id().await {
            Ok(id) => {
                info!("Received Gateway ID from backend, gateway_id: {}", id);
                break;
            }
            Err(e) => {
                debug!("Could not get Gateway ID from backend, error: {}", e);
            }
        }

        debug!("Waiting for backend to report Gateway ID");
        sleep(Duration::from_secs(1));
    }

    Ok(())
}

pub async fn get_gateway_id() -> Result<String> {
    if let Some(b) = BACKEND.get() {
        return b.get_gateway_id().await;
    }

    Err(anyhow!("BACKEND is not set"))
}

pub async fn send_downlink_frame(pl: &gw::DownlinkFrame) -> Result<()> {
    if let Some(b) = BACKEND.get() {
        return b.send_downlink_frame(pl).await;
    }

    Err(anyhow!("BACKEND is not set"))
}

pub async fn send_configuration_command(pl: &gw::GatewayConfiguration) -> Result<()> {
    if let Some(b) = BACKEND.get() {
        return b.send_configuration_command(pl).await;
    }

    Err(anyhow!("BACKEND is not set"))
}
