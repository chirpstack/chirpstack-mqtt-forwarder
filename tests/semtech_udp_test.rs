use std::env;
use std::io::Cursor;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rumqttc::v5::{mqttbytes::QoS, AsyncClient, Event, Incoming, MqttOptions};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::sleep;

use chirpstack_api::{common, gw, pbjson_types, prost::Message};
use chirpstack_mqtt_forwarder::config;

#[tokio::test]
async fn end_to_end() {
    dotenv::dotenv().ok();
    dotenv::from_filename(".env.local").ok();

    let mut buffer: [u8; 65535] = [0; 65535];

    let mut c = config::Configuration {
        backend: config::Backend {
            gateway_id: "0102030405060708".into(),
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

    // UDP
    let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    socket.connect("0.0.0.0:1700").await.unwrap();

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
                if let Ok(v) = eventloop.poll().await {
                    if let Event::Incoming(Incoming::Publish(p)) = v {
                        mqtt_tx.send(p).await.unwrap()
                    }
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

    // PULL_DATA
    socket
        .send(&[2, 0, 1, 2, 1, 2, 3, 4, 5, 6, 7, 8])
        .await
        .unwrap();

    // PULL_ACK
    let size = socket.recv(&mut buffer).await.unwrap();
    assert_eq!(&[2, 0, 1, 4], &buffer[..size]);

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

    // PUSH_DATA rxpk
    let pl_b = r#"{
            "rxpk": [
                {
                    "time": "2022-09-10T12:30:15Z",
                    "tmst": 1234,
                    "chan": 2,
                    "rfch": 0,
                    "freq": 868.1,
                    "stat": 1,
                    "modu": "LORA",
                    "datr": "SF7BW125",
                    "codr": "4/5",
                    "rssi": -35,
                    "lsnr": 5.1,
                    "size": 3,
                    "data": "AQID"
                }
            ]
        }"#;
    let ts = DateTime::parse_from_str("2022-09-10T12:30:15Z", "%+")
        .unwrap()
        .with_timezone(&Utc);
    let mut b = vec![2, 0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8];
    b.extend_from_slice(pl_b.as_bytes());
    socket.send(&b).await.unwrap();

    // PUSH_ACK
    let size = socket.recv(&mut buffer).await.unwrap();
    assert_eq!(&[2, 0, 1, 1], &buffer[..size]);

    // MQTT up event
    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/up",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let mut pl = gw::UplinkFrame::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_ne!(0, pl.rx_info.as_ref().unwrap().uplink_id);
    pl.rx_info.as_mut().unwrap().uplink_id = 0;
    assert_eq!(
        gw::UplinkFrame {
            phy_payload: vec![1, 2, 3],
            tx_info: Some(gw::UplinkTxInfo {
                frequency: 868100000,
                modulation: Some(gw::Modulation {
                    parameters: Some(gw::modulation::Parameters::Lora(gw::LoraModulationInfo {
                        bandwidth: 125000,
                        spreading_factor: 7,
                        code_rate: gw::CodeRate::Cr45.into(),
                        ..Default::default()
                    })),
                }),
            }),
            rx_info: Some(gw::UplinkRxInfo {
                gateway_id: "0102030405060708".into(),
                gw_time: Some(pbjson_types::Timestamp::from(ts)),
                rssi: -35,
                snr: 5.1,
                channel: 2,
                rf_chain: 0,
                context: vec![0, 0, 4, 210],
                crc_status: gw::CrcStatus::CrcOk.into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        pl
    );

    // MQTT downlink
    let pl = gw::DownlinkFrame {
        downlink_id: 1234,
        gateway_id: "0102030405060708".into(),
        items: vec![gw::DownlinkFrameItem {
            phy_payload: vec![1, 2, 3],
            tx_info: Some(gw::DownlinkTxInfo {
                frequency: 868300000,
                power: 16,
                modulation: Some(gw::Modulation {
                    parameters: Some(gw::modulation::Parameters::Lora(gw::LoraModulationInfo {
                        bandwidth: 125000,
                        spreading_factor: 8,
                        code_rate: gw::CodeRate::Cr45.into(),
                        ..Default::default()
                    })),
                }),
                timing: Some(gw::Timing {
                    parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                        delay: Some(pbjson_types::Duration::from(Duration::from_secs(1))),
                    })),
                }),
                context: vec![0, 0, 4, 210],
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };

    client
        .publish(
            "eu868/gateway/0102030405060708/command/down",
            QoS::AtLeastOnce,
            false,
            pl.encode_to_vec(),
        )
        .await
        .unwrap();

    // PULL_RESP
    let size = socket.recv(&mut buffer).await.unwrap();
    assert_eq!(&[2, 210, 4, 3], &buffer[..4]);
    let json = String::from_utf8_lossy(&buffer[4..size]);
    assert_eq!("{\"txpk\":{\"imme\":false,\"rfch\":0,\"powe\":16,\"ant\":0,\"brd\":0,\"tmst\":1001234,\"freq\":868.3,\"modu\":\"LORA\",\"datr\":\"SF8BW125\",\"codr\":\"4/5\",\"ipol\":false,\"size\":3,\"data\":\"AQID\"}}", json);

    // TX_ACK
    socket
        .send(&[2, 210, 4, 5, 1, 2, 3, 4, 5, 6, 7, 8])
        .await
        .unwrap();

    // MQTT ack event
    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/ack",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::DownlinkTxAck::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(
        gw::DownlinkTxAck {
            gateway_id: "0102030405060708".into(),
            downlink_id: 1234,
            items: vec![gw::DownlinkTxAckItem {
                status: gw::TxAckStatus::Ok.into(),
            }],
            ..Default::default()
        },
        pl
    );

    // PUSH_DATA stat
    let pl_b = r#"{
            "stat": {
                "time": "2022-09-10 12:30:15 GMT",
                "lati": 46.24000,
                "long": 3.25230,
                "alti": 145,
                "rxnb": 1,
                "rxok": 1,
                "rxfw": 1,
                "ackr": 100.0,
                "dwnb": 1,
                "txnb": 1
            }
        }"#;
    let mut b = vec![2, 0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8];
    b.extend_from_slice(pl_b.as_bytes());
    socket.send(&b).await.unwrap();

    // PUSH_ACK
    let size = socket.recv(&mut buffer).await.unwrap();
    assert_eq!(&[2, 0, 1, 1], &buffer[..size]);

    // MQTT stats event
    let mqtt_msg = mqtt_rx.recv().await.unwrap();
    assert_eq!(
        "eu868/gateway/0102030405060708/event/stats",
        String::from_utf8(mqtt_msg.topic.to_vec()).unwrap()
    );
    let pl = gw::GatewayStats::decode(&mut Cursor::new(mqtt_msg.payload.to_vec())).unwrap();
    assert_eq!(
        gw::GatewayStats {
            gateway_id: "0102030405060708".into(),
            time: Some(pbjson_types::Timestamp::from(ts)),
            location: Some(common::Location {
                latitude: 46.24,
                longitude: 3.2523,
                altitude: 145.0,
                source: common::LocationSource::Gps.into(),
                ..Default::default()
            }),
            rx_packets_received: 1,
            rx_packets_received_ok: 1,
            tx_packets_received: 1,
            tx_packets_emitted: 1,
            tx_packets_per_frequency: [(868300000, 1)].iter().cloned().collect(),
            rx_packets_per_frequency: [(868100000, 1)].iter().cloned().collect(),
            tx_packets_per_modulation: vec![gw::PerModulationCount {
                modulation: Some(gw::Modulation {
                    parameters: Some(gw::modulation::Parameters::Lora(gw::LoraModulationInfo {
                        bandwidth: 125000,
                        spreading_factor: 8,
                        code_rate: gw::CodeRate::Cr45.into(),
                        ..Default::default()
                    })),
                }),
                count: 1,
            }],
            rx_packets_per_modulation: vec![gw::PerModulationCount {
                modulation: Some(gw::Modulation {
                    parameters: Some(gw::modulation::Parameters::Lora(gw::LoraModulationInfo {
                        bandwidth: 125000,
                        spreading_factor: 7,
                        code_rate: gw::CodeRate::Cr45.into(),
                        ..Default::default()
                    })),
                }),
                count: 1,
            }],
            tx_packets_per_status: [(gw::TxAckStatus::Ok.into(), 1),].iter().cloned().collect(),
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
}
