use crate::time::time_now;
use clap::Parser;
use config::{ConfigError, File};
use regex::Regex;
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::time::Duration;

/// This function returns homeserver domain and url, ex:
///  - get_homeserver_url("matrix.domain.com") => ("matrix.domain.com", "https://matrix.domain.com")
///  - get_homeserver_url("synapse.domain.com") => ("domain.com", "https://matrix.domain.com")
pub fn get_homeserver_url(homeserver: &str, default_protocol: Option<&str>) -> (String, String) {
    let regex = Regex::new(r"https?://").unwrap();
    if regex.is_match(homeserver) {
        let parts: Vec<&str> = regex.splitn(homeserver, 2).collect();
        let domain = parts[1].to_string().replace("synapse.", "");
        (domain, homeserver.to_string())
    } else {
        let protocol = default_protocol.unwrap_or("https");
        (
            homeserver.to_string().replace("synapse.", ""),
            format!("{protocol}://{homeserver}"),
        )
    }
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Homeserver to use during the simulation
    #[clap(short, long, value_parser)]
    homeserver: String,

    /// Number of times to tick during the simulation
    #[clap(short, long, value_parser)]
    ticks: Option<i64>,

    /// Tick duration in seconds
    #[clap(short, long, value_parser)]
    duration: Option<i64>,

    /// Number of users to act during the simulation
    #[clap(short, long, value_parser)]
    users_per_tick: Option<i64>,

    /// Max number of users for current simulation
    #[clap(short, long, value_parser)]
    max_users: Option<i64>,

    /// Output folder for reports
    output: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Server {
    pub homeserver: String,
    pub wk_login: bool,
}

#[serde_as]
#[derive(Debug, Deserialize, Clone)]
pub struct Simulation {
    pub ticks: usize,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "tick_duration_in_secs")]
    pub tick_duration: Duration,
    pub max_users: usize,
    pub users_per_tick: usize,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "grace_period_duration_in_secs")]
    pub grace_period_duration: Duration,
    pub output: String,
    pub execution_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Requests {
    pub retry_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: Server,
    pub simulation: Simulation,
    pub requests: Requests,
}

impl Config {
    pub fn new() -> Result<Self, ConfigError> {
        let args = Args::parse();
        log::debug!("Args: {:#?}", args);

        let config = config::Config::builder()
            .add_source(File::with_name("configuration"))
            .set_override("server.homeserver", args.homeserver)?
            .set_override_option("simulation.ticks", args.ticks)?
            .set_override_option("simulation.duration", args.duration)?
            .set_override_option("simulation.max_users", args.max_users)?
            .set_override_option("simulation.users_per_tick", args.users_per_tick)?
            .set_override_option("simulation.output", args.output)?
            .set_default("simulation.execution_id", time_now().to_string())?
            .build()?;

        log::debug!("Config: {:#?}", config);
        config.try_deserialize()
    }
}
