use std::{
    env,
    time::{Duration, Instant},
};

use indicatif::{ProgressBar, ProgressStyle};
use lipsum::lipsum;
use rand::Rng;
use tokio::time::sleep;

pub fn get_random_string() -> String {
    let random_number: usize = rand::thread_rng().gen_range(5..15);
    lipsum(random_number)
}

pub fn create_simulation_bar(total_ticks: usize) -> ProgressBar {
    let progress_bar = ProgressBar::new(total_ticks.try_into().unwrap());
    let style = ProgressStyle::default_bar()
        .template(
            "{prefix:>12.cyan.bold}: {spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos:>7}/{len:7} ({eta})",
        )
        .progress_chars("=>-");
    progress_bar.set_style(style);
    progress_bar.set_prefix("Simulation");
    let is_ci = env::var("CI").is_ok();
    if !is_ci {
        progress_bar.enable_steady_tick(100);
    }
    progress_bar
}

pub fn create_users_bar(size: usize) -> ProgressBar {
    let progress_style = ProgressStyle::default_bar()
        .template("{prefix:>12.cyan.bold}: [{wide_bar:.cyan/blue}] {pos:>7}/{len:7}")
        .progress_chars("o/x");

    let progress_bar = ProgressBar::new(size.try_into().unwrap());
    progress_bar.set_style(progress_style);
    progress_bar.set_prefix("Users in sync");
    let is_ci = env::var("CI").is_ok();
    if !is_ci {
        progress_bar.enable_steady_tick(100);
    }

    progress_bar
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
