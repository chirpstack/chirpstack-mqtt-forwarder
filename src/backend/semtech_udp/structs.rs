use std::collections::HashMap;
use std::convert::TryInto;
use std::time::Duration;

use anyhow::Result;
use chirpstack_api::{common, gw};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

const PROTOCOL_VERSION: u8 = 0x02;

#[derive(PartialEq, Eq, Debug)]
pub enum Crc {
    Ok,
    Invalid,
    Missing,
}

impl<'de> Deserialize<'de> for Crc {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let crc = i32::deserialize(deserializer)?;
        match crc {
            1 => Ok(Crc::Ok),
            -1 => Ok(Crc::Invalid),
            _ => Ok(Crc::Missing),
        }
    }
}

impl Serialize for Crc {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Crc::Ok => serializer.serialize_i32(1),
            Crc::Invalid => serializer.serialize_i32(-1),
            Crc::Missing => serializer.serialize_i32(0),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodeRate {
    Undefined,
    Cr45,
    Cr46,
    Cr47,
    Cr48,
    Cr38,
    Cr26,
    Cr14,
    Cr16,
    Cr56,
    CrLi45,
    CrLi46,
    CrLi48,
}

impl CodeRate {
    fn to_proto(self) -> gw::CodeRate {
        match self {
            CodeRate::Undefined => gw::CodeRate::CrUndefined,
            CodeRate::Cr45 => gw::CodeRate::Cr45,
            CodeRate::Cr46 => gw::CodeRate::Cr46,
            CodeRate::Cr47 => gw::CodeRate::Cr47,
            CodeRate::Cr48 => gw::CodeRate::Cr48,
            CodeRate::Cr38 => gw::CodeRate::Cr38,
            CodeRate::Cr26 => gw::CodeRate::Cr26,
            CodeRate::Cr14 => gw::CodeRate::Cr14,
            CodeRate::Cr16 => gw::CodeRate::Cr16,
            CodeRate::Cr56 => gw::CodeRate::Cr45,
            CodeRate::CrLi45 => gw::CodeRate::CrLi45,
            CodeRate::CrLi46 => gw::CodeRate::CrLi46,
            CodeRate::CrLi48 => gw::CodeRate::CrLi48,
        }
    }

    fn from_proto(cr: gw::CodeRate) -> Self {
        match cr {
            gw::CodeRate::CrUndefined => CodeRate::Undefined,
            gw::CodeRate::Cr45 => CodeRate::Cr45,
            gw::CodeRate::Cr46 => CodeRate::Cr46,
            gw::CodeRate::Cr47 => CodeRate::Cr47,
            gw::CodeRate::Cr48 => CodeRate::Cr48,
            gw::CodeRate::Cr38 => CodeRate::Cr38,
            gw::CodeRate::Cr26 => CodeRate::Cr26,
            gw::CodeRate::Cr14 => CodeRate::Cr14,
            gw::CodeRate::Cr16 => CodeRate::Cr16,
            gw::CodeRate::Cr56 => CodeRate::Cr56,
            gw::CodeRate::CrLi45 => CodeRate::CrLi45,
            gw::CodeRate::CrLi46 => CodeRate::CrLi46,
            gw::CodeRate::CrLi48 => CodeRate::CrLi48,
        }
    }
}

impl Serialize for CodeRate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            CodeRate::Cr45 => serializer.serialize_str("4/5"),
            CodeRate::Cr46 => serializer.serialize_str("4/6"),
            CodeRate::Cr47 => serializer.serialize_str("4/7"),
            CodeRate::Cr48 => serializer.serialize_str("4/8"),
            CodeRate::Cr38 => serializer.serialize_str("3/8"),
            CodeRate::Cr26 => serializer.serialize_str("2/6"),
            CodeRate::Cr14 => serializer.serialize_str("1/4"),
            CodeRate::Cr16 => serializer.serialize_str("1/6"),
            CodeRate::Cr56 => serializer.serialize_str("5/6"),
            CodeRate::CrLi45 => serializer.serialize_str("4/5LI"),
            CodeRate::CrLi46 => serializer.serialize_str("4/6LI"),
            CodeRate::CrLi48 => serializer.serialize_str("4/5LI"),
            _ => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for CodeRate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "4/5" => CodeRate::Cr45,
            "4/6" | "2/3" => CodeRate::Cr46,
            "4/7" => CodeRate::Cr47,
            "4/8" | "2/4" | "1/2" => CodeRate::Cr48,
            "3/8" => CodeRate::Cr38,
            "2/6" | "1/3" => CodeRate::Cr26,
            "1/4" => CodeRate::Cr14,
            "1/6" => CodeRate::Cr16,
            "5/6" => CodeRate::Cr56,
            "4/5LI" => CodeRate::CrLi45,
            "4/6LI" => CodeRate::CrLi46,
            "4/8LI" => CodeRate::CrLi48,
            _ => CodeRate::Undefined,
        })
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum DataRate {
    Lora(u32, u32), // SF and BW (Hz)
    Fsk(u32),       // bitrate
    LrFhss(u32),    // OCW (Hz)
}

impl Serialize for DataRate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            DataRate::Lora(sf, bw) => serializer.serialize_str(&format!("SF{}BW{}", sf, bw / 1000)),
            DataRate::Fsk(bitrate) => serializer.serialize_u32(*bitrate),
            DataRate::LrFhss(ocw) => serializer.serialize_str(&format!("M0CW{}", ocw / 1000)),
        }
    }
}

