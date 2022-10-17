use std::collections::HashMap;

use anyhow::Result;
use log::error;
use once_cell::sync::OnceCell;
use tokio::process::Command;

use crate::config::Configuration;

static METADATA: OnceCell<HashMap<String, String>> = OnceCell::new();
static COMMANDS: OnceCell<HashMap<String, Vec<String>>> = OnceCell::new();

pub fn setup(conf: &Configuration) -> Result<()> {
    METADATA
        .set(conf.metadata.r#static.clone())
        .map_err(|_| anyhow!("OnceCell set error"))?;
    COMMANDS
        .set(conf.metadata.commands.clone())
        .map_err(|_| anyhow!("OnceCell set error"))?;
    Ok(())
}

pub async fn get() -> Result<HashMap<String, String>> {
    let mut metadata = METADATA
        .get()
        .ok_or_else(|| anyhow!("METADATA is not set"))?
        .clone();

    metadata.insert(
        "mqtt_forwarder_version".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    let commands = COMMANDS
        .get()
        .ok_or_else(|| anyhow!("COMMANDS is not set"))?;

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

    Ok(metadata)
}
