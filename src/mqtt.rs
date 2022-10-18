use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chirpstack_api::gw;
use futures::stream::StreamExt;
use handlebars::Handlebars;
use log::{debug, error, info, trace};
use once_cell::sync::OnceCell;
use paho_mqtt as mqtt;
use prost::Message;
use regex::Regex;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::backend::{get_gateway_id, send_downlink_frame};
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

struct State<'a> {
    client: mqtt::AsyncClient,
    templates: Handlebars<'a>,
    qos: usize,
    json: bool,
    gateway_id: String,
    command_topic_regex: Regex,
}

pub async fn setup(conf: &Configuration) -> Result<()> {
    debug!("Setting up MQTT client");

    // get gateway id
    let gateway_id = get_gateway_id().await?;

    // set client id
    let client_id = if conf.mqtt.client_id.is_empty() {
        gateway_id.clone()
    } else {
        conf.mqtt.client_id.clone()
    };

    // topic templates
    let mut templates = Handlebars::new();
    templates.register_escape_fn(handlebars::no_escape);
    templates.register_template_string("command_topic", &conf.mqtt.command_topic)?;
    templates.register_template_string("state_topic", &conf.mqtt.state_topic)?;
    templates.register_template_string("event_topic", &conf.mqtt.event_topic)?;

    // command regex
    let command_topic_regex = Regex::new(&templates.render(
        "command_topic",
        &CommandTopicContext {
            gateway_id: r"(?P<gateway_id>[a-fA-F0-9]{16})".into(),
            command: r"(?P<command>\w+)".into(),
        },
    )?)
    .unwrap();

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
    let lwt_topic = templates.render(
        "state_topic",
        &StateTopciContext {
            gateway_id: gateway_id.clone(),
            state: "conn".into(),
        },
    )?;
    let lwt_msg = mqtt::Message::new_retained(lwt_topic, lwt, conf.mqtt.qos as i32);

    // create client
    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri(&conf.mqtt.server)
        .client_id(&client_id)
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
    conn_opts_b.user_name(&conf.mqtt.username);
    conn_opts_b.password(&conf.mqtt.password);
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
        templates,
        command_topic_regex,
        qos: conf.mqtt.qos,
        json: conf.mqtt.json,
        gateway_id: gateway_id.clone(),
    };
    let state = Arc::new(state);

    // (Re)subscribe loop
    tokio::spawn({
        let state = state.clone();
        let command_topic = state.templates.render(
            "command_topic",
            &CommandTopicContext {
                gateway_id: gateway_id.clone(),
                command: "+".into(),
            },
        )?;
        let state_topic = state.templates.render(
            "state_topic",
            &StateTopciContext {
                gateway_id: gateway_id.clone(),
                state: "conn".into(),
            },
        )?;

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

                info!("Sending state, state: {}, topic: {}", "conn", state_topic);
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
    let topic = state.templates.render(
        "event_topic",
        &EventTopicContext {
            gateway_id: state.gateway_id.clone(),
            event: "up".to_string(),
        },
    )?;

    info!("Sending event, event: {}, topic: {}", "up", topic);
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
    let topic = state.templates.render(
        "event_topic",
        &EventTopicContext {
            gateway_id: state.gateway_id.clone(),
            event: "stats".to_string(),
        },
    )?;

    info!("Sending event, event: {}, topic: {}", "stats", topic);
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
    let topic = state.templates.render(
        "event_topic",
        &EventTopicContext {
            gateway_id: state.gateway_id.clone(),
            event: "ack".to_string(),
        },
    )?;

    info!("Sending event, event: {}, topic: {}", "ack", topic);
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

    let caps = state
        .command_topic_regex
        .captures(&topic)
        .ok_or_else(|| anyhow!("Topic does not match topic template"))?;
    let gateway_id = caps
        .name("gateway_id")
        .ok_or_else(|| anyhow!("Extract gateway_id from topic error"))?
        .as_str();
    let command = caps
        .name("command")
        .ok_or_else(|| anyhow!("Extract command from topic error"))?
        .as_str();

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
            send_downlink_frame(&pl).await
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

    let topic = state.templates.render(
        "event_topic",
        &EventTopicContext {
            gateway_id: pl.gateway_id.clone(),
            event: "exec".to_string(),
        },
    )?;

    info!("Sending event, event: {}, topic: {}", "state", topic);
    let msg = mqtt::Message::new(topic, b, state.qos as i32);
    state.client.publish(msg).await?;

    trace!("Message sent");

    Ok(())
}
