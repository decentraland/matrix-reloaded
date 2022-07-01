use clap::Parser;
use config::{Config, ConfigError, File};
use regex::Regex;
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::time::Duration;

/// This function returns homeserver domain and url, ex:
///  - get_homeserver_url("matrix.domain.com") => ("matrix.domain.com", "https://matrix.domain.com")
pub fn get_homeserver_url(homeserver: &str, protocol: Option<&str>) -> (String, String) {
    let regex = Regex::new(r"https?://").unwrap();
    if regex.is_match(homeserver) {
        let parts: Vec<&str> = regex.splitn(homeserver, 2).collect();
        (parts[1].to_string(), homeserver.to_string())
    } else {
        let protocol = protocol.unwrap_or("https");
        (homeserver.to_string(), format!("{protocol}://{homeserver}"))
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
    ticks: Option<u64>,

    /// Tick duration in seconds
    #[clap(short, long, value_parser)]
    duration: Option<u64>,

    /// Number of users to act during the simulation
    #[clap(short, long, value_parser)]
    users_per_tick: Option<u64>,

    /// Max number of users for current simulation
    #[clap(short, long, value_parser)]
    max_users: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Server {
    pub homeserver: String,
    pub wk_login: bool,
}

#[serde_as]
#[derive(Debug, Deserialize, Clone)]
pub struct Params {
    pub ticks: usize,
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(rename = "tick_duration_in_secs")]
    pub tick_duration: Duration,
    pub max_users: usize,
    pub users_per_tick: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Requests {
    pub retry_enabled: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SimulationConfig {
    pub server: Server,
    pub simulation: Params,
    pub requests: Requests,
}

impl SimulationConfig {
    pub fn new() -> Result<Self, ConfigError> {
        let args = Args::parse();
        println!("Args: {:#?}", args);

        let config = Config::builder()
            .add_source(File::with_name("configuration"))
            .set_override("server.homeserver", args.homeserver)?
            .set_override_option("simluation.ticks", args.ticks)?
            .set_override_option("simluation.duration", args.duration)?
            .set_override_option("simluation.max_users", args.max_users)?
            .set_override_option("simluation.users_per_tick", args.users_per_tick)?
            .build()?;

        println!("Config: {:#?}", config);
        config.try_deserialize()
    }
}
