use config::ConfigError;
use matrix_reloaded::{configuration::SimulationConfig, Simulation};

#[tokio::main]
async fn main() -> Result<(), ConfigError> {
    env_logger::init();

    let config = SimulationConfig::new()?;

    let mut simulation = Simulation::with_config(config);
    simulation.run().await;
    Ok(())
}
