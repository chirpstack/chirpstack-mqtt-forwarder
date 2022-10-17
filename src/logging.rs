use std::process;

use anyhow::Result;
use syslog::{BasicLogger, Facility, Formatter3164};

pub fn setup(name: &str, level: log::Level, syslog: bool) -> Result<()> {
    if syslog {
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: name.to_string(),
            pid: process::id() as u32,
        };
        let logger = syslog::unix(formatter).map_err(|e| anyhow!("{}", e))?;
        log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
            .map(|()| log::set_max_level(level.to_level_filter()))?;
    } else {
        simple_logger::init_with_level(level)?;
    }

    Ok(())
}
