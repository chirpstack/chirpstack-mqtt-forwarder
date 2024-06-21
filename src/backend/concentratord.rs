use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chirpstack_api::gw;
use log::{debug, error, info, trace, warn};
use prost::Message;
use tokio::task;

use super::Backend as BackendTrait;
use crate::config::Configuration;
use crate::metadata;
use crate::mqtt::{send_gateway_stats, send_mesh_stats, send_tx_ack, send_uplink_frame};

pub struct Backend {
    gateway_id: String,
    ctx: zmq::Context,
    cmd_url: String,
    cmd_sock: Mutex<zmq::Socket>,
}

impl Backend {
    pub async fn setup(conf: &Configuration) -> Result<Self> {
        info!("Setting up ChirpStack Concentratord backend");

        let zmq_ctx = zmq::Context::new();

        info!(
            "Connecting to Concentratord event API, event_url: {}",
            conf.backend.concentratord.event_url
        );
        let event_sock = zmq_ctx.socket(zmq::SUB)?;
        event_sock.connect(&conf.backend.concentratord.event_url)?;
        event_sock.set_subscribe("".as_bytes())?;

        info!(
            "Connecting to Concentratord command API, command_url: {}",
            conf.backend.concentratord.command_url
        );
        let cmd_sock = zmq_ctx.socket(zmq::REQ)?;
        cmd_sock.connect(&conf.backend.concentratord.command_url)?;

        info!("Reading gateway id");

        // send 'gateway_id' command with empty payload.
        cmd_sock.send("gateway_id", zmq::SNDMORE)?;
        cmd_sock.send("", 0)?;

        // set poller so that we can timeout after 100ms
        let mut items = [cmd_sock.as_poll_item(zmq::POLLIN)];
        zmq::poll(&mut items, 100)?;
        if !items[0].is_readable() {
            return Err(anyhow!("Could not read gateway id"));
        }
        let gateway_id = cmd_sock.recv_bytes(0)?;
        let gateway_id = hex::encode(gateway_id);

        info!("Received gateway id, gateway_id: {}", gateway_id);

        tokio::spawn({
            let forward_crc_ok = conf.backend.filters.forward_crc_ok;
            let forward_crc_invalid = conf.backend.filters.forward_crc_invalid;
            let forward_crc_missing = conf.backend.filters.forward_crc_missing;
            let filters = lrwn_filters::Filters {
                dev_addr_prefixes: conf.backend.filters.dev_addr_prefixes.clone(),
                join_eui_prefixes: conf.backend.filters.join_eui_prefixes.clone(),
            };

            async move {
                event_loop(
                    event_sock,
                    filters,
                    forward_crc_ok,
                    forward_crc_invalid,
                    forward_crc_missing,
                )
                .await;
            }
        });

        Ok(Backend {
            gateway_id,
            ctx: zmq_ctx,
            cmd_url: conf.backend.concentratord.command_url.clone(),
            cmd_sock: Mutex::new(cmd_sock),
        })
    }

    fn send_command(&self, cmd: &str, b: &[u8]) -> Result<Vec<u8>> {
        let res = || -> Result<Vec<u8>> {
            let cmd_sock = self.cmd_sock.lock().unwrap();
            cmd_sock.send(cmd, zmq::SNDMORE)?;
            cmd_sock.send(b, 0)?;

            // set poller so that we can timeout after 100ms
            let mut items = [cmd_sock.as_poll_item(zmq::POLLIN)];
            zmq::poll(&mut items, 100)?;
            if !items[0].is_readable() {
                return Err(anyhow!("Could not read down response"));
            }

            // red tx ack response
            let resp_b: &[u8] = &cmd_sock.recv_bytes(0)?;
            Ok(resp_b.to_vec())
        }();

        if res.is_err() {
            loop {
                // Reconnect the CMD socket in case we received an error.
                // In case there was an issue with receiving data from the socket, it could mean
                // it is in a 'dirty' state. E.g. due to the error we did not read the full
                // response.
                if let Err(e) = self.reconnect_cmd_sock() {
                    error!(
                        "Re-connecting to Concentratord command API error, error: {}",
                        e
                    );
                    sleep(Duration::from_secs(1));
                    continue;
                }

                break;
            }
        }

        res
    }

    fn reconnect_cmd_sock(&self) -> Result<()> {
        warn!(
            "Re-connecting to Concentratord command API, command_url: {}",
            self.cmd_url
        );
        let mut cmd_sock = self.cmd_sock.lock().unwrap();
        *cmd_sock = self.ctx.socket(zmq::REQ)?;
        cmd_sock.connect(&self.cmd_url)?;
        Ok(())
    }
}

