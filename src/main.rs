use clap::Parser;
use matrix_load_testing_tool::{Configuration, State};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long)]
    homeserver: String,

    // folder where reports will be generated
    #[clap(short, long, default_value = "output")]
    output_dir: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    let config = Configuration {
        total_steps: 2,
        users_per_step: 5,
        friendship_ratio: 0.5,
        homeserver_url: args.homeserver,
        time_to_run_per_step: 120,
    };
    let mut state = State::new(config);
    state.run(args.output_dir).await;

    Ok(())
}
