use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use anyhow::Result;
use log::error;
use tokio::process::Command;

use crate::config::Configuration;

static METADATA: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static COMMANDS: LazyLock<RwLock<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static DYNAMIC_COMMANDS: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static SPLIT_DELIMITER: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new("=".to_string()));

pub fn setup(conf: &Configuration) -> Result<()> {
    let mut metadata_w = METADATA.write().unwrap();
    let mut commands_w = COMMANDS.write().unwrap();
    let mut dynamic_commands_w = DYNAMIC_COMMANDS.write().unwrap();
    let mut split_delimiter_w = SPLIT_DELIMITER.write().unwrap();
    metadata_w.clone_from(&conf.metadata.r#static);
    commands_w.clone_from(&conf.metadata.commands);
    dynamic_commands_w.clone_from(&conf.metadata.dynamic.commands);
    *split_delimiter_w = conf.metadata.dynamic.split_delimiter.clone();

    Ok(())
}

pub async fn get() -> Result<HashMap<String, String>> {
    let mut metadata = METADATA.read().unwrap().clone();
    metadata.insert(
        "mqtt_forwarder_version".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    let commands = COMMANDS.read().unwrap().clone();
    let dynamic_commands = DYNAMIC_COMMANDS.read().unwrap().clone();
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
        metadata.insert(k.to_string(), out.trim().to_string());
    }

    for (k, v) in dynamic_commands {
        let mut cmd = Command::new(&v);
        let out = cmd.output().await?;

        if !out.status.success() {
            error!("Dynamic metadata command execution failed, command: {}", v);
            continue;
        }

        let out = String::from_utf8(out.stdout)?;
        for line in out.lines() {
            if let Some((key, value)) = line.split_once(&split_delimiter) {
                let prefixed_key = format!("{}_{}", k, key.trim());
                metadata.insert(prefixed_key, value.trim().to_string());
            } else {
                warn!(
                    "Dynamic metadata command output invalid (missing delimiter '{}'): {}",
                    split_delimiter, line
                );
            }
        }
    }

    Ok(metadata)
}
