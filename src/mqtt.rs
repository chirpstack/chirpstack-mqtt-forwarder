use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use chirpstack_api::{gw, prost::Message};
use log::{debug, error, info, trace};
use rumqttc::Transport;
use rumqttc::tokio_rustls::rustls;
use rumqttc::{
    AsyncClient, Broker, ConnectReturnCode, Event, Incoming, LastWill, MqttOptions, Publish,
    PublishOptions, QoS, SubscribeReasonCode,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::sync::{Mutex, OnceCell, broadcast, mpsc};
use tokio::task::JoinSet;
use tokio::time::sleep;

use crate::backend::{
    get_gateway_id, send_configuration_command, send_downlink_frame, send_mesh_command,
};
use crate::commands;
use crate::config::Configuration;

static JSON: AtomicBool = AtomicBool::new(false);
static PUB_CHANNEL_TX: OnceCell<mpsc::Sender<MqttMessage>> = OnceCell::const_new();
static PUB_CHANNEL_RX: OnceCell<Mutex<mpsc::Receiver<MqttMessage>>> = OnceCell::const_new();

struct MqttMessage {
    typ: String,
    bytes: Vec<u8>,
}

pub async fn run(conf: &Configuration, mut terminate_rx: broadcast::Receiver<()>) -> Result<()> {
    debug!("Setting up MQTT client");

    debug!("Setting up publish channel");
    if PUB_CHANNEL_TX.get().is_none() || PUB_CHANNEL_RX.get().is_none() {
        let (pub_channel_tx, pub_channel_rx) = mpsc::channel(conf.mqtt.publish_channel_size);
        PUB_CHANNEL_TX
            .set(pub_channel_tx)
            .map_err(|_| anyhow!("set PUB_CHANNEL_TX error"))?;
        PUB_CHANNEL_RX
            .set(Mutex::new(pub_channel_rx))
            .map_err(|_| anyhow!("set PUB_CHANNEL_RX error"))?;
    }

    debug!("Setting JSON flag, json = {}", conf.mqtt.json);
    JSON.store(conf.mqtt.json, Ordering::Relaxed);

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
        format!("gateway/{}", gateway_id)
    } else {
        format!("{}/gateway/{}", conf.mqtt.topic_prefix, gateway_id)
    };

    // Create subscribe channel
    // We need to re-subscribe on (re)connect to be sure we have a subscription. Even
    // in case of a persistent MQTT session, there is no guarantee that the MQTT persisted the
    // session and that a re-connect would recover the subscription.
    let (subscribe_tx, mut subscribe_rx) = mpsc::channel(1);

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
    let lwt_topic = format!("{}/state/conn", topic_prefix);
    let lwt_msg = LastWill {
        qos,
        topic: lwt_topic.into(),
        message: lwt.into(),
        retain: true,
        properties: None,
    };

    // create client
    // workaround for:
    // https://github.com/bytebeamio/rumqtt/issues/808
    let mqtt_url = url::Url::parse(&conf.mqtt.server)?;
    let mut mqtt_opts = match mqtt_url.scheme() {
        "mqtt" | "tcp" => MqttOptions::new(
            client_id,
            Broker::tcp(
                mqtt_url.host_str().unwrap_or_default(),
                mqtt_url.port().unwrap_or(1883),
            ),
        ),
        "mqtts" | "ssl" => {
            let mut m = MqttOptions::new(
                client_id,
                Broker::tcp(
                    mqtt_url.host_str().unwrap_or_default(),
                    mqtt_url.port().unwrap_or(8883),
                ),
            );
            m.set_transport(Transport::tls_with_default_config());
            m
        }
        "ws" => {
            let mut m = MqttOptions::new(client_id, Broker::websocket(&conf.mqtt.server)?);
            m.set_transport(Transport::ws());
            m
        }
        "wss" => MqttOptions::try_websocket_with_default_tls(client_id, &conf.mqtt.server)?,
        _ => return Err(anyhow!("Invalid scheme: {}", mqtt_url.scheme())),
    };

    mqtt_opts.set_last_will(lwt_msg);
    mqtt_opts.set_clean_start(conf.mqtt.clean_session);
    mqtt_opts.set_keep_alive(conf.mqtt.keep_alive_interval.as_secs() as u16);
    if !conf.mqtt.username.is_empty() || !conf.mqtt.password.is_empty() {
        mqtt_opts.set_credentials(&conf.mqtt.username, conf.mqtt.password.clone());
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
                .with_root_certificates(root_certs.clone())
                .with_no_client_auth()
        } else {
            rustls::ClientConfig::builder()
                .with_root_certificates(root_certs.clone())
                .with_client_auth_cert(
                    load_cert(&conf.mqtt.tls_cert)?,
                    load_key(&conf.mqtt.tls_key)?,
                )?
        };

        mqtt_opts.set_transport(match mqtt_opts.transport() {
            Transport::Tls(_) => Transport::tls_with_config(client_conf.into()),
            Transport::Wss(_) => Transport::wss_with_config(client_conf.into()),
            _ => return Err(anyhow!("Configured transport does not allow TLS config")),
        });
    }

    let (client, mut eventloop) = AsyncClient::builder(mqtt_opts).capacity(100).build();
    let mut set = JoinSet::new();

    // (Re)subscribe loop
    set.spawn({
        let client = client.clone();
        let command_topic = format!("{}/command/+", topic_prefix);
        let state_topic = format!("{}/state/conn", topic_prefix);

        let conn = gw::ConnState {
            gateway_id: gateway_id.clone(),
            state: gw::conn_state::State::Online.into(),
            ..Default::default()
        };
        let b = match conf.mqtt.json {
            true => serde_json::to_vec(&conn)?,
            false => conn.encode_to_vec(),
        };

        async move {
            info!("Starting (re)subscribe loop");
            while subscribe_rx.recv().await.is_some() {
                loop {
                    info!("Subscribing to command topic, topic: {}", command_topic);
                    if let Err(e) = client.subscribe(&command_topic, qos).await {
                        error!("Subscribing to command topic error, error: {}", e);
                    } else {
                        break;
                    }

                    sleep(Duration::from_secs(1)).await;
                }

                info!("Sending conn state, topic: {}", state_topic);
                if let Err(e) = client
                    .publish(&state_topic, b.clone(), PublishOptions::new(qos).retained())
                    .await
                {
                    error!("Sending state error: {}", e);
                }
            }
        }
    });

    // publish loop
    set.spawn({
        let client = client.clone();
        let topic_prefix = format!("{}/event", topic_prefix);
        let mut pub_chan_rx = PUB_CHANNEL_RX
            .get()
            .ok_or_else(|| anyhow!("PUB_CHANNEL_RX is None"))?
            .lock()
            .await;

        async move {
            info!("Starting MQTT publish loop");
            while let Some(msg) = pub_chan_rx.recv().await {
                let topic = format!("{}/{}", topic_prefix, msg.typ);
                info!(
                    "Publishing event, type = {}, qos = {:?}, topic = {}",
                    msg.typ, qos, topic
                );
                if let Err(e) = client
                    .publish(&topic, msg.bytes, PublishOptions::new(qos))
                    .await
                {
                    error!("Sending event error: {}", e);
                }
            }
        }
    });

    // MQTT event loop
    set.spawn({
        let on_mqtt_connected = conf.callbacks.on_mqtt_connected.clone();
        let on_mqtt_connection_error = conf.callbacks.on_mqtt_connection_error.clone();
        let reconnect_interval = conf.mqtt.reconnect_interval;

        async move {
            info!("Starting MQTT event loop");

            loop {
                match eventloop.poll().await {
                    Ok(v) => {
                        trace!("MQTT event: {:?}", v);

                        match v {
                            Event::Incoming(Incoming::Publish(p)) => {
                                tokio::spawn({
                                    async move {
                                        if let Err(e) = message_callback(p).await {
                                            error!("Handling message error, error: {}", e);
                                        }
                                    }
                                });
                            }
                            Event::Incoming(Incoming::ConnAck(v)) => {
                                if v.code == ConnectReturnCode::Success {
                                    commands::exec_callback(&on_mqtt_connected).await;

                                    if let Err(e) = subscribe_tx.try_send(()) {
                                        error!("Send to subscribe channel error, error: {}", e);
                                    }
                                } else {
                                    error!("Connection error, code: {:?}", v.code);
                                    sleep(reconnect_interval).await
                                }
                            }
                            Event::Incoming(Incoming::SubAck(v)) => {
                                let errors: Vec<SubscribeReasonCode> = v
                                    .return_codes
                                    .iter()
                                    .filter(|v| !matches!(v, SubscribeReasonCode::Success(_)))
                                    .cloned()
                                    .collect();

                                if !errors.is_empty() {
                                    error!("Subscribe ack returned errors, errors: {:?}", errors);

                                    if let Err(e) = subscribe_tx.try_send(()) {
                                        error!("Send to subscribe channel error, error: {}", e);
                                    }
                                }
                            }

                            _ => {}
                        }
                    }
                    Err(e) => {
                        commands::exec_callback(&on_mqtt_connection_error).await;

                        error!("MQTT error, error: {}", e);
                        sleep(reconnect_interval).await
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = terminate_rx.recv() => {
                set.shutdown().await;
                break;
            },
            res = set.join_next() => {
                match res {
                    Some(_) => {},
                    None => break,
                }
            },
        }
    }

    info!("MQTT client completed");

    Ok(())
}

pub async fn send_uplink_frame(pl: &gw::UplinkFrame) -> Result<()> {
    let b = match JSON.load(Ordering::Relaxed) {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };

    info!(
        "Sending uplink event, uplink_id: {}",
        pl.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
    );

    PUB_CHANNEL_TX
        .get()
        .ok_or_else(|| anyhow!("PUB_CHANNEL_TX is None"))?
        .try_send(MqttMessage {
            typ: "up".into(),
            bytes: b,
        })?;

    Ok(())
}

pub async fn send_gateway_stats(pl: &gw::GatewayStats) -> Result<()> {
    let b = match JSON.load(Ordering::Relaxed) {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };

    info!("Sending gateway stats event");

    PUB_CHANNEL_TX
        .get()
        .ok_or_else(|| anyhow!("PUB_CHANNEL_TX is None"))?
        .try_send(MqttMessage {
            typ: "stats".into(),
            bytes: b,
        })?;

    Ok(())
}

pub async fn send_mesh_event(pl: &gw::MeshEvent) -> Result<()> {
    let b = match JSON.load(Ordering::Relaxed) {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };

    info!("Sending mesh event");

    PUB_CHANNEL_TX
        .get()
        .ok_or_else(|| anyhow!("PUB_CHANNEL_TX is None"))?
        .try_send(MqttMessage {
            typ: "mesh".into(),
            bytes: b,
        })?;

    Ok(())
}

pub async fn send_tx_ack(pl: &gw::DownlinkTxAck) -> Result<()> {
    let b = match JSON.load(Ordering::Relaxed) {
        true => serde_json::to_vec(&pl)?,
        false => pl.encode_to_vec(),
    };

    info!("Sending ack event, downlink_id: {}", pl.downlink_id);

    PUB_CHANNEL_TX
        .get()
        .ok_or_else(|| anyhow!("PUB_CHANNEL_TX is None"))?
        .try_send(MqttMessage {
            typ: "ack".into(),
            bytes: b,
        })?;

    Ok(())
}

async fn message_callback(p: Publish) -> Result<()> {
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

    // get encoding (json or binary)
    let json = JSON.load(Ordering::Relaxed);

    match command {
        "down" => {
            let pl = match json {
                true => serde_json::from_slice(&b)?,
                false => gw::DownlinkFrame::decode(b.as_slice())?,
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
            send_downlink_frame(pl).await
        }
        "config" => {
            let pl = match json {
                true => serde_json::from_slice(&b)?,
                false => gw::GatewayConfiguration::decode(b.as_slice())?,
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
            send_configuration_command(pl).await
        }
        "exec" => {
            let pl = match json {
                true => serde_json::from_slice(&b)?,
                false => gw::GatewayCommandExecRequest::decode(b.as_slice())?,
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
        "mesh" => {
            let pl = match json {
                true => serde_json::from_slice(&b)?,
                false => gw::MeshCommand::decode(b.as_slice())?,
            };
            if pl.gateway_id != gateway_id {
                return Err(anyhow!(
                    "Gateway ID in payload does not match gateway ID in topic"
                ));
            }
            info!("Received mesh command, topic: {}", topic);
            send_mesh_command(pl).await
        }
        _ => Err(anyhow!("Unexpected command, command: {}", command)),
    }
}

async fn handle_command_exec(pl: &gw::GatewayCommandExecRequest) -> Result<()> {
    let resp = match commands::exec(pl).await {
        Ok(v) => v,
        Err(e) => gw::GatewayCommandExecResponse {
            gateway_id: pl.gateway_id.clone(),
            exec_id: pl.exec_id,
            error: e.to_string(),
            ..Default::default()
        },
    };

    let b = match JSON.load(Ordering::Relaxed) {
        true => serde_json::to_vec(&resp)?,
        false => resp.encode_to_vec(),
    };

    info!(
        "Sending gateway command exec event, exec_id: {}",
        pl.exec_id
    );

    PUB_CHANNEL_TX
        .get()
        .ok_or_else(|| anyhow!("PUB_CHANNEL_TX is None"))?
        .try_send(MqttMessage {
            typ: "exec".into(),
            bytes: b,
        })?;

    Ok(())
}

fn get_root_certs(ca_file: Option<String>) -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        roots.add(cert)?;
    }

    if let Some(ca_file) = &ca_file {
        let f = File::open(ca_file).context("Open CA certificate")?;
        let mut reader = BufReader::new(f);
        let certs = rustls_pemfile::certs(&mut reader);
        for cert in certs.flatten() {
            roots.add(cert)?;
        }
    }

    Ok(roots)
}

fn load_cert(cert_file: &str) -> Result<Vec<CertificateDer<'static>>> {
    let f = File::open(cert_file).context("Open TLS certificate")?;
    let mut reader = BufReader::new(f);
    let certs = rustls_pemfile::certs(&mut reader);
    let mut out = Vec::new();
    for cert in certs {
        out.push(cert?.into_owned());
    }
    Ok(out)
}

fn load_key(key_file: &str) -> Result<PrivateKeyDer<'static>> {
    let f = File::open(key_file).context("Open private key")?;
    let mut reader = BufReader::new(f);
    let mut keys = rustls_pemfile::pkcs8_private_keys(&mut reader);
    if let Some(key) = keys.next() {
        match key {
            Ok(v) => return Ok(PrivateKeyDer::Pkcs8(v.clone_key())),
            Err(e) => {
                return Err(anyhow!("Error parsing private key, error: {}", e));
            }
        }
    }

    Err(anyhow!("No private key found"))
}
