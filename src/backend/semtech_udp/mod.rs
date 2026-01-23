use std::collections::HashMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use chirpstack_api::{gw, prost::Message};
use log::{debug, info, trace, warn};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};

use super::Backend as BackendTrait;
use crate::config::Configuration;
use crate::metadata;
use crate::mqtt::{send_gateway_stats, send_tx_ack, send_uplink_frame};

mod structs;

struct State {
    socket: UdpSocket,
    gateway_id: Mutex<String>,
    downlink_cache: RwLock<HashMap<u16, DownlinkCache>>,
    pull_addr: RwLock<Option<SocketAddr>>,
    stats: Mutex<Stats>,
    time_fallback_enabled: bool,
    forward_crc_ok: bool,
    forward_crc_invalid: bool,
    forward_crc_missing: bool,
    filters: lrwn_filters::Filters,
}

#[derive(Clone)]
struct DownlinkCache {
    expire: SystemTime,
    frame: gw::DownlinkFrame,
    ack_items: Vec<gw::DownlinkTxAckItem>,
    index: usize,
}

#[derive(Default)]
struct Stats {
    pub rx_count: u32,
    pub tx_count: u32,
    pub rx_per_freq_count: HashMap<u32, u32>,
    pub tx_per_freq_count: HashMap<u32, u32>,
    pub rx_per_modulation_count: HashMap<Vec<u8>, u32>,
    pub tx_per_modulation_count: HashMap<Vec<u8>, u32>,
    pub tx_status_count: HashMap<String, u32>,
}

impl Stats {
    fn count_uplink(&mut self, pl: &gw::UplinkFrame) -> Result<()> {
        let tx_info = pl
            .tx_info
            .as_ref()
            .ok_or_else(|| anyhow!("tx_info is missing"))?;
        let modulation = tx_info
            .modulation
            .as_ref()
            .ok_or_else(|| anyhow!("modulation is missing"))?;

        let b = modulation.encode_to_vec();
        self.rx_count += 1;
        self.rx_per_freq_count
            .entry(tx_info.frequency)
            .and_modify(|v| *v += 1)
            .or_insert(1);
        self.rx_per_modulation_count
            .entry(b)
            .and_modify(|v| *v += 1)
            .or_insert(1);

        Ok(())
    }

    fn count_downlink(&mut self, pl: &gw::DownlinkFrame, ack: &gw::DownlinkTxAck) -> Result<()> {
        for (i, v) in ack.items.iter().enumerate() {
            if v.status() == gw::TxAckStatus::Ignored {
                continue;
            }

            let status = v.status().as_str_name().to_string();
            self.tx_status_count
                .entry(status)
                .and_modify(|v| *v += 1)
                .or_insert(1);

            if v.status() == gw::TxAckStatus::Ok && i < pl.items.len() {
                let item = pl
                    .items
                    .get(i)
                    .ok_or_else(|| anyhow!("Invalid items index"))?;
                let tx_info = item
                    .tx_info
                    .as_ref()
                    .ok_or_else(|| anyhow!("tx_info is missing"))?;
                let modulation = tx_info
                    .modulation
                    .as_ref()
                    .ok_or_else(|| anyhow!("modulation is missing"))?;
                let b = modulation.encode_to_vec();

                self.tx_count += 1;
                self.tx_per_freq_count
                    .entry(tx_info.frequency)
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
                self.tx_per_modulation_count
                    .entry(b)
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
            }
        }

        Ok(())
    }

