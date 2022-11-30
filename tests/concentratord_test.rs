use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::thread;

use futures::StreamExt;
use paho_mqtt as mqtt;
use prost::Message;

use chirpstack_api::gw;
use chirpstack_mqtt_forwarder::config;

#[tokio::test]
async fn end_to_end() {
    let mut c = config::Configuration {
        backend: config::Backend {
            enabled: "concentratord".into(),
            ..Default::default()
        },
        mqtt: config::Mqtt {
            server: "tcp://mosquitto:1883".into(),
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
    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri("tcp://mosquitto:1883")
        .persistence(None)
        .finalize();
    let mut client = mqtt::AsyncClient::new(create_opts).unwrap();
    let mut stream = client.get_stream(25);
    let conn_opts = mqtt::ConnectOptionsBuilder::new()
        .clean_session(true)
        .finalize();
    client.connect(conn_opts).await.unwrap();
    client
        .subscribe("eu868/gateway/0102030405060708/event/+", 0)
        .await
        .unwrap();
    client
        .subscribe("eu868/gateway/0102030405060708/state/+", 0)
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

    for _ in 0..stream.len() {
        stream.next().await.unwrap().unwrap();
    }

    chirpstack_mqtt_forwarder::metadata::setup(&c).unwrap();
    chirpstack_mqtt_forwarder::backend::setup(&c).await.unwrap();
    chirpstack_mqtt_forwarder::mqtt::setup(&c).await.unwrap();

    // MQTT conn state
    let mqtt_msg = stream.next().await.unwrap().unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/state/conn",
        mqtt_msg.topic()
    );
    let pl = gw::ConnState::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
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

    let mqtt_msg = stream.next().await.unwrap().unwrap();
    assert_eq!("eu868/gateway/0102030405060708/event/up", mqtt_msg.topic());
    let pl = gw::UplinkFrame::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
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

    let mqtt_msg = stream.next().await.unwrap().unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/stats",
        mqtt_msg.topic()
    );
    let pl = gw::GatewayStats::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
    assert_eq!(stats_pl, pl);

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

    let mqtt_msg = mqtt::Message::new(
        "eu868/gateway/0102030405060708/command/down",
        down_pl.encode_to_vec(),
        0,
    );
    client.publish(mqtt_msg).await.unwrap();

    let mqtt_msg = stream.next().await.unwrap().unwrap();
    assert_eq!("eu868/gateway/0102030405060708/event/ack", mqtt_msg.topic());
    let pl = gw::DownlinkTxAck::decode(&mut Cursor::new(mqtt_msg.payload())).unwrap();
    assert_eq!(ack_pl, pl);
}
