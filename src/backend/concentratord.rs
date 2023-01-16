use std::io::Cursor;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use chirpstack_api::gw;
use log::{debug, error, info, trace};
use prost::Message;
use tokio::task;

use super::Backend as BackendTrait;
use crate::config::Configuration;
use crate::mqtt::{send_gateway_stats, send_tx_ack, send_uplink_frame};

pub struct Backend {
    gateway_id: String,
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
            let forward_crc_ok = conf.backend.forward_crc_ok;
            let forward_crc_invalid = conf.backend.forward_crc_invalid;
            let forward_crc_missing = conf.backend.forward_crc_missing;

            async move {
                event_loop(
                    event_sock,
                    forward_crc_ok,
                    forward_crc_invalid,
                    forward_crc_missing,
                )
                .await;
            }
        });

        Ok(Backend {
            gateway_id,
            cmd_sock: Mutex::new(cmd_sock),
        })
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
            let cmd_sock = self.cmd_sock.lock().unwrap();

            let b = pl.encode_to_vec();

            // send 'down' command with payload
            cmd_sock.send("down", zmq::SNDMORE)?;
            cmd_sock.send(b, 0)?;

            // set poller so that we can timeout after 100ms
            let mut items = [cmd_sock.as_poll_item(zmq::POLLIN)];
            zmq::poll(&mut items, 100)?;
            if !items[0].is_readable() {
                return Err(anyhow!("Could not read down response"));
            }

            // red tx ack response
            let resp_b: &[u8] = &cmd_sock.recv_bytes(0)?;
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

        let cmd_sock = self.cmd_sock.lock().unwrap();
        let b = pl.encode_to_vec();

        // send 'config' command with payload
        cmd_sock.send("config", zmq::SNDMORE)?;
        cmd_sock.send(b, 0)?;

        // set poller so that we can timeout after 100ms
        let mut items = [cmd_sock.as_poll_item(zmq::POLLIN)];
        zmq::poll(&mut items, 100)?;
        if !items[0].is_readable() {
            return Err(anyhow!("Could not read down response"));
        }

        // read response
        let _: &[u8] = &cmd_sock.recv_bytes(0)?;

        Ok(())
    }
}

async fn event_loop(
    event_sock: zmq::Socket,
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

            info!(
                "Received uplink frame, uplink_id: {}",
                pl.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
            );
            send_uplink_frame(&pl).await?;
        }
        "stats" => {
            let pl = gw::GatewayStats::decode(pl)?;
            info!("Received gateway stats");
            send_gateway_stats(&pl).await?;
        }
        _ => {
            return Err(anyhow!("Unexpected event: {}", event));
        }
    }

    Ok(())
}
