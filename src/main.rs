use clap::Parser;
use config::Config;
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
    let config = Config::builder()
        .add_source(config::File::with_name("Config"))
        .set_override("homeserver_url", args.homeserver)
        .unwrap()
        .set_override("output_dir", args.output_dir)
        .unwrap()
        .build()
        .unwrap();

    let config = config.try_deserialize::<Configuration>();
    match config {
        Ok(config) => {
            let mut state = State::new(config);
            state.run().await;
        }
        Err(e) => {
            println!("Couldn't parse config {}", e);
        }
    };
    Ok(())
}
