use std::env;

use matrix_load_testing_tool::{Configuration, State};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut args = env::args();

    args.next().unwrap();

    if args.len() < 1 {
        eprintln!("Usage: cargo run -- <server>");
        return Ok(());
    }

    let homeserver_url = args
        .filter_map(|s| s.parse::<String>().ok())
        .take(1)
        .next()
        .unwrap();

    let config = Configuration {
        total_steps: 2,
        users_per_step: 5,
        friendship_ratio: 0.5,
        homeserver_url,
        time_to_run_per_step: 120,
    };
    let mut state = State::new(config);
    state.run().await;

    Ok(())
}