    fn export_stats(&mut self) -> Result<gw::GatewayStats> {
        let mut stats = gw::GatewayStats {
            rx_packets_received: self.rx_count,
            rx_packets_received_ok: self.rx_count,
            tx_packets_received: self.tx_count,
            tx_packets_emitted: self.tx_count,
            tx_packets_per_status: self.tx_status_count.clone(),
            ..Default::default()
        };

        for (k, v) in self.rx_per_freq_count.iter() {
            stats.rx_packets_per_frequency.insert(*k, *v);
        }

        for (k, v) in self.tx_per_freq_count.iter() {
            stats.tx_packets_per_frequency.insert(*k, *v);
        }

        for (k, v) in self.rx_per_modulation_count.iter() {
            let modulation = gw::Modulation::decode(&mut Cursor::new(k))?;
            stats
                .rx_packets_per_modulation
                .push(gw::PerModulationCount {
                    count: *v,
                    modulation: Some(modulation),
                });
        }

        for (k, v) in self.tx_per_modulation_count.iter() {
            let modulation = gw::Modulation::decode(&mut Cursor::new(k))?;
            stats
                .tx_packets_per_modulation
                .push(gw::PerModulationCount {
                    count: *v,
                    modulation: Some(modulation),
                });
        }

        *self = Stats::default();

        Ok(stats)
    }
}

impl State {
    async fn set_gateway_id(&self, gateway_id: &[u8]) {
        let mut gw_id = self.gateway_id.lock().await;
        *gw_id = hex::encode(gateway_id);
    }

    async fn get_gateway_id(&self) -> String {
        self.gateway_id.lock().await.clone()
    }

    async fn set_downlink_cache(&self, token: u16, dc: DownlinkCache) {
        let mut cache = self.downlink_cache.write().await;
        cache.insert(token, dc);
    }

    async fn get_downlink_cache(&self, token: u16) -> Option<DownlinkCache> {
        self.clean_downlink_cache().await;
        let cache = self.downlink_cache.read().await;
        cache.get(&token).cloned()
    }

    async fn clean_downlink_cache(&self) {
        let mut cache = self.downlink_cache.write().await;
        cache.retain(|k, v| {
            if v.expire < SystemTime::now() {
                trace!("Removing key from cache, key: {}", k);
                false
            } else {
                true
            }
        });
    }

    async fn set_pull_addr(&self, addr: &SocketAddr) {
        let mut pull_addr = self.pull_addr.write().await;
        *pull_addr = Some(*addr);
    }

    async fn get_pull_addr(&self) -> Result<SocketAddr> {
        self.pull_addr.read().await.ok_or(anyhow!("No pull_addr"))
    }

    async fn export_stats(&self) -> Result<gw::GatewayStats> {
        let mut stats = self.stats.lock().await;
        stats.export_stats()
    }

    async fn count_uplink(&self, pl: &gw::UplinkFrame) -> Result<()> {
        let mut stats = self.stats.lock().await;
        stats.count_uplink(pl)
    }

    async fn count_downlink(&self, pl: &gw::DownlinkFrame, ack: &gw::DownlinkTxAck) -> Result<()> {
        let mut stats = self.stats.lock().await;
        stats.count_downlink(pl, ack)
    }
}

pub struct Backend {
    state: Arc<State>,
}

impl Backend {
    pub async fn setup(conf: &Configuration) -> Result<Self> {
        info!("Setting up Semtech UDP packet-forwarder backend");

        info!(
            "Binding UDP socket, bind: {}",
            conf.backend.semtech_udp.bind
        );
        let socket = UdpSocket::bind(&conf.backend.semtech_udp.bind).await?;

        // setup state
        let state = State {
            socket,
            gateway_id: Mutex::new(conf.backend.gateway_id.clone()),
            downlink_cache: RwLock::new(HashMap::new()),
            pull_addr: RwLock::new(None),
            stats: Mutex::new(Stats::default()),
            time_fallback_enabled: conf.backend.semtech_udp.time_fallback_enabled,
            forward_crc_invalid: conf.backend.filters.forward_crc_invalid,
            forward_crc_missing: conf.backend.filters.forward_crc_missing,
            forward_crc_ok: conf.backend.filters.forward_crc_ok,
            filters: lrwn_filters::Filters {
                dev_addr_prefixes: conf.backend.filters.dev_addr_prefixes.clone(),
                join_eui_prefixes: conf.backend.filters.join_eui_prefixes.clone(),
                lorawan_only: conf.backend.filters.lorawan_only,
            },
        };
        let state = Arc::new(state);

        tokio::spawn({
            let state = state.clone();
            async move {
                udp_receive_loop(state).await;
            }
        });

        Ok(Backend { state })
    }
}

