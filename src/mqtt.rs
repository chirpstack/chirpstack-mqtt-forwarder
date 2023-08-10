use std::env;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chirpstack_api::gw;
use futures::stream::StreamExt;
use log::{debug, error, info, trace};
use once_cell::sync::OnceCell;
use paho_mqtt as mqtt;
use prost::Message;
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
    client: mqtt::AsyncClient,
    qos: usize,
    json: bool,
    gateway_id: String,
    topic_prefix: String,
}

pub async fn setup(conf: &Configuration) -> Result<()> {
    if STATE.get().is_some() {
        return Ok(());
    }

    debug!("Setting up MQTT client");

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
    // This is needed as we can't subscribe within the set_connected_callback as this would
    // block the callback (we want to wait for success or error), which would create a
    // deadlock. We need to re-subscribe on (re)connect to be sure we have a subscription. Even
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
    let lwt_msg = mqtt::Message::new_retained(lwt_topic, lwt, conf.mqtt.qos as i32);

    // create client
    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri(&conf.mqtt.server)
        .client_id(&client_id)
        .persistence(env::temp_dir())
        .finalize();

    let mut client = mqtt::AsyncClient::new(create_opts)?;
    client.set_connected_callback(move |_| {
        info!("Connected to MQTT broker");

        if let Err(e) = connect_tx.try_send(()) {
            error!("Send to subscribe channel error, error: {}", e);
        }
    });
    client.set_connection_lost_callback(|_| {
        error!("MQTT connection to broker lost");
    });

    // connection options
    let mut conn_opts_b = mqtt::ConnectOptionsBuilder::new();
    conn_opts_b.will_message(lwt_msg);
    conn_opts_b.automatic_reconnect(Duration::from_secs(1), Duration::from_secs(30));
    conn_opts_b.clean_session(conf.mqtt.clean_session);
    if !conf.mqtt.username.is_empty() {
        conn_opts_b.user_name(&conf.mqtt.username);
    }
    if !conf.mqtt.password.is_empty() {
        conn_opts_b.password(&conf.mqtt.password);
    }
    if !conf.mqtt.ca_cert.is_empty()
        || !conf.mqtt.tls_cert.is_empty()
        || !conf.mqtt.tls_key.is_empty()
    {
        info!(
            "Configuring client with TLS certificate, ca_cert: {}, tls_cert: {}, tls_key: {}",
            conf.mqtt.ca_cert, conf.mqtt.tls_cert, conf.mqtt.tls_key
        );

        let mut ssl_opts_b = mqtt::SslOptionsBuilder::new();

        if !conf.mqtt.ca_cert.is_empty() {
            ssl_opts_b.trust_store(&conf.mqtt.ca_cert)?;
        }

        if !conf.mqtt.tls_cert.is_empty() {
            ssl_opts_b.key_store(&conf.mqtt.tls_cert)?;
        }

        if !conf.mqtt.tls_key.is_empty() {
            ssl_opts_b.private_key(&conf.mqtt.tls_key)?;
        }

        conn_opts_b.ssl_options(ssl_opts_b.finalize());
    }
    let conn_opts = conn_opts_b.finalize();

    // get message stream
    let mut stream = client.get_stream(25);

    // connect
    info!(
        "Connecting to MQTT broker, server: {}, clean_session: {}, client_id: {}, qos: {}",
        conf.mqtt.server, conf.mqtt.clean_session, client_id, conf.mqtt.qos,
    );

    while let Err(e) = client.connect(conn_opts.clone()).await {
        error!("Connecting to MQTT broker error, error: {}", e);
        sleep(Duration::from_secs(1)).await;
    }

    let state = State {
        client,
        topic_prefix,
        qos: conf.mqtt.qos,
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
                if let Err(e) = state
                    .client
                    .subscribe(&command_topic, state.qos as i32)
                    .await
                {
                    error!("Subscribing to command topic error, error: {}", e);
                }

                info!("Sending conn state, topic: {}", state_topic);
                let msg = mqtt::Message::new_retained(&state_topic, b.clone(), state.qos as i32);
                if let Err(e) = state.client.publish(msg).await {
                    error!("Sending state error: {}", e);
                }
            }
        }
    });

    // Consume loop
    tokio::spawn({
        async move {
            info!("Starting MQTT consumer loop");
            while let Some(msg_opt) = stream.next().await {
                if let Some(msg) = msg_opt {
                    if let Err(e) = message_callback(msg).await {
                        error!("Handling message error, error: {}", e);
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
    let msg = mqtt::Message::new(topic, b, state.qos as i32);
    state.client.publish(msg).await?;

    trace!("Message sent");

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
    let msg = mqtt::Message::new(topic, b, state.qos as i32);
    state.client.publish(msg).await?;

    trace!("Message sent");

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
    let msg = mqtt::Message::new(topic, b, state.qos as i32);
    state.client.publish(msg).await?;

    trace!("Message sent");

    Ok(())
}

async fn message_callback(msg: mqtt::Message) -> Result<()> {
    let state = STATE.get().ok_or_else(|| anyhow!("STATE is not set"))?;

    let topic = msg.topic();
    let qos = msg.qos();
    let b = msg.payload();

    info!("Received message, topic: {}, qos: {}", topic, qos);

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
                true => serde_json::from_slice(b)?,
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
                true => serde_json::from_slice(b)?,
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
                true => serde_json::from_slice(b)?,
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
        _ => Err(anyhow!("Unexepcted command, command: {}", command)),
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
    let msg = mqtt::Message::new(topic, b, state.qos as i32);
    state.client.publish(msg).await?;

    trace!("Message sent");

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
