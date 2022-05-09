use clap::Parser;
use config::Config;
use matrix_load_testing_tool::{Configuration, State};

mod errors;

use errors::{Error, Result};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Matrix server to be tested
    #[clap(short, long)]
    homeserver: String,

    /// Folder where reports will be generated
    #[clap(short, long, default_value = "output")]
    output_dir: String,

    /// Should run the test
    #[clap(short, long)]
    run: bool,

    /// Should users be created
    #[clap(short, long, group = "user-abm")]
    create: bool,

    /// Should users be deleted
    #[clap(short, long, group = "user-abm")]
    delete: bool,

    /// Mode to run the CLI, if left empty the test with messages will be run
    #[clap(short, long, default_value = "users.json")]
    users_filename: String,

    /// The amount of users that will be created or deleted
    #[clap(
        short,
        long,
        requires = "user-abm",
        parse(try_from_str),
        default_value = "30"
    )]
    amount: i64,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();
    let config = parse_configuration("Config", args);
    match config {
        Ok(config) => {
            match config {
                Configuration { create: true, .. } => create_users(config).await,
                Configuration { delete: true, .. } => delete_users(config).await,
                Configuration { run: true, .. } => run_state(config).await,
                _ => println!("One of create delete or run modes must be selected"),
            };
        }
        Err(Error::ConfigError(e)) => {
            println!("Couldn't parse config {}", e);
        }
        Err(Error::StdError(e)) => {
            println!("Couldn't parse config {}", e);
        }
    }

    Ok(())
}

fn parse_configuration(file_name: &str, args: Args) -> Result<Configuration> {
    let config = Config::builder()
        .add_source(config::File::with_name(file_name))
        .set_override("homeserver_url", args.homeserver)?
        .set_override("output_dir", args.output_dir)?
        .set_override("create", args.create)?
        .set_override("delete", args.delete)?
        .set_override("run", args.run)?
        .set_override("user_count", args.amount)?
        .set_override_option("users_filename", Some(args.users_filename))?
        .build()?;

    let config = config.try_deserialize::<Configuration>();

    config.map_err(|e| e.into())
}

async fn run_state(config: Configuration) {
    let mut state = State::new(config);
    state.run().await
}

async fn create_users(config: Configuration) {
    let mut state = State::new(config);
    state.create_users().await
}

async fn delete_users(_config: Configuration) {
    //TODO! Implement delete
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn validate_config_example() -> Result<()> {
        let args = Args {
            homeserver: "home".to_owned(),
            output_dir: "output".to_owned(),
            create: true,
            delete: false,
            run: false,
            amount: 42,
            users_filename: "users".to_owned(),
        };

        parse_configuration("Config.example", args)?;

        Ok(())
    }
}
