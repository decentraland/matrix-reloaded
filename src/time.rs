use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;

pub fn time_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is valid")
        .as_millis()
}

pub fn execution_id() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}
