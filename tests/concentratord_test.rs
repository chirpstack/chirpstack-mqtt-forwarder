use std::env;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use prost::Message;
use rumqttc::v5::{mqttbytes::QoS, AsyncClient, Event, Incoming, MqttOptions};
use tokio::sync::mpsc;
use tokio::time::sleep;

use chirpstack_api::gw;
use chirpstack_mqtt_forwarder::config;

#[tokio::test]
async fn end_to_end() {
    dotenv::dotenv().ok();
    dotenv::from_filename(".env.local").ok();

    let mut c = config::Configuration {
        backend: config::Backend {
            enabled: "concentratord".into(),
            ..Default::default()
        },
        mqtt: config::Mqtt {
            server: env::var("TEST_MQTT_BROKER_URL").unwrap(),
            ..Default::default()
        },
        ..Default::default()
    };

    c.metadata
        .r#static
        .insert("foo".to_string(), "bar".to_string());
    c.metadata.commands.insert(
        "hello".to_string(),
        vec!["echo".to_string(), "hello world".to_string()],
    );

    // MQTT
    let mut mqtt_opts = MqttOptions::parse_url(format!(
        "{}?client_id=test",
        env::var("TEST_MQTT_BROKER_URL").unwrap()
    ))
    .unwrap();
    mqtt_opts.set_clean_start(true);
    let (client, mut eventloop) = AsyncClient::new(mqtt_opts, 100);
    let (mqtt_tx, mut mqtt_rx) = mpsc::channel(100);

    tokio::spawn({
        async move {
            loop {
                match eventloop.poll().await {
                    Ok(v) => match v {
                        Event::Incoming(Incoming::Publish(p)) => mqtt_tx.send(p).await.unwrap(),
                        _ => {}
                    },
                    Err(_) => {}
                }
            }
        }
    });

    client
        .subscribe("eu868/gateway/0102030405060708/event/+", QoS::AtLeastOnce)
        .await
        .unwrap();
    client
        .subscribe("eu868/gateway/0102030405060708/state/+", QoS::AtLeastOnce)
        .await
        .unwrap();

    // Setup "Concentratord" API sockets.
    let zmq_ctx = zmq::Context::new();
    let zmq_pub = zmq_ctx.socket(zmq::PUB).unwrap();
    zmq_pub.bind(&c.backend.concentratord.event_url).unwrap();
    let zmq_pub = Arc::new(Mutex::new(zmq_pub));

    let zmq_cmd = zmq_ctx.socket(zmq::REP).unwrap();
    zmq_cmd.bind(&c.backend.concentratord.command_url).unwrap();
    let zmq_cmd = Arc::new(Mutex::new(zmq_cmd));

    // Handle gateway_id request.
    thread::spawn({
        let zmq_cmd = zmq_cmd.clone();

        move || {
            let zmq_cmd = zmq_cmd.lock().unwrap();
            let msg = zmq_cmd.recv_multipart(0).unwrap();
            let cmd = String::from_utf8(msg[0].clone()).unwrap();
            assert_eq!("gateway_id", cmd);
            zmq_cmd.send(vec![1, 2, 3, 4, 5, 6, 7, 8], 0).unwrap();
        }
    });

    // Sleep some time to receive message from MQTT broker.
    // Drain the channel.
    sleep(Duration::from_millis(100)).await;
    loop {
        if mqtt_rx.try_recv().is_err() {
            break;
        }
    }

    chirpstack_mqtt_forwarder::metadata::setup(&c).unwrap();
    chirpstack_mqtt_forwarder::backend::setup(&c).await.unwrap();
    chirpstack_mqtt_forwarder::mqtt::setup(&c).await.unwrap();

    // MQTT conn state
    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/state/conn",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::ConnState::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(
        gw::ConnState {
            gateway_id: "0102030405060708".into(),
            state: gw::conn_state::State::Online.into(),
            ..Default::default()
        },
        pl
    );

    // Uplink
    let uplink_pl = gw::UplinkFrame {
        phy_payload: vec![1, 2, 3],
        ..Default::default()
    };
    thread::spawn({
        let zmq_pub = zmq_pub.clone();
        let uplink_pl = uplink_pl.encode_to_vec();

        move || {
            let zmq_pub = zmq_pub.lock().unwrap();
            zmq_pub.send("up", zmq::SNDMORE).unwrap();
            zmq_pub.send(uplink_pl, 0).unwrap();
        }
    });

    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/up",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::UplinkFrame::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(uplink_pl, pl);

    // Stats
    let stats_pl = gw::GatewayStats {
        gateway_id: "0102030405060708".into(),
        ..Default::default()
    };
    thread::spawn({
        let zmq_pub = zmq_pub.clone();
        let stats_pl = stats_pl.encode_to_vec();

        move || {
            let zmq_pub = zmq_pub.lock().unwrap();
            zmq_pub.send("stats", zmq::SNDMORE).unwrap();
            zmq_pub.send(stats_pl, 0).unwrap();
        }
    });

    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/stats",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::GatewayStats::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(
        gw::GatewayStats {
            gateway_id: "0102030405060708".into(),
            metadata: [
                (
                    "mqtt_forwarder_version".to_string(),
                    env!("CARGO_PKG_VERSION").to_string()
                ),
                ("foo".to_string(), "bar".to_string()),
                ("hello".to_string(), "hello world".to_string()),
            ]
            .iter()
            .cloned()
            .collect(),
            ..Default::default()
        },
        pl
    );

    // Mesh Heartbeat
    let mesh_heartbeat_pl = gw::MeshHeartbeat {
        gateway_id: "0102030405060708".into(),
        ..Default::default()
    };
    thread::spawn({
        let zmq_pub = zmq_pub.clone();
        let mesh_heartbeat_pl = mesh_heartbeat_pl.encode_to_vec();

        move || {
            let zmq_pub = zmq_pub.lock().unwrap();
            zmq_pub.send("mesh_heartbeat", zmq::SNDMORE).unwrap();
            zmq_pub.send(mesh_heartbeat_pl, 0).unwrap();
        }
    });

    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/mesh-heartbeat",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::MeshHeartbeat::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(
        gw::MeshHeartbeat {
            gateway_id: "0102030405060708".into(),
            ..Default::default()
        },
        pl
    );

    // Downlink
    let down_pl = gw::DownlinkFrame {
        gateway_id: "0102030405060708".into(),
        downlink_id: 1234,
        ..Default::default()
    };
    let ack_pl = gw::DownlinkTxAck {
        downlink_id: 1234,
        ..Default::default()
    };

    thread::spawn({
        let zmq_cmd = zmq_cmd.clone();
        let down_pl = down_pl.encode_to_vec();
        let ack_pl = ack_pl.encode_to_vec();

        move || {
            let zmq_cmd = zmq_cmd.lock().unwrap();
            let msg = zmq_cmd.recv_multipart(0).unwrap();
            let cmd = String::from_utf8(msg[0].clone()).unwrap();
            assert_eq!("down", cmd);
            assert_eq!(down_pl, msg[1]);
            zmq_cmd.send(ack_pl, 0).unwrap();
        }
    });

    client
        .publish(
            "eu868/gateway/0102030405060708/command/down",
            QoS::AtLeastOnce,
            false,
            down_pl.encode_to_vec(),
        )
        .await
        .unwrap();

    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/ack",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::DownlinkTxAck::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(ack_pl, pl);

    // Config
    let config_pl = gw::GatewayConfiguration {
        gateway_id: "0102030405060708".to_string(),
        version: "123".to_string(),
        ..Default::default()
    };

    client
        .publish(
            "eu868/gateway/0102030405060708/command/config",
            QoS::AtLeastOnce,
            false,
            config_pl.encode_to_vec(),
        )
        .await
        .unwrap();

    // Use spawn_blocking as else will will block the tokio thread,
    // which will also block the mqtt consume loop of the mqtt backend.
    let msg = tokio::task::spawn_blocking({
        let zmq_cmd = zmq_cmd.clone();

        move || {
            let zmq_cmd = zmq_cmd.lock().unwrap();
            let msg = zmq_cmd.recv_multipart(0).unwrap();
            zmq_cmd.send(vec![], 0).unwrap();

            msg
        }
    })
    .await
    .unwrap();

    let cmd = String::from_utf8(msg[0].clone()).unwrap();
    assert_eq!("config", cmd);
    assert_eq!(config_pl.encode_to_vec(), msg[1]);
}
