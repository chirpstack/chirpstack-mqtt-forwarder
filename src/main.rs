#[macro_use]
extern crate anyhow;

use std::str::FromStr;

use clap::{Parser, Subcommand};
use log::info;
use signal_hook::{consts::SIGINT, consts::SIGTERM, iterator::Signals};

mod backend;
mod config;
mod logging;
mod metadata;
mod mqtt;

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
    let log_level = log::Level::from_str(&config.logging.level).expect("Parse log_level error");
    logging::setup(
        env!("CARGO_PKG_NAME"),
        log_level,
        config.logging.log_to_syslog,
    )
    .expect("Setup logger error");

    info!(
        "Starting {} (version: {}, docs: {})",
        env!("CARGO_PKG_DESCRIPTION"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_HOMEPAGE"),
    );

    metadata::setup(&config).expect("Setup metadata error");
    backend::setup(&config).await.expect("Setup backend error");
    mqtt::setup(&config).await.expect("Setup MQTT client error");

    let mut signals = Signals::new(&[SIGINT, SIGTERM]).unwrap();
    signals.forever().next();
}