#[async_trait]
impl BackendTrait for Backend {
    async fn get_gateway_id(&self) -> Result<String> {
        let gw_id = self.state.get_gateway_id().await;
        if gw_id.is_empty() {
            return Err(anyhow!("Gateway ID not yet set"));
        }
        Ok(gw_id)
    }

    async fn send_downlink_frame(&self, pl: gw::DownlinkFrame) -> Result<()> {
        let mut acks: Vec<gw::DownlinkTxAckItem> = Vec::with_capacity(pl.items.len());
        for _ in &pl.items {
            acks.push(gw::DownlinkTxAckItem {
                status: gw::TxAckStatus::Ignored.into(),
            });
        }
        send_downlink_frame(&self.state, pl, acks, 0).await
    }

    async fn send_configuration_command(&self, _pl: gw::GatewayConfiguration) -> Result<()> {
        Ok(())
    }

    async fn send_mesh_command(&self, _pl: gw::MeshCommand) -> Result<()> {
        Ok(())
    }
}

async fn udp_receive_loop(state: Arc<State>) {
    let mut buffer: [u8; 65535] = [0; 65535];

    loop {
        let (size, remote) = match state.socket.recv_from(&mut buffer).await {
            Ok((size, remote)) => (size, remote),
            Err(e) => {
                warn!("UDP socket receive error: {}", e);
                continue;
            }
        };

        if size < 4 {
            warn!(
                "At least 4 bytes are expected, received: {}, remote: {}",
                size, remote
            );
            continue;
        }

        match buffer[3] {
            0x00 => {
                // PUSH_DATA
                if let Err(e) = handle_push_data(&state, &buffer[..size], &remote).await {
                    warn!("Handle PUSH_DATA error: {}, remote: {}", e, remote);
                }
            }
            0x02 => {
                // PULL_DATA
                if let Err(e) = handle_pull_data(&state, &buffer[..size], &remote).await {
                    warn!("Handle PULL_DATA error: {}, remote: {}", e, remote);
                }
            }
            0x05 => {
                // TX_ACK
                if let Err(e) = handle_tx_ack(&state, &buffer[..size], &remote).await {
                    warn!("Handle TX_ACK error: {}, remote: {}", e, remote);
                }
            }
            _ => {
                warn!(
                    "Unexepcted command received, cid: {}, remote: {}",
                    buffer[3], remote
                );
                continue;
            }
        }
    }
}

async fn handle_push_data(state: &Arc<State>, data: &[u8], remote: &SocketAddr) -> Result<()> {
    let pl = structs::PushData::from_slice(data)?;

    info!(
        "PUSH_DATA received, random_token: {}, remote: {}",
        pl.random_token, remote
    );

    info!(
        "Sending PUSH_ACK, random_token: {} remote: {}",
        pl.random_token, remote
    );
    let ack = structs::PushAck {
        random_token: pl.random_token,
    };
    state.socket.send_to(&ack.to_vec(), remote).await?;

    let uplink_frames = pl.to_proto_uplink_frames(state.time_fallback_enabled)?;
    let gateway_stats = pl.to_proto_gateway_stats()?;

    for uf in &uplink_frames {
        if let Some(rx_info) = &uf.rx_info {
            if !((rx_info.crc_status() == gw::CrcStatus::CrcOk && state.forward_crc_ok)
                || (rx_info.crc_status() == gw::CrcStatus::BadCrc && state.forward_crc_invalid)
                || (rx_info.crc_status() == gw::CrcStatus::NoCrc && state.forward_crc_missing))
            {
                debug!(
                    "Ignoring uplink frame because of forward_crc_ flags, uplink_id: {}",
                    uf.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
                );

                continue;
            }
        }

        if lrwn_filters::matches(&uf.phy_payload, &state.filters) {
            state.count_uplink(uf).await?;
            send_uplink_frame(uf).await?;
        } else {
            debug!(
                "Ignoring uplink frame because of dev_addr and join_eui filters, uplink_id: {}",
                uf.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default()
            );
        }
    }

    if let Some(mut stats) = gateway_stats {
        let s = state.export_stats().await?;
        stats.rx_packets_received_ok = s.rx_packets_received_ok;
        stats.tx_packets_emitted = s.tx_packets_emitted;
        stats
            .rx_packets_per_frequency
            .clone_from(&s.rx_packets_per_frequency);
        stats
            .tx_packets_per_frequency
            .clone_from(&s.tx_packets_per_frequency);
        stats
            .rx_packets_per_modulation
            .clone_from(&s.rx_packets_per_modulation);
        stats
            .tx_packets_per_modulation
            .clone_from(&s.tx_packets_per_modulation);
        stats
            .tx_packets_per_status
            .clone_from(&s.tx_packets_per_status);
        stats.metadata.extend(metadata::get().await?);

        send_gateway_stats(&stats).await?;
    }

    Ok(())
}

