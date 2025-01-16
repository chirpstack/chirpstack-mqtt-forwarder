use std::collections::HashMap;
use std::time::Duration;
use std::{env, fs};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Configuration {
    pub logging: Logging,
    pub mqtt: Mqtt,
    pub backend: Backend,
    pub metadata: Metadata,
    pub commands: HashMap<String, Vec<String>>,
    pub callbacks: Callbacks,
    pub gateway: Gateway,
}

impl Configuration {
    pub fn get(filenames: &[String]) -> Result<Configuration> {
        let mut content = String::new();

        for file_name in filenames {
            content.push_str(&fs::read_to_string(file_name)?);
        }

        // Replace environment variables in config.
        for (k, v) in env::vars() {
            content = content.replace(&format!("${}", k), &v);
        }

        let config: Configuration = toml::from_str(&content)?;
        Ok(config)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Logging {
    pub level: String,
    pub log_to_syslog: bool,
}

impl Default for Logging {
    fn default() -> Self {
        Logging {
            level: "info".to_string(),
            log_to_syslog: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Mqtt {
    pub topic_prefix: String,
    pub json: bool,
    pub server: String,
    pub username: String,
    pub password: String,
    pub qos: u8,
    pub clean_session: bool,
    pub client_id: String,
    #[serde(with = "humantime_serde")]
    pub keep_alive_interval: Duration,
    pub ca_cert: String,
    pub tls_cert: String,
    pub tls_key: String,
}

impl Default for Mqtt {
    fn default() -> Self {
        Mqtt {
            topic_prefix: "eu868".into(),
            json: false,
            server: "tcp://127.0.0.1:1883".into(),
            username: "".into(),
            password: "".into(),
            qos: 0,
            clean_session: false,
            client_id: "".into(),
            keep_alive_interval: Duration::from_secs(30),
            ca_cert: "".into(),
            tls_cert: "".into(),
            tls_key: "".into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Backend {
    pub enabled: String,
    pub filters: Filters,
    pub gateway_id: String,
    pub semtech_udp: SemtechUdp,
    pub concentratord: Concentratord,
}

impl Default for Backend {
    fn default() -> Self {
        Backend {
            enabled: "semtech_udp".to_string(),
            filters: Filters::default(),
            gateway_id: "".into(),
            semtech_udp: SemtechUdp::default(),
            concentratord: Concentratord::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub forward_crc_ok: bool,
    pub forward_crc_invalid: bool,
    pub forward_crc_missing: bool,
    pub dev_addr_prefixes: Vec<lrwn_filters::DevAddrPrefix>,
    pub join_eui_prefixes: Vec<lrwn_filters::EuiPrefix>,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            forward_crc_ok: true,
            forward_crc_invalid: false,
            forward_crc_missing: false,
            dev_addr_prefixes: vec![],
            join_eui_prefixes: vec![],
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Concentratord {
    pub event_url: String,
    pub command_url: String,
}

impl Default for Concentratord {
    fn default() -> Self {
        Concentratord {
            event_url: "ipc:///tmp/concentratord_event".into(),
            command_url: "ipc:///tmp/concentratord_command".into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SemtechUdp {
    pub bind: String,
    pub time_fallback_enabled: bool,
}

impl Default for SemtechUdp {
    fn default() -> Self {
        SemtechUdp {
            bind: "0.0.0.0:1700".to_string(),
            time_fallback_enabled: false,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Metadata {
    pub r#static: HashMap<String, String>,
    pub commands: HashMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Callbacks {
    pub on_mqtt_connected: Vec<String>,
    pub on_mqtt_connection_error: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Gateway {
    pub gateway_id: Option<String>,
}

impl Default for Gateway {
    fn default() -> Self {
        Gateway {
            gateway_id: None,
        }
    }
}