#[async_trait]
impl BackendTrait for Backend {
    async fn get_gateway_id(&self) -> Result<String> {
        Ok(self.gateway_id.clone())
    }

    async fn send_downlink_frame(&self, pl: &gw::DownlinkFrame) -> Result<()> {
        info!("Sending downlink frame, downlink_id: {}", pl.downlink_id);

        let tx_ack = {
            let b = pl.encode_to_vec();
            let resp_b = self.send_command("down", &b)?;
            gw::DownlinkTxAck::decode(&mut Cursor::new(resp_b))?
        };

        let ack_items: Vec<String> = tx_ack
            .items
            .iter()
            .map(|i| i.status().as_str_name().to_string())
            .collect();

        info!(
            "Received ack, items: {:?}, downlink_id: {}",
            ack_items, pl.downlink_id
        );

        send_tx_ack(&tx_ack).await
    }

    async fn send_configuration_command(&self, pl: &gw::GatewayConfiguration) -> Result<()> {
        info!("Sending configuration command, version: {}", pl.version);

        let b = pl.encode_to_vec();
        let _ = self.send_command("config", &b)?;

        Ok(())
    }
}

async fn event_loop(
    event_sock: zmq::Socket,
    filters: lrwn_filters::Filters,
    forward_crc_ok: bool,
    forward_crc_invalid: bool,
    forward_crc_missing: bool,
) {
    trace!("Starting event loop");
    let event_sock = Arc::new(Mutex::new(event_sock));

    loop {
        let res = task::spawn_blocking({
            let event_sock = event_sock.clone();

            move || -> Result<Vec<Vec<u8>>> {
                let event_sock = event_sock.lock().unwrap();

                // set poller so that we can timeout after 100ms
                let mut items = [event_sock.as_poll_item(zmq::POLLIN)];
                zmq::poll(&mut items, 100)?;
                if !items[0].is_readable() {
                    return Ok(vec![]);
                }

                let msg = event_sock.recv_multipart(0)?;
                if msg.len() != 2 {
                    return Err(anyhow!("Event must have two frames"));
                }
                Ok(msg)
            }
        })
        .await;

        match res {
            Ok(Ok(msg)) => {
                if msg.len() != 2 {
                    continue;
                }

                if let Err(err) = handle_event_msg(
                    &msg[0],
                    &msg[1],
                    &filters,
                    forward_crc_ok,
                    forward_crc_invalid,
                    forward_crc_missing,
                )
                .await
                {
                    error!("Handle event error: {}", err);
                    continue;
                }
            }
            Ok(Err(err)) => {
                error!("Receive event error, error: {}", err);
                continue;
            }
            Err(err) => {
                error!("{}", err);
                continue;
            }
        }
    }
}

async fn handle_event_msg(
    event: &[u8],
    pl: &[u8],
    filters: &lrwn_filters::Filters,
    forward_crc_ok: bool,
    forward_crc_invalid: bool,
    forward_crc_missing: bool,
) -> Result<()> {
    let event = String::from_utf8(event.to_vec())?;
    let pl = Cursor::new(pl.to_vec());

    match event.as_str() {
        "up" => {
            let pl = gw::UplinkFrame::decode(pl)?;
            if let Some(rx_info) = &pl.rx_info {
                if !((rx_info.crc_status() == gw::CrcStatus::CrcOk && forward_crc_ok)
                    || (rx_info.crc_status() == gw::CrcStatus::BadCrc && forward_crc_invalid)
                    || (rx_info.crc_status() == gw::CrcStatus::NoCrc && forward_crc_missing))
                {
                    debug!(
                        "Ignoring uplink frame because of forward_crc_ flags, uplink_id: {}",
                        pl.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
                    );
                    return Ok(());
                }
            }

            if lrwn_filters::matches(&pl.phy_payload, filters) {
                info!(
                    "Received uplink frame, uplink_id: {}",
                    pl.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
                );
                send_uplink_frame(&pl).await?;
            } else {
                debug!(
                    "Ignoring uplink frame because of dev_addr and join_eui filters, uplink_id: {}",
                    pl.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default()
                );
            }
        }
        "stats" => {
            let mut pl = gw::GatewayStats::decode(pl)?;
            info!("Received gateway stats");
            pl.metadata.extend(metadata::get().await?);
            send_gateway_stats(&pl).await?;
        }
        "mesh_stats" => {
            let pl = gw::MeshStats::decode(pl)?;
            info!("Received mesh stats");
            send_mesh_stats(&pl).await?;
        }
        _ => {
            return Err(anyhow!("Unexpected event: {}", event));
        }
    }

    Ok(())
}