async fn handle_pull_data(state: &Arc<State>, data: &[u8], remote: &SocketAddr) -> Result<()> {
    let pl = structs::PullData::from_slice(data)?;

    info!(
        "PULL_DATA received, random_token: {}, remote: {}",
        pl.random_token, remote
    );

    info!(
        "Sending PULL_ACK, random_token: {}, remote: {}",
        pl.random_token, remote
    );
    let ack = structs::PullAck {
        random_token: pl.random_token,
    };
    state.socket.send_to(&ack.to_vec(), remote).await?;

    // Set the Gateway ID.
    state.set_gateway_id(&pl.gateway_id).await;
    // Store the address from which the PULL_DATA is coming, and to which we need to respond with
    // PULL_RESP in case we have any data to send.
    state.set_pull_addr(remote).await;

    Ok(())
}

async fn handle_tx_ack(state: &Arc<State>, data: &[u8], remote: &SocketAddr) -> Result<()> {
    let pl = structs::TxAck::from_slice(data)?;

    info!(
        "TX_ACK received, random_token: {}, remote: {}, error: {}",
        pl.random_token,
        remote,
        pl.payload
            .as_ref()
            .cloned()
            .unwrap_or_default()
            .txpk_ack
            .error
    );

    let downlink_cache = state
        .get_downlink_cache(pl.random_token)
        .await
        .ok_or_else(|| anyhow!("No cache item for token, random_token: {}", pl.random_token))?;

    let ack_status = pl.to_proto_tx_ack_status();

    let mut ack_items = downlink_cache.ack_items.clone();
    ack_items[downlink_cache.index].status = ack_status.into();

    if ack_status == gw::TxAckStatus::Ok || downlink_cache.index >= ack_items.len() - 1 {
        let pl = gw::DownlinkTxAck {
            gateway_id: hex::encode(pl.gateway_id),
            downlink_id: downlink_cache.frame.downlink_id,
            items: ack_items,
            ..Default::default()
        };
        state.count_downlink(&downlink_cache.frame, &pl).await?;
        send_tx_ack(&pl).await
    } else {
        send_downlink_frame(
            state,
            downlink_cache.frame,
            ack_items,
            downlink_cache.index + 1,
        )
        .await
    }
}

async fn send_downlink_frame(
    state: &Arc<State>,
    pl: gw::DownlinkFrame,
    acks: Vec<gw::DownlinkTxAckItem>,
    i: usize,
) -> Result<()> {
    let token = pl.downlink_id as u16;
    state
        .set_downlink_cache(
            token,
            DownlinkCache {
                expire: SystemTime::now() + Duration::from_secs(60),
                frame: pl.clone(),
                ack_items: acks,
                index: i,
            },
        )
        .await;

    let pull_resp = structs::PullResp::from_proto(&pl, i, token)?;
    let pull_addr = state.get_pull_addr().await?;

    info!(
        "Sending PULL_RESP, random_token: {}, remote: {}",
        token, pull_addr
    );

    state
        .socket
        .send_to(&pull_resp.to_vec()?, pull_addr)
        .await?;

    Ok(())
}
