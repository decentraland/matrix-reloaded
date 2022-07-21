use std::{env, sync::Arc, thread};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

pub trait Progress
where
    Self: Sync + Send,
{
    fn start(&self);
    fn tick(&mut self, users_syncing: u64);
    fn finish(&self);
}

pub struct SimulationProgress {
    multi_progress: Arc<MultiProgress>,
    progress_bar: ProgressBar,
    users_bar: ProgressBar,
}

impl SimulationProgress {
    fn create_simulation_bar(total_ticks: usize) -> ProgressBar {
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

    fn create_users_bar(size: usize) -> ProgressBar {
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
}

impl SimulationProgress {
    pub fn new(total_ticks: usize, max_users: usize) -> Self {
        let multi_progress = Arc::new(MultiProgress::new());
        let progress_bar = multi_progress.add(Self::create_simulation_bar(total_ticks));
        let users_bar = multi_progress.add(Self::create_users_bar(max_users));
        Self {
            multi_progress,
            progress_bar,
            users_bar,
        }
    }
}

impl Progress for SimulationProgress {
    fn start(&self) {
        let is_ci = env::var("CI").is_ok();
        if !is_ci {
            let m = self.multi_progress.clone();
            thread::spawn(move || m.join_and_clear().unwrap());
        }
    }

    fn tick(&mut self, users_syncing: u64) {
        let is_ci = env::var("CI").is_ok();
        if is_ci {
            println!("users syncing: {users_syncing}");
        } else {
            self.progress_bar.inc(1);
            self.users_bar.set_position(users_syncing);
        }
    }

    fn finish(&self) {
        self.progress_bar.disable_steady_tick();
        self.users_bar.disable_steady_tick();

        let is_ci = env::var("CI").is_ok();
        if !is_ci {
            self.progress_bar
                .finish_with_message("Simulation finished!");

            self.users_bar.finish_and_clear();
            self.multi_progress.join_and_clear().unwrap();
        }
    }
}

#[derive(Default)]
pub struct QuietProgress {
    tick: usize,
    max_users_connected: u64,
}

impl Progress for QuietProgress {
    fn start(&self) {}

    fn tick(&mut self, users_syncing: u64) {
        if self.max_users_connected < users_syncing {
            self.max_users_connected = users_syncing;
        }
        self.tick += 1;
        println!("users syncing: {users_syncing}");
    }

    fn finish(&self) {
        println!(
            "max amount of users connected during simulation: {}",
            self.max_users_connected
        )
    }
}

pub fn create_progress(ticks: usize, max_users: usize) -> Box<dyn Progress> {
    let is_ci = env::var("CI").is_ok();
    match is_ci {
        true => Box::new(QuietProgress::default()),
        false => Box::new(SimulationProgress::new(ticks, max_users)),
    }
}
