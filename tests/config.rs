use matrix_reloaded::configuration::{Config, Requests, Server, Simulation};

pub fn create_test_config() -> Config {
    let server = Server {
        homeserver: "https://matrix.decentraland.zone".to_string(),
        wk_login: true,
    };
    let simulation = Simulation::default();
    let requests = Requests {
        retry_enabled: true,
    };
    Config {
        server,
        simulation,
        requests,
    }
}
