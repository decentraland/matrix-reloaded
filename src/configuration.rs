use crate::time::time_now;
use clap::Parser;
use config::{ConfigError, File};
use regex::Regex;
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::time::Duration;

/// This function returns homeserver domain and url, ex:
///  - get_homeserver_url("matrix.domain.com") => "https://matrix.domain.com"
pub fn get_homeserver_url(homeserver: &str, default_protocol: Option<&str>) -> String {
    let regex = Regex::new(r"https?://").unwrap();
    if regex.is_match(homeserver) {
        homeserver.to_string()
    } else {
        let protocol = default_protocol.unwrap_or("https");
        format!("{protocol}://{homeserver}")
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

    /// Execution ID to be used as part of the ID (as localpart)
    #[clap(short, long, value_parser)]
    execution_id: Option<String>,

    /// Probability of a user to act on a tick. Default is 100 (%).
    #[clap(long, value_parser)]
    probability_to_act: Option<i64>,

    /// Probability of a user to have a short life. Should be a number between 0 and 100. Default is 50 (%).
    #[clap(long, value_parser)]
    probability_for_short_lifes: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Server {
    pub homeserver: String,
    pub wk_login: bool,
}

#[serde_as]
#[derive(Debug, Deserialize, Default, Clone)]
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
    pub probability_to_act: usize,
    pub probability_for_short_lifes: usize,
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
            .set_override_option("simulation.execution_id", args.execution_id)?
            .set_default("simulation.probability_to_act", 100.)?
            .set_default("simulation.probability_for_short_lifes", 50.)?
            .set_override_option("simulation.probability_to_act", args.probability_to_act)?
            .set_override_option(
                "simulation.probability_for_short_lifes",
                args.probability_for_short_lifes,
            )?
            .build()?;

        log::debug!("Config: {:#?}", config);
        config.try_deserialize()
    }
}
