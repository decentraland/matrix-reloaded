use std::env;

use matrix_load_testing_tool::{Configuration, State};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut args = env::args();

    let program_path = args.next().unwrap();

    println!("{}", program_path);
    if args.len() < 1 {
        eprintln!("Usage: time {} <server1> [<server>]", program_path);
        return Ok(());
    }

    let homeserver_url = args
        .filter_map(|s| s.parse::<String>().ok())
        .take(1)
        .next()
        .unwrap();

    let config = Configuration {
        total_steps: 2,
        users_per_step: 50,
        friendship_ratio: 0.5,
        homeserver_url,
    };
    let mut state = State::new(config);
    state.run().await;

    Ok(())
}
