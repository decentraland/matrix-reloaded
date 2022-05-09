use config::ConfigError;

#[derive(Debug)]
pub enum Error {
    StdError(Box<dyn std::error::Error>),
    ConfigError(ConfigError),
}

impl From<Box<dyn std::error::Error>> for Error {
    fn from(err: Box<dyn std::error::Error>) -> Error {
        Error::StdError(err)
    }
}

impl From<ConfigError> for Error {
    fn from(err: ConfigError) -> Error {
        Error::ConfigError(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
