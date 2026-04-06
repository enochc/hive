/// Example: Hive with MQTT peer
///
/// Demonstrates connecting a Hive instance to an MQTT broker.
/// Hive properties are published/subscribed via MQTT topics under
/// the configured prefix.
///
/// Requires an MQTT broker running on localhost:1883.  A quick way
/// to get one:
///
///     docker run -it -p 1883:1883 eclipse-mosquitto:2 \
///         mosquitto -c /mosquitto-no-auth.conf
///
/// Run:
///     cargo run --example mqtt_example --features mqtt
///

#[cfg(feature = "mqtt")]
#[tokio::main]
async fn main() {
    use hive::hive::Hive;
    use hive::init_logging;
    use hive::LevelFilter;
    use hive::CancellationToken;
    use std::time::Duration;
    use log::info;

    init_logging(Some(LevelFilter::Debug));

    let config_str = r#"
        name = "MqttExample"

        [MQTT]
        host = "127.0.0.1"
        port = 1883
        client_id = "hive-mqtt-example"
        topic_prefix = "hive/demo"
        qos = 1
        keep_alive = 30
        subscribe = ["lightValue", "temperature"]

        [Properties]
        lightValue = 0
        temperature = "68"
    "#;

    let mut hive = Hive::new_from_str_unknown(config_str);

    // Register a callback on the lightValue property
    hive.get_mut_property_by_name("lightValue")
        .unwrap()
        .on_next(move |value| {
            info!("lightValue changed via MQTT: {:?}", value);
        });

    hive.get_mut_property_by_name("temperature")
        .unwrap()
        .on_next(move |value| {
            info!("temperature changed via MQTT: {:?}", value);
        });

    let cancellation_token = CancellationToken::new();
    let cancel_clone = cancellation_token.clone();

    // Ctrl+C handler
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("failed to listen for ctrl+c");
        info!("Ctrl+C received, shutting down");
        cancel_clone.cancel();
    });

    let mut handler = hive.go(false, cancellation_token).await;

    // Give the MQTT connection time to establish
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Publish a property change — this will go out to MQTT and to any
    // connected TCP peers.
    info!("Setting lightValue = 42");
    handler.set_property("lightValue", Some(&42.into())).await;

    tokio::time::sleep(Duration::from_secs(1)).await;

    info!("Setting temperature = 75");
    handler.set_property("temperature", Some(&"75".into())).await;

    // Keep running until Ctrl+C
    info!("Hive running with MQTT.  Use mosquitto_sub/mosquitto_pub to interact.");
    info!("  mosquitto_sub -t 'hive/demo/#' -v");
    info!("  mosquitto_pub -t 'hive/demo/lightValue' -m '99'");
    tokio::time::sleep(Duration::from_secs(3600)).await;
}

#[cfg(not(feature = "mqtt"))]
fn main() {
    eprintln!("This example requires the 'mqtt' feature.");
    eprintln!("Run with: cargo run --example mqtt_example --features mqtt");
}