impl<'de> Deserialize<'de> for DataRate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::String(v) => {
                if v.starts_with("SF") {
                    let parts: Vec<&str> = v.split("BW").collect();
                    if parts.len() != 2 {
                        return Err(D::Error::custom("unexpected string"));
                    }

                    let sf = parts[0]
                        .strip_prefix("SF")
                        .ok_or_else(|| D::Error::custom("Invalid LoRa data-rate"))?;
                    let sf = sf
                        .parse::<u32>()
                        .map_err(|_| D::Error::custom("Invalid LoRa data-rate"))?;
                    let bw = parts[1]
                        .parse::<u32>()
                        .map_err(|_| D::Error::custom("Invalid LoRa data-rate"))?;

                    return Ok(DataRate::Lora(sf, bw * 1000));
                } else if v.starts_with("M0CW") {
                    let lr_fhss_cw = v
                        .strip_prefix("M0CW")
                        .ok_or_else(|| D::Error::custom("Invalid LR-FHSS data-rate"))?;
                    let lr_fhss_cw = lr_fhss_cw
                        .parse::<u32>()
                        .map_err(|_| D::Error::custom("Invalid LR-FHSS data-rate"))?;

                    return Ok(DataRate::LrFhss(lr_fhss_cw * 1000));
                }

                Err(D::Error::custom("unexpected string"))
            }
            Value::Number(v) => {
                // Fsk
                let br = v.as_u64().unwrap();
                Ok(DataRate::Fsk(br as u32))
            }
            _ => Err(D::Error::custom("unexpected type")),
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum Modulation {
    Lora,
    Fsk,
    LrFhss,
}

impl Serialize for Modulation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Modulation::Lora => serializer.serialize_str("LORA"),
            Modulation::Fsk => serializer.serialize_str("Fsk"),
            Modulation::LrFhss => serializer.serialize_str("LR-FHSS"),
        }
    }
}

impl<'de> Deserialize<'de> for Modulation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "LORA" => Ok(Modulation::Lora),
            "Fsk" => Ok(Modulation::Fsk),
            "LR-FHSS" => Ok(Modulation::LrFhss),
            _ => Err(D::Error::custom("unexpected value"))?,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct PushData {
    pub random_token: u16,
    pub gateway_id: [u8; 8],
    pub payload: PushDataPayload,
}

impl PushData {
    pub fn from_slice(b: &[u8]) -> Result<Self> {
        if b.len() < 14 {
            return Err(anyhow!("Expected at least 14 bytes, got: {}", b.len()));
        }

        if b[0] != PROTOCOL_VERSION {
            return Err(anyhow!(
                "Expected protocol version: {}, got: {}",
                PROTOCOL_VERSION,
                b[0]
            ));
        }

        if b[3] != 0x00 {
            return Err(anyhow!("Invalid identifier: {}", b[3]));
        }

        let mut random_token: [u8; 2] = [0; 2];
        random_token.copy_from_slice(&b[1..3]);
        let mut gateway_id: [u8; 8] = [0; 8];
        gateway_id.copy_from_slice(&b[4..12]);

        Ok(PushData {
            gateway_id,
            random_token: u16::from_le_bytes(random_token),
            payload: serde_json::from_slice(&b[12..])?,
        })
    }

    pub fn to_proto_uplink_frames(
        &self,
        time_fallback_enabled: bool,
    ) -> Result<Vec<gw::UplinkFrame>> {
        let mut out: Vec<gw::UplinkFrame> = vec![];
        for rx in &self.payload.rxpk {
            for f in rx.to_proto(&self.gateway_id, time_fallback_enabled)? {
                out.push(f);
            }
        }
        Ok(out)
    }

    pub fn to_proto_gateway_stats(&self) -> Result<Option<gw::GatewayStats>> {
        if let Some(s) = &self.payload.stat {
            Ok(Some(s.to_proto(&self.gateway_id)?))
        } else {
            Ok(None)
        }
    }
}

pub struct PullAck {
    pub random_token: u16,
}

