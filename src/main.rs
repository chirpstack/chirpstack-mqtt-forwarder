use std::process;
use std::str::FromStr;
use std::time::Duration;

use clap::{Parser, Subcommand};
use log::info;
use signal_hook::{consts::SIGINT, consts::SIGTERM, iterator::Signals};
use tokio::time::sleep;

use chirpstack_mqtt_forwarder::{backend, cmd, commands, config, logging, metadata, mqtt};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    config: Vec<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Print the configuration template
    Configfile {},
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = config::Configuration::get(&cli.config).expect("Read configuration error");

    if let Some(Commands::Configfile {}) = &cli.command {
        cmd::configfile::run(&config);
        process::exit(0);
    }

    let log_level = log::Level::from_str(&config.logging.level).expect("Parse log_level error");

    // Loop until success, as this will fail when syslog hasn't been fully started.
    while let Err(e) = logging::setup(
        env!("CARGO_PKG_NAME"),
        log_level,
        config.logging.log_to_syslog,
    ) {
        println!("Setup log error: {}", e);
        sleep(Duration::from_secs(1)).await;
    }

    info!(
        "Starting {} (version: {}, docs: {})",
        env!("CARGO_PKG_DESCRIPTION"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_HOMEPAGE"),
    );

    commands::setup(&config).expect("Setup commands error");
    metadata::setup(&config).expect("Setup metadata error");
    mqtt::setup(&config).await.expect("Setup MQTT client error");
    backend::setup(&config).await.expect("Setup backend error");

    let mut signals = Signals::new([SIGINT, SIGTERM]).unwrap();
    signals.forever().next();
}
