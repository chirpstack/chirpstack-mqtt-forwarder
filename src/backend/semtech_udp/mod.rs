use std::collections::HashMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use chirpstack_api::gw;
use log::{info, trace, warn};
use prost::Message;
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
        info!(
            "Binding UDP socket, bind: {}",
            conf.backend.semtech_udp.udp_bind
        );
        let socket = UdpSocket::bind(&conf.backend.semtech_udp.udp_bind).await?;

        // setup state
        let state = State {
            socket,
            gateway_id: Mutex::new(conf.backend.gateway_id.clone()),
            downlink_cache: RwLock::new(HashMap::new()),
            pull_addr: RwLock::new(None),
            stats: Mutex::new(Stats::default()),
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

    async fn send_downlink_frame(&self, pl: &gw::DownlinkFrame) -> Result<()> {
        let mut acks: Vec<gw::DownlinkTxAckItem> = Vec::with_capacity(pl.items.len());
        for _ in &pl.items {
            acks.push(gw::DownlinkTxAckItem {
                status: gw::TxAckStatus::Ignored.into(),
            });
        }
        send_downlink_frame(&self.state, pl, acks, 0).await
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

    let uplink_frames = pl.to_proto_uplink_frames()?;
    let gateway_stats = pl.to_proto_gateway_stats()?;

    for uf in &uplink_frames {
        state.count_uplink(uf).await?;
        send_uplink_frame(uf).await?;
    }

    if let Some(mut stats) = gateway_stats {
        let s = state.export_stats().await?;
        stats.rx_packets_received_ok = s.rx_packets_received_ok;
        stats.tx_packets_emitted = s.tx_packets_emitted;
        stats.rx_packets_per_frequency = s.rx_packets_per_frequency.clone();
        stats.tx_packets_per_frequency = s.tx_packets_per_frequency.clone();
        stats.rx_packets_per_modulation = s.rx_packets_per_modulation.clone();
        stats.tx_packets_per_modulation = s.tx_packets_per_modulation.clone();
        stats.tx_packets_per_status = s.tx_packets_per_status.clone();
        stats.meta_data.extend(metadata::get().await?);

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
            gateway_id: hex::encode(&pl.gateway_id),
            downlink_id: downlink_cache.frame.downlink_id,
            items: ack_items,
            ..Default::default()
        };
        state.count_downlink(&downlink_cache.frame, &pl).await?;
        send_tx_ack(&pl).await
    } else {
        send_downlink_frame(
            state,
            &downlink_cache.frame,
            ack_items,
            downlink_cache.index + 1,
        )
        .await
    }
}

