use std::collections::HashMap;
use std::process::Stdio;

use anyhow::Result;
use chirpstack_api::gw;
use log::{error, info};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::config::Configuration;

static COMMANDS: OnceCell<HashMap<String, Vec<String>>> = OnceCell::const_new();

pub fn setup(conf: &Configuration) -> Result<()> {
    COMMANDS
        .set(conf.commands.clone())
        .map_err(|_| anyhow!("OnceCell set error"))?;
    Ok(())
}

pub async fn exec(pl: &gw::GatewayCommandExecRequest) -> Result<gw::GatewayCommandExecResponse> {
    let args = COMMANDS
        .get()
        .ok_or_else(|| anyhow!("COMMANDS is not set"))?
        .get(&pl.command)
        .ok_or_else(|| anyhow!("Command '{}' is not configured", pl.command))?
        .clone();

    if args.is_empty() {
        return Err(anyhow!("Command '{}' has no arguments", pl.command));
    }

    info!(
        "Executing command, command: {}, executable: {}",
        pl.command, args[0]
    );

    // construct cmd
    let mut cmd = Command::new(&args[0]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // add additional args
    if args.len() > 1 {
        cmd.args(&args[1..]);
    }

    // set environment variables
    for (k, v) in &pl.environment {
        cmd.env(k, v);
    }

    // spawn process
    let mut child = cmd.spawn()?;

    // write stdin
    let mut stdin = child.stdin.take().unwrap();
    tokio::spawn({
        let b = pl.stdin.clone();
        async move { stdin.write(&b).await }
    });

    // wait for output
    let out = child.wait_with_output().await?;

    Ok(gw::GatewayCommandExecResponse {
        gateway_id: pl.gateway_id.clone(),
        exec_id: pl.exec_id,
        stdout: out.stdout,
        stderr: out.stderr,
        ..Default::default()
    })
}

pub async fn exec_callback(cmd_args: &[String]) {
    tokio::spawn({
        let cmd_args = cmd_args.to_vec();

        async move {
            if cmd_args.is_empty() {
                return;
            }

            info!("Executing callback, callback: {:?}", cmd_args);

            let mut cmd = Command::new(&cmd_args[0]);
            if cmd_args.len() > 1 {
                cmd.args(&cmd_args[1..]);
            }

            if let Err(e) = cmd.output().await {
                error!(
                    "Execute callback error, callback: {:?}, error: {}",
                    cmd_args, e
                );
            }
        }
    });
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{env, fs};

    #[tokio::test]
    async fn test_exec_callback() {
        let temp_file = env::temp_dir().join("test.txt");
        fs::write(&temp_file, vec![]).unwrap();
        assert!(fs::exists(&temp_file).unwrap());

        exec_callback(&[
            "rm".into(),
            temp_file.clone().into_os_string().into_string().unwrap(),
        ])
        .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        assert!(!fs::exists(&temp_file).unwrap());
    }

    #[tokio::test]
    async fn test_commands() {
        let mut config = Configuration::default();
        config.commands.insert(
            "wordcount".to_string(),
            vec!["wc".to_string(), "-w".to_string()],
        );
        config.commands.insert(
            "printenv".to_string(),
            vec!["printenv".to_string(), "FOO".to_string()],
        );
        config.commands.insert(
            "echo".to_string(),
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo \"foo\" >&1; echo \"bar\" >&2".to_string(),
            ],
        );
        config
            .commands
            .insert("foobar".to_string(), vec!["foobartest".to_string()]);
        setup(&config).unwrap();

        // command not configured
        let res = exec(&gw::GatewayCommandExecRequest {
            command: "reboot".into(),
            ..Default::default()
        })
        .await;
        assert_eq!(
            "Command 'reboot' is not configured",
            res.err().unwrap().to_string()
        );

        // word count stdin
        let res = exec(&gw::GatewayCommandExecRequest {
            gateway_id: "0102030405060708".into(),
            exec_id: 123,
            command: "wordcount".into(),
            stdin: b"foo bar test bar".to_vec(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            gw::GatewayCommandExecResponse {
                gateway_id: "0102030405060708".into(),
                exec_id: 123,
                stdout: b"4\n".to_vec(),
                ..Default::default()
            },
            res
        );

        // env variables
        let res = exec(&gw::GatewayCommandExecRequest {
            gateway_id: "0102030405060708".into(),
            exec_id: 123,
            command: "printenv".into(),
            environment: [("FOO".to_string(), "bar".to_string())]
                .iter()
                .cloned()
                .collect(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            gw::GatewayCommandExecResponse {
                gateway_id: "0102030405060708".into(),
                exec_id: 123,
                stdout: b"bar\n".to_vec(),
                ..Default::default()
            },
            res
        );

        // stdout and stderr
        let res = exec(&gw::GatewayCommandExecRequest {
            gateway_id: "0102030405060708".into(),
            exec_id: 123,
            command: "echo".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            gw::GatewayCommandExecResponse {
                gateway_id: "0102030405060708".into(),
                exec_id: 123,
                stdout: b"foo\n".to_vec(),
                stderr: b"bar\n".to_vec(),
                ..Default::default()
            },
            res
        );

        // executable not found
        let res = exec(&gw::GatewayCommandExecRequest {
            command: "foobar".into(),
            ..Default::default()
        })
        .await;
        assert!(res.is_err());
        assert_eq!(
            "No such file or directory (os error 2)",
            res.err().unwrap().to_string()
        );
    }
}