impl PullAck {
    pub fn to_vec(&self) -> Vec<u8> {
        let mut b: Vec<u8> = Vec::with_capacity(4);
        b.push(PROTOCOL_VERSION);
        b.append(&mut self.random_token.to_le_bytes().to_vec());
        b.push(0x04);
        b
    }
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct PushDataPayload {
    #[serde(default)]
    pub rxpk: Vec<RxPk>,
    pub stat: Option<Stat>,
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct RxPk {
    /// UTC time of pkt RX, us precision, ISO 8601 'compact' format
    #[serde(default)]
    #[serde(with = "compact_time_format")]
    pub time: Option<DateTime<Utc>>,
    /// GPS time of pkt RX, number of milliseconds since 06.Jan.1980
    pub tmms: Option<u64>,
    /// Internal timestamp of "RX finished" event (32b unsigned)
    pub tmst: u32,
    /// Fine timestamp, number of nanoseconds since last PPS [0..999999999] (Optional).
    pub ftime: Option<u32>,
    /// RX central frequency in MHz (unsigned float, Hz precision)
    pub freq: f64,
    /// Concentrator "IF" channel used for RX (unsigned integer)
    #[serde(default)]
    pub chan: u32,
    /// Concentrator "RF chain" used for RX (unsigned integer)
    #[serde(default)]
    pub rfch: u32,
    /// Concentrator board used for RX (unsigned integer).
    #[serde(default)]
    pub brd: u32,
    /// CRC status: 1 = OK, -1 = fail, 0 = no CRC
    pub stat: Crc,
    /// Modulation identifier "LORA" or "Fsk"
    pub modu: Modulation,
    /// Lora datarate identifier (eg. SF12BW500)}
    pub datr: DataRate,
    /// Lora coding rate.
    pub codr: Option<CodeRate>,
    /// RSSI in dBm (signed integer, 1 dB precision).
    #[serde(default)]
    pub rssi: i32,
    /// Lora SNR ratio in dB (signed float, 0.1 dB precision).
    pub lsnr: Option<f32>,
    /// LR-FHSS hopping grid number of steps.
    pub hpw: Option<u8>,
    /// RF packet payload size in bytes (unsigned integer).
    pub size: u8,
    /// Base64 encoded RF packet payload, padded.
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
    #[serde(default)]
    pub rsig: Vec<RSig>,
    /// Custom meta-data (Optional, not part of PROTOCOL.TXT).
    pub meta: Option<HashMap<String, String>>,
}

impl RxPk {
    fn to_proto(
        &self,
        gateway_id: &[u8],
        time_fallback_enabled: bool,
    ) -> Result<Vec<gw::UplinkFrame>> {
        let mut rng = rand::thread_rng();
        let uplink_id = if cfg!(test) { 123 } else { rng.gen::<u32>() };

        let pl = gw::UplinkFrame {
            phy_payload: self.data.clone(),
            tx_info: Some(gw::UplinkTxInfo {
                frequency: (self.freq * 1_000_000.0) as u32,
                modulation: Some(gw::Modulation {
                    parameters: Some(match self.datr {
                        DataRate::Lora(sf, bw) => {
                            gw::modulation::Parameters::Lora(gw::LoraModulationInfo {
                                bandwidth: bw,
                                spreading_factor: sf,
                                code_rate: self
                                    .codr
                                    .ok_or_else(|| anyhow!("codr is missing"))?
                                    .to_proto()
                                    .into(),
                                ..Default::default()
                            })
                        }
                        DataRate::Fsk(bitrate) => {
                            gw::modulation::Parameters::Fsk(gw::FskModulationInfo {
                                datarate: bitrate,
                                ..Default::default()
                            })
                        }
                        DataRate::LrFhss(ocw) => {
                            gw::modulation::Parameters::LrFhss(gw::LrFhssModulationInfo {
                                operating_channel_width: ocw,
                                code_rate: self
                                    .codr
                                    .ok_or_else(|| anyhow!("codr is missing"))?
                                    .to_proto()
                                    .into(),
                                grid_steps: self
                                    .hpw
                                    .ok_or_else(|| anyhow!("hpw is missing"))?
                                    .into(),
                                ..Default::default()
                            })
                        }
                    }),
                }),
            }),
            rx_info: Some(gw::UplinkRxInfo {
                gateway_id: hex::encode(gateway_id),
                uplink_id,
                time: match self.time.map(pbjson_types::Timestamp::from) {
                    Some(v) => Some(v),
                    None => {
                        if time_fallback_enabled {
                            Some(pbjson_types::Timestamp::from(Utc::now()))
                        } else {
                            None
                        }
                    }
                },
                time_since_gps_epoch: self
                    .tmms
                    .map(|t| pbjson_types::Duration::from(Duration::from_millis(t))),
                fine_time_since_gps_epoch: {
                    if self.ftime.is_some() && self.tmms.is_some() {
                        let gps_time = Duration::from_millis(self.tmms.unwrap());
                        let f_time = Duration::from_nanos(self.ftime.unwrap().into());

                        // Truncate GPS time to seconds + add fine-timestamp fraction.
                        let f_time = Duration::from_secs(gps_time.as_secs()) + f_time;
                        Some(pbjson_types::Duration::from(f_time))
                    } else {
                        None
                    }
                },
                rssi: self.rssi,
                snr: self.lsnr.unwrap_or_default(),
                channel: self.chan,
                rf_chain: self.rfch,
                board: self.brd,
                antenna: 0,
                location: None,
                context: self.tmst.to_be_bytes().to_vec(),
                metadata: self.meta.as_ref().cloned().unwrap_or_default(),
                crc_status: match self.stat {
                    Crc::Ok => gw::CrcStatus::CrcOk,
                    Crc::Invalid => gw::CrcStatus::BadCrc,
                    Crc::Missing => gw::CrcStatus::NoCrc,
                }
                .into(),
            }),
            ..Default::default()
        };

        if self.rsig.is_empty() {
            Ok(vec![pl])
        } else {
            let mut out: Vec<gw::UplinkFrame> = vec![];
            for rs in &self.rsig {
                let uplink_id = if cfg!(test) { 123 } else { rng.gen::<u32>() };

                let mut pl = pl.clone();
                let rx_info = pl.rx_info.as_mut().unwrap();
                rx_info.uplink_id = uplink_id;

                rx_info.antenna = rs.ant.into();
                rx_info.channel = rs.chan.into();
                rx_info.rssi = rs.rssic.into();
                rx_info.snr = rs.lsnr.unwrap_or_default();

                out.push(pl);
            }
            Ok(out)
        }
    }
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct RSig {
    /// Antenna number on which signal has been received.
    pub ant: u8,
    /// Concentrator "IF" channel used for RX (unsigned integer).
    pub chan: u8,
    /// RSSI in dBm of the channel (signed integer, 1 dB precision).
    pub rssic: i16,
    /// Lora SNR ratio in dB (signed float, 0.1 dB precision).
    pub lsnr: Option<f32>,
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct Stat {
    /// UTC 'system' time of the gateway, ISO 8601 'expanded' format.
    #[serde(with = "expanded_time_format")]
    pub time: DateTime<Utc>,
    /// GPS latitude of the gateway in degree (float, N is +).
    #[serde(default)]
    pub lati: f64,
    /// GPS latitude of the gateway in degree (float, E is +).
    #[serde(default)]
    pub long: f64,
    /// GPS altitude of the gateway in meter RX (integer).
    #[serde(default)]
    pub alti: u32,
    /// Number of radio packets received (unsigned integer).
    pub rxnb: u32,
    /// Number of radio packets received with a valid PHY CRC.
    pub rxok: u32,
    /// Number of radio packets forwarded (unsigned integer).
    pub rxfw: u32,
    /// Percentage of upstream datagrams that were acknowledged.
    pub ackr: f32,
    /// Number of downlink datagrams received (unsigned integer).
    pub dwnb: u32,
    /// Number of packets emitted (unsigned integer).
    pub txnb: u32,
    /// Custom meta-data (Optional, not part of PROTOCOL.TXT).
    pub meta: Option<HashMap<String, String>>,
}

impl Stat {
    fn to_proto(&self, gateway_id: &[u8]) -> Result<gw::GatewayStats> {
        let pl = gw::GatewayStats {
            gateway_id: hex::encode(gateway_id),
            time: Some(pbjson_types::Timestamp::from(self.time)),
            location: {
                if self.lati != 0.0 || self.long != 0.0 || self.alti != 0 {
                    Some(common::Location {
                        latitude: self.lati,
                        longitude: self.long,
                        altitude: self.alti.into(),
                        source: common::LocationSource::Gps.into(),
                        ..Default::default()
                    })
                } else {
                    None
                }
            },
            rx_packets_received: self.rxnb,
            rx_packets_received_ok: self.rxok,
            tx_packets_received: self.dwnb,
            tx_packets_emitted: self.txnb,
            metadata: self.meta.as_ref().cloned().unwrap_or_default(),
            ..Default::default()
        };

        Ok(pl)
    }
}

pub struct PushAck {
    pub random_token: u16,
}

impl PushAck {
    pub fn to_vec(&self) -> Vec<u8> {
        let mut b: Vec<u8> = Vec::with_capacity(4);
        b.push(PROTOCOL_VERSION);
        b.append(&mut self.random_token.to_le_bytes().to_vec());
        b.push(0x01);
        b
    }
}

pub struct PullData {
    pub random_token: u16,
    pub gateway_id: [u8; 8],
}

impl PullData {
    pub fn from_slice(b: &[u8]) -> Result<Self> {
        if b.len() != 12 {
            return Err(anyhow!("Expected exactly 12 bytes, got: {}", b.len()));
        }

        if b[0] != PROTOCOL_VERSION {
            return Err(anyhow!(
                "Expected protocol version: {}, got: {}",
                PROTOCOL_VERSION,
                b[0]
            ));
        }

        if b[3] != 0x02 {
            return Err(anyhow!("Invalid identifier: {}", b[3]));
        }

        let mut random_token: [u8; 2] = [0; 2];
        random_token.copy_from_slice(&b[1..3]);
        let mut gateway_id: [u8; 8] = [0; 8];
        gateway_id.copy_from_slice(&b[4..12]);

        Ok(PullData {
            gateway_id,
            random_token: u16::from_le_bytes(random_token),
        })
    }
}

#[derive(PartialEq, Debug)]
pub struct PullResp {
    pub random_token: u16,
    pub payload: PullRespPayload,
}

impl PullResp {
    pub fn from_proto(pl: &gw::DownlinkFrame, index: usize, random_token: u16) -> Result<Self> {
        let item = pl
            .items
            .get(index)
            .ok_or_else(|| anyhow!("Invalid index"))?;
        let tx_info = item
            .tx_info
            .as_ref()
            .ok_or_else(|| anyhow!("tx_info is missing"))?;
        let timing = tx_info
            .timing
            .as_ref()
            .ok_or_else(|| anyhow!("tx_info.timing is missing"))?;
        let timing_params = timing
            .parameters
            .as_ref()
            .ok_or_else(|| anyhow!("tx_info.timing.parameters is missing"))?;
        let modulation = tx_info
            .modulation
            .as_ref()
            .ok_or_else(|| anyhow!("tx_info.modulation is missing"))?;
        let modulation_params = modulation
            .parameters
            .as_ref()
            .ok_or_else(|| anyhow!("tx_info.modulation.parameters is missing"))?;

        Ok(PullResp {
            random_token,
            payload: PullRespPayload {
                txpk: TxPk {
                    imme: matches!(
                        timing.parameters,
                        Some(gw::timing::Parameters::Immediately(_))
                    ),
                    rfch: 0,
                    powe: tx_info.power as u8,
                    ant: tx_info.antenna as u8,
                    brd: tx_info.board as u8,
                    tmst: match timing_params {
                        gw::timing::Parameters::Delay(v) => {
                            if tx_info.context.len() < 4 {
                                return Err(anyhow!("context must contain at least 4 bytes"));
                            }

                            let mut timestamp: [u8; 4] = [0; 4];
                            timestamp.copy_from_slice(&tx_info.context[0..4]);
                            let mut timestamp = u32::from_be_bytes(timestamp);

                            let delay = v
                                .delay
                                .as_ref()
                                .ok_or_else(|| anyhow!("delay is missing"))?
                                .clone();
                            let delay: Duration = delay.try_into()?;
                            timestamp += delay.as_micros() as u32;
                            Some(timestamp)
                        }
                        _ => None,
                    },
                    tmms: match timing_params {
                        gw::timing::Parameters::GpsEpoch(v) => {
                            let gps_time = v
                                .time_since_gps_epoch
                                .as_ref()
                                .ok_or_else(|| anyhow!("time_since_gps_epoch is missing"))?
                                .clone();
                            let gps_time: Duration = gps_time.try_into()?;
                            Some(gps_time.as_millis() as u64)
                        }
                        _ => None,
                    },
                    freq: (tx_info.frequency as f64) / 1_000_000.0,
                    modu: match modulation_params {
                        gw::modulation::Parameters::Lora(_) => Modulation::Lora,
                        gw::modulation::Parameters::Fsk(_) => Modulation::Fsk,
                        gw::modulation::Parameters::LrFhss(_) => Modulation::LrFhss,
                    },
                    datr: match modulation_params {
                        gw::modulation::Parameters::Lora(v) => {
                            DataRate::Lora(v.spreading_factor, v.bandwidth)
                        }
                        gw::modulation::Parameters::Fsk(v) => DataRate::Fsk(v.datarate),
                        gw::modulation::Parameters::LrFhss(v) => {
                            DataRate::LrFhss(v.operating_channel_width)
                        }
                    },
                    codr: match modulation_params {
                        gw::modulation::Parameters::Lora(v) => {
                            Some(CodeRate::from_proto(v.code_rate()))
                        }
                        _ => None,
                    },
                    fdev: match modulation_params {
                        gw::modulation::Parameters::Fsk(v) => Some(v.frequency_deviation as u16),
                        _ => None,
                    },
                    ncrc: None,
                    ipol: match modulation_params {
                        gw::modulation::Parameters::Lora(v) => Some(v.polarization_inversion),
                        _ => None,
                    },
                    prea: None,
                    size: item.phy_payload.len() as u16,
                    data: item.phy_payload.clone(),
                },
            },
        })
    }

    pub fn to_vec(&self) -> Result<Vec<u8>> {
        let mut b: Vec<u8> = Vec::new();
        b.push(PROTOCOL_VERSION);
        b.append(&mut self.random_token.to_le_bytes().to_vec());
        b.push(0x03);
        b.append(&mut serde_json::to_vec(&self.payload)?);
        Ok(b)
    }
}

#[derive(Serialize, PartialEq, Debug)]
pub struct PullRespPayload {
    pub txpk: TxPk,
}

#[derive(Serialize, PartialEq, Debug)]
pub struct TxPk {
    /// Send packet immediately (will ignore tmst & time).
    pub imme: bool,
    /// Concentrator "RF chain" used for TX (unsigned integer).
    pub rfch: u8,
    /// TX output power in dBm (unsigned integer, dBm precision).
    pub powe: u8,
    /// Antenna number on which signal has been received.
    pub ant: u8,
    /// Concentrator board used for RX (unsigned integer).
    pub brd: u8,
    /// Send packet on a certain timestamp value (will ignore time).
    pub tmst: Option<u32>,
    /// Send packet at a certain GPS time (GPS synchronization required).
    pub tmms: Option<u64>,
    /// TX central frequency in MHz (unsigned float, Hz precision).
    pub freq: f64,
    /// Modulation identifier "LORA" or "FSK".
    pub modu: Modulation,
    /// LoRa datarate identifier (eg. SF12BW500) || FSK datarate (unsigned, in bits per second).
    pub datr: DataRate,
    /// LoRa ECC coding rate identifier.
    pub codr: Option<CodeRate>,
    /// FSK frequency deviation (unsigned integer, in Hz).
    pub fdev: Option<u16>,
    /// If true, disable the CRC of the physical layer (optional).
    pub ncrc: Option<bool>,
    /// Lora modulation polarization inversion.
    pub ipol: Option<bool>,
    /// RF preamble size (unsigned integer).
    pub prea: Option<u16>,
    /// RF packet payload size in bytes (unsigned integer).
    pub size: u16,
    /// Base64 encoded RF packet payload, padding optional.
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

pub struct TxAck {
    pub random_token: u16,
    pub gateway_id: [u8; 8],
    pub payload: Option<TxAckPayload>,
}

impl TxAck {
    pub fn from_slice(b: &[u8]) -> Result<Self> {
        if b.len() < 12 {
            return Err(anyhow!("Expected at least 12 bytes, got: {}", b.len()));
        }

        if b[0] != PROTOCOL_VERSION {
            return Err(anyhow!(
                "Expected protocol version: {}, got: {}",
                PROTOCOL_VERSION,
                b[0]
            ));
        }

        if b[3] != 0x05 {
            return Err(anyhow!("Invalid identifier: {}", b[3]));
        }

        let mut random_token: [u8; 2] = [0; 2];
        random_token.copy_from_slice(&b[1..3]);
        let mut gateway_id: [u8; 8] = [0; 8];
        gateway_id.copy_from_slice(&b[4..12]);

        Ok(TxAck {
            gateway_id,
            random_token: u16::from_le_bytes(random_token),
            payload: if b.len() >= 14 {
                Some(serde_json::from_slice(&b[12..])?)
            } else {
                None
            },
        })
    }

    pub fn to_proto_tx_ack_status(&self) -> gw::TxAckStatus {
        if let Some(pl) = &self.payload {
            match pl.txpk_ack.error.as_ref() {
                "" | "NONE" => gw::TxAckStatus::Ok,
                "TOO_LATE" => gw::TxAckStatus::TooLate,
                "TOO_EARLY" => gw::TxAckStatus::TooEarly,
                "COLLISION_PACKET" => gw::TxAckStatus::CollisionPacket,
                "COLLISION_BEACON" => gw::TxAckStatus::CollisionBeacon,
                "TX_FREQ" => gw::TxAckStatus::TxFreq,
                "TX_POWER" => gw::TxAckStatus::TxPower,
                "GPS_UNLOCKED" => gw::TxAckStatus::GpsUnlocked,
                _ => gw::TxAckStatus::InternalError,
            }
        } else {
            gw::TxAckStatus::Ok
        }
    }
}

#[derive(Deserialize, Default, Clone)]
pub struct TxAckPayload {
    pub txpk_ack: TxPkAck,
}

#[derive(Deserialize, Default, Clone)]
pub struct TxPkAck {
    #[serde(default)]
    pub error: String,
}

mod compact_time_format {
    use chrono::{DateTime, Utc};
    use serde::Deserializer;

    pub fn deserialize<'a, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'a>,
    {
        let o: Option<&str> = serde::de::Deserialize::deserialize(deserializer)?;
        match o {
            Some(v) => {
                let d = DateTime::parse_from_str(v, "%+").map_err(serde::de::Error::custom)?;
                Ok(Some(d.with_timezone(&Utc)))
            }
            None => Ok(None),
        }
    }
}

mod expanded_time_format {
    use chrono::{DateTime, NaiveDateTime, Utc};
    use serde::Deserializer;

    pub fn deserialize<'a, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'a>,
    {
        let s: &str = serde::de::Deserialize::deserialize(deserializer)?;
        let d = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S %Z")
            .map_err(serde::de::Error::custom)?;
        Ok(d.and_local_timezone(Utc).unwrap())
    }
}

mod base64_bytes {
    use base64::{engine::general_purpose, Engine as _};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(b: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = general_purpose::STANDARD.encode(b);
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'a, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'a>,
    {
        let s: &str = serde::de::Deserialize::deserialize(deserializer)?;
        general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_push_data() {
        let b = vec![2, 123, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 123, 125];
        let pl = PushData::from_slice(&b).unwrap();
        assert_eq!(
            pl,
            PushData {
                gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
                random_token: 123,
                payload: PushDataPayload {
                    rxpk: vec![],
                    stat: None,
                },
            }
        )
    }

    #[test]
    fn test_gateway_stats_no_location() {
        let now = Utc::now();
        let stat = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                stat: Some(Stat {
                    time: now,
                    lati: 0.0,
                    long: 0.0,
                    alti: 0,
                    rxnb: 10,
                    rxok: 5,
                    rxfw: 5,
                    dwnb: 30,
                    txnb: 15,
                    ackr: 99.9,
                    meta: None,
                }),
                rxpk: vec![],
            },
        };
        let stat = stat.to_proto_gateway_stats().unwrap();
        assert!(stat.is_some());
        assert_eq!(
            stat.unwrap(),
            gw::GatewayStats {
                gateway_id: "0102030405060708".to_string(),
                time: Some(pbjson_types::Timestamp::from(now)),
                location: None,
                rx_packets_received: 10,
                rx_packets_received_ok: 5,
                tx_packets_received: 30,
                tx_packets_emitted: 15,
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_gateway_stats_location() {
        let now = Utc::now();
        let stat = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                stat: Some(Stat {
                    time: now,
                    lati: 1.1,
                    long: 2.2,
                    alti: 3,
                    rxnb: 10,
                    rxok: 5,
                    rxfw: 5,
                    dwnb: 30,
                    txnb: 15,
                    ackr: 99.9,
                    meta: None,
                }),
                rxpk: vec![],
            },
        };
        let stat = stat.to_proto_gateway_stats().unwrap();
        assert!(stat.is_some());
        assert_eq!(
            stat.unwrap(),
            gw::GatewayStats {
                gateway_id: "0102030405060708".to_string(),
                time: Some(pbjson_types::Timestamp::from(now)),
                location: Some(common::Location {
                    latitude: 1.1,
                    longitude: 2.2,
                    altitude: 3.0,
                    source: common::LocationSource::Gps.into(),
                    ..Default::default()
                }),
                rx_packets_received: 10,
                rx_packets_received_ok: 5,
                tx_packets_received: 30,
                tx_packets_emitted: 15,
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_gateway_stats_meta() {
        let now = Utc::now();
        let stat = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                stat: Some(Stat {
                    time: now,
                    lati: 0.0,
                    long: 0.0,
                    alti: 0,
                    rxnb: 10,
                    rxok: 5,
                    rxfw: 5,
                    dwnb: 30,
                    txnb: 15,
                    ackr: 99.9,
                    meta: Some(
                        [("gateway_name".to_string(), "test-gateway".to_string())]
                            .iter()
                            .cloned()
                            .collect(),
                    ),
                }),
                rxpk: vec![],
            },
        };
        let stat = stat.to_proto_gateway_stats().unwrap();
        assert!(stat.is_some());
        assert_eq!(
            stat.unwrap(),
            gw::GatewayStats {
                gateway_id: "0102030405060708".to_string(),
                time: Some(pbjson_types::Timestamp::from(now)),
                location: None,
                rx_packets_received: 10,
                rx_packets_received_ok: 5,
                tx_packets_received: 30,
                tx_packets_emitted: 15,
                metadata: [("gateway_name".to_string(), "test-gateway".to_string())]
                    .iter()
                    .cloned()
                    .collect(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_uplink_frame_no_pl() {
        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![],
                stat: None,
            },
        };
        assert_eq!(0, pl.to_proto_uplink_frames(false).unwrap().len());
    }

    #[test]
    fn test_uplink_frame_lora() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 120,
                    lsnr: Some(3.5),
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![4, 3, 2, 1],
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
                    time: Some(pbjson_types::Timestamp::from(now)),
                    rssi: 120,
                    snr: 3.5,
                    channel: 5,
                    rf_chain: 1,
                    board: 3,
                    context: vec![0, 0, 4, 210],
                    crc_status: gw::CrcStatus::CrcOk.into(),
                    ..Default::default()
                }),

                ..Default::default()
            },
            pl[0]
        );
    }

    #[test]
    fn test_uplink_frame_lora_gps_time() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: Some(5_000),
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 120,
                    lsnr: Some(3.5),
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![4, 3, 2, 1],
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
                    time: Some(pbjson_types::Timestamp::from(now)),
                    time_since_gps_epoch: Some(pbjson_types::Duration::from(Duration::from_secs(
                        5
                    ))),
                    rssi: 120,
                    snr: 3.5,
                    channel: 5,
                    rf_chain: 1,
                    board: 3,
                    context: vec![0, 0, 4, 210],
                    crc_status: gw::CrcStatus::CrcOk.into(),
                    ..Default::default()
                }),

                ..Default::default()
            },
            pl[0]
        );
    }

    #[test]
    fn test_uplink_frame_lora_gps_and_fine_timestamp() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: Some(5_100),
                    tmst: 1234,
                    ftime: Some(3_000_000), // 3 milliseconds
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 120,
                    lsnr: Some(3.5),
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![4, 3, 2, 1],
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
                    time: Some(pbjson_types::Timestamp::from(now)),
                    time_since_gps_epoch: Some(pbjson_types::Duration::from(
                        Duration::from_millis(5_100)
                    )),
                    fine_time_since_gps_epoch: Some(pbjson_types::Duration::from(
                        Duration::from_millis(5_003)
                    )),
                    rssi: 120,
                    snr: 3.5,
                    channel: 5,
                    rf_chain: 1,
                    board: 3,
                    context: vec![0, 0, 4, 210],
                    crc_status: gw::CrcStatus::CrcOk.into(),
                    ..Default::default()
                }),

                ..Default::default()
            },
            pl[0]
        );
    }

    #[test]
    fn test_uplink_frame_lora_rsig() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 0,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 0,
                    lsnr: None,
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![
                        RSig {
                            ant: 1,
                            chan: 5,
                            rssic: 120,
                            lsnr: Some(5.4),
                        },
                        RSig {
                            ant: 2,
                            chan: 6,
                            rssic: 130,
                            lsnr: Some(3.5),
                        },
                    ],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(2, pl.len());
        assert_eq!(
            vec![
                gw::UplinkFrame {
                    phy_payload: vec![4, 3, 2, 1],
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
                        time: Some(pbjson_types::Timestamp::from(now)),
                        rssi: 120,
                        snr: 5.4,
                        channel: 5,
                        antenna: 1,
                        rf_chain: 1,
                        board: 3,
                        context: vec![0, 0, 4, 210],
                        crc_status: gw::CrcStatus::CrcOk.into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                gw::UplinkFrame {
                    phy_payload: vec![4, 3, 2, 1],
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
                        time: Some(pbjson_types::Timestamp::from(now)),
                        rssi: 130,
                        snr: 3.5,
                        channel: 6,
                        antenna: 2,
                        rf_chain: 1,
                        board: 3,
                        context: vec![0, 0, 4, 210],
                        crc_status: gw::CrcStatus::CrcOk.into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            pl
        );
    }

    #[test]
    fn test_uplink_frame_fsk() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Fsk,
                    datr: DataRate::Fsk(50_000),
                    codr: None,
                    rssi: 120,
                    lsnr: None,
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![4, 3, 2, 1],
                tx_info: Some(gw::UplinkTxInfo {
                    frequency: 868100000,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Fsk(gw::FskModulationInfo {
                            datarate: 50_000,
                            ..Default::default()
                        })),
                    }),
                }),
                rx_info: Some(gw::UplinkRxInfo {
                    gateway_id: "0102030405060708".into(),
                    uplink_id: 123,
                    time: Some(pbjson_types::Timestamp::from(now)),
                    rssi: 120,
                    channel: 5,
                    rf_chain: 1,
                    board: 3,
                    context: vec![0, 0, 4, 210],
                    crc_status: gw::CrcStatus::CrcOk.into(),
                    ..Default::default()
                }),

                ..Default::default()
            },
            pl[0]
        );
    }

    #[test]
    fn test_uplink_frame_lr_fhss() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::LrFhss,
                    datr: DataRate::LrFhss(137_000),
                    codr: Some(CodeRate::Cr46),
                    rssi: 120,
                    lsnr: None,
                    hpw: Some(8),
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![4, 3, 2, 1],
                tx_info: Some(gw::UplinkTxInfo {
                    frequency: 868100000,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::LrFhss(
                            gw::LrFhssModulationInfo {
                                operating_channel_width: 137_000,
                                code_rate: gw::CodeRate::Cr46.into(),
                                grid_steps: 8,
                                ..Default::default()
                            }
                        )),
                    }),
                }),
                rx_info: Some(gw::UplinkRxInfo {
                    gateway_id: "0102030405060708".into(),
                    uplink_id: 123,
                    time: Some(pbjson_types::Timestamp::from(now)),
                    rssi: 120,
                    channel: 5,
                    rf_chain: 1,
                    board: 3,
                    context: vec![0, 0, 4, 210],
                    crc_status: gw::CrcStatus::CrcOk.into(),
                    ..Default::default()
                }),

                ..Default::default()
            },
            pl[0]
        );
    }

    #[test]
    fn test_uplink_frame_lora_with_meta() {
        let now = Utc::now();

        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: Some(now),
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 120,
                    lsnr: Some(3.5),
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: Some(
                        [("gateway_name".to_string(), "test-gateway".to_string())]
                            .iter()
                            .cloned()
                            .collect(),
                    ),
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert_eq!(
            gw::UplinkFrame {
                phy_payload: vec![4, 3, 2, 1],
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
                    time: Some(pbjson_types::Timestamp::from(now)),
                    rssi: 120,
                    snr: 3.5,
                    channel: 5,
                    rf_chain: 1,
                    board: 3,
                    context: vec![0, 0, 4, 210],
                    metadata: [("gateway_name".to_string(), "test-gateway".to_string())]
                        .iter()
                        .cloned()
                        .collect(),
                    crc_status: gw::CrcStatus::CrcOk.into(),
                    ..Default::default()
                }),

                ..Default::default()
            },
            pl[0]
        );
    }

    #[test]
    fn test_uplink_no_time() {
        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: None,
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 120,
                    lsnr: Some(3.5),
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(false).unwrap();
        assert_eq!(1, pl.len());
        assert!(pl[0].rx_info.as_ref().unwrap().time.is_none());
    }

    #[test]
    fn test_uplink_no_time_time_fallback_enabled() {
        let pl = PushData {
            random_token: 123,
            gateway_id: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: PushDataPayload {
                rxpk: vec![RxPk {
                    time: None,
                    tmms: None,
                    tmst: 1234,
                    ftime: None,
                    freq: 868.1,
                    chan: 5,
                    rfch: 1,
                    brd: 3,
                    stat: Crc::Ok,
                    modu: Modulation::Lora,
                    datr: DataRate::Lora(7, 125000),
                    codr: Some(CodeRate::Cr45),
                    rssi: 120,
                    lsnr: Some(3.5),
                    hpw: None,
                    size: 4,
                    data: vec![4, 3, 2, 1],
                    rsig: vec![],
                    meta: None,
                }],
                stat: None,
            },
        };
        let pl = pl.to_proto_uplink_frames(true).unwrap();
        assert_eq!(1, pl.len());
        assert!(pl[0].rx_info.as_ref().unwrap().time.is_some());
    }

    #[test]
    fn test_downlink_lora_delay() {
        let pl = gw::DownlinkFrame {
            items: vec![gw::DownlinkFrameItem {
                phy_payload: vec![1, 2, 3, 4],
                tx_info: Some(gw::DownlinkTxInfo {
                    frequency: 868100000,
                    power: 16,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 7,
                                code_rate: gw::CodeRate::Cr45.into(),
                                polarization_inversion: true,
                                ..Default::default()
                            },
                        )),
                    }),
                    board: 1,
                    antenna: 2,
                    timing: Some(gw::Timing {
                        parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                            delay: Some(pbjson_types::Duration::from(Duration::from_secs(1))),
                        })),
                    }),
                    context: vec![0, 0, 0, 1],
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let pl = PullResp::from_proto(&pl, 0, 4321).unwrap();
        assert_eq!(
            PullResp {
                random_token: 4321,
                payload: PullRespPayload {
                    txpk: TxPk {
                        imme: false,
                        rfch: 0,
                        powe: 16,
                        ant: 2,
                        brd: 1,
                        tmst: Some(1_000_001),
                        tmms: None,
                        freq: 868.1,
                        modu: Modulation::Lora,
                        datr: DataRate::Lora(7, 125_000),
                        codr: Some(CodeRate::Cr45),
                        fdev: None,
                        ncrc: None,
                        ipol: Some(true),
                        prea: None,
                        size: 4,
                        data: vec![1, 2, 3, 4],
                    },
                },
            },
            pl
        );
    }

    #[test]
    fn test_downlink_lora_gps_time() {
        let pl = gw::DownlinkFrame {
            items: vec![gw::DownlinkFrameItem {
                phy_payload: vec![1, 2, 3, 4],
                tx_info: Some(gw::DownlinkTxInfo {
                    frequency: 868100000,
                    power: 16,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 7,
                                code_rate: gw::CodeRate::Cr45.into(),
                                polarization_inversion: true,
                                ..Default::default()
                            },
                        )),
                    }),
                    board: 1,
                    antenna: 2,
                    timing: Some(gw::Timing {
                        parameters: Some(gw::timing::Parameters::GpsEpoch(
                            gw::GpsEpochTimingInfo {
                                time_since_gps_epoch: Some(pbjson_types::Duration::from(
                                    Duration::from_secs(5),
                                )),
                            },
                        )),
                    }),
                    context: vec![0, 0, 0, 1],
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let pl = PullResp::from_proto(&pl, 0, 4321).unwrap();
        assert_eq!(
            PullResp {
                random_token: 4321,
                payload: PullRespPayload {
                    txpk: TxPk {
                        imme: false,
                        rfch: 0,
                        powe: 16,
                        ant: 2,
                        brd: 1,
                        tmst: None,
                        tmms: Some(5_000),
                        freq: 868.1,
                        modu: Modulation::Lora,
                        datr: DataRate::Lora(7, 125_000),
                        codr: Some(CodeRate::Cr45),
                        fdev: None,
                        ncrc: None,
                        ipol: Some(true),
                        prea: None,
                        size: 4,
                        data: vec![1, 2, 3, 4],
                    },
                },
            },
            pl
        );
    }

    #[test]
    fn test_downlink_lora_immediately() {
        let pl = gw::DownlinkFrame {
            items: vec![gw::DownlinkFrameItem {
                phy_payload: vec![1, 2, 3, 4],
                tx_info: Some(gw::DownlinkTxInfo {
                    frequency: 868100000,
                    power: 16,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: 125000,
                                spreading_factor: 7,
                                code_rate: gw::CodeRate::Cr45.into(),
                                polarization_inversion: true,
                                ..Default::default()
                            },
                        )),
                    }),
                    board: 1,
                    antenna: 2,
                    timing: Some(gw::Timing {
                        parameters: Some(gw::timing::Parameters::Immediately(
                            gw::ImmediatelyTimingInfo {},
                        )),
                    }),
                    context: vec![0, 0, 0, 1],
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let pl = PullResp::from_proto(&pl, 0, 4321).unwrap();
        assert_eq!(
            PullResp {
                random_token: 4321,
                payload: PullRespPayload {
                    txpk: TxPk {
                        imme: true,
                        rfch: 0,
                        powe: 16,
                        ant: 2,
                        brd: 1,
                        tmst: None,
                        tmms: None,
                        freq: 868.1,
                        modu: Modulation::Lora,
                        datr: DataRate::Lora(7, 125_000),
                        codr: Some(CodeRate::Cr45),
                        fdev: None,
                        ncrc: None,
                        ipol: Some(true),
                        prea: None,
                        size: 4,
                        data: vec![1, 2, 3, 4],
                    },
                },
            },
            pl
        );
    }

    #[test]
    fn test_downlink_fsk_delay() {
        let pl = gw::DownlinkFrame {
            items: vec![gw::DownlinkFrameItem {
                phy_payload: vec![1, 2, 3, 4],
                tx_info: Some(gw::DownlinkTxInfo {
                    frequency: 868100000,
                    power: 16,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Fsk(gw::FskModulationInfo {
                            datarate: 50_000,
                            frequency_deviation: 25_000,
                        })),
                    }),
                    board: 1,
                    antenna: 2,
                    timing: Some(gw::Timing {
                        parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                            delay: Some(pbjson_types::Duration::from(Duration::from_secs(1))),
                        })),
                    }),
                    context: vec![0, 0, 0, 1],
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let pl = PullResp::from_proto(&pl, 0, 4321).unwrap();
        assert_eq!(
            PullResp {
                random_token: 4321,
                payload: PullRespPayload {
                    txpk: TxPk {
                        imme: false,
                        rfch: 0,
                        powe: 16,
                        ant: 2,
                        brd: 1,
                        tmst: Some(1_000_001),
                        tmms: None,
                        freq: 868.1,
                        modu: Modulation::Fsk,
                        datr: DataRate::Fsk(50_000),
                        codr: None,
                        fdev: Some(25_000),
                        ncrc: None,
                        ipol: None,
                        prea: None,
                        size: 4,
                        data: vec![1, 2, 3, 4],
                    },
                },
            },
            pl
        );
    }
}