async fn send_downlink_frame(
    state: &Arc<State>,
    pl: &gw::DownlinkFrame,
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

    let pull_resp = structs::PullResp::from_proto(pl, i, token)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use chirpstack_api::common;
    use chrono::{DateTime, Utc};
    use futures::StreamExt;
    use paho_mqtt as mqtt;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_udp_mqtt_end_to_end() {
        let mut buffer: [u8; 65535] = [0; 65535];

        let mut c = Configuration {
            backend: config::Backend {
                gateway_id: "0102030405060708".into(),
                ..Default::default()
            },
            mqtt: config::Mqtt {
                server: "tcp://mosquitto:1883".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        c.metadata
            .r#static
            .insert("foo".to_string(), "bar".to_string());
        c.metadata.commands.insert(
            "hello".to_string(),
            vec!["echo".to_string(), "hello world".to_string()],
        );

        crate::metadata::setup(&c).unwrap();
        crate::backend::setup(&c).await.unwrap();
        crate::mqtt::setup(&c).await.unwrap();

        // UDP
        let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        socket.connect("0.0.0.0:1700").await.unwrap();

        // MQTT
        let create_opts = mqtt::CreateOptionsBuilder::new()
            .server_uri("tcp://mosquitto:1883")
            .finalize();
        let mut client = mqtt::AsyncClient::new(create_opts).unwrap();
        let mut stream = client.get_stream(25);
        let conn_opts = mqtt::ConnectOptionsBuilder::new()
            .clean_session(true)
            .finalize();
        client.connect(conn_opts).await.unwrap();
        client
            .subscribe("eu868/gateway/0102030405060708/event/+", 0)
            .await
            .unwrap();
        client
            .subscribe("eu868/gateway/0102030405060708/state/+", 0)
            .await
            .unwrap();

        sleep(Duration::from_millis(200)).await;

        // PULL_DATA
        socket
            .send(&[2, 0, 1, 2, 1, 2, 3, 4, 5, 6, 7, 8])
            .await
            .unwrap();

        // PULL_ACK
        let size = socket.recv(&mut buffer).await.unwrap();
        assert_eq!(&[2, 0, 1, 4], &buffer[..size]);

        // MQTT conn state
        let mqtt_msg = stream.next().await.unwrap().unwrap();
        assert_eq!(
            "eu868/gateway/0102030405060708/state/conn",
            mqtt_msg.topic()
        );
        assert!(mqtt_msg.retained());
        let pl = gw::ConnState::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
        assert_eq!(
            gw::ConnState {
                gateway_id: "0102030405060708".into(),
                state: gw::conn_state::State::Online.into(),
                ..Default::default()
            },
            pl
        );

        // PUSH_DATA rxpk
        let pl_b = r#"{
            "rxpk": [
                {
                    "time": "2022-09-10T12:30:15Z",
                    "tmst": 1234,
                    "chan": 2,
                    "rfch": 0,
                    "freq": 868.1,
                    "stat": 1,
                    "modu": "LORA",
                    "datr": "SF7BW125",
                    "codr": "4/5",
                    "rssi": -35,
                    "lsnr": 5.1,
                    "size": 3,
                    "data": "AQID"
                }
            ]
        }"#;
        let ts = DateTime::parse_from_str("2022-09-10T12:30:15Z", "%+")
            .unwrap()
            .with_timezone(&Utc);
        let mut b = vec![2, 0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        b.extend_from_slice(&mut pl_b.as_bytes());
        socket.send(&b).await.unwrap();

        // PUSH_ACK
        let size = socket.recv(&mut buffer).await.unwrap();
        assert_eq!(&[2, 0, 1, 1], &buffer[..size]);

        // MQTT up event
        let mqtt_msg = stream.next().await.unwrap().unwrap();
        assert_eq!("eu868/gateway/0102030405060708/event/up", mqtt_msg.topic());
        let pl = gw::UplinkFrame::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![1, 2, 3],
                tx_info: Some(gw::UplinkTxInfo {
                    frequency: 868100000,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 7,
                                code_rate: gw::CodeRate::Cr45.into(),
                                ..Default::default()
                            }
                        )),
                    }),
                }),
                rx_info: Some(gw::UplinkRxInfo {
                    gateway_id: "0102030405060708".into(),
                    uplink_id: 123,
                    time: Some(pbjson_types::Timestamp::from(ts.clone())),
                    rssi: -35,
                    snr: 5.1,
                    channel: 2,
                    rf_chain: 0,
                    context: vec![0, 0, 4, 210],
                    ..Default::default()
                }),
                ..Default::default()
            },
            pl
        );

        // MQTT downlink
        let pl = gw::DownlinkFrame {
            downlink_id: 1234,
            gateway_id: "0102030405060708".into(),
            items: vec![gw::DownlinkFrameItem {
                phy_payload: vec![1, 2, 3],
                tx_info: Some(gw::DownlinkTxInfo {
                    frequency: 868300000,
                    power: 16,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 8,
                                code_rate: gw::CodeRate::Cr45.into(),
                                ..Default::default()
                            },
                        )),
                    }),
                    timing: Some(gw::Timing {
                        parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                            delay: Some(pbjson_types::Duration::from(Duration::from_secs(1))),
                        })),
                    }),
                    context: vec![0, 0, 4, 210],
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let msg = mqtt::Message::new(
            "eu868/gateway/0102030405060708/command/down",
            pl.encode_to_vec(),
            0,
        );
        client.publish(msg).await.unwrap();

        // PULL_RESP
        let size = socket.recv(&mut buffer).await.unwrap();
        assert_eq!(&[2, 210, 4, 3], &buffer[..4]);
        let json = String::from_utf8_lossy(&buffer[4..size]);
        assert_eq!("{\"txpk\":{\"imme\":false,\"rfch\":0,\"powe\":16,\"ant\":0,\"brd\":0,\"tmst\":1001234,\"tmms\":null,\"freq\":868.3,\"modu\":\"LORA\",\"datr\":\"SF8BW125\",\"codr\":\"4/5\",\"fdev\":null,\"ncrc\":null,\"ipol\":false,\"prea\":null,\"size\":3,\"data\":\"AQID\"}}", json);

        // TX_ACK
        socket
            .send(&[2, 210, 4, 5, 1, 2, 3, 4, 5, 6, 7, 8])
            .await
            .unwrap();

        // MQTT ack event
        let mqtt_msg = stream.next().await.unwrap().unwrap();
        assert_eq!("eu868/gateway/0102030405060708/event/ack", mqtt_msg.topic());
        let pl = gw::DownlinkTxAck::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
        assert_eq!(
            gw::DownlinkTxAck {
                gateway_id: "0102030405060708".into(),
                downlink_id: 1234,
                items: vec![gw::DownlinkTxAckItem {
                    status: gw::TxAckStatus::Ok.into(),
                }],
                ..Default::default()
            },
            pl
        );

        // PUSH_DATA stat
        let pl_b = r#"{
            "stat": {
                "time": "2022-09-10 12:30:15 GMT",
                "lati": 46.24000,
                "long": 3.25230,
                "alti": 145,
                "rxnb": 1,
                "rxok": 1,
                "rxfw": 1,
                "ackr": 100.0,
                "dwnb": 1,
                "txnb": 1
            }
        }"#;
        let mut b = vec![2, 0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        b.extend_from_slice(&mut pl_b.as_bytes());
        socket.send(&b).await.unwrap();

        // PUSH_ACK
        let size = socket.recv(&mut buffer).await.unwrap();
        assert_eq!(&[2, 0, 1, 1], &buffer[..size]);

        // MQTT stats event
        let mqtt_msg = stream.next().await.unwrap().unwrap();
        assert_eq!(
            "eu868/gateway/0102030405060708/event/stats",
            mqtt_msg.topic()
        );
        let pl = gw::GatewayStats::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
        assert_eq!(
            gw::GatewayStats {
                gateway_id: "0102030405060708".into(),
                time: Some(pbjson_types::Timestamp::from(ts)),
                location: Some(common::Location {
                    latitude: 46.24,
                    longitude: 3.2523,
                    altitude: 145.0,
                    source: common::LocationSource::Gps.into(),
                    ..Default::default()
                }),
                rx_packets_received: 1,
                rx_packets_received_ok: 1,
                tx_packets_received: 1,
                tx_packets_emitted: 1,
                tx_packets_per_frequency: [(868300000, 1)].iter().cloned().collect(),
                rx_packets_per_frequency: [(868100000, 1)].iter().cloned().collect(),
                tx_packets_per_modulation: vec![gw::PerModulationCount {
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 8,
                                code_rate: gw::CodeRate::Cr45.into(),
                                ..Default::default()
                            }
                        )),
                    }),
                    count: 1,
                }],
                rx_packets_per_modulation: vec![gw::PerModulationCount {
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 7,
                                code_rate: gw::CodeRate::Cr45.into(),
                                ..Default::default()
                            }
                        )),
                    }),
                    count: 1,
                }],
                tx_packets_per_status: [(gw::TxAckStatus::Ok.into(), 1),].iter().cloned().collect(),
                meta_data: [
                    (
                        "mqtt_forwarder_version".to_string(),
                        env!("CARGO_PKG_VERSION").to_string()
                    ),
                    ("foo".to_string(), "bar".to_string()),
                    ("hello".to_string(), "hello world".to_string()),
                ]
                .iter()
                .cloned()
                .collect(),
                ..Default::default()
            },
            pl
        );
    }
}
