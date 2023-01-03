use std::collections::HashMap;
use std::fs;

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
}

impl Configuration {
    pub fn get(filenames: &[String]) -> Result<Configuration> {
        let mut content = String::new();
        for file_name in filenames {
            content.push_str(&fs::read_to_string(file_name)?);
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
    pub qos: usize,
    pub clean_session: bool,
    pub client_id: String,
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
    pub gateway_id: String,
    pub semtech_udp: SemtechUdp,
    pub concentratord: Concentratord,
}

impl Default for Backend {
    fn default() -> Self {
        Backend {
            enabled: "semtech_udp".to_string(),
            gateway_id: "".into(),
            semtech_udp: SemtechUdp::default(),
            concentratord: Concentratord::default(),
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
}

impl Default for SemtechUdp {
    fn default() -> Self {
        SemtechUdp {
            bind: "0.0.0.0:1700".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Metadata {
    pub r#static: HashMap<String, String>,
    pub commands: HashMap<String, Vec<String>>,
}
