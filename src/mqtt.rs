use std::fs::File;
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chirpstack_api::gw;
use log::{debug, error, info, trace};
use once_cell::sync::OnceCell;
use prost::Message;
use rumqttc::tokio_rustls::rustls;
use rumqttc::v5::mqttbytes::v5::{ConnectReturnCode, LastWill, Publish};
use rumqttc::v5::{mqttbytes::QoS, AsyncClient, Event, Incoming, MqttOptions};
use rumqttc::Transport;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::backend::{get_gateway_id, send_configuration_command, send_downlink_frame};
use crate::commands;
use crate::config::Configuration;

static STATE: OnceCell<Arc<State>> = OnceCell::new();

#[derive(Serialize)]
struct CommandTopicContext {
    pub gateway_id: String,
    pub command: String,
}

#[derive(Serialize)]
struct EventTopicContext {
    pub gateway_id: String,
    pub event: String,
}

#[derive(Serialize)]
struct StateTopciContext {
    pub gateway_id: String,
    pub state: String,
}

struct State {
    client: AsyncClient,
    qos: QoS,
    json: bool,
    gateway_id: String,
    topic_prefix: String,
}

pub async fn setup(conf: &Configuration) -> Result<()> {
    if STATE.get().is_some() {
        return Ok(());
    }

    debug!("Setting up MQTT client");

    let qos = match conf.mqtt.qos {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        2 => QoS::ExactlyOnce,
        _ => return Err(anyhow!("Invalid QoS: {}", conf.mqtt.qos)),
    };

    // get gateway id
    let gateway_id = get_gateway_id().await?;

    // set client id
    let client_id = if conf.mqtt.client_id.is_empty() {
        gateway_id.clone()
    } else {
        conf.mqtt.client_id.clone()
    };

    // get mqtt prefix
    let topic_prefix = if conf.mqtt.topic_prefix.is_empty() {
        "".to_string()
    } else {
        format!("{}/", conf.mqtt.topic_prefix)
    };

    // Create connect channel
    // We need to re-subscribe on (re)connect to be sure we have a subscription. Even
    // in case of a persistent MQTT session, there is no guarantee that the MQTT persisted the
    // session and that a re-connect would recover the subscription.
    let (connect_tx, mut connect_rx) = mpsc::channel(1);

    // last will and testament
    let lwt = gw::ConnState {
        gateway_id: gateway_id.clone(),
        state: gw::conn_state::State::Offline.into(),
        ..Default::default()
    };
    let lwt = match conf.mqtt.json {
        true => serde_json::to_vec(&lwt)?,
        false => lwt.encode_to_vec(),
    };
    let lwt_topic = get_state_topic(&topic_prefix, &gateway_id, "conn");
    let lwt_msg = LastWill {
        qos,
        topic: lwt_topic.into(),
        message: lwt.into(),
        retain: true,
        properties: None,
    };

    // create client
    let mut mqtt_opts =
        MqttOptions::parse_url(format!("{}?client_id={}", conf.mqtt.server, client_id))?;
    mqtt_opts.set_last_will(lwt_msg);
    mqtt_opts.set_clean_start(conf.mqtt.clean_session);
    if !conf.mqtt.username.is_empty() || !conf.mqtt.password.is_empty() {
        mqtt_opts.set_credentials(&conf.mqtt.username, &conf.mqtt.password);
    }
    if !conf.mqtt.ca_cert.is_empty()
        || !conf.mqtt.tls_cert.is_empty()
        || !conf.mqtt.tls_key.is_empty()
    {
        info!(
            "Configuring client with TLS certificate, ca_cert: {}, tls_cert: {}, tls_key: {}",
            conf.mqtt.ca_cert, conf.mqtt.tls_cert, conf.mqtt.tls_key
        );

        let root_certs = get_root_certs(if conf.mqtt.ca_cert.is_empty() {
            None
        } else {
            Some(conf.mqtt.ca_cert.clone())
        })?;

        let client_conf = if conf.mqtt.tls_cert.is_empty() && conf.mqtt.tls_key.is_empty() {
            rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_certs.clone())
                .with_no_client_auth()
        } else {
            rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_certs.clone())
                .with_client_auth_cert(
                    load_cert(&conf.mqtt.tls_cert)?,
                    load_key(&conf.mqtt.tls_key)?,
                )?
        };

        mqtt_opts.set_transport(Transport::tls_with_config(client_conf.into()));
    }

    let (client, mut eventloop) = AsyncClient::new(mqtt_opts, 100);
    let state = State {
        client,
        topic_prefix,
        qos,
        json: conf.mqtt.json,
        gateway_id: gateway_id.clone(),
    };
    let state = Arc::new(state);

    // (Re)subscribe loop
    tokio::spawn({
        let state = state.clone();
        let command_topic = get_command_topic(&state.topic_prefix, &state.gateway_id, "+");
        let state_topic = get_state_topic(&state.topic_prefix, &state.gateway_id, "conn");

        let conn = gw::ConnState {
            gateway_id: gateway_id.clone(),
            state: gw::conn_state::State::Online.into(),
            ..Default::default()
        };
        let b = match state.json {
            true => serde_json::to_vec(&conn)?,
            false => conn.encode_to_vec(),
        };

        async move {
            while connect_rx.recv().await.is_some() {
                info!("Subscribing to command topic, topic: {}", command_topic);
                if let Err(e) = state.client.subscribe(&command_topic, state.qos).await {
                    error!("Subscribing to command topic error, error: {}", e);
                }

                info!("Sending conn state, topic: {}", state_topic);
                if let Err(e) = state
                    .client
                    .publish(&state_topic, state.qos, true, b.clone())
                    .await
                {
                    error!("Sending state error: {}", e);
                }
            }
        }
    });

    // Eventloop
    tokio::spawn({
        async move {
            info!("Starting MQTT event loop");

            loop {
                match eventloop.poll().await {
                    Ok(v) => {
                        trace!("MQTT event: {:?}", v);

                        match v {
                            Event::Incoming(Incoming::Publish(p)) => {
                                if let Err(e) = message_callback(p).await {
                                    error!("Handling message error, error: {}", e);
                                }
                            }
                            Event::Incoming(Incoming::ConnAck(v)) => {
                                if v.code == ConnectReturnCode::Success {
                                    if let Err(e) = connect_tx.try_send(()) {
                                        error!("Send to subscribe channel error, error: {}", e);
                                    }
                                } else {
                                    error!("Connection error, code: {:?}", v.code);
                                    sleep(Duration::from_secs(1)).await
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        error!("MQTT error, error: {}", e);
                        sleep(Duration::from_secs(1)).await
                    }
                }
            }
        }
    });

    STATE.set(state).map_err(|_| anyhow!("Set STATE error"))?;

    Ok(())
}

pub async fn send_uplink_frame(pl: &gw::UplinkFrame) -> Result<()> {
    let state = STATE.get().ok_or_else(|| anyhow!("STATE is not set"))?;

    let b = match state.json {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };
    let topic = get_event_topic(&state.topic_prefix, &state.gateway_id, "up");

    info!(
        "Sending uplink event, uplink_id: {}, topic: {}",
        pl.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
        topic
    );

    state.client.publish(topic, state.qos, false, b).await?;
    trace!("Message published");

    Ok(())
}

pub async fn send_gateway_stats(pl: &gw::GatewayStats) -> Result<()> {
    let state = STATE.get().ok_or_else(|| anyhow!("STATE is not set"))?;

    let b = match state.json {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };
    let topic = get_event_topic(&state.topic_prefix, &state.gateway_id, "stats");

    info!("Sending gateway stats event, topic: {}", topic);
    state.client.publish(topic, state.qos, false, b).await?;
    trace!("Message published");

    Ok(())
}

pub async fn send_tx_ack(pl: &gw::DownlinkTxAck) -> Result<()> {
    let state = STATE.get().ok_or_else(|| anyhow!("STATE is not set"))?;

    let b = match state.json {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };
    let topic = get_event_topic(&state.topic_prefix, &state.gateway_id, "ack");

    info!(
        "Sending ack event, downlink_id: {}, topic: {}",
        pl.downlink_id, topic
    );

    state.client.publish(topic, state.qos, false, b).await?;
    trace!("Message published");

    Ok(())
}

async fn message_callback(p: Publish) -> Result<()> {
    let state = STATE.get().ok_or_else(|| anyhow!("STATE is not set"))?;

    let topic = String::from_utf8(p.topic.to_vec())?;
    let qos = p.qos;
    let b = p.payload.to_vec();

    info!("Received message, topic: {}, qos: {:?}", topic, qos);

    let parts: Vec<&str> = topic.split('/').collect();
    if parts.len() < 4 {
        return Err(anyhow!("Topic does not contain enough paths"));
    }

    // Get the last three elements: .../[gateway_id]/command/[command]
    let gateway_id = parts[parts.len() - 3];
    let command = parts[parts.len() - 1];

    match command {
        "down" => {
            let pl = match state.json {
                true => serde_json::from_slice(&b)?,
                false => gw::DownlinkFrame::decode(&mut Cursor::new(b))?,
            };
            if pl.gateway_id != gateway_id {
                return Err(anyhow!(
                    "Gateway ID in payload does not match gateway ID in topic"
                ));
            }
            info!(
                "Received downlink command, downlink_id: {}, topic: {}",
                pl.downlink_id, topic
            );
            send_downlink_frame(&pl).await
        }
        "config" => {
            let pl = match state.json {
                true => serde_json::from_slice(&b)?,
                false => gw::GatewayConfiguration::decode(&mut Cursor::new(b))?,
            };
            if pl.gateway_id != gateway_id {
                return Err(anyhow!(
                    "Gateway ID in payload does not match gateway ID in topic"
                ));
            }
            info!(
                "Received configuration command, version: {}, topic: {}",
                pl.version, topic
            );
            send_configuration_command(&pl).await
        }
        "exec" => {
            let pl = match state.json {
                true => serde_json::from_slice(&b)?,
                false => gw::GatewayCommandExecRequest::decode(&mut Cursor::new(b))?,
            };
            if pl.gateway_id != gateway_id {
                return Err(anyhow!(
                    "Gateway ID in payload does not match gateway ID in topic"
                ));
            }
            info!(
                "Received gateway command exec command, exec_id: {}, topic: {}",
                pl.exec_id, topic
            );
            handle_command_exec(&pl).await
        }
        _ => Err(anyhow!("Unexpected command, command: {}", command)),
    }
}

async fn handle_command_exec(pl: &gw::GatewayCommandExecRequest) -> Result<()> {
    let state = STATE.get().ok_or_else(|| anyhow!("STATE is not set"))?;

    let resp = match commands::exec(pl).await {
        Ok(v) => v,
        Err(e) => gw::GatewayCommandExecResponse {
            gateway_id: pl.gateway_id.clone(),
            exec_id: pl.exec_id,
            error: e.to_string(),
            ..Default::default()
        },
    };

    let b = match state.json {
        true => serde_json::to_vec(&resp)?,
        false => resp.encode_to_vec(),
    };

    let topic = get_event_topic(&state.topic_prefix, &state.gateway_id, "exec");

    info!(
        "Sending gateway command exec event, exec_id: {}, topic: {}",
        pl.exec_id, topic
    );
    state.client.publish(topic, state.qos, false, b).await?;

    trace!("Message published");

    Ok(())
}

fn get_state_topic(prefix: &str, gateway_id: &str, state: &str) -> String {
    format!("{}gateway/{}/state/{}", prefix, gateway_id, state)
}

fn get_event_topic(prefix: &str, gateway_id: &str, event: &str) -> String {
    format!("{}gateway/{}/event/{}", prefix, gateway_id, event)
}

fn get_command_topic(prefix: &str, gateway_id: &str, command: &str) -> String {
    format!("{}gateway/{}/command/{}", prefix, gateway_id, command)
}

fn get_root_certs(ca_file: Option<String>) -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    let certs = rustls_native_certs::load_native_certs()?;
    let certs: Vec<_> = certs.into_iter().map(|cert| cert.0).collect();
    roots.add_parsable_certificates(&certs);

    if let Some(ca_file) = &ca_file {
        let f = File::open(ca_file).context("Open CA certificate")?;
        let mut reader = BufReader::new(f);
        let certs = rustls_pemfile::certs(&mut reader)?;
        for cert in certs
            .into_iter()
            .map(rustls::Certificate)
            .collect::<Vec<_>>()
        {
            roots.add(&cert)?;
        }
    }

    Ok(roots)
}

fn load_cert(cert_file: &str) -> Result<Vec<rustls::Certificate>> {
    let f = File::open(cert_file).context("Open TLS certificate")?;
    let mut reader = BufReader::new(f);
    let certs = rustls_pemfile::certs(&mut reader)?;
    let certs = certs
        .into_iter()
        .map(rustls::Certificate)
        .collect::<Vec<_>>();
    Ok(certs)
}

fn load_key(key_file: &str) -> Result<rustls::PrivateKey> {
    let f = File::open(key_file).context("Open private key")?;
    let mut reader = BufReader::new(f);
    let mut keys = rustls_pemfile::pkcs8_private_keys(&mut reader)?;
    match keys.len() {
        0 => Err(anyhow!("No private key found")),
        1 => Ok(rustls::PrivateKey(keys.remove(0))),
        _ => Err(anyhow!("More than one private key found")),
    }
}
