use std::time::{SystemTime, UNIX_EPOCH};

pub fn time_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is valid")
        .as_millis()
}
