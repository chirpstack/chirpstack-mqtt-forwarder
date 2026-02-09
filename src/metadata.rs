use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use anyhow::Result;
use log::{error, warn};
use tokio::process::Command;

use crate::config::Configuration;

static METADATA: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static COMMANDS: LazyLock<RwLock<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static SPLIT_DELIMITER: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new("=".to_string())); 

pub fn setup(conf: &Configuration) -> Result<()> {
    let mut metadata_w = METADATA.write().unwrap();
    let mut commands_w = COMMANDS.write().unwrap();
    let mut split_delimiter_w = SPLIT_DELIMITER.write().unwrap(); // Access the delimiter
    metadata_w.clone_from(&conf.metadata.r#static);
    commands_w.clone_from(&conf.metadata.commands);
    *split_delimiter_w = conf.metadata.split_delimiter.clone();

    Ok(())
}

pub async fn get() -> Result<HashMap<String, String>> {
    let mut metadata = METADATA.read().unwrap().clone();
    metadata.insert(
        "mqtt_forwarder_version".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    let commands = COMMANDS.read().unwrap().clone();
    let split_delimiter = SPLIT_DELIMITER.read().unwrap().clone();

    for (k, v) in commands {
        if v.is_empty() {
            error!("Metadata has no command: {}", k);
            continue;
        }

        let mut cmd = Command::new(&v[0]);
        if v.len() > 1 {
            cmd.args(&v[1..]);
        }

        let out = cmd.output().await?;
        if !out.status.success() {
            error!("Metadata command execution failed, command: {}", v[0]);
            continue;
        }

        let out = String::from_utf8(out.stdout)?;
        let lines: Vec<&str> = out.lines().collect();

        if lines.len() > 1 {
             for line in lines {
                 if let Some((key, value)) = line.split_once(&split_delimiter) {
                     let prefixed_key = format!("{}_{}", k, key.trim());
                     metadata.insert(prefixed_key, value.trim().to_string());
                 } else {
                     warn!("Multi-line command output, but no delimiter {} detected: {}", split_delimiter, line);
                 }
             }
        } else if let Some(line) = lines.first() {
            if let Some((key, value)) = line.split_once(&split_delimiter) {
                let prefixed_key = format!("{}_{}", k, key.trim());
                metadata.insert(prefixed_key, value.trim().to_string());
            } else {
                metadata.insert(k.to_string(), line.trim().to_string());
            }
        }
    }

    Ok(metadata)
}
