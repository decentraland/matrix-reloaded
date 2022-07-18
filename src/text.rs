use indicatif::{ProgressBar, ProgressStyle};
use lipsum::lipsum;
use rand::Rng;
use std::time::{Duration, Instant};
use tokio::time::sleep;

pub fn get_random_string() -> String {
    let random_number: usize = rand::thread_rng().gen_range(5..15);
    lipsum(random_number)
}

pub fn default_spinner() -> ProgressBar {
    ProgressBar::new_spinner().with_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{prefix:.bold.dim} {spinner} {wide_msg}"),
    )
}

pub async fn spin_for(time: Duration, spinner: &ProgressBar) {
    let wait_time = Instant::now();
    loop {
        if wait_time.elapsed().ge(&time) {
            break;
        }
        sleep(Duration::from_millis(100)).await;
        spinner.inc(1);
    }
}
